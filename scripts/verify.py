#!/usr/bin/env python3
"""Full local offline Rust development-package verification (optional safe app diagnostics)."""
import argparse
import datetime
import hashlib
import json
import os
import pathlib
import platform
import subprocess
import sys
import tempfile
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from product_inventory import inventory


def safe_inventory():
    try:
        return inventory(ROOT)
    except (OSError, ValueError) as error:
        return {'sha256': None, 'error': type(error).__name__}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app-smoke', action='store_true', help='run safe menu smoke and permission-output diagnostics; no ordinary GUI launch')
    args = parser.parse_args()
    evidence = ROOT / '.voiceport/evidence/vp012-revision-01' / ('verify-' + uuid.uuid4().hex)
    evidence.mkdir(parents=True)
    before = safe_inventory()
    (evidence / 'inputs-before.json').write_text(json.dumps(before, indent=2))
    report = {
        'started_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'system': platform.platform(), 'checks': [], 'source_sha256': before['sha256'],
        'evidence': str(evidence.relative_to(ROOT)),
    }
    success = not before.get('error') and not before.get('missing')
    with tempfile.TemporaryDirectory(prefix='voiceport-verify-') as scope:
        temp = pathlib.Path(scope).resolve()
        home = temp / 'synthetic-codex'
        home.mkdir()
        (home / 'auth.json').write_text('synthetic invalid JSON, not credentials')
        app = temp / 'VoicePort.app'
        env = dict(os.environ, CARGO_NET_OFFLINE='true', CODEX_HOME=str(home), TMPDIR=str(temp),
                   PYTHONDONTWRITEBYTECODE='1', VOICEPORT_APP_DIRECTORY=str(app))
        differences = {key: env[key] for key in ['CARGO_NET_OFFLINE', 'CODEX_HOME', 'TMPDIR', 'PYTHONDONTWRITEBYTECODE', 'VOICEPORT_APP_DIRECTORY']}
        report['environment_overrides'] = differences

        def run(cmd, expected=0, marker=None, timeout=600, cwd=ROOT, input=None):
            nonlocal success
            argv = list(map(str, cmd))
            number = len(report['checks']) + 1
            try:
                result = subprocess.run(argv, cwd=cwd, env=env, input=input, capture_output=True, timeout=timeout)
                out, err, code = result.stdout, result.stderr, result.returncode
            except subprocess.TimeoutExpired as error:
                out, err, code = error.stdout or b'', error.stderr or b'', 124
            except OSError as error:
                out, err, code = b'', str(error).encode(), 127
            stdout, stderr = evidence / f'{number:02d}.stdout.log', evidence / f'{number:02d}.stderr.log'
            stdout.write_bytes(out)
            stderr.write_bytes(err)
            passed = code == expected and (marker is None or marker in (out + err).decode(errors='replace'))
            report['checks'].append({
                'argv': argv, 'command': argv, 'cwd': str(cwd), 'environment_overrides': differences,
                'timeout_seconds': timeout, 'expected_exit_code': expected, 'exit_code': code,
                'required_output': marker, 'passed': passed,
                'stdout': str(stdout.relative_to(ROOT)), 'stderr': str(stderr.relative_to(ROOT)),
                'stdout_sha256': hashlib.sha256(out).hexdigest(), 'stderr_sha256': hashlib.sha256(err).hexdigest(),
            })
            print(f'{number:02d} exit={code} expected={expected} {argv}', flush=True)
            success = success and passed
            return out if passed else None

        checks = [
            (['cargo', 'test', '--workspace', '--all-targets', '--locked'], '0 failed'),
            (['cargo', 'fmt', '--all', '--', '--check'], None),
            (['cargo', 'clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings'], None),
            (['bash', str(ROOT / 'scripts/package_app.sh')], 'Built '),
            (['python3', '-I', '-m', 'unittest', 'discover', '-s', 'Tests/LoopTests', '-v'], 'OK'),
            (['python3', '-I', '-m', 'unittest', 'discover', '-s', 'Tests/PackagingTests', '-v'], 'OK'),
        ]
        for cmd, marker in checks:
            if not success:
                break
            if run(cmd, marker=marker, cwd=temp if cmd[0] == 'bash' else ROOT) is None:
                break
        try:
            if success:
                metadata = run(['cargo', 'metadata', '--offline', '--locked', '--no-deps', '--format-version', '1'])
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
                # The CLI host protocol remains independently verified; no CLI is installed in the app.
                wire = b'{"version":1,"action":"status","request_id":"q"}\n{"version":1,"action":"shutdown","request_id":"end"}\n'
                out = run([rust, 'host', '--auth', str(home / 'auth.json')], marker='shutdown', input=wire, timeout=10)
                (evidence / 'cli-host.stdin.jsonl').write_bytes(wire)
                if out is not None:
                    success = success and [json.loads(line)['event'] for line in out.splitlines()] == ['ready', 'status', 'shutdown']
                report['artifacts'] = {
                    str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else str(path): {
                        'bytes': path.stat().st_size, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                        'mode': path.stat().st_mode & 0o777,
                    } for path in [rust, native, app / 'Contents/MacOS/VoicePort']
                }
                if args.app_smoke:
                    out = run([app / 'Contents/MacOS/VoicePort', '--smoke-test'], marker='menu ready', timeout=20)
                    if out is not None:
                        success = success and out == b'VoicePort Rust app launched: menu ready, no recording or network\n'
                    out = run([app / 'Contents/MacOS/VoicePort', '--check-input-permission'], timeout=10)
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
                  passed=success and before == after, finished_at=datetime.datetime.now(datetime.timezone.utc).isoformat())
    (evidence / 'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print('Evidence:', evidence / 'report.json', flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
