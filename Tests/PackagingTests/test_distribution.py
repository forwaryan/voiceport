import json
import pathlib
import plistlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import distribution_signing as signing
import release_macos as release
import signing_identity

TEAM = 'ABCD123456'
IDENTITY = '1234567890ABCDEF1234567890ABCDEF12345678'
SUBMISSION = '12345678-1234-4234-8234-123456789abc'


class DistributionSigningTests(unittest.TestCase):
    def test_distribution_requires_developer_id_for_expected_team(self):
        with tempfile.TemporaryDirectory() as temp:
            config = pathlib.Path(temp).resolve() / 'absent.json'
            for selected in [None, '-']:
                with self.subTest(selected=selected), self.assertRaisesRegex(ValueError, 'ad-hoc signing is forbidden'):
                    signing_identity.resolve_identity(selected, config, distribution=True, team_id=TEAM)
            for name, accepted in [('Developer ID Application: Example (' + TEAM + ')', True),
                                   ('Apple Development: Example (' + TEAM + ')', False),
                                   ('Developer ID Application: Example (OTHER12345)', False),
                                   ('Developer ID Installer: Example (' + TEAM + ')', False)]:
                result = subprocess.CompletedProcess([], 0, f'  1) {IDENTITY} "{name}"\n', '')
                with self.subTest(name=name), patch.object(signing_identity.subprocess, 'run', return_value=result):
                    if accepted:
                        self.assertEqual(signing_identity.resolve_identity(IDENTITY, config, distribution=True, team_id=TEAM), IDENTITY)
                    else:
                        with self.assertRaisesRegex(ValueError, 'Developer ID Application'):
                            signing_identity.resolve_identity(IDENTITY, config, distribution=True, team_id=TEAM)
            with self.assertRaisesRegex(ValueError, 'Apple Team ID'):
                signing_identity.resolve_identity(IDENTITY, config, distribution=True, team_id='bad')

    def test_signed_metadata_needs_timestamp_runtime_and_exact_identity(self):
        good = (f'Identifier={signing.BUNDLE_ID}\nTeamIdentifier={TEAM}\n'
                f'Authority=Developer ID Application: Example ({TEAM})\n'
                'CodeDirectory v=20500 flags=0x10000(runtime)\nTimestamp=Nov 14, 2026\n')
        signing.require_metadata(good, team_id=TEAM, identifier=signing.BUNDLE_ID)
        for before, after in [('Timestamp=Nov 14, 2026', 'Signed Time=Nov 14, 2026'),
                              ('flags=0x10000(runtime)', 'flags=0x0(none)'),
                              ('Developer ID Application:', 'Apple Development:'),
                              ('TeamIdentifier=' + TEAM, 'TeamIdentifier=OTHER12345'),
                              ('Identifier=' + signing.BUNDLE_ID, 'Identifier=other')]:
            with self.subTest(before=before), self.assertRaises(ValueError):
                signing.require_metadata(good.replace(before, after), team_id=TEAM, identifier=signing.BUNDLE_ID)

    def test_entitlements_match_documented_runtime_requirements(self):
        entitlements = plistlib.loads((ROOT / 'resources/VoicePort.entitlements').read_bytes())
        self.assertEqual(entitlements, signing.APP_ENTITLEMENTS)
        self.assertNotIn('com.apple.security.get-task-allow', entitlements)
        self.assertNotIn('com.apple.security.cs.allow-jit', entitlements)
        cli = plistlib.loads((ROOT / 'resources/VoicePortCLI.entitlements').read_bytes())
        self.assertEqual(cli, {'com.apple.security.cs.disable-library-validation': True})
        self.assertEqual(cli, signing.CLI_ENTITLEMENTS)

    def test_distribution_preflight_cannot_build_without_certificate(self):
        with tempfile.TemporaryDirectory() as temp:
            output = pathlib.Path(temp).resolve() / 'release'
            with patch.object(release, 'resolve_identity', side_effect=ValueError('missing Developer ID')), \
                 patch.object(release, 'run') as run:
                with self.assertRaisesRegex(ValueError, 'missing Developer ID'):
                    release.prepare(output, TEAM, True)
                self.assertFalse(output.exists())
                run.assert_not_called()


class NotarizationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.folder = pathlib.Path(self.temp.name).resolve()
        (self.folder / 'notarization-upload.zip').write_bytes(b'signed candidate')
        prepared = patch.object(release, 'check_prepared', return_value={'schema_version': 1, 'team_id': TEAM})
        prepared.start()
        self.addCleanup(prepared.stop)

    def test_unknown_upload_result_fences_repeat_submission(self):
        with patch.object(release.subprocess, 'run', side_effect=subprocess.TimeoutExpired('notarytool', 600)) as run:
            with self.assertRaisesRegex(ValueError, 'result unknown'):
                release.submit(self.folder, 'profile', None)
            self.assertTrue((self.folder / 'submit-intent.json').exists())
            with self.assertRaisesRegex(ValueError, 'already attempted'):
                release.submit(self.folder, 'profile', None)
            self.assertEqual(run.call_count, 1)
        with self.assertRaisesRegex(ValueError, 'no confirmed submission ID'):
            release.recover_id(self.folder)
        self.assertFalse((self.folder / 'VoicePort-macos-arm64.zip').exists())

    def test_resume_recovers_original_success_receipt_without_upload(self):
        receipt = 'notary-submit-original.json'
        release.write_new(self.folder / receipt, {'returncode': 0, 'stdout': json.dumps({'id': SUBMISSION})})
        release.record(self.folder, {'stage': 'notary-submit', 'state': 'completed', 'receipt': receipt})
        with patch.object(release, 'complete') as complete:
            release.resume(self.folder, 'profile', None)
        self.assertEqual(json.loads((self.folder / 'submission.json').read_text())['id'], SUBMISSION)
        complete.assert_called_once_with(self.folder, SUBMISSION, ['--keychain-profile', 'profile'])

    def test_resume_recovers_fsynced_receipt_before_completed_journal_event(self):
        operation = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
        release.record(self.folder, {'stage': 'notary-submit', 'state': 'intent', 'operation': operation})
        release.write_new(self.folder / f'notary-submit-{operation}.json',
                          {'returncode': 0, 'stdout': json.dumps({'id': SUBMISSION})})
        self.assertEqual(release.recover_id(self.folder), SUBMISSION)

    def test_invalid_or_wrong_id_never_staples_or_creates_final_zip(self):
        for status, identifier in [('Invalid', SUBMISSION), ('Accepted', 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa')]:
            with self.subTest(status=status), patch.object(release, 'run', return_value=json.dumps({'id': identifier, 'status': status})) as run:
                with self.assertRaises(ValueError):
                    release.complete(self.folder, SUBMISSION, ['--keychain-profile', 'profile'])
                self.assertEqual([call.args[1] for call in run.call_args_list], ['notary-info'])
        self.assertFalse((self.folder / 'release.json').exists())
        self.assertFalse((self.folder / 'VoicePort-macos-arm64.zip').exists())

    def test_accepted_submission_requires_ticket_and_gatekeeper_before_final_archive(self):
        calls = []
        def run(folder, stage, argv, **kwargs):
            calls.append(stage)
            if stage == 'notary-info':
                return json.dumps({'id': SUBMISSION, 'status': 'Accepted'})
            if stage == 'archive-final':
                (folder / 'VoicePort-macos-arm64.zip').write_bytes(b'stapled archive')
            return ''
        with patch.object(release, 'run', side_effect=run):
            release.complete(self.folder, SUBMISSION, ['--keychain-profile', 'profile'])
        self.assertEqual(calls, ['notary-info', 'staple', 'validate-ticket', 'gatekeeper', 'archive-final'])
        receipt = json.loads((self.folder / 'release.json').read_text())
        self.assertEqual(receipt['notarization_status'], 'Accepted')
        self.assertEqual(receipt['release_sha256'], release.sha(self.folder / 'VoicePort-macos-arm64.zip'))

    def test_gatekeeper_failure_prevents_distributable_zip(self):
        def run(folder, stage, argv, **kwargs):
            if stage == 'notary-info':
                return json.dumps({'id': SUBMISSION, 'status': 'Accepted'})
            if stage == 'gatekeeper':
                raise ValueError('rejected')
            return ''
        with patch.object(release, 'run', side_effect=run), self.assertRaisesRegex(ValueError, 'rejected'):
            release.complete(self.folder, SUBMISSION, [])
        self.assertFalse((self.folder / 'release.json').exists())
        self.assertFalse((self.folder / 'VoicePort-macos-arm64.zip').exists())

    def test_changed_candidate_cannot_be_submitted(self):
        release.write_new(self.folder / 'prepared.json', {'schema_version': 1, 'team_id': TEAM, 'archive_sha256': '0' * 64})
        # Use the actual implementation rather than the fixture patch for this case.
        with self.assertRaisesRegex(ValueError, 'archive changed'):
            ORIGINAL_CHECK(self.folder)

    def test_release_paths_reject_symlink_ancestors(self):
        alias = self.folder / 'linked'
        alias.symlink_to(self.folder, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, 'must not contain links'):
            release.real_path(alias / 'release')


ORIGINAL_CHECK = release.check_prepared

if __name__ == '__main__':
    unittest.main()
