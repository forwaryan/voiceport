import json
import pathlib
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import install_app


class InstallTests(unittest.TestCase):
    def test_linked_ancestor_and_unstable_name_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp).resolve()
            (root / 'real').mkdir()
            (root / 'linked').symlink_to(root / 'real')
            with self.assertRaisesRegex(ValueError, 'linked install path'):
                install_app.real_path(root / 'linked/VoicePort.app')
            with self.assertRaisesRegex(ValueError, 'stable application name'):
                install_app.install(root / 'source.app', root / 'VoicePort-preview.app')

    def test_busy_app_never_quits(self):
        current = {'pid': 42, 'busy': True, 'pending': False, 'operation': None}
        with patch.object(install_app, 'running_status', return_value=current), \
             patch.object(install_app.subprocess, 'check_output', return_value='42 /test/Contents/MacOS/voiceport-app\n'), \
             patch.object(install_app.subprocess, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'unfinished work'):
                install_app.stop_idle(pathlib.Path('cli'), True)
            run.assert_not_called()

    def test_unobserved_process_blocks_install(self):
        with patch.object(install_app, 'running_status', return_value=None), \
             patch.object(install_app.subprocess, 'check_output', return_value='42 /test/Contents/MacOS/voiceport-app\n'):
            with self.assertRaisesRegex(ValueError, 'no matching IPC'):
                install_app.stop_idle(pathlib.Path('cli'), True)

    def test_install_preserves_backup_and_rejects_signing_change(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp).resolve()
            source, target = root / 'new.app', root / 'Applications/VoicePort.app'
            source.mkdir()
            (source / 'version').write_text('new')
            target.mkdir(parents=True)
            (target / 'version').write_text('old')
            with patch.object(install_app, 'bundle_info'), \
                 patch.object(install_app, 'signing_continues', return_value=False), \
                 patch.object(install_app, 'stop_idle') as stop:
                with self.assertRaisesRegex(ValueError, 'signing requirement'):
                    install_app.install(source, target)
                stop.assert_not_called()
                self.assertEqual((target / 'version').read_text(), 'old')
                result = install_app.install(source, target, allow_signing_change=True)
                self.assertEqual((target / 'version').read_text(), 'new')
                self.assertEqual((pathlib.Path(result['backup']) / 'version').read_text(), 'old')
                self.assertFalse((target.parent / '.voiceport-install.lock').exists())

    def test_failed_replacement_restores_old_bundle(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp).resolve()
            source, target = root / 'new.app', root / 'Applications/VoicePort.app'
            source.mkdir()
            target.mkdir(parents=True)
            (target / 'version').write_text('old')
            rename = pathlib.Path.rename

            def fail_staged(path, destination):
                if path.parent.name.startswith('.voiceport-update-'):
                    raise OSError('synthetic replacement failure')
                return rename(path, destination)

            with patch.object(install_app, 'bundle_info'), \
                 patch.object(install_app, 'signing_continues', return_value=True), \
                 patch.object(install_app, 'stop_idle'), \
                 patch.object(pathlib.Path, 'rename', fail_staged):
                with self.assertRaisesRegex(OSError, 'synthetic replacement'):
                    install_app.install(source, target)
                self.assertEqual((target / 'version').read_text(), 'old')

    def test_real_designated_requirement_detects_changed_binary_identity(self):
        # Uses an inert signed bundle and never launches or requests permissions.
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp).resolve()
            app = root / 'VoicePort.app'
            (app / 'Contents/MacOS').mkdir(parents=True)
            info = {'CFBundleIdentifier': install_app.BUNDLE_ID, 'CFBundleExecutable': 'voiceport-app',
                    'CFBundleVersion': '1', 'CFBundlePackageType': 'APPL'}
            (app / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
            shutil.copyfile('/usr/bin/true', app / 'Contents/MacOS/voiceport-app')
            (app / 'Contents/MacOS/voiceport-app').chmod(0o755)
            subprocess.run(['codesign', '--force', '--sign', '-', str(app)], check=True, capture_output=True)
            other = root / 'other/VoicePort.app'
            shutil.copytree(app, other)
            self.assertTrue(install_app.signing_continues(app, other))
            info['CFBundleVersion'] = '2'
            (other / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
            subprocess.run(['codesign', '--force', '--sign', '-', str(other)], check=True, capture_output=True)
            self.assertFalse(install_app.signing_continues(app, other))
