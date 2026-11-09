#!/usr/bin/python3
"""Example JSONL v1 bridge, NOT an ASR engine or a provider integration.

Example-only upstream contract: POST JSON {model, audio_base64, language?};
2xx JSON response {result: {transcript: STRING}}. Adapt to actual service docs.
No downloads, inherited credentials, subprocesses, retries or redirects.
"""
import base64
import ipaddress
import json
import ssl
import sys
import urllib.parse
import urllib.request

MAX_INPUT = 8_000_000
MAX_RESPONSE = 1_000_000
MAX_WAV = 5_760_044


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
    # Provider adaptation points: request fields, auth headers, result extraction.
    payload = {'model': config['model'], 'audio_base64': audio['base64']}
    if request.get('language'):
        payload['language'] = request['language']
    headers = {'Content-Type': 'application/json'}
    if config.get('api_key'):
        headers['Authorization'] = 'Bearer ' + config['api_key']
    opener = urllib.request.build_opener(
        urllib.request.ProxyHandler({}), NoRedirect(),
        urllib.request.HTTPSHandler(context=ssl.create_default_context()),
    )
    req = urllib.request.Request(
        config['endpoint'], json.dumps(payload).encode(), headers, method='POST',
    )
    with opener.open(req, timeout=30) as response:
        if not 200 <= response.status < 300:
            raise ValueError('status')
        result = response.read(MAX_RESPONSE + 1)
    if len(result) > MAX_RESPONSE:
        raise ValueError('response limit')
    value = json.loads(result)
    text = value['result']['transcript']
    if not isinstance(text, str):
        raise ValueError('response shape')
    output = json.dumps({'protocol_version': 1, 'event': 'final', 'text': text}, ensure_ascii=False).encode()
    if len(output) + 1 > MAX_RESPONSE:
        raise ValueError('output limit')
    sys.stdout.buffer.write(output + b'\n')


if __name__ == '__main__':
    try:
        main()
    except Exception:
        # Never include input, exception details, service body or credentials.
        print('{"protocol_version":1,"event":"error","code":"transcription_failed"}')
