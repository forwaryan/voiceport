#!/usr/bin/env python3
"""Explicit synthetic CLI/test-App integration. Never uses microphone or real credentials.

Requires a built package and a NEW evidence directory. Copies the package to a unique
independent bundle/domain and uses only an owned socket/config and loopback fixture.
"""
import argparse
import base64
import hashlib
import http.server
import json
import os
import pathlib
import plistlib
import select
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', required=True, type=pathlib.Path)
    parser.add_argument('--evidence', required=True, type=pathlib.Path)
    parser.add_argument('--continue-after-ui-error', action='store_true',
                        help='finish independent controls after a UI inspection error; overall result still fails')
    args = parser.parse_args()
    evidence = args.evidence.resolve()
    evidence.mkdir(parents=True, exist_ok=False)
    scope = tempfile.TemporaryDirectory(prefix='vpcli-', dir='/private/tmp')
    private = pathlib.Path(scope.name)
    private.chmod(0o700)
    control_socket = private / 'control.sock'
    config = evidence / 'sources.json'
    codex = evidence / 'synthetic-codex'
    codex.mkdir()
    auth = codex / 'auth.json'
    auth.write_text('synthetic invalid JSON, never read by these commands')
    app = evidence / 'test-instance/VoicePort.app'
    shutil.copytree(args.app.resolve(), app)
    domain = 'dev.voiceport.cli.test.' + uuid.uuid4().hex
    info_path = app / 'Contents/Info.plist'
    info = plistlib.loads(info_path.read_bytes())
    info['CFBundleIdentifier'] = domain
    info_path.write_bytes(plistlib.dumps(info))
    subprocess.run(['codesign', '--force', '--sign', '-', str(app)], check=True, capture_output=True)
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True, capture_output=True)
    cli = app / 'Contents/MacOS/voiceport'
    native = app / 'Contents/MacOS/voiceport-app'
    env = dict(os.environ, CODEX_HOME=str(codex), VOICEPORT_CONTROL_SOCKET=str(control_socket),
               VOICEPORT_SOURCES_PATH=str(config))
    # No environment enumeration or auth data is logged.
    checks = []
    observed = []
    slow_started = threading.Event()
    slow_closed = threading.Event()
    release = threading.Event()
    sentinel = evidence / 'adapter-executed'
    synthetic_secret = 'SYNTHETIC-ONLY-' + uuid.uuid4().hex
    replacement_secret = 'SYNTHETIC-REPLACEMENT-' + uuid.uuid4().hex
    expected_secret = synthetic_secret
    credential_refs = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            length = int(self.headers['Content-Length'])
            body = self.rfile.read(length)
            assert b'RIFF' in body and b'WAVE' in body
            assert length < 40000, 'only one-second synthetic sample is allowed'
            if self.path == '/secret':
                assert self.headers.get('Authorization') == 'Bearer ' + expected_secret
            observed.append({'path': self.path, 'bytes': length, 'has_wav': True})
            if self.path == '/slow':
                slow_started.set()
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not release.is_set():
                    ready, _, _ = select.select([self.connection], [], [], 0.05)
                    if ready and self.connection.recv(1, socket.MSG_PEEK) == b'':
                        slow_closed.set()
                        return
                return
            if self.path == '/error':
                self.send_response(500)
                self.end_headers()
                self.wfile.write(b'SYNTHETIC_PRIVATE_SERVICE_BODY')
                return
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(b'{"text":""}')

    service = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    service.daemon_threads = True
    server_thread = threading.Thread(target=service.serve_forever, daemon=True)
    server_thread.start()
    endpoint = 'http://127.0.0.1:' + str(service.server_port)
    proc = None
    app_logs = []
    operation = None
    success = False
    ui_verified = False
    ui_error = None

    def command(words, expected=0, input_data=None):
        argv = [str(cli), '--json', '--socket', str(control_socket), *words]
        result = subprocess.run(argv, env=env, input=input_data, capture_output=True, timeout=8)
        number = len(checks) + 1
        (evidence / f'{number:03}.stdout.log').write_bytes(result.stdout)
        (evidence / f'{number:03}.stderr.log').write_bytes(result.stderr)
        row = {'argv': argv, 'exit': result.returncode, 'expected': expected,
               'stdout_sha256': hashlib.sha256(result.stdout).hexdigest(),
               'stderr_sha256': hashlib.sha256(result.stderr).hexdigest()}
        checks.append(row)
        assert result.returncode == expected, row
        assert result.stderr == b'', row
        value = json.loads(result.stdout)
        assert value['schema_version'] == 1
        assert b'SYNTHETIC_PRIVATE_SERVICE_BODY' not in result.stdout + result.stderr
        assert b'SYNTHETIC_ADAPTER_STDERR' not in result.stdout + result.stderr
        assert synthetic_secret.encode() not in result.stdout + result.stderr
        assert replacement_secret.encode() not in result.stdout + result.stderr
        return value

    def wait(predicate, seconds=8):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            value = predicate()
            if value:
                return value
            time.sleep(0.05)
        raise AssertionError('bounded condition timed out')

    def idle():
        reply = command(['status'])['data']
        return reply if not reply['busy'] and not reply['discovering'] else None

    def launch():
        nonlocal proc
        out = (evidence / f'app-{len(app_logs)}.stdout.log').open('wb')
        err = (evidence / f'app-{len(app_logs)}.stderr.log').open('wb')
        app_logs.append((out, err))
        proc = subprocess.Popen([str(native)], env=env, stdout=out, stderr=err)
        wait(lambda: control_socket.exists() if proc.poll() is None else False)
        status = wait(idle)
        assert status['pid'] == proc.pid
        assert pathlib.Path(status['bundle_path']) == app
        return status

    def quit_app():
        nonlocal proc
        command(['app', 'quit'])
        proc.wait(timeout=8)
        assert proc.returncode == 0
        assert not control_socket.exists(), 'graceful shutdown must remove only its owned socket'
        proc = None

    def result(op, expected=0):
        def ready():
            value = command(['dictation', 'result', op], expected)
            return value if value['status'] == 'error' or 'text' in value['data'] else None
        return wait(ready)

    try:
        assert command(['status'], 3)['error']['code'] == 'app_not_running'
        assert not config.exists() and not control_socket.exists()
        for guide in ['core', 'cli', 'workflows']:
            exported = command(['skills', 'get', guide])['data']['text']
            filename = {'core': 'SKILL.md', 'cli': 'references/cli.md', 'workflows': 'references/workflows.md'}[guide]
            assert exported == (ROOT / 'skills/voiceport' / filename).read_text()
        assert command(['dictation', 'start'], 2)['error']['code'] == 'invalid_arguments'
        launch()
        command(['app', 'show'])
        assert 'transcript' not in command(['status'])['data']
        command(['plugins', 'list'])
        command(['plugins', 'show', 'voiceport.http-transcription'])
        assert command(['sources', 'show', 'missing'], 6)['error']['code'] == 'not_found'
        common = ['--plugin', 'voiceport.http-transcription', '--name', 'Synthetic HTTP', '--enabled', 'true',
                  '--set', 'endpoint=' + endpoint + '/ok', '--set', 'model=synthetic']
        command(['sources', 'add', 'lab-http', *common])
        wait(idle)
        second = common.copy()
        second[3] = 'Synthetic second instance'
        command(['sources', 'add', 'lab-http-2', *second])
        wait(idle)
        assert len(json.loads(config.read_text())['sources']) == 2
        command(['sources', 'select', 'lab-http'])
        assert command(['status'])['data']['source'] == 'lab-http'
        command(['app', 'show', '--settings'])
        ui = subprocess.run(['orca', 'computer', 'get-app-state', '--app', f'pid:{proc.pid}', '--json'], capture_output=True, timeout=20)
        (evidence / 'ui-selected-source.json').write_bytes(ui.stdout)
        (evidence / 'ui-selected-source.stderr.log').write_bytes(ui.stderr)
        if ui.returncode != 0:
            ui_error = json.loads(ui.stderr or ui.stdout)
            assert args.continue_after_ui_error, 'owned test App UI inspection failed; do not request new system permissions'
        else:
            ui_value = json.loads(ui.stdout)
            assert 'Synthetic HTTP' in ui_value['result']['snapshot']['treeText'], 'GUI must display the CLI-selected configured source'
            ui_verified = True
        proposed = command(['sources', 'test', 'lab-http'], 5)
        assert proposed['error']['code'] == 'confirmation_required'
        assert observed == []
        assert command(['sources', 'secret', 'lab-http', 'api_key', '--stdin'], 1, b'')['error']['code'] == 'missing_secret'
        operation = command(['sources', 'test', 'lab-http', '--accept-upload'])['data']['operation']
        answer = result(operation)
        assert answer['data']['text'] == '' and answer['data']['empty'] is True
        assert observed == [{'path': '/ok', 'bytes': observed[0]['bytes'], 'has_wav': True}]
        assert 'text' not in command(['status'])['data']
        # Only newly generated synthetic secrets and this owned Store/Keychain references.
        # Existing Keychain APIs fail instead of displaying authentication UI.
        secret_options = common.copy()
        secret_options[7] = 'endpoint=' + endpoint + '/secret'
        created = command(['sources', 'add', 'lab-secret', *secret_options,
                           '--secret-stdin', 'api_key'], input_data=synthetic_secret.encode())
        credential_refs.append(created['data']['source']['credential_refs']['api_key'])
        assert synthetic_secret not in config.read_text()
        wait(idle)
        operation = command(['sources', 'test', 'lab-secret', '--accept-upload'])['data']['operation']
        assert result(operation)['data']['empty'] is True
        replaced = command(['sources', 'update', 'lab-secret', '--secret-stdin', 'api_key'], input_data=replacement_secret.encode())
        credential_refs.append(replaced['data']['source']['credential_refs']['api_key'])
        assert len(set(credential_refs)) == 2 and replacement_secret not in config.read_text()
        expected_secret = replacement_secret
        wait(idle)
        operation = command(['sources', 'test', 'lab-secret', '--accept-upload'])['data']['operation']
        assert result(operation)['data']['empty'] is True
        command(['sources', 'secret', 'lab-secret', 'api_key', '--remove'])
        wait(idle)
        assert command(['sources', 'show', 'lab-secret'])['data']['credential_refs'] == {}
        command(['sources', 'remove', 'lab-secret'])
        wait(idle)
        keychain_cleanup = []
        for reference in credential_refs:
            lookup = subprocess.run(['security', 'find-generic-password', '-s', 'VoicePort.TranscriptionSources.v1',
                                     '-a', reference], capture_output=True, timeout=5)
            keychain_cleanup.append({'reference': reference, 'metadata_lookup_exit': lookup.returncode})
            assert lookup.returncode == 44, 'only the newly created reference must be absent after replacement/removal'
        (evidence / 'synthetic-keychain-cleanup.json').write_text(json.dumps(keychain_cleanup, indent=2))
        command(['sources', 'update', 'lab-http', '--set', 'endpoint=' + endpoint + '/slow'])
        wait(idle)
        operation = command(['sources', 'test', 'lab-http', '--accept-upload'])['data']['operation']
        assert slow_started.wait(timeout=5)
        assert command(['status'])['data']['busy'] is True
        assert command(['sources', 'remove', 'lab-http-2'], 4)['error']['code'] == 'busy'
        assert command(['app', 'quit'], 4)['error']['code'] == 'busy'
        assert command(['dictation', 'cancel', 'unrelated'], 6)['error']['code'] == 'not_found'
        command(['dictation', 'cancel', operation])
        wait(idle)
        assert result(operation, 130)['error']['code'] == 'cancelled'
        assert slow_closed.wait(timeout=3), 'cancel must release the controlled HTTP connection'
        command(['sources', 'update', 'lab-http', '--enabled', 'false'])
        wait(idle)
        assert command(['status'])['data']['source'] == 'lab-http'
        assert command(['sources', 'select', 'lab-http'], 2)['error']['code'] == 'invalid_config'
        quit_app()
        restarted = launch()
        assert restarted['source'] == 'lab-http'
        command(['sources', 'remove', 'lab-http'])
        wait(idle)
        assert command(['status'])['data']['source'] == 'lab-http'
        quit_app()
        assert launch()['source'] == 'lab-http'
        command(['sources', 'select', 'lab-http-2'])
        command(['sources', 'update', 'lab-http-2', '--set', 'endpoint=' + endpoint + '/error'])
        wait(idle)
        operation = command(['sources', 'test', 'lab-http-2', '--accept-upload'])['data']['operation']
        wait(idle)
        assert result(operation, 1)['error']['code'] == 'plugin_error'
        adapter = evidence / 'synthetic_adapter.py'
        adapter.write_text('import sys,json,base64,pathlib\nr=json.loads(sys.stdin.readline())\nw=base64.b64decode(r["audio"]["base64"])\nassert len(w)==32044 and set(w[44:])=={0}\npathlib.Path(' + repr(str(sentinel)) + ').write_text("synthetic one-second silence")\nprint("SYNTHETIC_ADAPTER_STDERR",file=sys.stderr)\nprint(json.dumps({"protocol_version":1,"event":"final","text":"synthetic plugin result"}))\n')
        manifest = {'schema_version': 1, 'protocol_version': 1, 'id': 'lab.adapter', 'version': '1',
                    'name': 'Synthetic adapter', 'origin': 'controlled local fixture', 'adapter': 'jsonl',
                    'capabilities': {'batch': True, 'streaming': False, 'system_preview': False},
                    'fields': [], 'program': {'executable': sys.executable, 'args': ['-I', str(adapter)]}}
        manifest_path = evidence / 'manifest.json'
        manifest_path.write_text(json.dumps(manifest))
        before = config.read_bytes()
        preview = command(['plugins', 'import', str(manifest_path)], 5)
        assert preview['data']['manifest'] == manifest
        assert config.read_bytes() == before and not sentinel.exists()
        command(['plugins', 'import', str(manifest_path), '--accept-program'])
        wait(idle)
        assert not sentinel.exists()
        command(['plugins', 'show', 'lab.adapter'])
        command(['sources', 'add', 'lab-plugin', '--plugin', 'lab.adapter', '--name', 'Synthetic plugin', '--enabled', 'true'])
        wait(idle)
        operation = command(['sources', 'test', 'lab-plugin', '--accept-upload'])['data']['operation']
        assert result(operation)['data']['text'] == 'synthetic plugin result'
        assert sentinel.read_text() == 'synthetic one-second silence'
        # These frames target only this owned test instance; invalid requests must not change it.
        for payload, code in [(struct.pack('>I', 262145), 'frame_too_large'),
                              (b'\x00\x00\x00\x01?', 'invalid_request')]:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.settimeout(2)
                stream.connect(str(control_socket))
                stream.sendall(payload)
                size = struct.unpack('>I', stream.recv(4))[0]
                data = bytearray()
                while len(data) < size:
                    part = stream.recv(size - len(data))
                    assert part
                    data.extend(part)
                assert json.loads(data)['error']['code'] == code
        assert command(['status'])['data']['source'] == 'lab-http-2'
        quit_app()
        assert auth.read_text() == 'synthetic invalid JSON, never read by these commands'
        success = True
    finally:
        release.set()
        cleanup = 'no live child'
        if proc is not None and proc.poll() is None:
            try:
                if operation is not None:
                    subprocess.run([str(cli), '--json', '--socket', str(control_socket), 'dictation', 'cancel', operation], env=env, capture_output=True, timeout=6)
                if config.exists() and any(s['id'] == 'lab-secret' for s in json.loads(config.read_text())['sources']):
                    removal = subprocess.run([str(cli), '--json', '--socket', str(control_socket), 'sources', 'remove', 'lab-secret'],
                                             env=env, capture_output=True, timeout=6)
                    (evidence / 'credential-cleanup.stdout.log').write_bytes(removal.stdout)
                    (evidence / 'credential-cleanup.stderr.log').write_bytes(removal.stderr)
                subprocess.run([str(cli), '--json', '--socket', str(control_socket), 'app', 'quit'], env=env, capture_output=True, timeout=6)
                proc.wait(timeout=6)
                cleanup = 'owned test child quit'
            except (subprocess.TimeoutExpired, OSError):
                proc.terminate()
                try:
                    proc.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=3)
                cleanup = 'owned test child terminated after failed cleanup'
        service.shutdown()
        service.server_close()
        server_thread.join(timeout=2)
        for out, err in app_logs:
            out.close()
            err.close()
        report = {'passed': success and ui_verified, 'synthetic_controls_passed': success,
                  'ui_verified': ui_verified, 'ui_error': ui_error,
                  'checks': checks, 'controlled_requests': observed,
                  'test_bundle': str(app), 'test_bundle_id': domain, 'socket': str(control_socket),
                  'config': str(config), 'cleanup': cleanup,
                  'scope': 'development self-check, synthetic silence only; no microphone, permissions request, real credentials, production App, user audio or target input'}
        (evidence / 'report.json').write_text(json.dumps(report, indent=2))
        scope.cleanup()
        print(evidence / 'report.json')
    return 0 if success and ui_verified else 1


if __name__ == '__main__':
    sys.exit(main())
