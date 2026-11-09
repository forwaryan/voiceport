"""Exercise the distributed Skill files with synthetic data and localhost only."""
import base64
import contextlib
import http.server
import io
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
import unittest
import wave

ROOT = pathlib.Path(__file__).resolve().parents[2]
SKILL = ROOT / 'skills/voiceport-plugins'
PREPARE = SKILL / 'scripts/prepare_example.py'
TEMPLATE = SKILL / 'templates/json_http_adapter.py'
SECRET = 'synthetic-skill-test-secret'
ERROR = {'protocol_version': 1, 'event': 'error', 'code': 'transcription_failed'}


def run_python(script, *args, data=b'', env=None):
    return subprocess.run(
        [sys.executable, '-I', str(script), *map(str, args)],
        input=data, capture_output=True, cwd=os.path.abspath(os.sep),
        env=env, timeout=10,
    )


def request(endpoint, language='zh'):
    buffer = io.BytesIO()
    with wave.open(buffer, 'wb') as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(16000)
        audio.writeframes(b'\0' * 32000)
    return {
        'protocol_version': 1, 'event': 'transcribe',
        'plugin_id': 'example.json-http', 'plugin_version': '1.0.0',
        'config': {'endpoint': endpoint, 'model': 'synthetic-model', 'api_key': SECRET},
        'language': language,
        'audio': {'format': 'wav_pcm16_mono_16000', 'base64': base64.b64encode(buffer.getvalue()).decode()},
    }


@contextlib.contextmanager
def fixture():
    received = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            received.append((self.path, body, self.headers.get('Authorization')))
            status = 302 if self.path == '/redirect' else 503 if self.path == '/error' else 200
            self.send_response(status)
            if status == 302:
                self.send_header('Location', '/ok')
            self.end_headers()
            response = {'result': {'transcript': '模拟接口返回文本'}}
            if self.path == '/bad-shape':
                response = {'result': {'transcript': [SECRET]}}
            elif self.path == '/error':
                response = {'error': SECRET}
            elif self.path == '/oversize':
                response = {'result': {'transcript': SECRET * 50000}}
            try:
                self.wfile.write(json.dumps(response).encode())
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
    server.timeout = 1
    thread = threading.Thread(target=lambda: server.serve_forever(poll_interval=0.02))
    thread.start()
    try:
        yield 'http://127.0.0.1:' + str(server.server_port), received
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=3)
        if thread.is_alive():
            raise AssertionError('owned fixture thread was not released')


class PluginSkillTests(unittest.TestCase):
    def adapter(self, payload, script=TEMPLATE, env=None):
        result = run_python(script, data=json.dumps(payload).encode() + b'\n', env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, b'')
        self.assertEqual(len(result.stdout.splitlines()), 1)
        self.assertTrue(result.stdout.endswith(b'\n'))
        self.assertNotIn(SECRET.encode(), result.stdout)
        return json.loads(result.stdout)

    def test_generated_files_run_from_root_with_only_lang_and_refuse_overwrite(self):
        with tempfile.TemporaryDirectory(prefix='vp-skill-') as temp:
            directory = pathlib.Path(temp) / 'example with spaces'
            generated = run_python(PREPARE, '--directory', directory)
            self.assertEqual(generated.returncode, 0, generated.stderr)
            paths = json.loads(generated.stdout)
            self.assertFalse(paths['executed'])
            manifest_path = pathlib.Path(paths['manifest'])
            manifest = json.loads(manifest_path.read_text())
            script = pathlib.Path(paths['adapter'])
            self.assertEqual(script.read_bytes(), TEMPLATE.read_bytes())
            self.assertEqual(manifest['program']['args'], ['-I', str(script)])
            self.assertTrue(pathlib.Path(manifest['program']['executable']).is_absolute())
            self.assertTrue(os.access(manifest['program']['executable'], os.X_OK))
            self.assertEqual(manifest['capabilities'], {'batch': True, 'streaming': False, 'system_preview': False})
            self.assertEqual(manifest['fields'][-1]['kind'], 'secret')
            self.assertLess(manifest_path.stat().st_size, 32768)
            before = {p.name: p.read_bytes() for p in directory.iterdir()}
            refused = run_python(PREPARE, '--directory', directory)
            self.assertEqual(refused.returncode, 1)
            self.assertEqual({p.name: p.read_bytes() for p in directory.iterdir()}, before)
            with fixture() as (url, received):
                payload = request(url + '/ok')
                response = self.adapter(payload, script, env={'LANG': 'C.UTF-8'})
                self.assertEqual(response, {'protocol_version': 1, 'event': 'final', 'text': '模拟接口返回文本'})
                self.assertEqual(len(received), 1)
                self.assertEqual(received[0][1], {'model': 'synthetic-model', 'audio_base64': payload['audio']['base64'], 'language': 'zh'})
                self.assertEqual(received[0][2], 'Bearer ' + SECRET)

    def test_auto_language_is_omitted_from_upstream_request(self):
        with fixture() as (url, received):
            self.adapter(request(url + '/ok', language=None))
            self.assertNotIn('language', received[0][1])

    def test_upstream_failures_are_single_sanitized_errors_without_retries_or_redirects(self):
        with fixture() as (url, received):
            for path in ['/redirect', '/error', '/bad-shape', '/oversize']:
                with self.subTest(path=path):
                    count = len(received)
                    self.assertEqual(self.adapter(request(url + path)), ERROR)
                    self.assertEqual(len(received), count + 1)
                    self.assertEqual(received[-1][0], path)

    def test_invalid_input_and_remote_plaintext_never_contact_fixture(self):
        with fixture() as (url, received):
            for change in [
                lambda r: r.update(protocol_version=2),
                lambda r: r['audio'].update(base64='invalid base64'),
                lambda r: r['config'].update(endpoint='http://example.com/recognize'),
                lambda r: r['config'].update(endpoint=url + '/ok#fragment'),
                lambda r: r['config'].update(endpoint=url.replace('://', '://user:password@') + '/ok'),
            ]:
                payload = request(url + '/ok')
                change(payload)
                self.assertEqual(self.adapter(payload), ERROR)
            self.assertEqual(received, [])
            for data in [b'{}', b'not JSON\n', b'x' * 8_000_001]:
                result = run_python(TEMPLATE, data=data)
                self.assertEqual(result.returncode, 0)
                self.assertEqual(json.loads(result.stdout), ERROR)
                self.assertEqual(result.stderr, b'')

    def test_missing_parent_does_not_create_partial_directories(self):
        with tempfile.TemporaryDirectory(prefix='vp-skill-') as temp:
            directory = pathlib.Path(temp) / 'missing-parent/example'
            result = run_python(PREPARE, '--directory', directory)
            self.assertEqual(result.returncode, 1)
            self.assertFalse(directory.parent.exists())


if __name__ == '__main__':
    unittest.main()
