"""Build, then run an Item 45 fixture without initializing VoicePort's user features."""

import argparse
from contextlib import closing
import ctypes
import os
from pathlib import Path
import platform
import plistlib
import re
import select
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
from verification_run import (cleanup_group, deferred_termination, full_suite_lock,
                              group_exists, log_facts, run_check, termination_signals, write_report)

BUNDLE_ID = 'org.voiceport.item45-update-menu-fixture'
EXE = 'item45_update_menu'
NOTE_EXITSTATUS = 0x04000000  # Darwin sys/event.h
AQUA_TIMEOUT = 55
FROZEN_BASE = '2a4a9cf21ebfb92243bc28d410f800d7d85bd91d'


def read(*argv):
    return subprocess.check_output(argv, cwd=ROOT, text=True, timeout=8).strip()


def aqua_identity():
    if (sys.platform, platform.machine()) != ('darwin', 'arm64'):
        raise ValueError('macOS arm64 required')
    if (read('/usr/bin/sw_vers', '-productVersion').split('.')[0] != '15'
            or read('/usr/sbin/sysctl', '-n', 'kern.hv_vmm_present') != '1'):
        raise ValueError('macOS 15 arm64 isolated virtual guest required')
    session = read('/bin/launchctl', 'print', f'gui/{os.geteuid()}')
    audit_id = re.search(r'^\s*asid\s*=\s*(\d+)\s*$', session, re.MULTILINE)
    if not audit_id:
        raise ValueError('Aqua audit session ID unavailable')
    return read('/usr/sbin/sysctl', '-n', 'kern.bootsessionuuid'), audit_id.group(1)


def watch_peer(peer, expected, events):
    pid = struct.unpack('=i', peer.getsockopt(0, 2, 4))[0]  # SOL_LOCAL, LOCAL_PEERPID
    pidpath = ctypes.CDLL('/usr/lib/libproc.dylib').proc_pidpath
    pidpath.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
    pidpath.restype = ctypes.c_int
    buf = ctypes.create_string_buffer(4096)  # PROC_PIDPATHINFO_MAXSIZE
    if pid <= 0 or pidpath(pid, buf, len(buf)) <= 0 or Path(os.fsdecode(buf.value)) != expected:
        raise ValueError('launch peer is outside the exact fixture bundle')
    monitor = select.kevent(pid, filter=select.KQ_FILTER_PROC,
                            flags=select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                            fflags=select.KQ_NOTE_EXIT | NOTE_EXITSTATUS)
    events.control([monitor], 0, 0)
    return pid


def observe_exit(events, pid, timeout):
    got = events.control(None, 1, max(0, timeout))
    if not got:
        raise TimeoutError('exact app exit was not observed')
    event = got[0]
    if (event.ident != pid or event.filter != select.KQ_FILTER_PROC
            or event.flags & select.KQ_EV_ERROR or not event.fflags & select.KQ_NOTE_EXIT):
        raise ValueError('unexpected kernel process event')
    return event.data if event.fflags & NOTE_EXITSTATUS else None


