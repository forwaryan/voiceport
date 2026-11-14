'use strict';

const net = require('node:net');
const path = require('node:path');
const { decodeWav, encodeWav, normalizeSilk } = require('./codec.cjs');
const MAX_INPUT = 8_000_000;
const MAX_WIRE = 4_000_000;
const MAX_RESPONSE = 1_000_000;
const MAX_TIMEOUT_MS = 90_000;
const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
const BRIDGE_ERRORS = new Set([
  'not_ready', 'busy', 'unauthorized', 'invalid_request', 'invalid_audio',
  'transcription_failed', 'timeout', 'cancelled',
  'qq_not_ready', 'qq_busy', 'qq_cleanup_failed', 'invalid_silk',
  'qq_audio_storage_failed', 'qq_response_limit', 'qq_invalid_callback',
  'qq_smartptt_error', 'qq_smartptt_timeout', 'qq_smartptt_call_failed', 'qq_bridge_stopped',
]);

function validateRequest(bytes) {
  if (!Buffer.isBuffer(bytes) || !bytes.length || bytes.length > MAX_INPUT ||
      bytes.indexOf(10) !== bytes.length - 1) throw new Error('invalid_request');
  const request = JSON.parse(decoder.decode(bytes));
  const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
  if (!object(request) || request.protocol_version !== 1 || request.event !== 'transcribe' ||
      request.plugin_id !== 'qq.smartptt' || request.plugin_version !== '1.0.0' ||
      ![null, 'zh', 'en'].includes(request.language) || !object(request.config) ||
      !object(request.audio) || request.audio.format !== 'wav_pcm16_mono_16000') {
    throw new Error('invalid_request');
  }
  const { socket_path: socketPath, pairing_token: token } = request.config;
  if (typeof socketPath !== 'string' || !path.isAbsolute(socketPath) ||
      Buffer.byteLength(socketPath) > 4096 || /[\x00-\x1f\x7f]/.test(socketPath) ||
      typeof token !== 'string' || token.length !== 64 || !/^[0-9a-f]{64}$/.test(token)) throw new Error('invalid_request');
  return { socketPath, token, wav: decodeWav(request.audio.base64) };
}

function run({ input = process.stdin, output = process.stdout, processObject = process,
  encode = encodeWav, timeoutMs = MAX_TIMEOUT_MS } = {}) {
  return new Promise(resolve => {
    let settled = false;
    let socket;
    let inputBytes = 0;
    let inputChunks = [];
    const duration = Math.max(1, Math.min(MAX_TIMEOUT_MS,
      Number.isFinite(timeoutMs) ? timeoutMs : MAX_TIMEOUT_MS));
    const deadline = Date.now() + duration;
    const timer = setTimeout(() => fail('timeout'), duration);
    function finish(record) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      input.removeListener('data', onInput);
      input.removeListener('end', onEnd);
      input.removeListener('error', onInputError);
      input.pause();
      inputChunks = [];
      processObject.removeListener('SIGTERM', onCancel);
      if (socket) socket.destroy();
      let line = JSON.stringify(record) + '\n';
      if (Buffer.byteLength(line) > MAX_RESPONSE) {
        record = { protocol_version: 1, event: 'error', code: 'response_too_large' };
        line = JSON.stringify(record) + '\n';
      }
      const onWriteError = () => resolve(record);
      output.once('error', onWriteError);
      try {
        output.write(line, error => {
          if (!error) output.removeListener('error', onWriteError);
          resolve(record);
        });
      } catch {
        output.removeListener('error', onWriteError);
        resolve(record);
      }
    }
    function fail(code) { finish({ protocol_version: 1, event: 'error', code }); }
    function onCancel() { fail('cancelled'); }
    function onInputError() { fail('invalid_request'); }
    function onInput(chunk) {
      if (settled) return;
      const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
      inputBytes += bytes.length;
      if (inputBytes > MAX_INPUT) { fail('invalid_request'); return; }
      inputChunks.push(bytes);
    }
    function onEnd() {
      if (settled) return;
      const bytes = Buffer.concat(inputChunks, inputBytes);
      inputChunks = [];
      submit(bytes).catch(() => fail('transcription_failed'));
    }
    async function submit(bytes) {
      let request;
      try { request = validateRequest(bytes); }
      catch { fail('invalid_request'); return; }
      let silk;
      try { silk = normalizeSilk(await encode(request.wav)); }
      catch { fail('encoding_failed'); return; }
      if (settled) return;
      if (Date.now() >= deadline) { fail('timeout'); return; }
      const wire = JSON.stringify({ op: 'transcribe', token: request.token, silk_base64: silk.toString('base64') }) + '\n';
      if (Buffer.byteLength(wire) > MAX_WIRE) { fail('audio_too_large'); return; }
      let replyBytes = 0;
      let pending = [];
      let pendingBytes = 0;
      function receive(chunk) {
        if (settled) return;
        replyBytes += chunk.length;
        if (replyBytes > MAX_RESPONSE) { fail('response_too_large'); return; }
        for (let start = 0; start < chunk.length;) {
          const newline = chunk.indexOf(10, start);
          const end = newline === -1 ? chunk.length : newline;
          const part = chunk.subarray(start, end);
          pending.push(part);
          pendingBytes += part.length;
          if (newline === -1) return;
          let response;
          try { response = JSON.parse(decoder.decode(Buffer.concat(pending, pendingBytes))); }
          catch { fail('invalid_response'); return; }
          pending = [];
          pendingBytes = 0;
          if (!response || typeof response !== 'object' || Array.isArray(response)) {
            fail('invalid_response'); return;
          }
          if (response.type === 'final' && typeof response.text === 'string') {
            // Also contain a bridge accidentally echoing its authentication token.
            finish({ protocol_version: 1, event: 'final', text: response.text.split(request.token).join('[redacted]') });
            return;
          }
          if (response.type === 'error') {
            fail(BRIDGE_ERRORS.has(response.code) ? response.code : 'transcription_failed');
            return;
          }
          if (response.type !== 'accepted' && response.type !== 'partial') {
            fail('invalid_response'); return;
          }
          start = newline + 1;
        }
      }
      try {
        socket = net.createConnection({ path: request.socketPath });
        socket.once('connect', () => { if (!settled) socket.write(wire); });
        socket.on('data', receive);
        socket.once('error', () => fail('bridge_unavailable'));
        socket.once('end', () => fail('bridge_disconnected'));
        socket.once('close', () => fail('bridge_disconnected'));
      } catch { fail('bridge_unavailable'); }
    }
    processObject.once('SIGTERM', onCancel);
    input.on('error', onInputError);
    input.on('data', onInput);
    input.once('end', onEnd);
  });
}

module.exports = { run, validateRequest };
if (require.main === module) run();
