"""Real small subprocess checks; never run the product build or real ledger."""
from contextlib import redirect_stdout, redirect_stderr
import io
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[2] / 'scripts'
sys.path.insert(0, str(SCRIPTS))
import verification_run as runner
import verify


class VerificationRunTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)

    def run_python(self, code, **kwargs):
        return runner.run_check([sys.executable, '-I', '-c', code], self.root / 'stdout.log',
                                self.root / 'stderr.log', cwd=self.root, **kwargs)

    def test_complete_disk_logs_and_real_exit_status(self):
        check, output = self.run_python("import sys; print('x'*70000); print('verified',file=sys.stderr); sys.exit(7)", expected=7, marker='verified')
        self.assertTrue(check['passed'])
        self.assertEqual(check['exit_code'], 7)
        self.assertEqual(output, b'')
        self.assertGreater((self.root / 'stdout.log').stat().st_size, 70000)
        self.assertEqual((self.root / 'stderr.log').read_text(), 'verified\n')
        self.assertTrue(check['cleanup_confirmed'])

    def test_requested_small_stdout_is_available_for_metadata_parsing(self):
        check, output = self.run_python('import json; print(json.dumps({"target_directory":"fixture"}))', read_stdout=True)
        self.assertTrue(check['passed'])
        self.assertEqual(json.loads(output), {'target_directory': 'fixture'})

    def test_timeout_escalates_for_process_ignoring_term(self):
        check, _ = self.run_python('import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); print("ready",flush=True); time.sleep(60)', timeout=1)
        self.assertEqual(check['status'], 'timeout')
        self.assertEqual(check['exit_code'], -signal.SIGKILL)
        self.assertTrue(check['cleanup_confirmed'])
        self.assertFalse(check['passed'])

    def test_nonzero_failure_and_launch_error_are_not_success(self):
        check, output = self.run_python("print('original failure'); raise SystemExit(3)")
        self.assertFalse(check['passed'])
        self.assertIsNone(output)
        self.assertEqual(check['exit_code'], 3)
        self.assertIn('original failure', (self.root / 'stdout.log').read_text())
        check, _ = runner.run_check([str(self.root / 'absent-executable')], self.root / 'absent.out',
                                    self.root / 'absent.err', cwd=self.root)
        self.assertEqual(check['status'], 'launch_error')
        self.assertIsNone(check['exit_code'])
        self.assertFalse(check['passed'])

    def test_timeout_stops_owned_child_and_retains_partial_logs(self):
        child_pid = self.root / 'child.pid'
        code = (
            'import os,pathlib,signal,subprocess,sys,time\n'
            'child=subprocess.Popen([sys.executable,"-I","-c","import time; time.sleep(60)"])\n'
            f'pathlib.Path({str(child_pid)!r}).write_text(str(child.pid))\n'
            'def stop(sig,frame):\n child.kill(); child.wait(); sys.exit(99)\n'
            'signal.signal(signal.SIGTERM,stop)\n'
            'print("child started",flush=True)\n'
            'time.sleep(60)\n'
        )
        check, output = self.run_python(code, timeout=1.5, expected=99)
        self.assertEqual(check['status'], 'timeout')
        self.assertTrue(check['timed_out'])
        self.assertFalse(check['passed'])  # Matching actual exit never turns a timeout green.
        self.assertIsNone(output)
        self.assertEqual(check['exit_code'], 99)
        self.assertTrue(check['cleanup_confirmed'])
        self.assertIn('child started', (self.root / 'stdout.log').read_text())
        with self.assertRaises(ProcessLookupError):
            os.kill(int(child_pid.read_text()), 0)

    def test_term_request_cleans_the_current_owned_process(self):
        ready = self.root / 'ready'
        failed = []
        def terminate_when_ready():
            deadline = time.monotonic() + 5
            while not ready.exists() and time.monotonic() < deadline:
                threading.Event().wait(0.01)
            if ready.exists():
                os.kill(os.getpid(), signal.SIGTERM)
            else:
                failed.append('child did not start')
        thread = threading.Thread(target=terminate_when_ready)
        cleanup = runner.cleanup_group
        def cleanup_with_repeated_stop(process):
            os.kill(os.getpid(), signal.SIGTERM)
            os.kill(os.getpid(), signal.SIGINT)
            return cleanup(process)
        with runner.termination_signals(), patch.object(runner, 'cleanup_group', side_effect=cleanup_with_repeated_stop):
            thread.start()
            check, _ = self.run_python(f'import pathlib,time; pathlib.Path({str(ready)!r}).touch(); time.sleep(60)', timeout=6)
            thread.join(timeout=6)
        self.assertFalse(thread.is_alive())
        self.assertEqual(failed, [])
        self.assertEqual(check['status'], 'interrupted')
        self.assertFalse(check['passed'])
        self.assertEqual(check['exit_code'], -signal.SIGTERM)
        # Some macOS sandboxes deny probing a group whose leader has exited.
        # That is an explicit failed/unknown result, never evidence of group exit.
        if not check['cleanup_confirmed']:
            self.assertEqual(check['cleanup_error']['type'], 'PermissionError')
            self.assertIsNone(check['left_descendants'])

    def test_denied_group_observation_retains_exit_logs_and_failed_result(self):
        with patch.object(runner, 'group_exists', side_effect=PermissionError(1, 'fixture observation denied')):
            check, output = self.run_python('print("complete original output")')
        self.assertEqual(check['exit_code'], 0)
        self.assertFalse(check['passed'])
        self.assertIsNone(output)
        self.assertFalse(check['cleanup_confirmed'])
        self.assertIsNone(check['left_descendants'])
        self.assertEqual(check['cleanup_error']['type'], 'PermissionError')
        self.assertEqual((self.root / 'stdout.log').read_text(), 'complete original output\n')
        self.assertEqual(check['stdout_bytes'], len(b'complete original output\n'))

    def test_denied_group_signal_reaps_direct_child_without_claiming_group_exit(self):
        with patch.object(runner, 'signal_group', side_effect=PermissionError(1, 'fixture signal denied')):
            check, _ = self.run_python('import time; print("ready",flush=True); time.sleep(60)', timeout=0.3)
        self.assertEqual(check['status'], 'timeout')
        self.assertEqual(check['exit_code'], -signal.SIGKILL)
        self.assertFalse(check['passed'])
        self.assertFalse(check['cleanup_confirmed'])
        self.assertEqual(check['cleanup_error']['type'], 'PermissionError')
        self.assertIn('ready', (self.root / 'stdout.log').read_text())

    def test_stop_during_timeout_cleanup_finishes_before_lock_release(self):
        cleanup = runner.cleanup_group
        def cleanup_with_stop(process):
            os.kill(os.getpid(), signal.SIGTERM)
            os.kill(os.getpid(), signal.SIGINT)
            with self.assertRaises(ValueError):
                with runner.suite_lock(self.root):
                    self.fail('checkout lock released during cleanup')
            return cleanup(process)
        with runner.suite_lock(self.root), runner.termination_signals(), patch.object(runner, 'cleanup_group', side_effect=cleanup_with_stop):
            check, _ = self.run_python('import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); print("ready",flush=True); time.sleep(60)', timeout=0.3)
        self.assertEqual(check['status'], 'interrupted')
        self.assertTrue(check['timed_out'])
        self.assertEqual(check['exit_code'], -signal.SIGKILL)
        self.assertTrue(check['cleanup_confirmed'])
        self.assertFalse(check['passed'])
        with runner.suite_lock(self.root):
            pass

    def test_stop_during_log_hashing_preserves_completed_check_and_final_report(self):
        checkout = self.root / 'checkout'
        checkout.mkdir()
        evidence = self.root / 'evidence'
        args = type('Args', (), {'evidence_directory': evidence, 'app_directory': None, 'app_smoke': False})()
        log_facts = runner.log_facts
        def hash_with_stop(path, marker):
            os.kill(os.getpid(), signal.SIGTERM)
            return log_facts(path, marker)
        def small_check(argv, stdout, stderr, **kwargs):
            return runner.run_check([sys.executable, '-I', '-c', 'print("retained output")'], stdout, stderr, cwd=checkout)
        with patch.object(verify, 'ROOT', checkout), patch.object(verify, 'safe_inventory', return_value={'sha256': 'fixture'}), patch.object(verify, 'run_check', side_effect=small_check), patch.object(runner, 'log_facts', side_effect=hash_with_stop):
            with runner.suite_lock(checkout), runner.termination_signals(), redirect_stdout(io.StringIO()):
                self.assertEqual(verify.verify(args), 130)
        report = json.loads((evidence / 'report.json').read_text())
        self.assertEqual(report['status'], 'interrupted')
        self.assertFalse(report['passed'])
        self.assertIsNone(report['active_check'])
        self.assertIn('finished_at', report)
        self.assertEqual(len(report['checks']), 1)
        self.assertEqual(report['checks'][0]['exit_code'], 0)
        self.assertEqual(report['checks'][0]['stdout_bytes'], len(b'retained output\n'))

    def test_stop_at_final_timestamp_cannot_return_success(self):
        timestamp = runner.timestamp
        calls = []
        def timestamp_with_late_stop():
            calls.append(True)
            if len(calls) == 2:  # After log facts and the initial pass decision.
                os.kill(os.getpid(), signal.SIGTERM)
            return timestamp()
        with runner.termination_signals(), patch.object(runner, 'timestamp', side_effect=timestamp_with_late_stop):
            check, output = self.run_python('print("completed before stop")')
        self.assertEqual(check['exit_code'], 0)
        self.assertEqual(check['status'], 'interrupted')
        self.assertTrue(check['interrupted'])
        self.assertFalse(check['passed'])
        self.assertIsNone(output)
        self.assertIn('finished_at', check)

    def test_stop_at_final_report_write_cannot_persist_success(self):
        evidence = self.root / 'evidence'
        args = type('Args', (), {'evidence_directory': evidence})()
        write = verify.write_report
        sent = []
        def finish_success(args, evidence, report):
            report['passed'] = True
        def write_with_stop(path, report):
            write(path, report)
            if report['status'] == 'passed' and not sent:
                sent.append(True)
                os.kill(os.getpid(), signal.SIGTERM)
        with patch.object(verify, 'verify_checks', side_effect=finish_success), patch.object(verify, 'write_report', side_effect=write_with_stop):
            with runner.termination_signals(), redirect_stdout(io.StringIO()):
                self.assertEqual(verify.verify(args), 130)
        report = json.loads((evidence / 'report.json').read_text())
        self.assertEqual(sent, [True])
        self.assertEqual(report['status'], 'interrupted')
        self.assertFalse(report['passed'])

    def test_interrupt_outside_process_wait_finalizes_incomplete_report(self):
        checkout = self.root / 'checkout'
        checkout.mkdir()
        evidence = self.root / 'evidence'
        args = type('Args', (), {'evidence_directory': evidence, 'app_directory': None, 'app_smoke': False})()
        def stopped(*args, **kwargs):
            os.kill(os.getpid(), signal.SIGTERM)
        with patch.object(verify, 'ROOT', checkout), patch.object(verify, 'safe_inventory', return_value={'sha256': 'fixture'}), patch.object(verify, 'run_check', side_effect=stopped):
            with runner.termination_signals(), redirect_stdout(io.StringIO()):
                self.assertEqual(verify.verify(args), 130)
        report = json.loads((evidence / 'report.json').read_text())
        self.assertEqual(report['status'], 'interrupted')
        self.assertFalse(report['passed'])
        self.assertIsNone(report['active_check'])
        self.assertIn('finished_at', report)
        self.assertIn('incomplete_check', report)
        self.assertEqual(report['checks'], [])

    def test_missing_marker_and_oversize_parsed_output_fail_with_full_logs(self):
        check, output = self.run_python("print('x'*1100000)", read_stdout=True)
        self.assertFalse(check['passed'])
        self.assertEqual(check['error']['type'], 'OutputLimit')
        self.assertIsNone(output)
        self.assertGreater((self.root / 'stdout.log').stat().st_size, 1024 * 1024)
        facts, found = runner.log_facts(self.root / 'stdout.log', 'never emitted')
        self.assertFalse(found)
        self.assertEqual(facts['bytes'], check['stdout_bytes'])

    def test_suite_lock_refuses_overlap_and_releases_after_failure(self):
        with runner.suite_lock(self.root):
            with self.assertRaisesRegex(ValueError, 'another full verification'):
                with runner.suite_lock(self.root):
                    self.fail('second suite acquired the same checkout')
            with patch.object(verify, 'ROOT', self.root), patch.object(sys, 'argv', ['verify.py']), patch.object(verify, 'verify') as execute:
                with redirect_stderr(io.StringIO()):
                    self.assertEqual(verify.main(), 1)
                execute.assert_not_called()
        with self.assertRaisesRegex(RuntimeError, 'fixture failure'):
            with runner.suite_lock(self.root):
                raise RuntimeError('fixture failure')
        with runner.suite_lock(self.root):
            pass

    def test_progress_report_exists_before_check_and_failure_stops_following_checks(self):
        checkout = self.root / 'checkout'
        checkout.mkdir()
        evidence = self.root / 'evidence-outside-checkout'
        args = type('Args', (), {'evidence_directory': evidence, 'app_directory': None, 'app_smoke': False})()
        observations = []
        def small_check(argv, stdout, stderr, **kwargs):
            snapshot = json.loads((evidence / 'report.json').read_text())
            observations.append(snapshot)
            self.assertFalse(snapshot['passed'])
            self.assertEqual(snapshot['status'], 'running')
            self.assertEqual(snapshot['active_check']['argv'], argv)
            return runner.run_check([sys.executable, '-I', '-c', 'print("fixture first failure"); raise SystemExit(4)'],
                                    stdout, stderr, cwd=checkout)
        with patch.object(verify, 'ROOT', checkout), patch.object(verify, 'safe_inventory', return_value={'sha256': 'fixture-tree'}), patch.object(verify, 'run_check', side_effect=small_check):
            with redirect_stdout(io.StringIO()):
                self.assertEqual(verify.verify(args), 1)
        self.assertEqual(len(observations), 1)
        report = json.loads((evidence / 'report.json').read_text())
        self.assertEqual(report['status'], 'failed')
        self.assertFalse(report['passed'])
        self.assertIsNone(report['active_check'])
        self.assertEqual(len(report['checks']), 1)
        self.assertEqual(report['checks'][0]['exit_code'], 4)
        self.assertEqual(report['evidence'], str(evidence))
        self.assertIn('fixture first failure', Path(report['checks'][0]['stdout']).read_text())


if __name__ == '__main__':
    unittest.main()
