#!/usr/bin/env python3
"""Offline CI checks after the explicit release build; at most three check processes."""
import argparse
from collections import deque
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True  # -I ignores PYTHONDONTWRITEBYTECODE at interpreter startup.
sys.path.insert(0, str(ROOT / 'scripts'))
from verification_run import (cleanup_group, deferred_termination, group_exists,
                              signal_group, suite_lock, termination_signals, timestamp, write_report)


def flatten(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from flatten(item)
        else:
            yield item


def packaging_partition(root):
    loader = unittest.TestLoader()
    cases = list(flatten(loader.discover(str(root / 'Tests/PackagingTests'))))
    if loader.errors:
        raise ValueError('packaging discovery failed:\n' + '\n'.join(loader.errors))
    ids = [case.id() for case in cases]
    if not ids or len(ids) != len(set(ids)):
        raise ValueError('packaging discovery returned zero tests or duplicate IDs')
    shards = {'bundle': [], 'boundaries': []}
    for case in cases:
        cls = type(case)
        shard = 'bundle' if (cls.__module__, cls.__name__) == ('test_packaging', 'RealBundleTests') else 'boundaries'
        shards[shard].append(case)
    partition_ids = [case.id() for shard in shards.values() for case in shard]
    if (not all(shards.values()) or len(partition_ids) != len(ids)
            or len(set(partition_ids)) != len(ids) or set(partition_ids) != set(ids)):
        raise ValueError('packaging shards must be nonempty, disjoint and cover discovery')
    return cases, shards


class RecordingResult(unittest.TextTestResult):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.executed_ids = []

    def startTest(self, test):
        self.executed_ids.append(test.id())
        super().startTest(test)


def packaging_shard(shard, report_path=None, root=ROOT):
    report = {'shard': shard, 'passed': False, 'started_at': timestamp()}
    started = time.monotonic()
    try:
        cases, shards = packaging_partition(root)
        selected = shards[shard]
        selected_ids = [case.id() for case in selected]
        report.update(discovered_ids=[case.id() for case in cases], selected_ids=selected_ids,
                      discovered_count=len(cases), selected_count=len(selected))
        # A flat TestSuite still applies unittest's class/module setup and teardown.
        result = unittest.TextTestRunner(verbosity=2, resultclass=RecordingResult).run(unittest.TestSuite(selected))
        report.update(executed_ids=result.executed_ids, executed_count=result.testsRun,
                      skipped=[(test.id(), reason) for test, reason in result.skipped],
                      failures=len(result.failures), errors=len(result.errors),
                      passed=(result.wasSuccessful() and not result.skipped
                              and result.testsRun == len(selected_ids)
                              and result.executed_ids == selected_ids))
    except Exception as error:
        report['error'] = {'type': type(error).__name__, 'detail': str(error)}
    report.update(finished_at=timestamp(), duration_seconds=time.monotonic() - started)
    if report_path is not None:
        write_report(report_path, report)
    print(json.dumps(report, sort_keys=True), flush=True)
    return 0 if report['passed'] else 1


OWNER_GRACE_SECONDS = 20  # Outer interpreter deadline includes all nested cleanup.
SESSION_GRACE_SECONDS = 5  # Shared across every tracked child, never per child.
SESSION_KILL_GRACE_SECONDS = 3


def unittest_owner(arguments, receipt):
    """Run unittest in this interpreter and prove cleanup of Python-owned sessions.

    Tests keep the real Popen API. Track new-session handles in this interpreter,
    including verification_run.run_check, without changing that shared helper.
    An abrupt interpreter death cannot produce a completed ownership receipt.
    """
    owner = {'owner_pid': os.getpid(), 'complete': False, 'cleanup_confirmed': False,
             'interrupted': False, 'groups': []}
    active = {}
    result = None
    original_popen = subprocess.Popen
    previous = {}

    def stop(signum, frame):
        owner['interrupted'] = True
        if result is not None:
            result.stop()
        raise KeyboardInterrupt

    class Result(RecordingResult):
        def __init__(self, *args, **kwargs):
            nonlocal result
            super().__init__(*args, **kwargs)
            result = self

    class OwnedPopen(original_popen):
        def __init__(self, *args, **kwargs):
            self.ownership = None
            if not kwargs.get('start_new_session'):
                super().__init__(*args, **kwargs)
                return
            with deferred_termination() as stops:
                super().__init__(*args, **kwargs)
                self.ownership = {'pid': self.pid, 'cleanup_confirmed': False}
                owner['groups'].append(self.ownership)
                active[self.pid] = self
            if stops:
                stop(stops[0], None)

        def observe_exit(self):
            # Relinquish ownership promptly, so final cleanup never signals a
            # previously settled group whose numeric ID might later be reused.
            if self.ownership is not None and self.returncode is not None:
                try:
                    if not group_exists(self.pid):
                        self.ownership.update(cleanup_confirmed=True, exit_code=self.returncode)
                        active.pop(self.pid, None)
                except OSError:
                    pass  # Final cleanup must observe it independently or fail.

        def poll(self):
            code = super().poll()
            self.observe_exit()
            return code

        def wait(self, *args, **kwargs):
            code = super().wait(*args, **kwargs)
            self.observe_exit()
            return code

    code = 1
    try:
        write_report(receipt, owner)
        for signum in (signal.SIGTERM, signal.SIGINT):
            previous[signum] = signal.signal(signum, stop)
        subprocess.Popen = OwnedPopen
        program = unittest.TestProgram(module=None, argv=['unittest', *arguments], exit=False,
                                       testRunner=unittest.TextTestRunner(verbosity=2, resultclass=Result))
        code = 0 if program.result.wasSuccessful() else 1
    except KeyboardInterrupt:
        owner['interrupted'] = True
    finally:
        def stopped_during_cleanup():
            owner['interrupted'] = True
            write_report(receipt, owner)
        with deferred_termination(stopped_during_cleanup):
            subprocess.Popen = original_popen
            def observe_children():
                for process in list(active.values()):
                    try:
                        process.poll()
                    except OSError as error:
                        process.ownership.update(cleanup_confirmed=False, error=str(error))

            def wait_for_children(seconds):
                deadline = time.monotonic() + seconds
                while active:
                    observe_children()
                    if not active or time.monotonic() >= deadline:
                        break
                    time.sleep(0.02)

            observe_children()
            for process in list(active.values()):
                process.ownership['left_running'] = True
                try:
                    # Give signal-aware interpreters a chance to unwind first.
                    if process.returncode is None:
                        process.send_signal(signal.SIGTERM)
                    else:
                        signal_group(process.pid, signal.SIGTERM)
                except OSError as error:
                    process.ownership.update(cleanup_confirmed=False, error=str(error))
            wait_for_children(SESSION_GRACE_SECONDS)
            for process in list(active.values()):
                process.ownership['owner_grace_expired'] = True
                try:
                    signal_group(process.pid, signal.SIGKILL)
                except OSError as error:
                    process.ownership.update(cleanup_confirmed=False, error=str(error))
            wait_for_children(SESSION_KILL_GRACE_SECONDS)
            for process in active.values():
                process.ownership.update(cleanup_confirmed=False, exit_code=process.returncode)
            owner.update(complete=True, cleanup_confirmed=all(group['cleanup_confirmed'] for group in owner['groups']))
            write_report(receipt, owner)
        for signum, handler in previous.items():
            signal.signal(signum, handler)
    clean = owner['cleanup_confirmed'] and not any(group.get('left_running') for group in owner['groups'])
    return 130 if owner['interrupted'] else code if clean else 1


def owned_unittest_command(argv, receipt):
    """Keep the original discover command in reports, with a cooperative entry point."""
    if '-m' in argv:
        index = argv.index('-m')
        if argv[index:index + 3] == ['-m', 'unittest', 'discover']:
            return argv[:index] + [str(Path(__file__).resolve()), '--ownership-report', str(receipt),
                                   '--unittest-args', *argv[index + 2:]]
    return argv


def check_commands(root, evidence):
    python = [sys.executable, '-I', '-B']
    runner = str(root / 'scripts/ci_checks.py')
    node_tests = sorted((root / 'Tests/QQBridgeTests').glob('*.test.cjs'))
    if not node_tests:
        raise ValueError('no QQ Node test files discovered')
    return [
        ('packaging-bundle', python + [runner, '--packaging-shard', 'bundle', '--packaging-report', str(evidence / 'packaging-bundle.json')]),
        ('packaging-boundaries', python + [runner, '--packaging-shard', 'boundaries', '--packaging-report', str(evidence / 'packaging-boundaries.json')]),
        ('rust', ['bash', 'scripts/test_rust.sh']),
        ('loop', python + ['-m', 'unittest', 'discover', '-s', 'Tests/LoopTests', '-v']),
        ('hooks', python + ['-m', 'unittest', 'discover', '-s', '.claude/hooks', '-p', 'test_*.py', '-v']),
        ('qq-node', ['node', '--test', *map(str, node_tests)]),
        ('qq-python', python + ['-m', 'unittest', 'discover', '-s', 'Tests/QQBridgeTests', '-p', 'test_*.py', '-v']),
        ('ci-runner', python + ['-m', 'unittest', 'discover', '-s', 'Tests/CITests', '-p', 'test_ci_checks.py', '-v']),
    ]


def print_logs(check):
    print(f'::group::{check["name"]}: {check["status"]} ({check["duration_seconds"]:.3f}s)', flush=True)
    for kind in ('stdout', 'stderr'):
        print(f'--- {kind} ---', flush=True)
        path = Path(check[kind])
        if path.exists():
            with path.open('r', errors='replace') as source:
                # Prefix lines so check output cannot inject Actions group commands.
                for line in source:
                    sys.stdout.write('  ' + line)
            print(flush=True)
    print('::endgroup::', flush=True)


def run_checks(commands, *, root, evidence, workers=3, timeout=600):
    """Own every Popen directly; stop queued/running work on the first failure."""
    if not 1 <= workers <= 3 or timeout <= 0:
        raise ValueError('workers must be 1..3 and timeout must be positive')
    env = dict(os.environ, CARGO_NET_OFFLINE='true', PYTHONDONTWRITEBYTECODE='1')
    checks = [dict(name=name, argv=list(map(str, argv)), status='pending', passed=False,
                   exit_code=None, duration_seconds=0, cleanup_confirmed=True,
                   stdout=str(evidence / f'{index:02d}-{name}.stdout.log'),
                   stderr=str(evidence / f'{index:02d}-{name}.stderr.log'))
              for index, (name, argv) in enumerate(commands, 1)]
    report = dict(started_at=timestamp(), status='running', passed=False, checks=checks,
                  workers=workers, timeout_seconds=timeout, cwd=str(root),
                  environment_overrides={key: env[key] for key in ('CARGO_NET_OFFLINE', 'PYTHONDONTWRITEBYTECODE')})
    for check in checks:
        receipt = evidence / (check['name'] + '.ownership.json')
        check['launch_argv'] = owned_unittest_command(check['argv'], receipt)
        if check['launch_argv'] != check['argv']:
            check['ownership_report'] = str(receipt)
            check['cleanup_scope'] = 'direct process group and receipt-proven Python-owned session groups'
    pending = deque(checks)
    active = {}
    started = time.monotonic()

    def save():
        write_report(evidence / 'report.json', report)

    def interrupted():
        report.update(interrupted=True, passed=False, status='interrupted')

    def finish(check, process, began, status):
        check['status'] = status
        if process is not None:
            try:
                if check.get('ownership_report') and process.poll() is None:
                    # Signal only the interpreter first. It must finish nested
                    # helper cleanup before we escalate its outer process group.
                    process.send_signal(signal.SIGTERM)
                    try:
                        process.wait(timeout=OWNER_GRACE_SECONDS)
                    except subprocess.TimeoutExpired:
                        check['owner_grace_expired'] = True
                check['left_descendants'] = process.poll() is not None and group_exists(process.pid)
                if process.returncode is None or check['left_descendants']:
                    check['cleanup_confirmed'] = cleanup_group(process)
                if status == 'passed' and check['left_descendants']:
                    check['status'] = 'leaked_descendants'
            except OSError as error:
                check.update(cleanup_confirmed=False, cleanup_error=str(error))
                # Observation may be denied while descendants still live. Make
                # a bounded group cleanup attempt, retaining the failed proof.
                try:
                    check['group_cleanup_after_error'] = cleanup_group(process)
                except OSError as group_error:
                    check['group_cleanup_error'] = str(group_error)
                try:
                    if process.poll() is None:
                        process.kill()
                    process.wait(timeout=2)
                except (OSError, subprocess.TimeoutExpired) as child_error:
                    check['direct_child_cleanup_error'] = str(child_error)
            check['exit_code'] = process.returncode
            if check.get('ownership_report'):
                try:
                    ownership = json.loads(Path(check['ownership_report']).read_text())
                    check['ownership'] = ownership
                    confirmed = (ownership['owner_pid'] == process.pid and ownership['complete'] is True
                                 and ownership['cleanup_confirmed'] is True
                                 and all(group['cleanup_confirmed'] is True for group in ownership['groups']))
                    check['cleanup_confirmed'] &= confirmed
                except (OSError, ValueError, KeyError, TypeError) as error:
                    check.update(cleanup_confirmed=False, ownership_error=str(error))
        if check['status'] == 'passed' and not check['cleanup_confirmed']:
            check['status'] = 'cleanup_failed'
        check.update(finished_at=timestamp(), duration_seconds=time.monotonic() - began)
        check['passed'] = check['status'] == 'passed' and check['cleanup_confirmed']
        save()
        print_logs(check)

    try:
        save()
        while pending or active:
            # Reap every ready check before filling vacancies: a fast success
            # cannot launch queued work while a failed sibling is already ready.
            failed = False
            with deferred_termination() as stops:
                for name, (process, check, began) in list(active.items()):
                    code = process.poll()
                    if code is not None or time.monotonic() - began >= timeout:
                        status = ('passed' if code == 0 else 'failed') if code is not None else 'timeout'
                        finish(check, process, began, status)
                        del active[name]
                        failed |= not check['passed']
            if stops:
                raise KeyboardInterrupt
            if failed:
                break
            while pending and len(active) < workers:
                check = pending.popleft()
                began = time.monotonic()
                check.update(status='launching', started_at=timestamp())
                save()  # Record the intent before creating a process.
                with deferred_termination() as stops:
                    try:
                        with Path(check['stdout']).open('xb') as out, Path(check['stderr']).open('xb') as err:
                            process = subprocess.Popen(check['launch_argv'], cwd=root, env=env,
                                                       stdin=subprocess.DEVNULL, stdout=out, stderr=err,
                                                       start_new_session=True)
                            active[check['name']] = (process, check, began)
                        check.update(status='running', pid=process.pid)
                        save()
                    except OSError as error:
                        check['error'] = {'type': type(error).__name__, 'detail': str(error)}
                        owned = active.get(check['name'])
                        finish(check, owned[0] if owned else None, began, 'launch_error')
                        active.pop(check['name'], None)
                        failed = True
                if stops:
                    raise KeyboardInterrupt
                if failed:
                    break
            if failed:
                break
            if active:
                time.sleep(0.02)
    except KeyboardInterrupt:
        interrupted()
    except Exception as error:
        report['error'] = {'type': type(error).__name__, 'detail': str(error)}
    finally:
        # Repeated stop requests cannot interrupt bounded owned-group cleanup.
        with deferred_termination(interrupted) as stops:
            for process, check, began in active.values():
                try:
                    finish(check, process, began, 'interrupted' if report.get('interrupted') else 'cancelled')
                except Exception as error:
                    # A broken log/report sink must not strand the remaining groups.
                    report['error'] = {'type': type(error).__name__, 'detail': str(error)}
            for check in checks:
                if check['status'] in ('pending', 'launching'):
                    check['status'] = 'cancelled'
            if stops:
                interrupted()
            report.update(finished_at=timestamp(), duration_seconds=time.monotonic() - started)
            report['passed'] = bool(checks) and all(check['passed'] for check in checks) and not report.get('error') and not report.get('interrupted')
            report['status'] = 'interrupted' if report.get('interrupted') else 'passed' if report['passed'] else 'failed'
            save()
        # A signal arriving during the last save must also reach the disk report.
        if report.get('interrupted'):
            with deferred_termination():
                save()
    return report


def validate_packaging_reports(evidence):
    reports = [json.loads((evidence / f'packaging-{name}.json').read_text()) for name in ('bundle', 'boundaries')]
    discovered = reports[0]['discovered_ids']
    selected = [test_id for report in reports for test_id in report['selected_ids']]
    if (not discovered or len(discovered) != len(set(discovered))
            or any(report['discovered_ids'] != discovered or not report['passed']
                   or not report['selected_ids'] or report['selected_ids'] != report['executed_ids']
                   or report['discovered_count'] != len(discovered)
                   or report['selected_count'] != len(report['selected_ids'])
                   or report['executed_count'] != len(report['selected_ids']) for report in reports)
            or len(selected) != len(set(selected)) or set(selected) != set(discovered)):
        raise ValueError('packaging reports do not prove complete disjoint execution')
    return {'discovered_count': len(discovered), 'executed_count': len(selected), 'ids': discovered}


def machine_report(report, evidence):
    # Successful coverage already contains the common discovered ID list; avoid
    # repeating that list in each shard on the machine-readable Actions line.
    payload = dict(report, evidence=str(evidence))
    payload['checks'] = []
    for check in report['checks']:
        row = dict(check)
        if 'packaging' in row and 'packaging_coverage' in report:
            row['packaging'] = {key: value for key, value in row['packaging'].items() if key != 'discovered_ids'}
        payload['checks'].append(row)
    print('CI_REPORT=' + json.dumps(payload, separators=(',', ':')), flush=True)


def timing_summary(report, evidence):
    lines = ['### Offline CI checks', '', f'Evidence: `{evidence}`', '',
             '| Check | Status | Seconds |', '| --- | --- | ---: |']
    lines += [f'| {check["name"]} | {check["status"]} | {check["duration_seconds"]:.3f} |' for check in report['checks']]
    lines += ['', f'Overall: **{report["status"]}**, elapsed {report["duration_seconds"]:.3f}s.', '']
    text = '\n'.join(lines)
    print(text, flush=True)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
            summary.write(text + '\n')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workers', type=int, choices=range(1, 4), default=3)
    parser.add_argument('--timeout', type=float, default=600, help='per-check timeout in seconds (default: 600)')
    parser.add_argument('--packaging-shard', choices=('bundle', 'boundaries'), help=argparse.SUPPRESS)
    parser.add_argument('--packaging-report', type=Path, help=argparse.SUPPRESS)
    parser.add_argument('--ownership-report', type=Path, help=argparse.SUPPRESS)
    parser.add_argument('--unittest-args', nargs=argparse.REMAINDER, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args.unittest_args is not None and args.ownership_report is None:
        parser.error('--unittest-args requires --ownership-report')
    if args.timeout <= 0 or not args.timeout < float('inf'):
        parser.error('--timeout must be finite and positive')
    os.environ.update(CARGO_NET_OFFLINE='true', PYTHONDONTWRITEBYTECODE='1')
    sys.dont_write_bytecode = True
    try:
        with termination_signals():
            if args.unittest_args is not None:
                return unittest_owner(args.unittest_args, args.ownership_report)
            if args.packaging_shard:
                return packaging_shard(args.packaging_shard, args.packaging_report, ROOT)
            with suite_lock(ROOT):
                base = ROOT / '.voiceport/evidence'
                base.mkdir(parents=True, exist_ok=True)
                evidence = Path(tempfile.mkdtemp(prefix='ci-checks-', dir=base))
                print('Evidence:', evidence / 'report.json', flush=True)
                report = run_checks(check_commands(ROOT, evidence), root=ROOT, evidence=evidence,
                                    workers=args.workers, timeout=args.timeout)
                with deferred_termination() as stops:
                    for check in report['checks']:
                        if check['name'].startswith('packaging-'):
                            try:
                                check['packaging'] = json.loads((evidence / (check['name'] + '.json')).read_text())
                            except (OSError, ValueError) as error:
                                check['packaging_report_error'] = str(error)
                    try:
                        if report['passed']:
                            report['packaging_coverage'] = validate_packaging_reports(evidence)
                    except (OSError, ValueError, KeyError, TypeError) as error:
                        report.update(passed=False, status='failed', coverage_error=str(error))
                    if stops:
                        report.update(passed=False, status='interrupted', interrupted=True)
                    try:
                        timing_summary(report, evidence)
                    except OSError as error:
                        report.update(passed=False, status='failed', summary_error=str(error))
                    write_report(evidence / 'report.json', report)
                def final_interruption():
                    report.update(passed=False, status='interrupted', interrupted=True)
                    write_report(evidence / 'report.json', report)
                    machine_report(report, evidence)
                with deferred_termination(final_interruption):
                    if stops:
                        report.update(passed=False, status='interrupted', interrupted=True)
                        write_report(evidence / 'report.json', report)
                    machine_report(report, evidence)
                return 130 if report.get('interrupted') else 0 if report['passed'] else 1
    except (OSError, ValueError) as error:
        print('ci-checks: ' + str(error), file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130


if __name__ == '__main__':
    sys.exit(main())
