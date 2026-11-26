"""LaunchServices supervision from the accepted focused fixture, shared by private routes."""
from contextlib import closing
import ctypes
import os
from pathlib import Path
import select
import socket
import struct
import subprocess
import tempfile
import time

from verification_run import cleanup_group, deferred_termination, group_exists, log_facts, timestamp

ROOT = Path(__file__).resolve().parent.parent
PASS_MARKER = "status-overlay-focus: passed positive native key/AX baseline and all overlay transitions"
FOCUS_TIMEOUT = 20

# Darwin SDK sys/un.h and sys/event.h. Python does not export these on all versions.
SOL_LOCAL = 0
LOCAL_PEERPID = 2
NOTE_EXITSTATUS = 0x04000000


def watch_peer(peer, binary, events):
    pid = struct.unpack('=i', peer.getsockopt(SOL_LOCAL, LOCAL_PEERPID, 4))[0]
    pidpath = ctypes.CDLL('/usr/lib/libproc.dylib').proc_pidpath
    pidpath.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
    pidpath.restype = ctypes.c_int
    path = ctypes.create_string_buffer(4096)  # PROC_PIDPATHINFO_MAXSIZE
    if pid <= 0 or pidpath(pid, path, len(path)) <= 0 or Path(os.fsdecode(path.value)).resolve() != binary:
        raise ValueError('launch socket peer is outside this run’s fixture bundle')
    events.control([select.kevent(pid, filter=select.KQ_FILTER_PROC,
                                 flags=select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                                 fflags=select.KQ_NOTE_EXIT | NOTE_EXITSTATUS)], 0, 0)
    return pid


def wait_for_app(events, pid, timeout):
    ready = events.control(None, 1, max(0, timeout))
    if not ready:
        raise TimeoutError('fixture exit was not observed before the deadline')
    event = ready[0]
    if (event.ident != pid or event.filter != select.KQ_FILTER_PROC
            or event.flags & select.KQ_EV_ERROR or not event.fflags & select.KQ_NOTE_EXIT):
        raise ValueError('unexpected kernel process event')
    # A NOTE_EXIT alone proves termination, but cannot supply an exit code.
    return event.data if event.fflags & NOTE_EXITSTATUS else None


