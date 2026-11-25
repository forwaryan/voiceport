#!/usr/bin/env python3
"""Run only the synthetic overlay focus fixture in an authorized macOS VM."""
import argparse
from contextlib import closing
import ctypes
import json
import os
from pathlib import Path
import plistlib
import select
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from verification_run import (cleanup_group, deferred_termination, full_suite_lock, group_exists,
                              log_facts, run_check, termination_signals, timestamp, write_report)


ROOT = Path(__file__).resolve().parent.parent
TARGET = ROOT / '.voiceport' / 'overlay-target'
BUNDLE_ID = 'org.voiceport.status-overlay-focus-fixture'
PASS_MARKER = 'status-overlay-focus: passed positive native key/AX baseline and all overlay transitions'
FOCUS_TIMEOUT = 20


def read_sysctl(name):
    result = subprocess.run(['/usr/sbin/sysctl', '-n', name], capture_output=True, text=True,
                            check=False, timeout=3)
    return result.stdout.strip() if result.returncode == 0 else None


def test_executable(build_log):
    with Path(build_log).open() as source:
        matches = {
            Path(event['executable']).resolve(strict=True)
            for line in source if line.startswith('{')
            if (event := json.loads(line)).get('reason') == 'compiler-artifact'
            and event['target']['name'] == 'stop_failure' and event['target']['kind'] == ['test']
            and event.get('executable')
        }
    if len(matches) != 1:
        raise ValueError('Cargo did not identify exactly one stop_failure test executable')
    executable = matches.pop()
    if not executable.is_relative_to(TARGET.resolve()) or not executable.is_file():
        raise ValueError('Cargo returned an invalid executable path for the fixture')
    return executable


def fixture_bundle(directory, executable):
    contents = directory / 'VoicePort Overlay Focus Fixture.app' / 'Contents'
    binary = contents / 'MacOS' / 'status-overlay-focus-fixture'
    binary.parent.mkdir(parents=True)
    shutil.copy2(executable, binary)
    with (contents / 'Info.plist').open('xb') as output:
        plistlib.dump({
            'CFBundleExecutable': binary.name,
            'CFBundleIdentifier': BUNDLE_ID,
            'CFBundleName': 'VoicePort Overlay Focus Fixture',
            'CFBundlePackageType': 'APPL',
            'CFBundleVersion': '1',
        }, output)
    return binary


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


def launch_services_focus(binary, native_args, out, env):
    stdout, stderr = out / 'focus.stdout.log', out / 'focus.stderr.log'
    stdout.touch(mode=0o600, exist_ok=False)
    stderr.touch(mode=0o600, exist_ok=False)
    timeout = FOCUS_TIMEOUT
    started = time.monotonic()
    deadline = started + timeout
    check = {'started_at': timestamp(), 'timeout_seconds': timeout, 'expected_exit_code': 0,
             'required_output': PASS_MARKER, 'status': 'running', 'passed': False,
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
        argv = ['/usr/bin/open', '-n', '-g', '-W', '--stdin', '/dev/null',
                '--stdout', str(stdout), '--stderr', str(stderr), str(bundle),
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
    stdout_facts, marker = log_facts(stdout, PASS_MARKER)
    stderr_facts, _ = log_facts(stderr, None)
    check.update(stdout=str(stdout), stderr=str(stderr), stdout_sha256=stdout_facts['sha256'],
                 stderr_sha256=stderr_facts['sha256'], stdout_bytes=stdout_facts['bytes'],
                 stderr_bytes=stderr_facts['bytes'], finished_at=timestamp(),
                 duration_seconds=time.monotonic() - started)
    check['passed'] = (check['status'] == 'completed' and check['exit_code'] == 0 and marker
                       and check['cleanup_confirmed'] and not check['left_descendants'])
    return check


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--authorized-isolated-vm', nargs=2, required=True,
                        metavar=('BOOT_UUID', 'AQUA_SESSION_ID'),
                        help='attest that this guest console is reserved and authorized for the fixture')
    parser.add_argument('--launch-services', action='store_true',
                        help='use the LaunchServices bundle entry in the authorized guest')
    args = parser.parse_args()
    boot_id, session_id = args.authorized_isolated_vm
    if sys.platform != 'darwin' or read_sysctl('kern.hv_vmm_present') != '1':
        print('status-overlay-focus: refused before build (macOS virtual guest required)', file=sys.stderr)
        return 78
    if not boot_id or read_sysctl('kern.bootsessionuuid') != boot_id or not session_id.isdecimal():
        print('status-overlay-focus: refused before build (invalid boot/session authorization)', file=sys.stderr)
        return 78

    env = dict(os.environ, CARGO_TARGET_DIR=str(TARGET), CARGO_NET_OFFLINE='true',
               CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0', CARGO_INCREMENTAL='0',
               CARGO_BUILD_JOBS='2')
    evidence_root = ROOT / '.voiceport' / 'evidence'
    evidence_root.mkdir(parents=True, exist_ok=True)
    out = Path(tempfile.mkdtemp(prefix='status-overlay-focus.', dir=evidence_root))
    print(out, flush=True)
    report = {'status': 'running', 'passed': False, 'checks': [], 'cwd': str(ROOT)}
    report_path = out / 'report.json'
    write_report(report_path, report)

    def check(name, argv, timeout, marker=None):
        report['active_check'] = argv
        write_report(report_path, report)
        result, _ = run_check(argv, out / f'{name}.stdout.log', out / f'{name}.stderr.log',
                              cwd=ROOT, env=env, timeout=timeout, marker=marker)
        report['checks'].append(result)
        report.pop('active_check')
        write_report(report_path, report)
        return result

    try:
        with full_suite_lock(ROOT), termination_signals():
            result = check('build', ['cargo', 'test', '--offline', '--locked', '-p',
                                     'voiceport-macos', '--test', 'stop_failure', '--no-run',
                                     '--message-format=json'], timeout=360)
            if result['passed']:
                executable = test_executable(result['stdout'])
                with tempfile.TemporaryDirectory(prefix='bundle.', dir=out) as directory:
                    binary = fixture_bundle(Path(directory), executable)
                    native_args = ['--status-overlay-focus', '--authorized-isolated-vm',
                                   boot_id, session_id]
                    if not args.launch_services:
                        result = check('focus', [str(binary), *native_args],
                                       timeout=FOCUS_TIMEOUT, marker=PASS_MARKER)
                    else:
                        result = check('aqua', [str(binary), *native_args, '--attest-aqua'],
                                       timeout=5, marker='launcher Aqua session and fixture bundle verified')
                        if result['passed']:
                            report['active_check'] = ['/usr/bin/open', str(binary.parent.parent.parent)]
                            write_report(report_path, report)
                            result = launch_services_focus(binary.resolve(), native_args, out, env)
                            report['checks'].append(result)
                            del report['active_check']
            exit_code = result['exit_code']
            if args.launch_services and not result['passed']:
                if (result['status'] != 'completed' or not result['cleanup_confirmed']
                        or result['left_descendants'] or exit_code != 78):
                    exit_code = 1
            report.update(status='completed' if result['passed'] else 'check_failed',
                          passed=result['passed'], exit_code=exit_code)
    except (ValueError, OSError, KeyboardInterrupt) as error:
        report.update(status='interrupted' if isinstance(error, KeyboardInterrupt) else 'error',
                      failure=str(error))
    write_report(report_path, report)
    return 0 if report['passed'] else (report.get('exit_code') or 1)


if __name__ == '__main__':
    sys.exit(main())
