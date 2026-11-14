"""Offline installer/lifecycle controls; no QQ, network, signing or account access."""
import base64
import hashlib
import importlib.util
import io
import json
from pathlib import Path
from types import SimpleNamespace
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / 'scripts/qq_bridge/setup.py'
spec = importlib.util.spec_from_file_location('qq_setup_fixture', SCRIPT)
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)


def archive(extra=None):
    stream = io.BytesIO()
    files = {'package/package.json': json.dumps({'name': 'silk-wasm', 'version': '3.7.1'}).encode(),
             'package/LICENSE': b'synthetic license', 'package/lib/index.cjs': b'synthetic code; never run',
             'package/lib/silk.wasm': b'synthetic opaque bytes; never load'}
    with tarfile.open(fileobj=stream, mode='w:gz') as handle:
        for name, content in files.items():
            member = tarfile.TarInfo(name)
            member.size = len(content)
            handle.addfile(member, io.BytesIO(content))
        if extra:
            handle.addfile(extra, io.BytesIO(b'x') if extra.isfile() else None)
    return stream.getvalue()


class SetupTests(unittest.TestCase):
    def test_encoder_rejects_unverified_archive_before_writing(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            source = root / 'untrusted.tgz'
            source.write_bytes(archive())
            with self.assertRaisesRegex(RuntimeError, 'integrity_mismatch'):
                setup.materialize_codec(root / 'output', source)
            self.assertFalse((root / 'output').exists())

    def test_verified_archive_materializes_only_reviewed_runtime_and_license(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            data = archive()
            source = root / 'fixture.tgz'
            source.write_bytes(data)
            with patch.object(setup, 'INTEGRITY', base64.b64encode(hashlib.sha512(data).digest()).decode()):
                setup.materialize_codec(root / 'output', source)
            self.assertEqual(sorted(str(p.relative_to(root / 'output')) for p in (root / 'output').rglob('*') if p.is_file()),
                             ['LICENSE', 'lib/index.cjs', 'lib/silk.wasm', 'package.json'])
            self.assertEqual((root / 'output/lib/index.cjs').stat().st_mode & 0o777, 0o600)

    def test_even_checksum_verified_links_traversal_and_duplicates_are_rejected(self):
        link = tarfile.TarInfo('package/link')
        link.type = tarfile.SYMTYPE
        link.linkname = '/outside'
        traversal = tarfile.TarInfo('package/../../escape')
        traversal.size = 1
        duplicate = tarfile.TarInfo('package/LICENSE')
        duplicate.size = 1
        for entry in [link, traversal, duplicate]:
            with self.subTest(entry=entry.name), tempfile.TemporaryDirectory() as root:
                root = Path(root)
                data = archive(entry)
                source = root / 'fixture.tgz'
                source.write_bytes(data)
                with patch.object(setup, 'INTEGRITY', base64.b64encode(hashlib.sha512(data).digest()).decode()):
                    with self.assertRaises(RuntimeError):
                        setup.materialize_codec(root / 'output', source)
                self.assertFalse((root / 'output').exists())

    def test_unresolved_launch_blocks_another_spawn(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            setup.write_json(root / 'launch.json', {'stage': 'launch_attempt'})
            plan = {'app': '/fixture/OwnedQQ.app', 'original_app': '/fixture/OriginalQQ.app'}
            with patch.object(setup, 'fingerprint', return_value=setup.ORIGINAL_FILES), \
                 patch.object(setup, 'signature'), patch.object(setup, 'processes', return_value=[]), \
                 patch.object(setup.subprocess, 'Popen') as launch:
                with self.assertRaisesRegex(RuntimeError, 'previous_launch_requires_status_and_stop'):
                    setup.start(SimpleNamespace(output=root, replace_qq=False), plan, {})
                launch.assert_not_called()

    def test_stop_signals_only_owned_copy_and_leaves_original_running(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            tasks = root / 'tasks'
            tasks.mkdir(mode=0o700)
            plan = {'app': '/fixture/OwnedQQ.app', 'original_app': '/fixture/OriginalQQ.app'}
            config = {'socketPath': str(root / 'missing.sock'), 'tasksDirectory': str(tasks)}
            own = {'pid': 123, 'executable': '/fixture/OwnedQQ.app/Contents/MacOS/QQ', 'parent': 1}
            original = {'pid': 456, 'executable': '/fixture/OriginalQQ.app/Contents/MacOS/QQ', 'parent': 1}
            rows = [own, original]
            signals = []
            def kill(pid, sig):
                signals.append(pid)
                rows[:] = [row for row in rows if row['pid'] != pid]
            with patch.object(setup, 'processes', side_effect=lambda _: list(rows)), \
                 patch.object(setup, 'identity', return_value='exact fixture identity'), \
                 patch.object(setup.os, 'kill', side_effect=kill), \
                 patch.object(setup, 'fingerprint', return_value=setup.ORIGINAL_FILES):
                report = setup.stop(SimpleNamespace(output=root, restore_qq=False), plan, config)
            self.assertEqual(signals, [123])
            self.assertEqual(rows, [original])
            self.assertEqual(report['status'], 'stopped')
            self.assertEqual(json.loads((root / 'launch.json').read_bytes())['stage'], 'stopped')


if __name__ == '__main__':
    unittest.main()
