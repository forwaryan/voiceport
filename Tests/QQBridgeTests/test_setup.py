"""Offline installer/lifecycle controls; no QQ, network, signing or account access."""
import base64
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import select
import shutil
import socket
import subprocess
import sys
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
    def test_prepare_rejects_output_inside_original_before_any_side_effect(self):
        with tempfile.TemporaryDirectory() as root:
            source = Path(root).resolve() / 'QQ.app'
            source.mkdir()
            marker = source / 'original-file'
            marker.write_bytes(b'original signed installation fixture')
            for relative in ('prepared', 'Contents/Resources/app/prepared'):
                output = source / relative
                with self.subTest(output=relative), patch.object(setup, 'QQ_APP', source), \
                     patch.object(setup.sys, 'platform', 'darwin'), \
                     patch.object(setup, 'fingerprint') as fingerprint, \
                     patch.object(setup, 'signature') as signature, \
                     patch.object(setup.subprocess, 'run') as run, \
                     patch.object(setup, 'write_json') as write:
                    with self.assertRaisesRegex(RuntimeError, 'output_must_be_outside_qq_installation'):
                        setup.prepare(SimpleNamespace(output=output, qq_app=source))
                    fingerprint.assert_not_called()
                    signature.assert_not_called()
                    run.assert_not_called()
                    write.assert_not_called()
                self.assertFalse(output.exists())
                self.assertEqual(list(source.iterdir()), [marker])
                self.assertEqual(marker.read_bytes(), b'original signed installation fixture')

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


class StatusTests(unittest.TestCase):
    MAIN = '/fixture/OwnedQQ.app/Contents/MacOS/QQ'
    HELPER = '/fixture/OwnedQQ.app/Contents/Frameworks/QQ Helper.app/Contents/MacOS/QQ Helper'
    ORIGINAL = '/fixture/OriginalQQ.app/Contents/MacOS/QQ'

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='vpqq-test-', dir='/tmp')
        self.addCleanup(temporary.cleanup)
        self.output = Path(temporary.name).resolve()
        self.socket = self.output / 'bridge.sock'
        self.admin_token = 'a' * 64
        config = self.output / 'config.json'
        setup.write_json(config, {'socketPath': str(self.socket), 'adminToken': self.admin_token})
        setup.write_json(self.output / 'installation.json', {
            'app': '/fixture/OwnedQQ.app', 'original_app': '/fixture/OriginalQQ.app',
            'config': str(config), 'runtime': str(self.output),
        })

    def status(self, *executables):
        # Replace only the OS process inventory; run the CLI and socket path as shipped.
        inventory = '\n'.join(f'{pid} 1 {exe}' for pid, exe in enumerate(executables, start=1))
        launcher = '''
import runpy, subprocess, sys
from unittest.mock import patch
inventory, *sys.argv = sys.argv[1:]

def ps(args, **kwargs):
    assert args == ['/bin/ps', '-ww', '-axo', 'pid=,ppid=,comm='], args
    return subprocess.CompletedProcess(args, 0, inventory, '')

with patch.object(subprocess, 'run', side_effect=ps):
    runpy.run_path(sys.argv[0], run_name='__main__')
'''
        return subprocess.run([sys.executable, '-I', '-B', '-c', launcher, inventory,
                               str(SCRIPT), '--output', str(self.output), 'status'],
                              capture_output=True, text=True, timeout=8)

    def assert_status_error(self, result, code, *executables):
        self.assertEqual(result.returncode, 1)
        self.assertTrue(result.stdout, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual([row['executable'] for row in report['processes']], list(executables))
        self.assertIsNone(report['bridge'])
        self.assertEqual(report['bridge_error'], code)
        self.assertNotIn(self.admin_token, result.stdout + result.stderr)

    def test_exited_copy_with_helpers_preserves_processes(self):
        result = self.status(self.ORIGINAL, self.HELPER)
        self.assert_status_error(result, 'bridge_main_not_running', self.HELPER)
        result = self.status(self.ORIGINAL)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {'processes': [], 'bridge': None})

    def test_running_copy_reports_missing_and_refused_sockets(self):
        self.assert_status_error(self.status(self.MAIN), 'bridge_socket_missing', self.MAIN)
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(self.socket))
            self.assert_status_error(self.status(self.MAIN), 'bridge_status_unavailable', self.MAIN)

    def test_status_rejects_invalid_readiness_and_reports_timeout(self):
        plan = {'app': '/fixture/OwnedQQ.app'}
        main = {'pid': 1, 'parent': 1, 'executable': self.MAIN}
        with patch.object(setup, 'processes', return_value=[main]), \
             patch.object(setup, 'control') as control:
            control.return_value = {'type': 'status', 'ready': 'false', 'busy': False, 'cleanup_failed': False}
            self.assertEqual(setup.status(plan, {}), {
                'processes': [main], 'bridge': None, 'bridge_error': 'bridge_status_invalid_response',
            })
            control.side_effect = TimeoutError
            self.assertEqual(setup.status(plan, {}), {
                'processes': [main], 'bridge': None, 'bridge_error': 'bridge_status_timeout',
            })

    @unittest.skipUnless(shutil.which('node'), 'requires Node for the real bridge fixture')
    def test_status_reads_the_real_bridge_before_and_after_synthetic_service(self):
        harness = '''
const { createBridge } = require(process.argv[1]);
const [socketPath, dataDirectory] = process.argv.slice(2);
const bridge = createBridge({ socketPath, dataDirectory,
  token: 'b'.repeat(64), adminToken: 'a'.repeat(64) });
bridge.ready.then(() => {
  console.log('listening');
  process.stdin.once('data', () => {
    bridge.setService({ uploadSmartPttSlice() { throw Error('unexpected upload'); },
      addKernelBdhUploadListener() {}, removeKernelBdhUploadListener() {} });
    console.log('service');
  });
  process.stdin.once('end', bridge.stop);
});
'''
        with subprocess.Popen(['node', '-e', harness, str(SCRIPT.with_name('bridge.cjs')),
                               str(self.socket), str(self.output)],
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE) as server:
            try:
                for marker, ready in [(b'listening\n', False), (b'service\n', True)]:
                    self.assertTrue(select.select([server.stdout], [], [], 5)[0], 'bridge fixture stalled')
                    self.assertEqual(server.stdout.readline(), marker)
                    result = self.status(self.MAIN)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    report = json.loads(result.stdout)
                    self.assertNotIn('bridge_error', report)
                    self.assertEqual(report['bridge'], {
                        'type': 'status', 'ready': ready, 'busy': False, 'cleanup_failed': False,
                        'submitted': 0, 'completed': 0, 'failed': 0, 'abandoned': 0,
                    })
                    if ready:
                        self.assert_status_error(self.status(self.HELPER), 'bridge_main_not_running', self.HELPER)
                    else:
                        server.stdin.write(b'set service\n')
                        server.stdin.flush()
            finally:
                server.stdin.close()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()
                    self.fail('bridge fixture did not exit')
        self.assertEqual(server.returncode, 0)


if __name__ == '__main__':
    unittest.main()
