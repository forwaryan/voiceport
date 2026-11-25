#!/usr/bin/env python3
"""Run only the synthetic overlay focus fixture in an authorized macOS VM."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from verification_run import full_suite_lock, run_check, termination_signals, write_report


ROOT = Path(__file__).resolve().parent.parent
TARGET = ROOT / '.voiceport' / 'overlay-target'
BUNDLE_ID = 'org.voiceport.status-overlay-focus-fixture'
PASS_MARKER = 'status-overlay-focus: passed positive native key/AX baseline and all overlay transitions'


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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--authorized-isolated-vm', nargs=2, required=True,
                        metavar=('BOOT_UUID', 'AQUA_SESSION_ID'),
                        help='attest that this guest console is reserved and authorized for the fixture')
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
                    # Direct child execution retains process-group ownership. LaunchServices is unused.
                    result = check('focus', [str(binary), '--status-overlay-focus',
                                             '--authorized-isolated-vm', boot_id, session_id],
                                   timeout=20, marker=PASS_MARKER)
            report.update(status='completed' if result['passed'] else 'check_failed',
                          passed=result['passed'], exit_code=result['exit_code'])
    except (ValueError, OSError, KeyboardInterrupt) as error:
        report.update(status='interrupted' if isinstance(error, KeyboardInterrupt) else 'error',
                      failure=str(error))
    write_report(report_path, report)
    return 0 if report['passed'] else (report.get('exit_code') or 1)


if __name__ == '__main__':
    sys.exit(main())
