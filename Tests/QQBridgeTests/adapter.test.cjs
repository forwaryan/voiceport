'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const net = require('node:net');
const path = require('node:path');
const { once, EventEmitter } = require('node:events');
const { PassThrough, Writable } = require('node:stream');
const { run, validateRequest } = require('../../scripts/qq_bridge/adapter.cjs');

// Only synthetic in-memory PCM, opaque frame records, and temporary test sockets.
const token = 'ab'.repeat(32);
const header = Buffer.from('\x02#!SILK_V3', 'ascii');
const silk = Buffer.concat([header, Buffer.from([1, 0, 7])]);
function wav() {
  const bytes = Buffer.alloc(44 + 640);
  bytes.write('RIFF', 0);
  bytes.writeUInt32LE(bytes.length - 8, 4);
  bytes.write('WAVEfmt ', 8);
  bytes.writeUInt32LE(16, 16);
  bytes.writeUInt16LE(1, 20);
  bytes.writeUInt16LE(1, 22);
  bytes.writeUInt32LE(16000, 24);
  bytes.writeUInt32LE(32000, 28);
  bytes.writeUInt16LE(2, 32);
  bytes.writeUInt16LE(16, 34);
  bytes.write('data', 36);
  bytes.writeUInt32LE(640, 40);
  return bytes;
}
function request(socketPath = '/synthetic/not-connected.sock') {
  return {
    protocol_version: 1, event: 'transcribe', plugin_id: 'qq.smartptt', plugin_version: '1.0.0',
    config: { socket_path: socketPath, pairing_token: token }, language: null,
    audio: { format: 'wav_pcm16_mono_16000', base64: wav().toString('base64') },
  };
}
const line = value => JSON.stringify(value) + '\n';
const turn = () => new Promise(resolve => setImmediate(resolve));

function invocation(t, options = {}) {
  const input = new PassThrough();
  const outputs = [];
  const output = new Writable({ write(chunk, _encoding, callback) { outputs.push(Buffer.from(chunk)); callback(); } });
  const processObject = new EventEmitter();
  const encodeCalls = [];
  const done = run({
    input, output, processObject,
    encode: async bytes => { encodeCalls.push(bytes); return silk; },
    timeoutMs: 1000,
    ...options,
  });
  t.after(async () => { processObject.emit('SIGTERM'); await done; input.destroy(); output.destroy(); });
  return {
    input, output, processObject, encodeCalls, done,
    stdout: () => Buffer.concat(outputs).toString('utf8'),
  };
}
function onlyRecord(f, expected) {
  const stdout = f.stdout();
  assert.ok(stdout.endsWith('\n'));
  assert.equal(stdout.split('\n').length, 2, 'exactly one JSONL result');
  assert.equal(stdout.includes(token), false, 'stdout never contains the pairing token');
  assert.ok(Buffer.byteLength(stdout) <= 1_000_000);
  const record = JSON.parse(stdout);
  assert.equal(record.protocol_version, 1);
  if (expected) assert.deepEqual(record, { protocol_version: 1, ...expected });
  assert.equal(f.processObject.listenerCount('SIGTERM'), 0);
  return record;
}
async function bridgeFixture(t, respond) {
  const directory = fs.mkdtempSync('/tmp/vp-qq-adapter-');
  const socketPath = path.join(directory, 's');
  const sockets = new Set();
  const requests = [];
  const wires = [];
  let connections = 0;
  let received;
  let closed;
  const seen = new Promise(resolve => { received = resolve; });
  const disconnected = new Promise(resolve => { closed = resolve; });
  const server = net.createServer(socket => {
    connections++;
    sockets.add(socket);
    socket.on('error', () => {});
    socket.on('close', () => { sockets.delete(socket); closed(); });
    let bytes = Buffer.alloc(0);
    socket.on('data', chunk => {
      bytes = Buffer.concat([bytes, chunk]);
      assert.ok(bytes.length <= 4_000_000);
      const newline = bytes.indexOf(10);
      if (newline === -1) return;
      assert.equal(newline, bytes.length - 1);
      const value = JSON.parse(bytes.toString('utf8'));
      requests.push(value);
      wires.push(Buffer.from(bytes));
      received(value);
      respond(socket, value);
      bytes = Buffer.alloc(0);
    });
  });
  server.listen(socketPath);
  await once(server, 'listening');
  t.after(async () => {
    for (const socket of sockets) socket.destroy();
    await new Promise(resolve => server.close(resolve));
    fs.rmSync(directory, { recursive: true });
  });
  return { socketPath, requests, wires, seen, disconnected, connections: () => connections };
}

