import io
from itertools import product
import json
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
from distribution_signing import verify_requirement
import macos_artifacts as artifacts


class ArtifactTests(unittest.TestCase):
    app_name = 'VoicePort Test.app'

    def setUp(self):
        scope = tempfile.TemporaryDirectory(prefix='voiceport-artifact-tests-')
        self.addCleanup(scope.cleanup)
        self.root = Path(scope.name)
        self.attachments = self.root / 'attachments'
        self.attachments.mkdir()

    def signed_app(self, *, test):
        app = self.root / (self.app_name if test else 'VoicePort.app')
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
        info = plistlib.loads((ROOT / 'resources/Info.plist').read_bytes())
        cli_name = artifacts.bundle.CLI
        cli_id = cli_name
        if test:
            info.update(CFBundleName=artifacts.bundle.TEST_NAME, CFBundleDisplayName=artifacts.bundle.TEST_NAME,
                        CFBundleIdentifier=artifacts.bundle.TEST_ID, CFBundleExecutable=artifacts.bundle.TEST_MAIN)
            cli_name = artifacts.bundle.TEST_CLI
            cli_id = artifacts.bundle.TEST_ID + '.cli'
        (contents / 'Info.plist').write_bytes(plistlib.dumps(info))
        main = executables / info['CFBundleExecutable']
        cli = executables / cli_name
        subprocess.run(['/usr/bin/clang', '-arch', 'arm64', '-x', 'c', '-', '-o', main],
                       input=b'int main(void) { return 0; }', check=True, capture_output=True)
        cli.write_bytes(main.read_bytes())
        cli.chmod(0o755)
        artifacts.codesign('--force', '--sign', '-', '--identifier', cli_id, cli)
        artifacts.codesign('--force', '--sign', '-', app)
        return app, info, main, cli

    def test_real_ad_hoc_signature_and_late_archive_tampering(self):
        app, info, main, cli = self.signed_app(test=True)
        resources = app / 'Contents/Resources'
        subprocess.run(['/usr/bin/xattr', '-w', 'io.voiceport.synthetic', 'fixture',
                        resources / 'AppIcon.icns'], check=True)
        archive = self.attachments / artifacts.TEST_ARCHIVE
        attachment = self.attachments / 'receipt.json'
        attachment.write_text('{}\n')

        def package():
            archive.unlink(missing_ok=True)
            (self.attachments / artifacts.MANIFEST).unlink(missing_ok=True)
            subprocess.run(['/usr/bin/ditto', '-c', '-k', '--sequesterRsrc', '--keepParent', app, archive], check=True)
            result = subprocess.check_output([sys.executable, '-I', str(ROOT / 'scripts/macos_artifacts.py'),
                                              'manifest', self.attachments, '--archive', archive.name,
                                              '--attachment', attachment.name])
            return json.loads(result)['manifest_sha256']

        def archive_mode(binary):
            with zipfile.ZipFile(archive) as final_zip:
                entry = final_zip.getinfo(binary.relative_to(app.parent).as_posix())
                self.assertIn(entry.create_system, (3, 19))
                return stat.S_IMODE(entry.external_attr >> 16)

        digest = package()
        self.assertEqual(archive_mode(main), 0o755)
        self.assertEqual(archive_mode(cli), 0o755)
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

        # Keep the signature bytes unchanged and replace the archive manifest.
        # Group and other users retain execute permission, while the owner lacks it.
        for binary, role in [(main, 'main'), (cli, 'CLI')]:
            with self.subTest(binary=binary.name):
                binary.chmod(0o655)
                digest = package()
                self.assertEqual(archive_mode(binary), 0o655)
                with self.assertRaisesRegex(AssertionError, f'{role} must be executable mode 0755'):
                    artifacts.verify(self.attachments, digest, **params)
                binary.chmod(0o755)
        digest = package()
        self.assertEqual(artifacts.verify(self.attachments, digest, **params)['status'], 'verified')

        # Recompute digests after damage to prove the final ZIP is itself verified.
        (resources / 'AppIcon.icns').write_text('changed after signing')
        digest = package()
        with self.assertRaises(subprocess.CalledProcessError):
            artifacts.verify(self.attachments, digest, **params)

    def test_zip_checks_recorded_directory_modes_in_either_order(self):
        name = f'{self.app_name}/Contents/'
        child = name + 'Info.plist'
        for system, mode, directory_first in product((3, 19), (0o655, 0o355, 0o550), (True, False)):
            with self.subTest(system=system, mode=oct(mode), directory_first=directory_first):
                entry = zipfile.ZipInfo(name)
                entry.create_system = system
                entry.external_attr = (stat.S_IFDIR | mode) << 16 | 0x10
                members = [(entry, b''), (child, b'fixture')]
                if not directory_first:
                    members.reverse()
                data = io.BytesIO()
                with zipfile.ZipFile(data, 'w') as archive:
                    for member, content in members:
                        archive.writestr(member, content)
                with tempfile.TemporaryDirectory(dir=self.root) as extraction:
                    if mode == 0o550:
                        app = artifacts.extract_zip(data.getvalue(), Path(extraction), self.app_name)
                        directory = app / 'Contents'
                        self.assertEqual(stat.S_IMODE(directory.stat().st_mode), mode)
                        self.assertEqual((directory / 'Info.plist').read_bytes(), b'fixture')
                    else:
                        with self.assertRaisesRegex(ValueError, 'directory must be readable and searchable'):
                            artifacts.extract_zip(data.getvalue(), Path(extraction), self.app_name)

    def test_real_codesign_evaluates_inline_requirements(self):
        app, info, _, _ = self.signed_app(test=False)
        verify_requirement(app, f'identifier "{info["CFBundleIdentifier"]}"')
        with self.assertRaises(subprocess.CalledProcessError) as mismatch:
            verify_requirement(app, 'identifier "io.voiceport.synthetic.other"')
        self.assertIn('failed to satisfy specified code requirement', mismatch.exception.stderr)

        version = info['CFBundleShortVersionString']
        build = info['CFBundleVersion']
        for signature, identity in [('preview', {'certificate': 'A' * 40}),
                                    ('distribution', {'team_id': 'ABCD123456'})]:
            with self.subTest(signature=signature):
                with self.assertRaises(subprocess.CalledProcessError) as mismatch:
                    artifacts.verify_app(app, version=version, build=build, signature=signature, **identity)
                self.assertIn('failed to satisfy specified code requirement', mismatch.exception.stderr)

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