def launch(binary, library, identity, out, report):
    stdout, stderr = out / 'aqua.out', out / 'aqua.err'
    stdout.touch(mode=0o600, exist_ok=False)
    stderr.touch(mode=0o600, exist_ok=False)
    frames = out / 'frames'
    frames.mkdir(mode=0o700)
    deadline = time.monotonic() + AQUA_TIMEOUT
    result = {'stage': 'launch', 'app_exit_observed': False, 'opener_exit_code': None,
              'exit_code': None, 'raw_wait_status': None, 'cleanup_confirmed': False}
    opener, peer, pid = None, None, None
    # sockaddr_un has a short path limit. Use the listener only for this launch.
    with tempfile.TemporaryDirectory(prefix='vp45.', dir='/private/tmp') as lease_dir, \
            socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener, closing(select.kqueue()) as events:
        lease = str(Path(lease_dir) / 'lease')
        listener.bind(lease)
        listener.listen(1)
        argv = ['/usr/bin/open', '-n', '-g', '-W', '--stdin', '/dev/null', '--stdout', str(stdout),
                '--stderr', str(stderr), str(binary.parents[2]), '--args', '--aqua', *identity,
                str(library), str(frames), lease]
        report['launch_intent'] = {'argv': argv, 'native_exit_source': 'Darwin EVFILT_PROC NOTE_EXITSTATUS'}
        write_report(out / 'report.json', report)
        try:
            with deferred_termination() as launch_signals, (out / 'opener.out').open('xb') as open_out, \
                    (out / 'opener.err').open('xb') as open_err:
                opener = subprocess.Popen(argv, cwd=ROOT, start_new_session=True,
                                          stdin=subprocess.DEVNULL, stdout=open_out, stderr=open_err)
            if launch_signals:
                raise KeyboardInterrupt
            listener.settimeout(max(0.001, deadline - time.monotonic()))
            peer, _ = listener.accept()
            pid = watch_peer(peer, binary, events)
            result.update(stage='running', pid=pid)
            report['aqua'] = result
            write_report(out / 'report.json', report)
            peer.settimeout(max(0.001, deadline - time.monotonic()))
            peer.sendall(b'\x01')
            result['raw_wait_status'] = observe_exit(events, pid, deadline - time.monotonic())
            result['app_exit_observed'] = True
            opener.wait(timeout=max(0, deadline - time.monotonic()))
            result['stage'] = 'exited'
        except (OSError, subprocess.TimeoutExpired, ValueError, KeyboardInterrupt) as error:
            result['error'] = f'{type(error).__name__}: {error}'
        finally:
            with deferred_termination():
                if peer is not None:
                    peer.close()  # The leased app exits itself on EOF, even in AppKit tracking.
                if pid is not None and not result['app_exit_observed']:
                    try:
                        result['raw_wait_status'] = observe_exit(events, pid, 3)
                        result['app_exit_observed'] = True
                    except (OSError, ValueError) as error:
                        result['cleanup_error'] = str(error)
                if opener is not None:
                    try:
                        try:
                            opener.wait(timeout=1)
                        except subprocess.TimeoutExpired:
                            pass
                        clean = (opener.returncode is not None and not group_exists(opener.pid)) or cleanup_group(opener)
                        result['cleanup_confirmed'] = clean and result['app_exit_observed']
                    except OSError as error:
                        result['opener_cleanup_error'] = str(error)
                        # This Popen is ours. Never signal the LaunchServices app by numeric PID.
                        if opener.poll() is None:
                            opener.kill()
                        try:
                            opener.wait(timeout=2)
                        except subprocess.TimeoutExpired:
                            pass
                    result['opener_exit_code'] = opener.returncode
    if result['raw_wait_status'] is not None:
        result['exit_code'] = os.waitstatus_to_exitcode(result['raw_wait_status'])
    result['stdout'], _ = log_facts(stdout, None)
    result['stderr'], _ = log_facts(stderr, None)
    result['presentation_verified'] = presentation_verified(stdout.read_text(errors='replace'), frames)
    result['passed'] = (result['stage'] == 'exited' and result['exit_code'] == 0
                        and result['opener_exit_code'] == 0 and result['cleanup_confirmed']
                        and result['presentation_verified'])
    return result


def presentation_verified(text, frames):
    generations = {
        1: ['rate-50', 'stale-50'],
        2: ['reopened-75'],
        3: ['retry-25', 'verifying-100'],
    }
    tracking = re.compile(r"tracking (begin|end) generation=(\d+)")
    sample = re.compile(r"presentation phase=([a-z0-9-]+) generation=(\d+) window=(\d+) tracking=1 ax=1 pixels=1 stage=sampled")
    timeline, samples = [], []
    for line in text.splitlines():
        if match := tracking.fullmatch(line):
            kind, generation = match.groups()
            timeline.append((kind, int(generation)))
        elif line.startswith('presentation phase='):
            match = sample.fullmatch(line)
            if not match:
                return False
            phase, generation, window = match.groups()
            timeline.append((phase, int(generation)))
            samples.append((int(generation), int(window)))
    if timeline != [(phase, generation) for generation, phases in generations.items()
                    for phase in ['begin', *phases, 'end']]:
        return False
    for generation, phases in generations.items():
        windows = {window for gen, window in samples if gen == generation}
        if len(windows) != 1 or 0 in windows:
            return False
        for phase in phases:
            image = frames / f'{phase}.png'
            if not image.is_file() or image.stat().st_size < 32:
                return False
            with image.open('rb') as stream:
                if stream.read(8) != b'\x89PNG\r\n\x1a\n':
                    return False
    return ('PASS item45 aqua action-to-stage' in text and 'synthetic target unchanged and removed' in text)


def bundle(root, executable):
    contents = root / 'VoicePort Item 45 Fixture.app' / 'Contents'
    binary = contents / 'MacOS' / EXE
    binary.parent.mkdir(parents=True)
    shutil.copy2(executable, binary)
    with (contents / 'Info.plist').open('xb') as stream:
        plistlib.dump({'CFBundleIdentifier': BUNDLE_ID, 'CFBundleExecutable': EXE,
                       'CFBundleName': 'VoicePort Item 45 Fixture', 'CFBundlePackageType': 'APPL',
                       'CFBundleVersion': '1', 'LSMinimumSystemVersion': '15.0', 'LSUIElement': True}, stream)
    return binary.resolve()