test('validates the plugin identity, auth configuration, language, and audio before encoding', async t => {
  const mutations = [
    value => { value.protocol_version = 2; }, value => { value.event = 'other'; },
    value => { value.plugin_id = 'other'; }, value => { value.plugin_version = '2.0.0'; },
    value => { value.language = 'fr'; }, value => { delete value.language; },
    value => { value.config = []; }, value => { value.config.socket_path = ''; },
    value => { value.config.socket_path = 'relative.sock'; }, value => { value.config.socket_path = '/tmp/a\0b'; },
    value => { value.config.pairing_token = token.toUpperCase(); },
    value => { value.config.pairing_token = token.slice(1); },
    value => { value.config.pairing_token = token + '\n'; },
    value => { value.config.pairing_token = 123; },
    value => { value.audio.format = 'mp3'; }, value => { value.audio.base64 += '\n'; },
    value => { value.audio.base64 = Buffer.alloc(44).toString('base64'); },
  ];
  for (const mutate of mutations) {
    const value = request();
    mutate(value);
    const f = invocation(t);
    f.input.end(line(value));
    await f.done;
    onlyRecord(f, { event: 'error', code: 'invalid_request' });
    assert.equal(f.encodeCalls.length, 0);
  }
  for (const language of [null, 'zh', 'en']) {
    const value = request();
    value.language = language;
    const parsed = validateRequest(Buffer.from(line(value)));
    assert.deepEqual(parsed.wav, wav());
    assert.equal(parsed.token, token);
    assert.equal(parsed.socketPath, value.config.socket_path);
  }
});

test('rejects malformed UTF-8/JSON, missing newline, and any second input line', async t => {
  const valid = line(request());
  const invalid = ['null\n', '[]\n', '{\n', '', valid.trimEnd(), valid + '\n', valid + valid,
    Buffer.concat([Buffer.from([255]), Buffer.from(valid)])];
  for (const bytes of invalid) {
    const f = invocation(t);
    f.input.end(bytes);
    await f.done;
    onlyRecord(f, { event: 'error', code: 'invalid_request' });
    assert.equal(f.encodeCalls.length, 0);
  }
  const f = invocation(t);
  f.input.write(valid);
  await turn();
  assert.equal(f.encodeCalls.length, 0, 'wait for EOF to detect later input lines');
  f.input.end(valid);
  await f.done;
  onlyRecord(f, { event: 'error', code: 'invalid_request' });
  assert.equal(f.encodeCalls.length, 0);
});

test('accepts the exact 8 MB request budget and rejects overflow before EOF', async t => {
  const valid = line(request());
  const exact = Buffer.from(valid.slice(0, -1) + ' '.repeat(8_000_000 - Buffer.byteLength(valid)) + '\n');
  assert.equal(exact.length, 8_000_000);
  assert.deepEqual(validateRequest(exact).wav, wav());
  const f = invocation(t);
  f.input.write(Buffer.alloc(8_000_001, 32));
  await f.done;
  onlyRecord(f, { event: 'error', code: 'invalid_request' });
  assert.equal(f.encodeCalls.length, 0);
});

