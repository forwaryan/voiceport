import hashlib
import json
from pathlib import Path
import plistlib
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
CLI = REPO / 'scripts/qq_bridge/diagnose.py'
NODE = shutil.which('node')


class DiagnosticCLI(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='voiceport-qq-cli-')
        self.root = Path(self.temporary.name).resolve()
        self.app = self.root / 'QQ.app'
        self.resources = self.app / 'Contents/Resources/app'
        self.resources.mkdir(parents=True)
        self.package = {
            'name': 'qq-chat', 'version': '7.0.1-52892', 'buildVersion': '52892',
            'main': './application.asar/app_launcher/index.js',
            'isAppStore': True, 'isByteCodeShell': True,
        }
        self.write_package()
        (self.app / 'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable': 'QQ'}))
        (self.resources / 'wrapper.node').write_bytes(b'synthetic placeholder, never executed')
        content = b"module.exports = 'synthetic launcher';\n"
        header = json.dumps({'files': {'app_launcher': {'files': {
            'index.js': {'size': len(content), 'offset': '0'},
        }}}}).encode()
        padding = (-len(header)) % 4
        (self.resources / 'application.asar').write_bytes(
            struct.pack('<4I', 4, 8 + len(header) + padding,
                        4 + len(header) + padding, len(header)) + header + b'\0' * padding + content)

    def tearDown(self):
        self.temporary.cleanup()

    def write_package(self):
        (self.resources / 'package.json').write_text(json.dumps(self.package))

    def run_cli(self, *arguments, expect=0):
        result = subprocess.run([sys.executable, '-I', str(CLI), *map(str, arguments)],
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, expect, result.stdout + result.stderr)
        self.assertEqual(result.stderr, '')
        return json.loads(result.stdout)

    def inventory(self):
        return {str(p.relative_to(self.app)): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in self.app.rglob('*') if p.is_file()}

    def test_inspect_reads_archive_without_execution_or_mutation(self):
        before = self.inventory()
        result = self.run_cli('inspect', '--qq', self.app)['data']
        self.assertFalse(result['runtime_verified'])
        self.assertFalse(result['smartptt_verified'])
        self.assertTrue(result['app_store'])
        self.assertEqual(result['qq_version'], '7.0.1-52892')
        self.assertEqual(result['entry_sha256'], hashlib.sha256(
            b"module.exports = 'synthetic launcher';\n").hexdigest())
        self.assertEqual(self.inventory(), before)

    def test_prepare_is_private_and_reversible_proposal_only(self):
        before = self.inventory()
        output = self.root / 'prepared'
        plan = self.run_cli('prepare', '--qq', self.app, '--output', output)['data']
        self.assertFalse(plan['installed'])
        self.assertTrue(plan['requires_qq_restart'])
        self.assertFalse(plan['signing_and_sandbox_verified'])
        self.assertEqual((output.stat().st_mode & 0o777), 0o700)
        self.assertEqual((output / 'original-package.json').read_bytes(),
                         (self.resources / 'package.json').read_bytes())
        proposed = json.loads((output / 'proposed-package.json').read_text())
        self.assertEqual(proposed, {**self.package, 'main': './voiceport-qq-diagnostic/bootstrap.cjs'})
        for path in output.rglob('*'):
            if path.is_file():
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.inventory(), before)
        self.run_cli('prepare', '--qq', self.app, '--output', output, expect=1)
        self.assertEqual(self.inventory(), before)

    def test_prepare_rejects_destinations_inside_the_app_before_writing(self):
        before = self.inventory()
        for output in (self.app / 'prepared', self.resources / 'prepared'):
            self.run_cli('prepare', '--qq', self.app, '--output', output, expect=1)
            self.assertFalse(output.exists())
            self.assertEqual(self.inventory(), before)

    @unittest.skipUnless(NODE, 'Node required to exercise bootstrap')
    def test_bootstrap_preserves_main_launcher_guard_and_argv(self):
        self.package['main'] = './guarded.cjs'
        self.write_package()
        launcher = self.resources / 'guarded.cjs'
        launcher.write_text("if (require.main === module) console.log('ORIGINAL_MAIN:' + process.argv[1]);\n")
        output = self.root / 'prepared-main'
        self.run_cli('prepare', '--qq', self.app, '--output', output)
        copied = self.resources / 'voiceport-qq-diagnostic'
        shutil.copytree(output / 'voiceport-qq-diagnostic', copied)
        # No Electron module in this fixture: diagnostic failure must still launch main.
        result = subprocess.run([NODE, str(copied / 'bootstrap.cjs')],
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), 'ORIGINAL_MAIN:' + str(launcher))

    def test_unsafe_main_and_oversized_header_are_rejected(self):
        self.package['main'] = '../PRIVATE_CREDENTIALS'
        self.write_package()
        result = self.run_cli('inspect', '--qq', self.app, expect=1)
        self.assertNotIn('PRIVATE_CREDENTIALS', json.dumps(result))
        self.package['main'] = './application.asar/app_launcher/index.js'
        self.write_package()
        (self.resources / 'application.asar').write_bytes(struct.pack('<4I', 4, 3_000_000, 2_999_996, 1))
        self.run_cli('inspect', '--qq', self.app, expect=1)

    def test_linked_and_special_files_are_rejected_without_blocking(self):
        archive = self.resources / 'application.asar'
        archive.unlink()
        archive.symlink_to('/dev/null')
        self.run_cli('inspect', '--qq', self.app, expect=1)
        archive.unlink()
        import os
        os.mkfifo(archive)
        self.run_cli('inspect', '--qq', self.app, expect=1)

    def test_status_projects_booleans_and_rejects_untrusted_shapes(self):
        directory = self.root / 'reports'
        directory.mkdir(mode=0o700)
        report_path = directory / 'report.json'
        report = {
            'schema_version': 1, 'diagnostic_only': True,
            'observed': dict.fromkeys(('wrapper_loaded', 'session_created', 'init_observed',
                                      'ready', 'bdh_service'), False),
            'capabilities': {}, 'unknown': 'PRIVATE_UNTRUSTED_TEXT',
        }
        report_path.write_text(json.dumps(report))
        report_path.chmod(0o600)
        result = self.run_cli('status', '--report', report_path)['data']
        self.assertNotIn('PRIVATE_UNTRUSTED_TEXT', json.dumps(result))
        self.assertFalse(result['transcription_verified'])
        for bad in ([], {'schema_version': 1, 'diagnostic_only': True, 'observed': []},
                    {**report, 'capabilities': []},
                    {**report, 'observed': {**report['observed'], 'ready': 'PRIVATE_TEXT'}}):
            report_path.write_text(json.dumps(bad))
            result = self.run_cli('status', '--report', report_path, expect=1)
            self.assertNotIn('PRIVATE', json.dumps(result))
        report_path.write_text(json.dumps(report))
        report_path.chmod(0o644)
        self.run_cli('status', '--report', report_path, expect=1)

    @unittest.skipUnless(NODE, 'Node required to exercise bootstrap')
    def test_bootstrap_runs_original_launcher_when_diagnostics_succeed_or_fail(self):
        self.package['main'] = './original.cjs'
        # Placeholder-looking strings in version must not alter generated code.
        self.package['version'] = 'QQ_VERSION ORIGINAL_ENTRY $original_entry'
        self.write_package()
        (self.resources / 'original.cjs').write_text('global.qqOriginalLoads = (global.qqOriginalLoads || 0) + 1;\n')
        output = self.root / 'prepared'
        self.run_cli('prepare', '--qq', self.app, '--output', output)
        copied = self.resources / 'voiceport-qq-diagnostic'
        shutil.copytree(output / 'voiceport-qq-diagnostic', copied)
        bootstrap = copied / 'bootstrap.cjs'
        reports = self.root / 'runtime'
        reports.mkdir(mode=0o700)
        harness = r'''
const assert = require('node:assert/strict');
const Module = require('node:module');
const original = Module._load;
Module._load = function(name, parent, isMain) {
  if (name === 'electron') return { app: { getPath() {
    if (process.argv[3] === 'fail') throw new Error('PRIVATE_INITIALIZATION_ERROR');
    return process.argv[2];
  } } };
  return Reflect.apply(original, this, [name, parent, isMain]);
};
require(process.argv[1]);
assert.equal(global.qqOriginalLoads, 1);
'''
        for mode in ('success', 'fail'):
            result = subprocess.run([NODE, '-e', harness, str(bootstrap), str(reports), mode],
                                    capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertNotIn('PRIVATE_INITIALIZATION_ERROR', result.stderr)
        paths = list(reports.glob('voiceport-qq-diagnostic-*/report.json'))
        self.assertEqual(len(paths), 1)
        report = self.run_cli('status', '--report', paths[0])['data']
        self.assertFalse(report['observed']['wrapper_loaded'])
        self.assertFalse(report['transcription_verified'])


if __name__ == '__main__':
    unittest.main()
