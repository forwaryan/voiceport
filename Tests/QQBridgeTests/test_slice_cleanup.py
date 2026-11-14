"""Offline cleanup of abandoned numbered slice files; never opens QQ."""
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / 'scripts/qq_bridge/setup.py'
spec = importlib.util.spec_from_file_location('slice_cleanup_setup', SCRIPT)
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)


class SliceCleanupTests(unittest.TestCase):
    def stop(self, root):
        plan = {'app': '/fixture/OwnedQQ.app', 'original_app': '/fixture/OriginalQQ.app'}
        config = {'socketPath': str(root / 'missing.sock'), 'tasksDirectory': str(root / 'tasks')}
        with patch.object(setup, 'processes', return_value=[]), \
             patch.object(setup, 'fingerprint', return_value=setup.ORIGINAL_FILES):
            return setup.stop(SimpleNamespace(output=root, restore_qq=False), plan, config)

    def task(self, root):
        tasks = root / 'tasks'
        tasks.mkdir(mode=0o700)
        directory = tasks / 'task-owned'
        directory.mkdir(mode=0o700)
        return directory

    def test_dead_owned_task_cleans_old_and_numbered_slices(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            directory = self.task(root)
            for filename in ['input.silk', 'chunk_0000.silk', 'chunk_0001.silk', 'chunk_0044.silk']:
                (directory / filename).write_bytes(b'owned synthetic bytes')
            self.assertEqual(self.stop(root)['status'], 'stopped')
            self.assertEqual(list((root / 'tasks').iterdir()), [])

    def test_unknown_filename_preserves_task_and_blocks_cleanup(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            directory = self.task(root)
            target = directory / 'chunk_00000.silk'
            target.write_bytes(b'unknown file')
            with self.assertRaisesRegex(RuntimeError, 'unexpected_task_file'):
                self.stop(root)
            self.assertEqual(target.read_bytes(), b'unknown file')

    def test_symlink_slice_never_deletes_its_destination(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            directory = self.task(root)
            outside = root / 'outside'
            outside.write_bytes(b'preserve me')
            (directory / 'chunk_0000.silk').symlink_to(outside)
            with self.assertRaisesRegex(RuntimeError, 'unexpected_task_file'):
                self.stop(root)
            self.assertEqual(outside.read_bytes(), b'preserve me')
            self.assertTrue((directory / 'chunk_0000.silk').is_symlink())


if __name__ == '__main__':
    unittest.main()