def launch_services_focus(binary, native_args, out, env, *, app_env=None, timeout_seconds=None,
                          pass_marker=PASS_MARKER, on_ready=None, foreground=False):
    stdout, stderr = out / 'focus.stdout.log', out / 'focus.stderr.log'
    stdout.touch(mode=0o600, exist_ok=False)
    stderr.touch(mode=0o600, exist_ok=False)
    timeout = FOCUS_TIMEOUT if timeout_seconds is None else timeout_seconds
    started = time.monotonic()
    deadline = started + timeout
    check = {'started_at': timestamp(), 'timeout_seconds': timeout, 'expected_exit_code': 0,
             'required_output': pass_marker, 'status': 'running', 'passed': False,
             'timed_out': False, 'interrupted': False, 'exit_code': None,
             'app_exit_observed': False, 'raw_wait_status': None, 'opener_exit_code': None,
             'exit_source': 'Darwin EVFILT_PROC NOTE_EXITSTATUS',
             'cleanup_scope': 'private app socket and exact kernel process event; opener process group',
             'cleanup_confirmed': False, 'left_descendants': None}
    opener, peer, pid = None, None, None
    # Use /tmp because Darwin sockaddr_un paths are limited to 103 bytes.
    with tempfile.TemporaryDirectory(prefix='vp23.', dir='/tmp') as directory, \
            socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener, closing(select.kqueue()) as events:
        lease = str(Path(directory) / 'lease')
        listener.bind(lease)
        listener.listen(1)
        bundle = binary.parent.parent.parent
        environment_args = [arg for key, value in (app_env or {}).items() for arg in ('--env', f'{key}={value}')]
        argv = ['/usr/bin/open', '-n', *([] if foreground else ['-g']), '-W', '--stdin', '/dev/null',
                '--stdout', str(stdout), '--stderr', str(stderr), *environment_args, str(bundle),
                '--args', *native_args, '--launch-socket', lease]
        check.update(argv=argv, command=argv, cwd=str(ROOT))
        try:
            with deferred_termination() as launching:
                with (out / 'open.stdout.log').open('xb') as open_stdout, \
                        (out / 'open.stderr.log').open('xb') as open_stderr:
                    opener = subprocess.Popen(argv, cwd=ROOT, env=env, stdin=subprocess.DEVNULL,
                                              stdout=open_stdout, stderr=open_stderr,
                                              start_new_session=True)
            if launching:
                raise KeyboardInterrupt
            listener.settimeout(max(0.001, deadline - time.monotonic()))
            peer, _ = listener.accept()
            pid = watch_peer(peer, binary, events)
            check['pid'] = pid
            if time.monotonic() >= deadline:
                raise TimeoutError('fixture launch observation exceeded the deadline')
            peer.settimeout(deadline - time.monotonic())
            peer.sendall(b'\x01')
            if on_ready is not None:
                on_ready(pid, deadline)
            check['raw_wait_status'] = wait_for_app(events, pid, deadline - time.monotonic())
            check['app_exit_observed'] = True
            opener.wait(timeout=max(0, deadline - time.monotonic()))
            check['status'] = 'completed' if opener.returncode == 0 else 'launch_error'
        except (TimeoutError, subprocess.TimeoutExpired) as error:
            check.update(status='timeout', timed_out=True, error=str(error))
        except KeyboardInterrupt:
            check.update(status='interrupted', interrupted=True)
        except (OSError, ValueError) as error:
            check.update(status='launch_error', error=str(error))
        finally:
            def mark_interrupted():
                check.update(status='interrupted', interrupted=True)

            with deferred_termination(mark_interrupted):
                listener.close()
                if peer is not None:
                    peer.close()  # Acknowledged app exits on EOF, even if AppKit is blocked.
                if pid is not None and not check['app_exit_observed']:
                    try:
                        check['raw_wait_status'] = wait_for_app(events, pid, 3)
                        check['app_exit_observed'] = True
                    except (OSError, ValueError) as error:
                        check['app_cleanup_error'] = str(error)
                if opener is not None:
                    try:
                        # Give open -W a bounded chance to collect the app's exit.
                        try:
                            opener.wait(timeout=1)
                        except subprocess.TimeoutExpired:
                            pass
                        check['left_descendants'] = opener.returncode is not None and group_exists(opener.pid)
                        opener_clean = (opener.returncode is not None and not check['left_descendants']
                                        or cleanup_group(opener))
                        check['cleanup_confirmed'] = opener_clean and check['app_exit_observed']
                    except OSError as error:
                        check['opener_cleanup_error'] = str(error)
                        try:
                            if opener.poll() is None:
                                opener.kill()
                            opener.wait(timeout=2)
                        except (OSError, subprocess.TimeoutExpired) as child_error:
                            check['opener_reap_error'] = str(child_error)
                    check['opener_exit_code'] = opener.returncode
    if check['raw_wait_status'] is not None:
        check['exit_code'] = os.waitstatus_to_exitcode(check['raw_wait_status'])
    stdout_facts, marker = log_facts(stdout, pass_marker)
    stderr_facts, _ = log_facts(stderr, None)
    check.update(stdout=str(stdout), stderr=str(stderr), stdout_sha256=stdout_facts['sha256'],
                 stderr_sha256=stderr_facts['sha256'], stdout_bytes=stdout_facts['bytes'],
                 stderr_bytes=stderr_facts['bytes'], finished_at=timestamp(),
                 duration_seconds=time.monotonic() - started)
    check['passed'] = (check['status'] == 'completed' and check['exit_code'] == 0 and marker
                       and check['cleanup_confirmed'] and not check['left_descendants'])
    return check
