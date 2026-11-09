#!/usr/bin/python3
"""JSONL v1 reference adapter for a compatible HTTP transcription service.
No installation, subprocesses, inherited credentials, retries or redirects.
This is transport code, not an ASR engine. Run only through an enabled source.
"""
import base64
import ipaddress
import json
import ssl
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid

MAX_INPUT = 8_000_000
MAX_RESPONSE = 1_000_000
MAX_WAV = 44 + 180 * 16_000 * 2


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def main():
    line = sys.stdin.buffer.readline(MAX_INPUT + 1)
    if len(line) > MAX_INPUT or not line.endswith(b'\n'):
        raise ValueError('input limit')
    request = json.loads(line)
    if request['protocol_version'] != 1 or request['event'] != 'transcribe':
        raise ValueError('protocol')
    config = request['config']
    url = urllib.parse.urlsplit(config['endpoint'])
    loopback = url.hostname == 'localhost'
    try:
        loopback = loopback or ipaddress.ip_address(url.hostname).is_loopback
    except ValueError:
        pass
    if not url.hostname or url.username is not None or url.password is not None or url.fragment:
        raise ValueError('endpoint')
    if url.scheme != 'https' and not (url.scheme == 'http' and loopback):
        raise ValueError('transport')
    audio = request['audio']
    if audio['format'] != 'wav_pcm16_mono_16000':
        raise ValueError('audio format')
    wav = base64.b64decode(audio['base64'], validate=True)
    if len(wav) > MAX_WAV or wav[:4] != b'RIFF' or wav[8:12] != b'WAVE':
        raise ValueError('audio size or format')
    boundary = 'VoicePort' + uuid.uuid4().hex
    body = bytearray()
    values = {'model': config['model'], 'response_format': 'json'}
    if request.get('language'):
        values['language'] = request['language']
    for name, value in values.items():
        body.extend(('--' + boundary + '\r\nContent-Disposition: form-data; name="' + name + '"\r\n\r\n' + value + '\r\n').encode())
    body.extend(('--' + boundary + '\r\nContent-Disposition: form-data; name="file"; filename="dictation.wav"\r\nContent-Type: audio/wav\r\n\r\n').encode())
    body.extend(wav)
    body.extend(('\r\n--' + boundary + '--\r\n').encode())
    headers = {'Content-Type': 'multipart/form-data; boundary=' + boundary}
    if config.get('api_key'):
        headers['Authorization'] = 'Bearer ' + config['api_key']
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect(), urllib.request.HTTPSHandler(context=ssl.create_default_context()))
    req = urllib.request.Request(config['endpoint'], bytes(body), headers, method='POST')
    with opener.open(req, timeout=30) as response:
        if not 200 <= response.status < 300:
            raise ValueError('status')
        result = response.read(MAX_RESPONSE + 1)
    if len(result) > MAX_RESPONSE:
        raise ValueError('response limit')
    value = json.loads(result)
    if not isinstance(value, dict) or not isinstance(value.get('text'), str):
        raise ValueError('response shape')
    output = json.dumps({'protocol_version': 1, 'event': 'final', 'text': value['text']}, ensure_ascii=False).encode()
    if len(output) + 1 > MAX_RESPONSE:
        raise ValueError('output limit')
    sys.stdout.buffer.write(output + b'\n')


if __name__ == '__main__':
    try:
        main()
    except Exception:
        # Never echo input, URL, upstream body, exception details or credentials.
        print('{"protocol_version":1,"event":"error","code":"transcription_failed"}')
