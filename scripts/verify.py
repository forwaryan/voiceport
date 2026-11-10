#!/usr/bin/env python3
"""Full local offline Rust development-package verification (optional safe app diagnostics)."""
import argparse
import datetime
import hashlib
import json
import os
import pathlib
import platform
import sys
import tempfile
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from product_inventory import inventory
from verification_run import deferred_termination, run_check, suite_lock, termination_signals, write_report


def evidence_path(path):
    path = pathlib.Path(path).absolute()
    return str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else str(path)


def safe_inventory():
    try:
        return inventory(ROOT)
    except (OSError, ValueError) as error:
        return {'sha256': None, 'error': type(error).__name__}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app-smoke', action='store_true', help='run safe menu smoke and permission-output diagnostics; no ordinary GUI launch')
    parser.add_argument('--evidence-directory', type=pathlib.Path, help='new owned evidence directory; never reuse an existing path')
    parser.add_argument('--app-directory', type=pathlib.Path, help='new owned package path; default is a temporary bundle')
    args = parser.parse_args()
    try:
        with suite_lock(ROOT), termination_signals():
            return verify(args)
    except (ValueError, OSError) as error:
        print('verification: ' + str(error), file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130


def verify(args):
    evidence = args.evidence_directory or ROOT / '.voiceport/evidence/vp012-revision-01' / ('verify-' + uuid.uuid4().hex)
    evidence = evidence.absolute()
    evidence.mkdir(parents=True, exist_ok=False)
    report = {
        'started_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'system': platform.platform(), 'checks': [], 'source_sha256': None,
        'evidence': evidence_path(evidence), 'status': 'running', 'passed': False,
        'active_check': None,
    }
    try:
        write_report(evidence / 'report.json', report)
        verify_checks(args, evidence, report)
    except KeyboardInterrupt:
        report.update(interrupted=True, passed=False)
    except Exception as error:
        report.update(passed=False, verification_error={'type': type(error).__name__, 'detail': str(error)})
    finally:
        def save_interruption():
            report.update(interrupted=True, passed=False, status='interrupted')
            write_report(evidence / 'report.json', report)
        with deferred_termination(save_interruption):
            if report['active_check'] is not None:
                # Preserve the unfinished command and log locations without inventing its exit.
                report['incomplete_check'] = report['active_check']
                report['active_check'] = None
                report['passed'] = False
            report['finished_at'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
            report['status'] = 'interrupted' if report.get('interrupted') else 'passed' if report['passed'] else 'failed'
            write_report(evidence / 'report.json', report)
    print('Evidence:', evidence / 'report.json', flush=True)
    return 130 if report.get('interrupted') else 0 if report['passed'] else 1


def verify_checks(args, evidence, report):
    before = safe_inventory()
    report['source_sha256'] = before['sha256']
    (evidence / 'inputs-before.json').write_text(json.dumps(before, indent=2))
    success = not before.get('error') and not before.get('missing')
    with tempfile.TemporaryDirectory(prefix='voiceport-verify-') as scope:
        temp = pathlib.Path(scope).resolve()
        home = temp / 'synthetic-codex'
        home.mkdir()
        (home / 'auth.json').write_text('synthetic invalid JSON, not credentials')
        app = args.app_directory.absolute() if args.app_directory else temp / 'VoicePort.app'
        env = dict(os.environ, CARGO_NET_OFFLINE='true', CODEX_HOME=str(home), TMPDIR=str(temp),
                   PYTHONDONTWRITEBYTECODE='1', VOICEPORT_APP_DIRECTORY=str(app))
        differences = {key: env[key] for key in ['CARGO_NET_OFFLINE', 'CODEX_HOME', 'TMPDIR', 'PYTHONDONTWRITEBYTECODE', 'VOICEPORT_APP_DIRECTORY']}
        report['environment_overrides'] = differences

        def run(cmd, expected=0, marker=None, timeout=600, cwd=ROOT, input=None, read_stdout=False):
            nonlocal success
            if not success:
                return None  # A failure or interruption cannot launch later checks.
            argv = list(map(str, cmd))
            number = len(report['checks']) + 1
            stdout, stderr = evidence / f'{number:02d}.stdout.log', evidence / f'{number:02d}.stderr.log'
            report['active_check'] = {'argv': argv, 'started_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                                      'timeout_seconds': timeout, 'stdout': evidence_path(stdout),
                                      'stderr': evidence_path(stderr)}
            write_report(evidence / 'report.json', report)
            check, out = run_check(argv, stdout, stderr, cwd=cwd, env=env, expected=expected,
                                   marker=marker, timeout=timeout, input=input, read_stdout=read_stdout)
            def save_interruption():
                check.update(status='interrupted', interrupted=True, passed=False)
                report['interrupted'] = True
                write_report(evidence / 'report.json', report)
            with deferred_termination(save_interruption):
                check.update(environment_overrides=differences, stdout=evidence_path(stdout), stderr=evidence_path(stderr))
                report['checks'].append(check)
                report['active_check'] = None
                if check['interrupted']:
                    report['interrupted'] = True
                write_report(evidence / 'report.json', report)
            success = success and check['passed']
            print(f'{number:02d} status={check["status"]} exit={check["exit_code"]} expected={expected} {argv}', flush=True)
            return out if success else None

        checks = [
            (['cargo', 'test', '--workspace', '--all-targets', '--locked'], '0 failed'),
            (['cargo', 'fmt', '--all', '--', '--check'], None),
            (['cargo', 'clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings'], None),
            (['bash', str(ROOT / 'scripts/package_app.sh')], 'Built '),
            (['python3', '-I', '-m', 'unittest', 'discover', '-s', 'Tests/LoopTests', '-v'], 'OK'),
            (['python3', '-I', '-m', 'unittest', 'discover', '-s', '.claude/hooks', '-p', 'test_*.py', '-v'], 'OK'),
            (['python3', '-I', '-m', 'unittest', 'discover', '-s', 'Tests/PackagingTests', '-v'], 'OK'),
        ]
        for cmd, marker in checks:
            if not success:
                break
            if run(cmd, marker=marker, cwd=temp if cmd[0] == 'bash' else ROOT) is None:
                break
        try:
            if success:
                metadata = run(['cargo', 'metadata', '--offline', '--locked', '--no-deps', '--format-version', '1'], read_stdout=True)
                target = pathlib.Path(json.loads(metadata)['target_directory'])
                rust, native = target / 'release/voiceport-rs', target / 'release/voiceport-macos'
                cases = [
                    (['--help'], 0, 'transcribe'), (['demo'], 0, '离线模拟'),
                    (['doctor', '--auth', str(home / 'auth.json')], 0, 'Codex 登录文件'),
                    (['bad-command'], 2, '参数无效'), (['transcribe'], 2, '参数无效'),
                    (['transcribe', str(temp / 'missing.m4a'), '--auth', str(home / 'auth.json')], 1, '登录文件格式无效'),
                ]
                for command, code, marker in cases:
                    run([rust] + command, code, marker, timeout=10)
                run(['python3', '-I', str(ROOT / 'scripts/check_bundle.py'), app, native], marker='main_signed_copy_matches')
                # Portable host compatibility remains independent of the new local control socket.
                public_cli = target / 'release/voiceport'
                run([public_cli, '--json', '--version'], marker='control_schema', timeout=10)
                run([public_cli, '--json', 'skills', 'list'], marker='workflows', timeout=10)
                run([public_cli, '--json', '--socket', pathlib.Path('/private/tmp') / ('vp-absent-' + uuid.uuid4().hex) / 'control.sock', 'status'], 3, 'app_not_running', timeout=10)
                run([public_cli, '--json', 'dictation', 'start'], 2, 'invalid_arguments', timeout=10)
                wire = b'{"version":1,"action":"status","request_id":"q"}\n{"version":1,"action":"shutdown","request_id":"end"}\n'
                out = run([rust, 'host', '--auth', str(home / 'auth.json')], marker='shutdown', input=wire, timeout=10, read_stdout=True)
                (evidence / 'cli-host.stdin.jsonl').write_bytes(wire)
                if out is not None:
                    success = success and [json.loads(line)['event'] for line in out.splitlines()] == ['ready', 'status', 'shutdown']
                report['artifacts'] = {
                    str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else str(path): {
                        'bytes': path.stat().st_size, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                        'mode': path.stat().st_mode & 0o777,
                    } for path in [rust, public_cli, native, app / 'Contents/MacOS/voiceport-app', app / 'Contents/MacOS/voiceport']
                }
                if args.app_smoke:
                    out = run([app / 'Contents/MacOS/voiceport-app', '--smoke-test'], marker='menu ready', timeout=20, read_stdout=True)
                    if out is not None:
                        success = success and out == b'VoicePort Rust app launched: menu ready, no recording or network\n'
                    out = run([app / 'Contents/MacOS/voiceport-app', '--check-input-permission'], timeout=10, read_stdout=True)
                    if out is not None:
                        diagnostic = json.loads(out)
                        success = success and all(type(diagnostic.get(key)) is bool for key in ['accessibilityTrusted', 'secureInput'])
                        success = success and all(isinstance(diagnostic.get(key), str) for key in ['bundlePath', 'version'])
                        success = success and diagnostic.get('implementation') == 'rust'
                    report['permission_diagnostic_scope'] = 'Direct-execution output shape only; effective Launch Services/TCC identity is not verified.'
        except Exception as error:
            success = False
            report['verification_error'] = {'type': type(error).__name__, 'detail': str(error)}
        report['normal_package_includes'] = ['cargo build --workspace --release --locked']
        report['unverified'] = ['real auth/cloud/microphone/target reception', 'Windows', 'active GUI exit',
                                'effective Launch Services/TCC identity', 'notarization/external distribution/legal approval']
    after = safe_inventory()
    (evidence / 'inputs-after.json').write_text(json.dumps(after, indent=2))
    report.update(source_sha256_at_finish=after['sha256'], inputs_unchanged=before == after,
                  passed=success and before == after and not report.get('interrupted', False))


if __name__ == '__main__':
    sys.exit(main())
