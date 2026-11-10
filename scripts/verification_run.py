#!/usr/bin/env python3
"""Owned offline verification processes, disk logs and one suite per checkout."""
from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import tempfile
import time


def timestamp():
    return datetime.now(timezone.utc).isoformat()


def write_report(path, report):
    """Replace this run's progress report atomically; never reuse another run's directory."""
    path = Path(path)
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as out:
        temporary = Path(out.name)
        try:
            json.dump(report, out, ensure_ascii=False, indent=2)
            out.write('\n')
            out.flush()
            os.fsync(out.fileno())
        except BaseException:
            temporary.unlink(missing_ok=True)
            raise
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


@contextmanager
def suite_lock(root):
    """Fail promptly instead of running two full suites against the same checkout."""
    directory = Path(root) / '.voiceport'
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd = os.open(directory / 'verification.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'r+') as lock:
        if not stat.S_ISREG(os.fstat(lock.fileno()).st_mode):
            raise ValueError('verification lock must be a regular file')
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError('another full verification owns this checkout; wait for its actual completion') from error
        lock.seek(0)
        lock.truncate()
        json.dump({'pid': os.getpid(), 'started_at': timestamp()}, lock)
        lock.flush()
        try:
            yield
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


@contextmanager
def termination_signals():
    """Let the runner clean its owned process group on a normal termination request."""
    def interrupted(signum, frame):
        raise KeyboardInterrupt('verification interrupted by signal ' + str(signum))
    previous = signal.signal(signal.SIGTERM, interrupted)
    try:
        yield
    finally:
        signal.signal(signal.SIGTERM, previous)


@contextmanager
def deferred_termination(on_interrupt=None):
    """Finish bounded work, then consume its stop requests before restoring handlers."""
    pending = []
    previous = {}
    try:
        for signum in (signal.SIGINT, signal.SIGTERM):
            previous[signum] = signal.signal(signum, lambda received, frame: pending.append(received))
        yield pending
    finally:
        try:
            if pending and on_interrupt is not None:
                on_interrupt()
        finally:
            for signum, handler in previous.items():
                signal.signal(signum, handler)


def group_exists(group):
    try:
        os.killpg(group, 0)
        return True
    except ProcessLookupError:
        return False


def signal_group(group, sig):
    try:
        os.killpg(group, sig)
    except ProcessLookupError:
        pass


def cleanup_group(process):
    """Only the new session/process group created by this runner is signalled."""
    signal_group(process.pid, signal.SIGTERM)
    end = time.monotonic() + 1
    while time.monotonic() < end:
        process.poll()  # Reap the direct child before testing whether the group still exists.
        if not group_exists(process.pid):
            return True
        time.sleep(0.02)
    signal_group(process.pid, signal.SIGKILL)
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        return False
    end = time.monotonic() + 1
    while group_exists(process.pid) and time.monotonic() < end:
        time.sleep(0.02)
    return not group_exists(process.pid)


def log_facts(path, marker):
    digest, size, found, tail = hashlib.sha256(), 0, marker is None, b''
    needle = marker.encode() if marker is not None else b''
    with Path(path).open('rb') as source:
        for chunk in iter(lambda: source.read(65536), b''):
            digest.update(chunk)
            size += len(chunk)
            if needle:
                found = found or needle in tail + chunk
                tail = chunk[-(len(needle) - 1):] if len(needle) > 1 else b''
    return {'sha256': digest.hexdigest(), 'bytes': size}, found


def run_check(argv, stdout, stderr, *, cwd, env=None, expected=0, marker=None,
              timeout=600, input=None, read_stdout=False):
    """Stream complete logs; timeout/interrupt/leaked descendants can never pass.

    Cleanup covers our new process group. A program deliberately daemonizing into
    another session is outside this runner's proof and must not be used as a check.
    """
    argv = list(map(str, argv))
    started = time.monotonic()
    check = {'argv': argv, 'command': argv, 'cwd': str(cwd), 'started_at': timestamp(),
             'timeout_seconds': timeout, 'expected_exit_code': expected,
             'required_output': marker, 'exit_code': None, 'passed': False,
             'status': 'running', 'timed_out': False, 'interrupted': False,
             'cleanup_confirmed': True, 'left_descendants': False,
             'cleanup_scope': 'new process group only'}
    def mark_interrupted():
        check.update(status='interrupted', interrupted=True, passed=False)

    process = None
    with Path(stdout).open('xb') as out, Path(stderr).open('xb') as err:
        try:
            # Do not lose ownership if a stop arrives between spawn and assignment.
            with deferred_termination() as launching:
                process = subprocess.Popen(argv, cwd=cwd, env=env, stdout=out, stderr=err,
                                           stdin=subprocess.PIPE if input is not None else subprocess.DEVNULL,
                                           start_new_session=True)
                check['pid'] = process.pid
            if launching:
                raise KeyboardInterrupt
            process.communicate(input=input, timeout=timeout)
            check['status'] = 'completed'
        except subprocess.TimeoutExpired:
            check.update(status='timeout', timed_out=True)
        except KeyboardInterrupt:
            check.update(status='interrupted', interrupted=True)
        except OSError as error:
            check.update(status='launch_error', error={'type': type(error).__name__, 'detail': str(error)})
        finally:
            with deferred_termination(mark_interrupted):
                if process is not None:
                    try:
                        if check['status'] == 'completed':
                            check['left_descendants'] = group_exists(process.pid)
                        if check['status'] != 'completed' or check['left_descendants']:
                            check['cleanup_confirmed'] = cleanup_group(process)
                            if not check['cleanup_confirmed']:
                                check['cleanup_error'] = {'type': 'CleanupTimeout', 'detail': 'owned group exit not confirmed'}
                    except OSError as error:
                        check.update(cleanup_confirmed=False, left_descendants=None,
                                     cleanup_error={'type': type(error).__name__, 'detail': str(error)})
                        # A denied group observation is unknown, even if our direct
                        # child can be stopped/reaped through its owned Popen handle.
                        try:
                            if process.poll() is None:
                                process.kill()
                            process.wait(timeout=2)
                        except (OSError, subprocess.TimeoutExpired) as child_error:
                            check['direct_child_cleanup_error'] = {'type': type(child_error).__name__, 'detail': str(child_error)}
                    check['exit_code'] = process.returncode
    # A later ordinary stop still leaves complete log facts and a failed result.
    with deferred_termination(mark_interrupted):
        check.update(stdout=str(stdout), stderr=str(stderr))
        output = b''
        try:
            output_facts, output_marker = log_facts(stdout, marker)
            error_facts, error_marker = log_facts(stderr, marker)
            check.update(stdout_sha256=output_facts['sha256'], stderr_sha256=error_facts['sha256'],
                         stdout_bytes=output_facts['bytes'], stderr_bytes=error_facts['bytes'])
            check['passed'] = (check['status'] == 'completed' and check['exit_code'] == expected
                               and not check['left_descendants'] and check['cleanup_confirmed']
                               and (marker is None or output_marker or error_marker))
            if read_stdout and check['passed']:
                with Path(stdout).open('rb') as source:
                    output = source.read(1024 * 1024 + 1)
                if len(output) > 1024 * 1024:
                    check.update(passed=False, error={'type': 'OutputLimit', 'detail': 'parsed stdout exceeds 1MiB; full log retained'})
        except OSError as error:
            check.update(passed=False, error={'type': type(error).__name__, 'detail': str(error)})
        check.update(finished_at=timestamp(), duration_seconds=time.monotonic() - started)
    return check, output if check['passed'] else None
