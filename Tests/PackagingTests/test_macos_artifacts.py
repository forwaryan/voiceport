import io
import os
from pathlib import Path
import plistlib
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import warnings
import zipfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import macos_artifacts as artifacts


class ArtifactTests(unittest.TestCase):
    app_name = 'VoicePort Test.app'

    def setUp(self):
        scope = tempfile.TemporaryDirectory(prefix='voiceport-artifact-tests-')
        self.addCleanup(scope.cleanup)
        self.root = Path(scope.name)
        self.attachments = self.root / 'attachments'
        self.attachments.mkdir()

    def test_real_ad_hoc_signature_and_late_archive_tampering(self):
        app = self.root / self.app_name
        contents = app / 'Contents'
        executables = contents / 'MacOS'
        executables.mkdir(parents=True)
        resources = contents / 'Resources'
        resources.mkdir()
        for name in ['AppIcon.icns', 'THIRD_PARTY_NOTICES.md']:
            (resources / name).write_text('synthetic')
        for name in ['skills', 'third-party']:
            (resources / name).mkdir()
            (resources / name / 'fixture').write_text('synthetic')
        (contents / 'Info.plist').write_bytes(artifacts.bundle.expected_info(test=True))
        info = plistlib.loads((contents / 'Info.plist').read_bytes())
        main, cli = (executables / name for name in (artifacts.bundle.TEST_MAIN, artifacts.bundle.TEST_CLI))
        subprocess.run(['/usr/bin/clang', '-arch', 'arm64', '-x', 'c', '-', '-o', main],
                       input=b'int main(void) { return 0; }', check=True, capture_output=True)
        cli.write_bytes(main.read_bytes())
        cli.chmod(0o755)
        artifacts.codesign('--force', '--sign', '-', '--identifier', artifacts.bundle.TEST_ID + '.cli', cli)
        artifacts.codesign('--force', '--sign', '-', app)
        subprocess.run(['/usr/bin/xattr', '-w', 'io.voiceport.synthetic', 'fixture',
                        resources / 'AppIcon.icns'], check=True)
        archive = self.attachments / artifacts.TEST_ARCHIVE
        attachment = self.attachments / 'receipt.json'
        attachment.write_text('{}\n')

        def package():
            archive.unlink(missing_ok=True)
            (self.attachments / artifacts.MANIFEST).unlink(missing_ok=True)
            subprocess.run(['/usr/bin/ditto', '-c', '-k', '--sequesterRsrc', '--keepParent', app, archive], check=True)
            return artifacts.write_manifest(self.attachments, archive.name, [attachment.name])

        digest = package()
        params = dict(version=info['CFBundleShortVersionString'], build=info['CFBundleVersion'], signature='test')
        command = [sys.executable, '-I', str(ROOT / 'scripts/macos_artifacts.py'), 'verify', str(self.attachments),
                   '--manifest-sha256', digest, '--version', params['version'], '--build', params['build'], '--signature', 'test']
        result = subprocess.run(command, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(artifacts.verify(self.attachments, digest, **params)['status'], 'verified')
        with self.assertRaisesRegex(ValueError, 'CFBundleVersion mismatch'):
            artifacts.verify(self.attachments, digest, **dict(params, build='999'))
        with self.assertRaisesRegex(ValueError, 'CFBundleShortVersionString mismatch'):
            artifacts.verify(self.attachments, digest, **dict(params, version='99.9.9'))
        with self.assertRaisesRegex(ValueError, 'manifest SHA-256 mismatch'):
            artifacts.verify(self.attachments, '0' * 64, **params)
        extra = self.attachments / 'unexpected.txt'
        extra.write_text('extra')
        with self.assertRaisesRegex(ValueError, 'missing or unexpected'):
            artifacts.verify(self.attachments, digest, **params)
        extra.unlink()
        attachment.write_text('[]\n')
        with self.assertRaisesRegex(ValueError, 'attachment SHA-256 mismatch'):
            artifacts.verify(self.attachments, digest, **params)
        attachment.unlink()
        with self.assertRaisesRegex(ValueError, 'missing or unexpected'):
            artifacts.verify(self.attachments, digest, **params)
        attachment.write_text('{}\n')
        archive.write_bytes(archive.read_bytes() + b'!')
        with self.assertRaisesRegex(ValueError, 'attachment size mismatch'):
            artifacts.verify(self.attachments, digest, **params)

        # Recompute digests after damage to prove the final ZIP is itself verified.
        (resources / 'AppIcon.icns').write_text('changed after signing')
        digest = package()
        with self.assertRaises(subprocess.CalledProcessError):
            artifacts.verify(self.attachments, digest, **params)

    def test_zip_rejects_foreign_roots_aliases_and_unsafe_entries(self):
        expected = self.app_name + '/Contents/Info.plist'
        link = zipfile.ZipInfo(self.app_name + '/link')
        link.create_system = 3
        link.external_attr = (stat.S_IFLNK | 0o777) << 16
        for additional in ['another.app/x', self.app_name + '/../escape', expected, expected.lower(),
                           self.app_name + '/contents/extra', '__MACOSX/._other.app', link]:
            with self.subTest(member=additional), warnings.catch_warnings():
                warnings.simplefilter('ignore', UserWarning)
                data = io.BytesIO()
                with zipfile.ZipFile(data, 'w') as archive:
                    archive.writestr(expected, b'fixture')
                    archive.writestr(additional, b'bad')
                with tempfile.TemporaryDirectory(dir=self.root) as extraction:
                    with self.assertRaises(ValueError):
                        artifacts.extract_zip(data.getvalue(), Path(extraction), self.app_name)
                    self.assertFalse((Path(extraction) / 'escape').exists())

    def test_unapproved_attachments_and_signing_flags_fail_before_verification(self):
        zip_path = self.attachments / artifacts.TEST_ARCHIVE
        zip_path.write_bytes(b'fixture')
        linked = self.attachments / 'linked.json'
        linked.symlink_to(zip_path)
        with self.assertRaises(OSError):
            artifacts.write_manifest(self.attachments, zip_path.name, [linked.name])
        self.assertFalse((self.attachments / artifacts.MANIFEST).exists())
        linked.unlink()
        digest = artifacts.write_manifest(self.attachments, zip_path.name, [])
        with patch.object(artifacts, 'extract_zip') as extract:
            for signature, certificate, team in [('preview', None, None), ('test', None, 'A' * 10),
                                                  ('distribution', 'A' * 40, 'A' * 10)]:
                with self.subTest(signature=signature), self.assertRaises(ValueError):
                    artifacts.verify(self.attachments, digest, version='0.3.16', build='22',
                                     signature=signature, certificate=certificate, team_id=team)
            extract.assert_not_called()

    def test_test_and_distribution_are_mutually_exclusive(self):
        result = subprocess.run(['/bin/bash', str(ROOT / 'scripts/package_app.sh'), '--test', '--distribution'],
                                env=dict(os.environ, VOICEPORT_APP_DIRECTORY=str(self.root / self.app_name)),
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn('mutually exclusive', result.stderr)
        self.assertFalse((self.root / self.app_name).exists())
