import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / 'scripts/ci_checks.py'
SPEC = importlib.util.spec_from_file_location('ci_checks', RUNNER)
ci_checks = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ci_checks)
PYTHON = [sys.executable, '-I', '-B']
ENV = dict(os.environ, CARGO_NET_OFFLINE='true', PYTHONDONTWRITEBYTECODE='1')
BOOTSTRAP = f'''import importlib.util, pathlib, sys
spec = importlib.util.spec_from_file_location('ci_checks', {str(RUNNER)!r})
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
'''


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='voiceport-ci-checks-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.evidence = self.root / 'evidence'
        self.evidence.mkdir()

    def command(self, code):
        return PYTHON + ['-c', code]

    def run_checks(self, commands, **kwargs):
        with contextlib.redirect_stdout(io.StringIO()) as logs:
            report = ci_checks.run_checks(commands, root=self.root, evidence=self.evidence, **kwargs)
        self.assertEqual(json.loads((self.evidence / 'report.json').read_text()), report)
        self.logs = logs.getvalue()
        return report

    def assert_cleanup_or_denied_observation(self, check):
        if not check['cleanup_confirmed']:
            # Match verification_run's existing macOS sandbox policy: a denied
            # observation stays unknown and failed, even after the child exits.
            self.assertIn('Operation not permitted', check['cleanup_error'])
            self.assertIsNotNone(check['exit_code'])
            self.assertFalse(check['passed'])
        self.assertFalse(ci_checks.group_exists(check['pid']))

    def test_processes_overlap_using_barrier_and_queue_waits_for_capacity(self):
        # All three must enter before any can finish. No elapsed-time assertion.
        barrier = '''import os, pathlib, sys, time
root = pathlib.Path(sys.argv[1]); name = sys.argv[2]
(root / (name + '.ready')).write_text(str(os.getpid()))
deadline = time.monotonic() + 8
while len(list(root.glob('*.ready'))) < 3:
    if time.monotonic() > deadline: raise SystemExit('barrier not reached')
    time.sleep(.01)
(root / (name + '.done')).touch()
print(name + ' complete')
'''
        commands = [(str(i), self.command(barrier) + [str(self.root), str(i)]) for i in range(3)]
        commands.append(('queued', self.command(
            f'import pathlib; assert list(pathlib.Path({str(self.root)!r}).glob("*.done"))')))
        report = self.run_checks(commands, timeout=10)
        self.assertTrue(report['passed'], report)
        pids = {(self.root / f'{i}.ready').read_text() for i in range(3)}
        self.assertEqual(len(pids), 3)
        self.assertEqual(self.logs.count('::group::'), 4)
        self.assertEqual(self.logs.count('::endgroup::'), 4)
        for check in report['checks']:
            self.assertGreaterEqual(check['duration_seconds'], 0)
            self.assertTrue(Path(check['stdout']).exists())
            self.assertFalse(ci_checks.group_exists(check['pid']))
        self.assertEqual(report['environment_overrides'],
                         {'CARGO_NET_OFFLINE': 'true', 'PYTHONDONTWRITEBYTECODE': '1'})

    def test_workers_one_serial_baseline_and_child_environment(self):
        first = self.root / 'first'
        commands = [('one', self.command(f'import pathlib; pathlib.Path({str(first)!r}).touch()')),
                    ('two', self.command(f'import os, pathlib; assert pathlib.Path({str(first)!r}).exists(); '
                                         'assert os.environ["CARGO_NET_OFFLINE"] == "true"; '
                                         'assert os.environ["PYTHONDONTWRITEBYTECODE"] == "1"'))]
        report = self.run_checks(commands, workers=1)
        self.assertTrue(report['passed'])
        self.assertLessEqual(report['checks'][0]['finished_at'], report['checks'][1]['started_at'])

    def test_failure_is_not_hidden_by_success_and_pending_never_passes(self):
        marker = self.root / 'must-not-run'
        report = self.run_checks([
            ('fast-success', self.command('pass')),
            ('failed-shard', self.command('raise SystemExit(7)')),
            ('pending', self.command(f'import pathlib; pathlib.Path({str(marker)!r}).touch()')),
        ], workers=2)
        self.assertFalse(report['passed'])
        self.assertEqual(report['checks'][1]['status'], 'failed')
        self.assertEqual(report['checks'][1]['exit_code'], 7)
        # The queued child can start if success is observed before failure, but
        # its success must never hide the failed check in the overall report.
        self.assertFalse(report['checks'][1]['passed'])

    def test_failure_cleans_running_sibling_and_keeps_queue_cancelled(self):
        ready = self.root / 'ready'
        slow = self.command(f'import pathlib, time; pathlib.Path({str(ready)!r}).touch(); time.sleep(30)')
        fail = self.command(f'''import pathlib, time
end = time.monotonic() + 8
while not pathlib.Path({str(ready)!r}).exists():
    if time.monotonic() > end: raise SystemExit(99)
    time.sleep(.01)
raise SystemExit(7)
''')
        report = self.run_checks([('slow', slow), ('fail', fail), ('pending', self.command('pass'))], workers=2)
        self.assertFalse(report['passed'])
        self.assertEqual([check['status'] for check in report['checks']], ['cancelled', 'failed', 'cancelled'])
        self.assert_cleanup_or_denied_observation(report['checks'][0])
        self.assertEqual(report['checks'][1]['exit_code'], 7)
        self.assertNotIn('pid', report['checks'][2])

    def test_serial_failure_cancels_pending_without_launch(self):
        report = self.run_checks([('fail', self.command('raise SystemExit(3)')),
                                  ('pending', self.command('raise SystemExit(99)'))], workers=1)
        pending = report['checks'][1]
        self.assertEqual(pending['status'], 'cancelled')
        self.assertFalse(pending['passed'])
        self.assertIsNone(pending['exit_code'])
        self.assertNotIn('pid', pending)

    def test_timeout_cleans_owned_group_and_does_not_kill_other_group(self):
        unrelated = subprocess.Popen(self.command('import time; time.sleep(30)'), start_new_session=True,
                                     env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        def stop_unrelated():
            if unrelated.poll() is None:
                unrelated.terminate()
            try:
                unrelated.wait(timeout=2)
            except subprocess.TimeoutExpired:
                unrelated.kill()
                unrelated.wait(timeout=2)
        self.addCleanup(stop_unrelated)
        code = '''import pathlib, signal, subprocess, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen([sys.executable, '-I', '-c', 'import time; time.sleep(30)'])
pathlib.Path(sys.argv[1]).write_text(str(child.pid))
time.sleep(30)
'''
        report = self.run_checks([('timeout', self.command(code) + [str(self.root / 'descendant')])], timeout=.5)
        check = report['checks'][0]
        self.assertEqual(check['status'], 'timeout')
        self.assertFalse(report['passed'])
        self.assert_cleanup_or_denied_observation(check)
        self.assertIsNone(unrelated.poll())
        with self.assertRaises(ProcessLookupError):
            os.kill(int((self.root / 'descendant').read_text()), 0)

    def test_success_with_leaked_descendant_fails_and_cleans(self):
        code = '''import subprocess, sys
subprocess.Popen([sys.executable, '-I', '-c', 'import time; time.sleep(30)'])
'''
        report = self.run_checks([('leak', self.command(code))])
        check = report['checks'][0]
        self.assertFalse(report['passed'])
        if check['status'] == 'cleanup_failed':
            self.assertIn('Operation not permitted', check['cleanup_error'])
        else:
            self.assertEqual(check['status'], 'leaked_descendants')
            self.assertTrue(check['left_descendants'])
        self.assertEqual(check['exit_code'], 0)
        self.assert_cleanup_or_denied_observation(check)

    def test_denied_observation_still_attempts_owned_descendant_cleanup(self):
        descendant = self.root / 'descendant.pid'
        child = f'import os, pathlib, time; pathlib.Path({str(descendant)!r}).write_text(str(os.getpid())); time.sleep(30)'
        code = f'''import pathlib, subprocess, time
subprocess.Popen({PYTHON + ['-c', child]!r})
end = time.monotonic() + 8
while not pathlib.Path({str(descendant)!r}).exists():
    if time.monotonic() > end: raise SystemExit(99)
    time.sleep(.01)
'''
        with patch.object(ci_checks, 'group_exists', side_effect=PermissionError(1, 'Operation not permitted')):
            report = self.run_checks([('leak', self.command(code))])
        check = report['checks'][0]
        self.assertFalse(report['passed'])
        self.assertFalse(check['cleanup_confirmed'])
        self.assertTrue('group_cleanup_after_error' in check or 'group_cleanup_error' in check)
        self.assertFalse(ci_checks.group_exists(check['pid']))
        with self.assertRaises(ProcessLookupError):
            os.kill(int(descendant.read_text()), 0)

    def test_launch_failure_cleans_running_sibling_and_cancels_queue(self):
        report = self.run_checks([
            ('running', self.command('import time; time.sleep(30)')),
            ('missing', [str(self.root / 'nonexistent executable')]),
            ('pending', self.command('pass')),
        ])
        self.assertFalse(report['passed'])
        self.assertEqual([check['status'] for check in report['checks']],
                         ['cancelled', 'launch_error', 'cancelled'])
        self.assert_cleanup_or_denied_observation(report['checks'][0])
        self.assertIsNone(report['checks'][2]['exit_code'])
        self.assertIn('launch_error', self.logs)

    def test_sigterm_and_sigint_clean_groups_and_save_interrupted_report(self):
        for signum in (signal.SIGTERM, signal.SIGINT):
            with self.subTest(signum=signum):
                evidence = self.root / str(signum)
                evidence.mkdir()
                ready = evidence / 'ready'
                child = self.command(f'import pathlib, time; pathlib.Path({str(ready)!r}).touch(); time.sleep(30)')
                code = BOOTSTRAP + f'''
with m.termination_signals():
    report = m.run_checks([('active', {child!r}), ('pending', {self.command('pass')!r})],
                          root=pathlib.Path({str(self.root)!r}), evidence=pathlib.Path({str(evidence)!r}), workers=1)
sys.exit(130 if report.get('interrupted') else 1)
'''
                with (evidence / 'harness.log').open('wb') as log:
                    parent = subprocess.Popen(self.command(code), env=ENV, stdout=log, stderr=log,
                                              start_new_session=True)
                    self.addCleanup(lambda process=parent: ci_checks.cleanup_group(process) if process.poll() is None else None)
                    end = time.monotonic() + 8
                    while not ready.exists() and parent.poll() is None and time.monotonic() < end:
                        time.sleep(.01)
                    self.assertTrue(ready.exists(), (evidence / 'harness.log').read_text())
                    parent.send_signal(signum)
                    self.assertEqual(parent.wait(timeout=8), 130)
                report = json.loads((evidence / 'report.json').read_text())
                self.assertEqual(report['status'], 'interrupted')
                self.assertFalse(report['passed'])
                self.assertEqual(report['checks'][0]['status'], 'interrupted')
                self.assertFalse(ci_checks.group_exists(report['checks'][0]['pid']))
                self.assertEqual(report['checks'][1]['status'], 'cancelled')

    def nested_helper_command(self):
        tests = self.root / 'nested-tests'
        tests.mkdir()
        ready = self.root / 'nested.pid'
        child_code = (f'import os, pathlib, signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); '
                      f'pathlib.Path({str(ready)!r}).write_text(str(os.getpid())); time.sleep(60)')
        (tests / 'test_nested.py').write_text(f'''import pathlib, sys, unittest
sys.path.insert(0, {str(ROOT / 'scripts')!r})
import verification_run
class Nested(unittest.TestCase):
    def test_nested(self):
        check, _ = verification_run.run_check({PYTHON + ['-c', child_code]!r},
            pathlib.Path({str(self.root / 'nested.stdout')!r}), pathlib.Path({str(self.root / 'nested.stderr')!r}),
            cwd={str(self.root)!r}, timeout=60)
        self.assertTrue(check['cleanup_confirmed'])
    def test_z_pending(self):
        pathlib.Path({str(self.root / 'must-not-run')!r}).touch()
''')
        def remove_survivor():
            if ready.exists():
                pid = int(ready.read_text())
                try:
                    os.killpg(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        # Safety teardown for a failed regression; assertions run before this.
        self.addCleanup(remove_survivor)
        return PYTHON + ['-m', 'unittest', 'discover', '-s', str(tests), '-v'], ready

    def assert_nested_cleanup(self, report, ready):
        check = report['checks'][0]
        self.assertTrue(ready.exists(), check)
        pid = int(ready.read_text())
        self.assertFalse(report['passed'])
        self.assertTrue(check['cleanup_confirmed'], check)
        self.assertTrue(check['ownership']['complete'])
        groups = check['ownership']['groups']
        self.assertEqual([group['pid'] for group in groups], [pid])
        self.assertTrue(groups[0]['cleanup_confirmed'])
        self.assertEqual(groups[0]['exit_code'], -signal.SIGKILL)
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)
        self.assertFalse(ci_checks.group_exists(check['pid']))
        self.assertFalse((self.root / 'must-not-run').exists(), 'cancelled unittest owner launched another case')

    def test_sibling_failure_waits_for_real_helper_nested_group_cleanup(self):
        command, ready = self.nested_helper_command()
        sibling = self.command(f'''import pathlib, time
end = time.monotonic() + 8
while not pathlib.Path({str(ready)!r}).exists():
    if time.monotonic() > end: raise SystemExit(99)
    time.sleep(.01)
raise SystemExit(7)
''')
        report = self.run_checks([('loop', command), ('fail', sibling), ('pending', self.command('pass'))], workers=2)
        self.assertEqual([check['status'] for check in report['checks']], ['cancelled', 'failed', 'cancelled'])
        self.assert_nested_cleanup(report, ready)
        self.assertEqual(report['checks'][1]['exit_code'], 7)
        self.assertEqual(report['checks'][0]['exit_code'], 130)
        self.assertNotIn('pid', report['checks'][2])

    def test_timeout_waits_for_real_helper_nested_group_cleanup(self):
        command, ready = self.nested_helper_command()
        report = self.run_checks([('loop', command)], timeout=2)
        self.assertEqual(report['checks'][0]['status'], 'timeout')
        self.assert_nested_cleanup(report, ready)

    def test_sigterm_waits_for_real_helper_nested_group_cleanup(self):
        command, ready = self.nested_helper_command()
        code = BOOTSTRAP + f'''
with m.termination_signals():
    report = m.run_checks([('loop', {command!r}), ('pending', {self.command('pass')!r})],
                          root=pathlib.Path({str(self.root)!r}), evidence=pathlib.Path({str(self.evidence)!r}), workers=1)
sys.exit(130 if report.get('interrupted') else 1)
'''
        with (self.root / 'harness.log').open('wb') as log:
            parent = subprocess.Popen(self.command(code), env=ENV, stdout=log, stderr=log, start_new_session=True)
            self.addCleanup(lambda: ci_checks.cleanup_group(parent) if parent.poll() is None else None)
            end = time.monotonic() + 8
            while not ready.exists() and parent.poll() is None and time.monotonic() < end:
                time.sleep(.01)
            self.assertTrue(ready.exists(), (self.root / 'harness.log').read_text())
            parent.send_signal(signal.SIGTERM)
            self.assertEqual(parent.wait(timeout=12), 130)
        report = json.loads((self.evidence / 'report.json').read_text())
        self.assertEqual(report['checks'][0]['status'], 'interrupted')
        self.assertEqual(report['checks'][1]['status'], 'cancelled')
        self.assert_nested_cleanup(report, ready)

    def test_multiple_uncooperative_sessions_fit_within_outer_cleanup_budget(self):
        tests = self.root / 'direct-sessions'
        tests.mkdir()
        ready = [self.root / f'direct-{index}.pid' for index in range(3)]
        child = ('import os, pathlib, signal, sys, time; '
                 'signal.signal(signal.SIGTERM, signal.SIG_IGN); '
                 'pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(60)')
        commands = [PYTHON + ['-c', child, str(path)] for path in ready]
        (tests / 'test_direct.py').write_text(f'''import subprocess, time, unittest
class DirectSessions(unittest.TestCase):
    def test_children(self):
        for command in {commands!r}:
            subprocess.Popen(command, start_new_session=True)
        time.sleep(60)
''')
        def clean_survivors():
            for path in ready:
                if path.exists():
                    try:
                        os.killpg(int(path.read_text()), signal.SIGKILL)
                    except ProcessLookupError:
                        pass
        self.addCleanup(clean_survivors)
        command = PYTHON + ['-m', 'unittest', 'discover', '-s', str(tests), '-v']
        sibling = self.command(f'''import pathlib, time
paths = {list(map(str, ready))!r}
end = time.monotonic() + 8
while not all(pathlib.Path(path).exists() for path in paths):
    if time.monotonic() > end: raise SystemExit(99)
    time.sleep(.01)
raise SystemExit(7)
''')
        def fast_owner(argv, receipt):
            arguments = argv[argv.index('-m') + 2:]
            return self.command(BOOTSTRAP + f'''
m.SESSION_GRACE_SECONDS = .1
m.SESSION_KILL_GRACE_SECONDS = 1
sys.exit(m.unittest_owner({arguments!r}, pathlib.Path({str(receipt)!r})))
''')
        with patch.object(ci_checks, 'owned_unittest_command', side_effect=lambda argv, receipt: fast_owner(argv, receipt) if '-m' in argv else argv), \
                patch.object(ci_checks, 'OWNER_GRACE_SECONDS', 2):
            report = self.run_checks([('loop', command), ('failed', sibling)], workers=2)
        owner = report['checks'][0]
        self.assertFalse(report['passed'])
        self.assertEqual(owner['status'], 'cancelled')
        self.assertEqual(owner['exit_code'], 130)
        self.assertTrue(owner['cleanup_confirmed'], owner)
        self.assertTrue(owner['ownership']['complete'])
        groups = owner['ownership']['groups']
        self.assertEqual(len(groups), 3)
        self.assertTrue(all(group['cleanup_confirmed'] for group in groups))
        self.assertTrue(all(group['exit_code'] == -signal.SIGKILL for group in groups))
        for path in ready:
            self.assertTrue(path.exists())
            with self.assertRaises(ProcessLookupError):
                os.kill(int(path.read_text()), 0)

    def test_missing_ownership_receipt_cannot_claim_cleanup(self):
        command = PYTHON + ['-m', 'unittest', 'discover', '-s', 'not-executed']
        # Simulate a child that exits successfully before writing its receipt.
        with patch.object(ci_checks, 'owned_unittest_command', side_effect=lambda argv, receipt: self.command('pass')):
            report = self.run_checks([('loop', command)])
        self.assertFalse(report['passed'])
        check = report['checks'][0]
        self.assertEqual(check['exit_code'], 0)
        self.assertEqual(check['status'], 'cleanup_failed')
        self.assertFalse(check['cleanup_confirmed'])
        self.assertIn('ownership_error', check)

    def test_incomplete_wrong_owner_or_failed_group_receipt_cannot_claim_cleanup(self):
        command = PYTHON + ['-m', 'unittest', 'discover', '-s', 'not-executed']
        variants = [dict(complete=False, cleanup_confirmed=True, groups=[]),
                    dict(complete=True, cleanup_confirmed=True, groups=[], owner_pid=-1),
                    dict(complete=True, cleanup_confirmed=True, groups=[dict(pid=-1, cleanup_confirmed=False)])]
        for index, receipt_data in enumerate(variants):
            with self.subTest(receipt=receipt_data):
                evidence = self.root / f'invalid-{index}'
                evidence.mkdir()
                def fake_owner(argv, receipt):
                    code = (f'import json, os, pathlib; data = {receipt_data!r}; '
                            'data.setdefault("owner_pid", os.getpid()); '
                            f'pathlib.Path({str(receipt)!r}).write_text(json.dumps(data))')
                    return self.command(code)
                with patch.object(ci_checks, 'owned_unittest_command', side_effect=fake_owner), \
                        contextlib.redirect_stdout(io.StringIO()):
                    report = ci_checks.run_checks([('loop', command)], root=self.root, evidence=evidence)
                self.assertFalse(report['passed'])
                self.assertFalse(report['checks'][0]['cleanup_confirmed'])
                self.assertEqual(report['checks'][0]['status'], 'cleanup_failed')

    def test_command_inventory_preserves_checks_and_safely_expands_node_paths(self):
        directory = self.root / 'Tests/QQBridgeTests'
        directory.mkdir(parents=True)
        paths = [directory / 'a space.test.cjs', directory / '$(unsafe).test.cjs']
        for path in paths:
            path.touch()
        commands = dict(ci_checks.check_commands(self.root, self.evidence))
        self.assertEqual(list(commands)[:3], ['packaging-bundle', 'packaging-boundaries', 'rust'])
        self.assertEqual(commands['rust'], ['bash', 'scripts/test_rust.sh'])
        self.assertEqual(commands['qq-node'], ['node', '--test', *map(str, sorted(paths))])
        for name, directory in [('loop', 'Tests/LoopTests'), ('hooks', '.claude/hooks'), ('qq-python', 'Tests/QQBridgeTests')]:
            self.assertIn(directory, commands[name])
            self.assertEqual(commands[name][:2], [sys.executable, '-I'])
        self.assertEqual(len(commands), 8)
        self.assertEqual(list(commands)[-1], 'ci-runner')
        self.assertEqual(commands['ci-runner'], [sys.executable, '-I', '-B', '-m', 'unittest', 'discover',
                                                '-s', 'Tests/CITests', '-p', 'test_ci_checks.py', '-v'])
        for path in paths:
            path.unlink()
        with self.assertRaisesRegex(ValueError, 'no QQ Node'):
            ci_checks.check_commands(self.root, self.evidence)

    def test_timing_summary_appends_to_github_summary(self):
        report = self.run_checks([('ok', self.command('print("full stdout"); import sys; print("full stderr", file=sys.stderr)'))])
        self.assertIn('full stdout', self.logs)
        self.assertIn('full stderr', self.logs)
        summary = self.root / 'summary'
        summary.write_text('previous\n')
        with patch.dict(os.environ, GITHUB_STEP_SUMMARY=str(summary)), contextlib.redirect_stdout(io.StringIO()):
            ci_checks.timing_summary(report, self.evidence)
        text = summary.read_text()
        self.assertTrue(text.startswith('previous\n'))
        self.assertIn('| ok | passed |', text)
        self.assertIn(str(self.evidence), text)

    def test_suite_lock_prevents_concurrent_main_and_main_has_no_build(self):
        with ci_checks.suite_lock(self.root), patch.object(ci_checks, 'ROOT', self.root), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(ci_checks.main([]), 1)
        self.assertFalse((self.root / '.voiceport/evidence').exists())
        for workers in (0, 4):
            with self.assertRaises(ValueError):
                self.run_checks([], workers=workers)


class PackagingShardTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='voiceport-ci-shards-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.tests = self.root / 'Tests/PackagingTests'
        self.tests.mkdir(parents=True)
        self.events = self.root / 'events'
        self.fixture = f'''import pathlib, unittest
EVENTS = pathlib.Path({str(self.events)!r})
def event(value):
    with EVENTS.open('a') as out: out.write(value + '\\n')
class RealBundleTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls): event('setup')
    @classmethod
    def tearDownClass(cls): event('teardown')
    def test_a(self): event('a')
    def test_b(self): event('b')
class BoundaryTests(unittest.TestCase):
    def test_boundary(self): pass
'''
        (self.tests / 'test_packaging.py').write_text(self.fixture)
        (self.tests / 'test_future.py').write_text('import unittest\nclass FutureTests(unittest.TestCase):\n    def test_new(self): pass\n')

    def shard(self, shard='bundle', root=None):
        root = root or self.root
        report_path = self.root / f'{shard}.json'
        code = BOOTSTRAP + f'sys.exit(m.packaging_shard({shard!r}, pathlib.Path({str(report_path)!r}), pathlib.Path({str(root)!r})))'
        result = subprocess.run(PYTHON + ['-c', code], env=ENV, capture_output=True, timeout=10)
        self.assertTrue(report_path.exists(), result.stderr.decode())
        report = json.loads(report_path.read_text())
        self.assertEqual(json.loads(result.stdout.decode().splitlines()[-1]), report)
        return result, report

    def test_lossless_partition_in_separate_processes_preserves_class_fixtures_and_order(self):
        bundle_result, bundle = self.shard('bundle')
        boundary_result, boundaries = self.shard('boundaries')
        self.assertEqual(bundle_result.returncode, 0, bundle_result.stderr)
        self.assertEqual(boundary_result.returncode, 0, boundary_result.stderr)
        self.assertEqual(bundle['selected_ids'], ['test_packaging.RealBundleTests.test_a', 'test_packaging.RealBundleTests.test_b'])
        self.assertEqual(boundaries['selected_ids'], ['test_future.FutureTests.test_new', 'test_packaging.BoundaryTests.test_boundary'])
        self.assertEqual(bundle['discovered_ids'], boundaries['discovered_ids'])
        self.assertEqual(self.events.read_text().splitlines(), ['setup', 'a', 'b', 'teardown'])
        for name, report in [('bundle', bundle), ('boundaries', boundaries)]:
            ci_checks.write_report(self.root / f'packaging-{name}.json', report)
        coverage = ci_checks.validate_packaging_reports(self.root)
        self.assertEqual(coverage['ids'], bundle['discovered_ids'])
        self.assertEqual(coverage['executed_count'], len(bundle['selected_ids']) + len(boundaries['selected_ids']))

    def test_current_product_discovery_is_lossless_without_running_product_tests(self):
        code = BOOTSTRAP + f'''
cases, shards = m.packaging_partition(pathlib.Path({str(ROOT)!r}))
print(m.json.dumps({{'all': [case.id() for case in cases], 'shards': {{key: [case.id() for case in suite] for key, suite in shards.items()}}}}))
'''
        result = subprocess.run(PYTHON + ['-c', code], env=ENV, capture_output=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        discovered = json.loads(result.stdout)
        ids = discovered['all']
        shards = discovered['shards']
        self.assertTrue(all(shards.values()))
        self.assertEqual(len(ids), len(set(ids)))
        combined = shards['bundle'] + shards['boundaries']
        self.assertEqual(len(combined), len(set(combined)))
        self.assertEqual(set(ids), set(combined))
        self.assertTrue(all(test_id.startswith('test_packaging.RealBundleTests.') for test_id in shards['bundle']))
        self.assertFalse(any(test_id.startswith('test_packaging.RealBundleTests.') for test_id in shards['boundaries']))
        for shard in shards.values():
            self.assertEqual(shard, [test_id for test_id in ids if test_id in shard])

    def test_failed_test_and_unexpected_skip_fail_shard(self):
        for body in ["self.fail('synthetic failure')", "self.skipTest('unexpected skip')"]:
            with self.subTest(body=body):
                (self.tests / 'test_packaging.py').write_text(self.fixture.replace("event('a')", body))
                result, report = self.shard()
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertFalse(report['passed'])
                self.assertEqual(report['executed_count'], len(report['selected_ids']))

    def test_class_setup_failure_or_skip_cannot_pass_with_zero_executed(self):
        for body in ["raise RuntimeError('setup failed')", "raise unittest.SkipTest('setup skipped')"]:
            with self.subTest(body=body):
                (self.tests / 'test_packaging.py').write_text(self.fixture.replace("event('setup')", body))
                result, report = self.shard()
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertFalse(report['passed'])
                self.assertEqual(report['executed_count'], 0)

    def test_zero_tests_empty_shard_and_discovery_errors_fail_before_execution(self):
        variants = [
            ('empty', {'test_empty.py': 'import unittest\n'}),
            ('no-boundaries', {'test_packaging.py': self.fixture.split('class BoundaryTests')[0]}),
            ('no-bundle', {'test_other.py': 'import unittest\nclass Other(unittest.TestCase):\n def test_a(self): pass\n'}),
            ('syntax-error', {'test_packaging.py': self.fixture, 'test_bad.py': 'invalid python !'}),
            ('import-error', {'test_packaging.py': self.fixture, 'test_bad.py': 'raise ImportError("synthetic")'}),
        ]
        for name, files in variants:
            with self.subTest(name=name):
                root = self.root / name
                tests = root / 'Tests/PackagingTests'
                tests.mkdir(parents=True)
                for filename, content in files.items():
                    (tests / filename).write_text(content)
                result, report = self.shard(root=root)
                self.assertEqual(result.returncode, 1)
                self.assertFalse(report['passed'])
                self.assertIn('error', report)
                self.assertFalse(self.events.exists())

    def test_duplicate_ids_are_rejected(self):
        (self.tests / 'test_duplicate.py').write_text('''import unittest
class Duplicate(unittest.TestCase):
    def test_a(self): pass
    def id(self): return 'duplicate'
    def test_b(self): pass
''')
        result, report = self.shard()
        self.assertEqual(result.returncode, 1)
        self.assertIn('duplicate IDs', report['error']['detail'])
        self.assertFalse(self.events.exists())

    def test_executed_id_mismatch_and_forged_coverage_are_rejected(self):
        # A case that never calls startTest cannot masquerade as a full run.
        (self.tests / 'test_packaging.py').write_text(self.fixture.replace(
            'class RealBundleTests(unittest.TestCase):',
            'class RealBundleTests(unittest.TestCase):\n    def run(self, result): return result'))
        result, report = self.shard()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(report['executed_count'], 0)
        (self.tests / 'test_packaging.py').write_text(self.fixture)
        _, bundle = self.shard('bundle')
        _, boundaries = self.shard('boundaries')
        for name, data in [('bundle', bundle), ('boundaries', boundaries)]:
            ci_checks.write_report(self.root / f'packaging-{name}.json', data)
        ci_checks.validate_packaging_reports(self.root)
        boundaries['selected_ids'] = bundle['selected_ids']
        boundaries['executed_ids'] = bundle['selected_ids']
        ci_checks.write_report(self.root / 'packaging-boundaries.json', boundaries)
        with self.assertRaisesRegex(ValueError, 'complete disjoint'):
            ci_checks.validate_packaging_reports(self.root)

    def test_main_reports_complete_coverage_and_fresh_evidence_on_success_and_failure(self):
        def commands(root, evidence):
            checks = []
            for shard in ('bundle', 'boundaries'):
                code = BOOTSTRAP + f'sys.exit(m.packaging_shard({shard!r}, pathlib.Path({str(evidence / ("packaging-" + shard + ".json"))!r}), pathlib.Path({str(root)!r})))'
                checks.append(('packaging-' + shard, PYTHON + ['-c', code]))
            checks.extend((name, PYTHON + ['-c', 'print("synthetic check")'])
                          for name in ('rust', 'loop', 'hooks', 'qq-node', 'qq-python', 'ci-runner'))
            return checks

        reports = []
        for broken in (False, True):
            def selected_commands(root, evidence):
                checks = commands(root, evidence)
                if broken:
                    checks[0] = (checks[0][0], PYTHON + ['-c', 'raise SystemExit(4)'])
                return checks
            with patch.object(ci_checks, 'ROOT', self.root), patch.object(ci_checks, 'check_commands', selected_commands), \
                    patch.dict(os.environ, GITHUB_STEP_SUMMARY=str(self.root / 'summary')), \
                    contextlib.redirect_stdout(io.StringIO()) as logs:
                code = ci_checks.main(['--workers', '1'])
            self.assertEqual(code, 1 if broken else 0)
            lines = logs.getvalue().splitlines()
            self.assertTrue(lines[-1].startswith('CI_REPORT='))
            machine = json.loads(lines[-1].removeprefix('CI_REPORT='))
            evidence = Path(machine['evidence'])
            self.assertEqual(evidence.parent, self.root / '.voiceport/evidence')
            self.assertTrue(evidence.name.startswith('ci-'))
            disk = json.loads((evidence / 'report.json').read_text())
            self.assertEqual(machine['passed'], not broken)
            self.assertEqual(len(machine['checks']), 8)
            self.assertEqual(machine['status'], disk['status'])
            if broken:
                self.assertEqual(machine['checks'][0]['exit_code'], 4)
                self.assertTrue(all(check['status'] == 'cancelled' for check in machine['checks'][1:]))
            else:
                discovered = machine['packaging_coverage']['ids']
                selected = [test_id for check in machine['checks'][:2] for test_id in check['packaging']['selected_ids']]
                self.assertEqual(set(discovered), set(selected))
                self.assertEqual(len(discovered), len(selected))
                self.assertTrue(all(check['packaging']['executed_ids'] == check['packaging']['selected_ids'] for check in machine['checks'][:2]))
            reports.append(evidence)
        self.assertNotEqual(*reports)

    def test_internal_cli_dispatches_shard_in_its_own_process(self):
        report = self.root / 'cli.json'
        code = BOOTSTRAP + f'm.ROOT = pathlib.Path({str(self.root)!r})\nsys.exit(m.main(sys.argv[1:]))'
        result = subprocess.run(PYTHON + ['-c', code, '--packaging-shard', 'bundle', '--packaging-report', str(report)],
                                env=ENV, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(json.loads(report.read_text())['passed'])
        self.assertFalse((self.root / '.voiceport/verification.lock').exists(), 'child must not reacquire parent suite lock')


if __name__ == '__main__':
    unittest.main()