test('actual UDS request carries only authentication and framed SILK; only bridge final reaches stdout', async t => {
  const bridge = await bridgeFixture(t, socket => {
    socket.write(line({ type: 'accepted' }) + line({ type: 'partial', text: 'SYNTHETIC_PARTIAL_MUST_NOT_ESCAPE' }));
    const final = Buffer.from(line({ type: 'final', text: '合成夹具结果' }));
    const split = final.indexOf(Buffer.from('合')) + 1;
    socket.write(final.subarray(0, split));
    setImmediate(() => socket.end(final.subarray(split)));
  });
  const f = invocation(t);
  const bytes = Buffer.from(line(request(bridge.socketPath)));
  f.input.write(bytes.subarray(0, 17));
  f.input.end(bytes.subarray(17));
  await f.done;
  onlyRecord(f, { event: 'final', text: '合成夹具结果' });
  assert.equal(f.encodeCalls.length, 1);
  assert.deepEqual(f.encodeCalls[0], wav());
  assert.deepEqual(bridge.requests, [{ op: 'transcribe', token, silk_base64: silk.toString('base64') }]);
  assert.ok(bridge.wires[0].length <= 4_000_000);
  assert.equal(f.stdout().includes('SYNTHETIC_PARTIAL'), false);
  await bridge.disconnected;
  assert.equal(bridge.connections(), 1);
});

test('not-ready/auth failures return immediately without retries or upstream error details', async t => {
  for (const code of ['not_ready', 'unauthorized', 'busy', token]) {
    const bridge = await bridgeFixture(t, socket => socket.end(line({ type: 'error', code, text: token })));
    const f = invocation(t);
    f.input.end(line(request(bridge.socketPath)));
    await f.done;
    onlyRecord(f, { event: 'error', code: code === token ? 'transcription_failed' : code });
    await bridge.disconnected;
    assert.equal(bridge.connections(), 1);
  }
});

test('empty final is allowed and duplicate final/error messages never produce a second result', async t => {
  const bridge = await bridgeFixture(t, socket => socket.end(
    line({ type: 'final', text: '' }) + line({ type: 'final', text: 'second' }) + line({ type: 'error', code: 'timeout' }),
  ));
  const f = invocation(t);
  f.input.end(line(request(bridge.socketPath)));
  await f.done;
  onlyRecord(f, { event: 'final', text: '' });
});

test('bridge final cannot echo the pairing secret to stdout', async t => {
  const bridge = await bridgeFixture(t, socket => socket.end(line({ type: 'final', text: `synthetic ${token}` })));
  const f = invocation(t);
  f.input.end(line(request(bridge.socketPath)));
  await f.done;
  onlyRecord(f, { event: 'final', text: 'synthetic [redacted]' });
});

test('disconnect after accepted/partial yields an error, never a partial fallback', async t => {
  const bridge = await bridgeFixture(t, socket => socket.end(
    line({ type: 'accepted' }) + line({ type: 'partial', text: 'synthetic partial' }),
  ));
  const f = invocation(t);
  f.input.end(line(request(bridge.socketPath)));
  await f.done;
  onlyRecord(f, { event: 'error', code: 'bridge_disconnected' });
  assert.equal(bridge.connections(), 1);
});

test('socket failure is bounded and never echoes the configured path or credentials', async t => {
  const f = invocation(t);
  f.input.end(line(request('/tmp/voiceport-nonexistent-' + token + '/socket')));
  await f.done;
  onlyRecord(f, { event: 'error', code: 'bridge_unavailable' });
});

test('malformed or unknown bridge messages fail without exposing raw contents', async t => {
  for (const response of [
    'not-json-' + token + '\n', line({ type: 'unknown', text: token }),
    line({ type: 'final', text: 123 }), 'null\n', '[]\n', '\n', Buffer.from([255, 10]),
  ]) {
    const bridge = await bridgeFixture(t, socket => socket.end(response));
    const f = invocation(t);
    f.input.end(line(request(bridge.socketPath)));
    await f.done;
    onlyRecord(f, { event: 'error', code: 'invalid_response' });
  }
});