def verify_inputs(args):
    tree = read('git', 'write-tree')
    if (tree != args.expect_tree or read('git', 'diff', '--name-only')
            or read('git', 'ls-files', '--others', '--exclude-standard')):
        raise ValueError('staged fixture tree or checkout bytes differ from the pin')
    if args.verify_hosted:
        if read('git', 'rev-parse', 'HEAD^') != FROZEN_BASE or read('git', 'rev-parse', 'HEAD^{tree}') != tree:
            raise ValueError('hosted source must be an exact candidate on the frozen parent')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('local', 'aqua'))
    parser.add_argument('--expected-tree', dest='expect_tree', required=True)
    parser.add_argument('--target', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--assignment-root', type=Path, default=ROOT)
    parser.add_argument('--authorized-isolated-vm', nargs=2, metavar=('BOOT_UUID', 'AQUA_ASID'))
    parser.add_argument('--verify-hosted', action='store_true')
    args = parser.parse_args()
    if args.mode == 'aqua':
        try:
            if not args.authorized_isolated_vm or tuple(args.authorized_isolated_vm) != aqua_identity():
                raise ValueError('explicit matching boot UUID and Aqua ASID required')
        except (OSError, subprocess.SubprocessError, ValueError) as error:
            print(f'item45 refused before LaunchServices: {error}', file=sys.stderr)
            return 78
    os.umask(0o077)
    args.out = args.out.resolve()
    target = args.target.resolve()
    report = {'tree': args.expect_tree, 'mode': args.mode, 'status': 'running', 'checks': []}
    env = {**os.environ, 'CARGO_TARGET_DIR': str(target), 'CARGO_NET_OFFLINE': 'true',
           'MACOSX_DEPLOYMENT_TARGET': '15.0'}
    with full_suite_lock(args.assignment_root.resolve()), termination_signals():
        from contextlib import nullcontext
        from verification_run import suite_lock
        lock = suite_lock(ROOT) if args.assignment_root.resolve() != ROOT else nullcontext()
        with lock:
            args.out.mkdir(mode=0o700)
            report_path = args.out / 'report.json'
            write_report(report_path, report)
            def check(name, argv, timeout, marker=None, expected=0):
                report['active_check'] = {'name': name, 'argv': argv}
                write_report(report_path, report)
                result, _ = run_check(argv, args.out / f'{name}.out', args.out / f'{name}.err', cwd=ROOT,
                                      env=env, timeout=timeout, marker=marker, expected=expected)
                report['checks'].append({'name': name, **result})
                report.pop('active_check')
                write_report(report_path, report)
                print(f'{name} exit={result["exit_code"]} passed={result["passed"]}', flush=True)
                if not result['passed']:
                    raise ValueError(f'{name} failed; see local evidence')
            try:
                verify_inputs(args)
                check('format', ['cargo', 'fmt', '--all', '--', '--check'], 45)
                check('build', ['cargo', 'build', '--offline', '--locked', '-j', '2', '-p', 'voiceport-macos',
                                '--features', 'item45-acceptance', '--example', EXE], 420)
                executable = target / 'debug' / 'examples' / EXE
                check('detached', [str(executable), '--detached'], 40, 'synthetic target unchanged and removed')
                library = args.out / 'presentation.dylib'
                check('presentation-build', ['xcrun', 'clang', '-arch', 'arm64', '-mmacosx-version-min=15.0',
                      '-fobjc-arc', '-Wall', '-Wextra', '-Werror', '-dynamiclib', '-framework', 'AppKit',
                      '-framework', 'ApplicationServices', '-framework', 'ScreenCaptureKit', '-framework',
                      'Vision', str(Path(__file__).with_name('presentation.m')), '-o', str(library)], 45)
                with tempfile.TemporaryDirectory(prefix='bundle.', dir=args.out) as directory:
                    binary = bundle(Path(directory), executable)
                    check('fixture-sign', ['/usr/bin/codesign', '--force', '--sign', '-', str(binary.parents[2])], 25)
                    check('fixture-sign-readback', ['/usr/bin/codesign', '--verify', '--strict', str(binary.parents[2])], 25)
                    check('bare-refusal', [str(binary)], 5, expected=78)
                    check('invalid-guest-refusal', [str(binary), '--attest-aqua', 'invalid', '0'], 5, expected=78)
                    if args.mode == 'aqua':
                        check('aqua-attest', [str(binary), '--attest-aqua', *args.authorized_isolated_vm], 8,
                              'isolated Aqua console and fixture bundle verified')
                        report['aqua'] = launch(binary, library, args.authorized_isolated_vm, args.out, report)
                report['bundle_deleted'] = not Path(directory).exists()
                verify_inputs(args)
                if args.mode == 'aqua' and not (report['aqua']['passed'] and report['bundle_deleted']):
                    raise ValueError('Aqua evidence incomplete; inspect exact tracking, AX, pixels, launch and exit')
                report['status'] = 'passed'
            except (OSError, subprocess.SubprocessError, ValueError, KeyboardInterrupt) as error:
                report.update(status='failed', error=f'{type(error).__name__}: {error}')
            finally:
                write_report(report_path, report)
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    sys.exit(main())
