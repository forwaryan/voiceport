"""Connection prerequisites: fixture metadata and verifier responses, never QQ code."""
import hashlib
import json
from pathlib import Path
import plistlib
import runpy
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

REPO = Path(__file__).resolve().parents[2]
CLI = REPO / 'scripts/qq_bridge/diagnose.py'
MODULE = runpy.run_path(str(CLI))
doctor = MODULE['doctor']


class ConnectionDoctor(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='qq-doctor-')
        self.root = Path(self.temporary.name).resolve()
        self.apps = self.root / 'Applications'
        self.apps.mkdir()
        self.app = self.make_app(self.apps)
        self.calls = []
        self.entitlements = {'com.apple.security.app-sandbox': True,
                             'PRIVATE_UNUSED_FIELD': 'PRIVATE_VALUE'}

    def tearDown(self):
        self.temporary.cleanup()

    def make_app(self, directory):
        app = directory / 'QQ.app'
        resources = app / 'Contents/Resources/app'
        resources.mkdir(parents=True)
        (app / 'Contents/Info.plist').write_bytes(plistlib.dumps({
            'CFBundleIdentifier': 'com.tencent.qq', 'CFBundleExecutable': 'QQ',
        }))
        (resources / 'package.json').write_text(json.dumps({
            'version': '7.0.1-52892', 'main': './original.cjs', 'isAppStore': True,
        }))
        # Even a file that throws must only be hashed, never executed.
        (resources / 'original.cjs').write_text("throw new Error('MUST_NEVER_EXECUTE');\n")
        signature = app / 'Contents/_CodeSignature'
        signature.mkdir()
        (signature / 'CodeResources').write_bytes(plistlib.dumps({'files2': {
            'Resources/app/' + name: {'hash2': hashlib.sha256((resources / name).read_bytes()).digest()}
            for name in ('package.json', 'original.cjs')
        }}))
        return app

    def verifier(self, args, **kwargs):
        self.calls.append((args, kwargs))
        self.assertEqual(args[0], '/usr/bin/codesign')
        self.assertEqual(Path(args[-1]), self.app)
        self.assertTrue(kwargs['capture_output'])
        self.assertLessEqual(kwargs['timeout'], 30)
        self.assertFalse(kwargs['check'])
        if '--verify' in args:
            self.assertTrue(args[args.index('-R') + 1].startswith('=anchor apple generic'))
            self.assertIn('identifier "com.tencent.qq"', args[args.index('-R') + 1])
            return subprocess.CompletedProcess(args, 0, b'', b'')
        self.assertIn('--verbose=4', args)
        return subprocess.CompletedProcess(args, 0, plistlib.dumps(self.entitlements),
                                           b'TeamIdentifier=FN2V63AD2J\nPRIVATE_STDERR')

    def inspect(self, **kwargs):
        return doctor(system='darwin', run=self.verifier, **kwargs)

    def codes(self, result):
        return [item['code'] for item in result['blockers']]

    def inventory(self):
        return {str(p.relative_to(self.root)): p.read_bytes()
                for p in self.root.rglob('*') if p.is_file()}

    def write_archive(self, node):
        resources = self.app / 'Contents/Resources/app'
        package = resources / 'package.json'
        metadata = json.loads(package.read_bytes())
        metadata['main'] = './application.asar/launcher/index.js'
        package.write_text(json.dumps(metadata))
        header = json.dumps(node).encode()
        padding = (-len(header)) % 4
        (resources / 'application.asar').write_bytes(
            struct.pack('<4I', 4, 8 + len(header) + padding,
                        4 + len(header) + padding, len(header)) + header + b'\0' * padding + b'x')

    def test_auto_discovery_reads_only_installation_and_never_claims_connection(self):
        before = self.inventory()
        result = self.inspect(applications_directories=[self.apps])
        self.assertEqual(result['app'], str(self.app))
        self.assertEqual(result['connection_status'], 'not_tested')
        self.assertFalse(result['transcription_verified'])
        self.assertFalse(result['automatic_setup_supported'])
        self.assertTrue(result['sandbox_enabled'])
        self.assertTrue(result['entry_resources_sealed'])
        self.assertIn('signature_preserving_route_unverified', self.codes(result))
        self.assertEqual(self.inventory(), before)
        self.assertNotIn('PRIVATE', json.dumps(result))
        self.assertEqual(len(self.calls), 2)

    def test_no_installation_and_unsupported_platform_do_not_call_verifier(self):
        result = self.inspect(applications_directories=[self.root / 'empty'])
        self.assertEqual(self.codes(result), ['qq_not_found'])
        for system in ('linux', 'win32'):
            result = doctor(self.app, system=system, run=self.verifier)
            self.assertEqual(self.codes(result), ['platform_unverified'])
        self.assertEqual(self.calls, [])

    def test_default_discovery_uses_only_system_and_current_user_applications(self):
        real_lexists = MODULE['os'].path.lexists
        searched = []
        def applications_only(path):
            searched.append(Path(path))
            return real_lexists(path) if Path(path) == self.app else False
        with mock.patch.object(Path, 'home', return_value=self.root), \
                mock.patch.object(MODULE['os'].path, 'lexists', side_effect=applications_only):
            result = self.inspect()
        self.assertEqual(searched, [Path('/Applications/QQ.app'), self.app])
        self.assertEqual(result['app'], str(self.app))

    def test_two_installations_require_explicit_choice(self):
        other = self.make_app(self.root / 'User Applications')
        result = self.inspect(applications_directories=[self.apps, other.parent])
        self.assertEqual(self.codes(result), ['multiple_qq_installations'])
        self.assertEqual(len(result['candidates']), 2)
        self.assertEqual(self.calls, [])
        self.assertEqual(self.inspect(app=self.app)['app'], str(self.app))

    def test_user_installation_and_spaces_need_no_developer_path(self):
        self.app = self.make_app(self.root / 'Clean User' / 'Applications')
        result = self.inspect(applications_directories=[self.root / 'absent', self.app.parent])
        self.assertEqual(result['app'], str(self.app))
        self.assertEqual(len(self.calls), 2)

    def test_duplicate_search_roots_do_not_create_false_ambiguity(self):
        self.assertNotIn('multiple_qq_installations', self.codes(
            self.inspect(applications_directories=[self.apps, self.apps])))

    def test_version_number_alone_does_not_allow_installation(self):
        result = self.inspect(app=self.app)
        self.assertEqual(result['qq_version'], '7.0.1-52892')
        self.assertFalse(result['historical_copy_build_match'])
        self.assertIn('qq_build_unverified', self.codes(result))
        self.assertFalse(result['automatic_setup_supported'])

    def test_historical_fingerprint_match_is_not_connectivity_or_setup_permission(self):
        known = {
            'version': '7.0.1-52892',
            'package_sha256': hashlib.sha256(
                (self.app / 'Contents/Resources/app/package.json').read_bytes()).hexdigest(),
            'info_sha256': hashlib.sha256(
                (self.app / 'Contents/Info.plist').read_bytes()).hexdigest(),
        }
        with mock.patch.dict(doctor.__globals__, {'KNOWN_COPY_BUILD': known}):
            result = self.inspect(app=self.app)
        self.assertTrue(result['historical_copy_build_match'])
        self.assertEqual(result['connection_status'], 'not_tested')
        self.assertFalse(result['transcription_verified'])
        self.assertFalse(result['automatic_setup_supported'])
        self.assertEqual(self.codes(result), ['signature_preserving_route_unverified'])

    def test_invalid_signature_does_not_read_entitlements_or_echo_verifier_output(self):
        calls = []
        def invalid(args, **kwargs):
            calls.append(args)
            return subprocess.CompletedProcess(args, 1, b'PRIVATE_VALUE', b'PRIVATE_STDERR')
        result = doctor(self.app, system='darwin', run=invalid)
        self.assertEqual(self.codes(result), ['official_signature_invalid'])
        self.assertEqual(len(calls), 1)
        self.assertNotIn('PRIVATE', json.dumps(result))

    def test_verifier_failure_and_timeout_are_structured(self):
        for error in (FileNotFoundError('PRIVATE_PATH'),
                      subprocess.TimeoutExpired('PRIVATE_COMMAND', 30)):
            def fail(*args, **kwargs):
                raise error
            result = doctor(self.app, system='darwin', run=fail)
            self.assertEqual(self.codes(result), ['signature_inspection_unavailable'])
            self.assertNotIn('PRIVATE', json.dumps(result))

    def test_apple_anchor_alone_does_not_accept_another_team(self):
        for metadata in (b'TeamIdentifier=OTHER_TEAM\n', b'',
                         b'TeamIdentifier=FN2V63AD2J\nTeamIdentifier=OTHER_TEAM\n'):
            def wrong_team(args, **kwargs):
                return subprocess.CompletedProcess(args, 0, plistlib.dumps(self.entitlements), metadata)
            result = doctor(self.app, system='darwin', run=wrong_team)
            self.assertEqual(self.codes(result), ['official_signature_invalid'])
            self.assertFalse(any(x['id'] == 'official_signature' for x in result['checks']))

    def test_invalid_entitlements_and_resources_do_not_become_success(self):
        for value in ([], {'com.apple.security.app-sandbox': 'PRIVATE_TRUE'}):
            self.entitlements = value
            result = self.inspect(app=self.app)
            self.assertIn('signature_inspection_unavailable', self.codes(result))
            self.assertNotIn('PRIVATE', json.dumps(result))
        self.entitlements = {}
        seal = self.app / 'Contents/_CodeSignature/CodeResources'
        for value in ([], {'files2': []}):
            seal.write_bytes(plistlib.dumps(value))
            self.assertIn('signature_inspection_unavailable', self.codes(self.inspect(app=self.app)))

    def test_sandbox_absence_is_explicit_and_never_automatic_setup(self):
        for value in ({}, {'com.apple.security.app-sandbox': False}):
            self.entitlements = value
            result = self.inspect(app=self.app)
            self.assertFalse(result['sandbox_enabled'])
            self.assertIn('sandbox_disabled', self.codes(result))
            self.assertFalse(result['automatic_setup_supported'])

    def test_resource_seal_checks_actual_entry_and_requires_a_digest(self):
        seal = self.app / 'Contents/_CodeSignature/CodeResources'
        digest = {'hash2': b'x' * 32}
        for entries in (
            {'Resources/app/package.json': digest, 'Resources/app/application.asar': digest},
            {'Resources/app/package.json': digest, 'Resources/app/original.cjs': {}},
            {'Resources/app/package.json': digest,
             'Resources/app/original.cjs': {**digest, 'omit': True}},
        ):
            seal.write_bytes(plistlib.dumps({'files2': entries}))
            result = self.inspect(app=self.app)
            self.assertFalse(result['entry_resources_sealed'])
            self.assertIn('entry_resources_unsealed', self.codes(result))
        seal.write_bytes(plistlib.dumps({'files': {
            name: b'x' * 20 for name in ('Resources/app/package.json', 'Resources/app/original.cjs')
        }}))
        self.assertTrue(self.inspect(app=self.app)['entry_resources_sealed'])

    def test_archive_entry_seal_uses_archive_resource(self):
        self.write_archive({'files': {'launcher': {'files': {
            'index.js': {'size': 1, 'offset': '0'},
        }}}})
        seal = self.app / 'Contents/_CodeSignature/CodeResources'
        seal.write_bytes(plistlib.dumps({'files2': {
            name: {'hash2': b'x' * 32}
            for name in ('Resources/app/package.json', 'Resources/app/application.asar')
        }}))
        self.assertTrue(self.inspect(app=self.app)['entry_resources_sealed'])

    def test_linked_or_corrupt_installation_is_bounded_and_does_not_execute(self):
        link = self.root / 'linked.app'
        link.symlink_to(self.app)
        self.assertEqual(self.codes(self.inspect(app=link)), ['qq_installation_unreadable'])
        (self.app / 'Contents/Resources/app/package.json').write_text('{broken PRIVATE_JSON')
        result = self.inspect(app=self.app)
        self.assertEqual(self.codes(result), ['qq_installation_unreadable'])
        self.assertNotIn('PRIVATE', json.dumps(result))
        self.assertEqual(self.calls, [])

    def test_malformed_plist_and_archive_shapes_are_structured(self):
        info = self.app / 'Contents/Info.plist'
        original = info.read_bytes()
        info.write_bytes(plistlib.dumps(['PRIVATE_VALUE']))
        result = self.inspect(app=self.app)
        self.assertEqual(self.codes(result), ['qq_installation_unreadable'])
        self.assertNotIn('PRIVATE', json.dumps(result))
        info.write_bytes(original)
        for node in ([], {'files': {'launcher': 'PRIVATE_VALUE'}},
                     {'files': {'launcher': {'files': {'index.js': []}}}},
                     {'files': {'launcher': {'files': {'index.js': {'size': float('inf'), 'offset': '0'}}}}}):
            self.write_archive(node)
            result = self.inspect(app=self.app)
            self.assertEqual(self.codes(result), ['qq_installation_unreadable'])
            self.assertNotIn('PRIVATE', json.dumps(result))
        self.assertEqual(self.calls, [])

    def test_signature_metadata_parent_links_are_not_followed(self):
        signature = self.app / 'Contents/_CodeSignature'
        external = self.root / 'external-signature'
        signature.rename(external)
        signature.symlink_to(external, target_is_directory=True)
        before = self.inventory()
        result = self.inspect(app=self.app)
        self.assertIn('signature_inspection_unavailable', self.codes(result))
        self.assertEqual(self.calls, [])
        self.assertEqual(self.inventory(), before)

    def test_untrusted_metadata_is_not_projected_and_version_is_bounded(self):
        package = self.app / 'Contents/Resources/app/package.json'
        metadata = json.loads(package.read_bytes())
        for version in ('PRIVATE_VERSION', '1' * 1000 + '-1', ['PRIVATE_VALUE']):
            package.write_text(json.dumps({**metadata, 'version': version,
                                          'buildVersion': 'PRIVATE_BUILD'}))
            result = self.inspect(app=self.app)
            self.assertIsNone(result['qq_version'])
            self.assertNotIn('PRIVATE', json.dumps(result))
            self.assertLess(len(json.dumps(result)), 4096)

    def test_oversized_verifier_metadata_is_not_projected(self):
        def oversized(args, **kwargs):
            return subprocess.CompletedProcess(args, 0, plistlib.dumps(self.entitlements),
                                               b'TeamIdentifier=FN2V63AD2J\n' + b'x' * 1_000_000)
        result = doctor(self.app, system='darwin', run=oversized)
        self.assertEqual(self.codes(result), ['signature_inspection_unavailable'])
        self.assertLess(len(json.dumps(result)), 4096)

    def test_unsigned_fixture_cli_reports_blocker_without_mutation(self):
        before = self.inventory()
        result = subprocess.run([sys.executable, '-I', str(CLI), 'doctor', '--qq', str(self.app)],
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, '')
        data = json.loads(result.stdout)['data']
        expected = 'official_signature_invalid' if sys.platform == 'darwin' else 'platform_unverified'
        self.assertEqual(self.codes(data), [expected])
        self.assertFalse(data['transcription_verified'])
        self.assertEqual(self.inventory(), before)


class BoundedVerifier(unittest.TestCase):
    def run_helper(self, script, timeout=2):
        return MODULE['run_codesign']([sys.executable, '-I', '-c', script], timeout=timeout)

    def test_both_pipes_are_drained_without_deadlock(self):
        result = self.run_helper("import os; os.write(1,b'o'*65536); os.write(2,b'e'*65536)")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b'o' * 65536)
        self.assertEqual(result.stderr, b'e' * 65536)

    def test_each_pipe_has_a_byte_bound(self):
        for stream in ('stdout', 'stderr'):
            with self.assertRaisesRegex(ValueError, '^verifier_output_limit$'):
                self.run_helper(f"import sys; sys.{stream}.buffer.write(b'x'*1000001)")

    def test_timeout_reaps_the_helper_and_preserves_no_raw_output(self):
        with self.assertRaises(subprocess.TimeoutExpired) as raised:
            self.run_helper('import time; time.sleep(5)', timeout=0.1)
        self.assertIsNone(raised.exception.output)
        self.assertIsNone(raised.exception.stderr)


if __name__ == '__main__':
    unittest.main()