test('1 MB reply budget includes accepted/partial messages and incomplete lines', async t => {
  const bridge = await bridgeFixture(t, socket => {
    socket.write(line({ type: 'accepted' }));
    socket.end(Buffer.alloc(1_000_000, 32));
  });
  const f = invocation(t);
  f.input.end(line(request(bridge.socketPath)));
  await f.done;
  onlyRecord(f, { event: 'error', code: 'response_too_large' });
  await bridge.disconnected;
});

test('a bridge final that would exceed the stdout budget becomes one fixed error', async t => {
  const overhead = Buffer.byteLength(line({ type: 'final', text: '' }));
  const response = line({ type: 'final', text: 'x'.repeat(1_000_000 - overhead) });
  assert.equal(Buffer.byteLength(response), 1_000_000);
  const bridge = await bridgeFixture(t, socket => socket.end(response));
  const f = invocation(t);
  f.input.end(line(request(bridge.socketPath)));
  await f.done;
  onlyRecord(f, { event: 'error', code: 'response_too_large' });
});

test('codec failures produce a fixed error without connecting or leaking exception details', async t => {
  const bridge = await bridgeFixture(t, () => {});
  const f = invocation(t, { encode: async () => { throw new Error(token); } });
  f.input.end(line(request(bridge.socketPath)));
  await f.done;
  onlyRecord(f, { event: 'error', code: 'encoding_failed' });
  assert.equal(bridge.connections(), 0);
});

test('the 4 MB authenticated wire budget includes its JSON envelope', async t => {
  const large = Buffer.alloc(3_000_000, 7);
  header.copy(large);
  for (let offset = header.length; offset < large.length;) {
    const size = Math.min(32767, large.length - offset - 2);
    large.writeInt16LE(size, offset);
    offset += 2 + size;
  }
  const bridge = await bridgeFixture(t, () => {});
  const f = invocation(t, { encode: async () => large });
  f.input.end(line(request(bridge.socketPath)));
  await f.done;
  onlyRecord(f, { event: 'error', code: 'audio_too_large' });
  assert.equal(bridge.connections(), 0);
});

test('timeout while reading input is bounded and removes its SIGTERM handler', async t => {
  const f = invocation(t, { timeoutMs: 20 });
  await f.done;
  onlyRecord(f, { event: 'error', code: 'timeout' });
  assert.equal(f.input.readableFlowing, false);
});

test('timeout during encoding emits once and late encoder completion cannot open a socket', async t => {
  let complete;
  const encoding = new Promise(resolve => { complete = resolve; });
  const bridge = await bridgeFixture(t, () => {});
  const f = invocation(t, { encode: () => encoding, timeoutMs: 20 });
  f.input.end(line(request(bridge.socketPath)));
  await f.done;
  onlyRecord(f, { event: 'error', code: 'timeout' });
  complete(silk);
  await turn();
  assert.equal(bridge.connections(), 0);
  onlyRecord(f, { event: 'error', code: 'timeout' });
});

test('timeout on a live bridge destroys the actual socket without retries', async t => {
  const bridge = await bridgeFixture(t, socket => socket.write(line({ type: 'accepted' })));
  const f = invocation(t, { timeoutMs: 30 });
  f.input.end(line(request(bridge.socketPath)));
  await bridge.seen;
  await f.done;
  onlyRecord(f, { event: 'error', code: 'timeout' });
  await bridge.disconnected;
  assert.equal(bridge.connections(), 1);
});

test('SIGTERM cancellation destroys the actual socket and emits exactly one error', async t => {
  const bridge = await bridgeFixture(t, socket => socket.write(line({ type: 'accepted' })));
  const f = invocation(t);
  f.input.end(line(request(bridge.socketPath)));
  await bridge.seen;
  f.processObject.emit('SIGTERM');
  await f.done;
  onlyRecord(f, { event: 'error', code: 'cancelled' });
  await bridge.disconnected;
  assert.equal(bridge.connections(), 1);
});
