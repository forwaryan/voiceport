'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');
const { EventEmitter, once } = require('node:events');
const { createBridge, MAX_REQUEST } = require('../../scripts/qq_bridge/bridge.cjs');
const token = 'a'.repeat(64);
const adminToken = 'b'.repeat(64);
const silk = Buffer.concat([Buffer.from('\x02#!SILK_V3'), Buffer.from([2, 0, 17, 18])]);
const request = () => ({ op: 'transcribe', token, silk_base64: silk.toString('base64') });
async function fixture(t, options = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'vpqq-test-'));
  fs.chmodSync(root, 0o700);
  const tasks = path.join(root, 'tasks'); fs.mkdirSync(tasks, { mode: 0o700 });
  const socketPath = path.join(root, 'bridge.sock');
  const native = { listener: null, calls: [], removed: 0,
    addKernelBdhUploadListener(listener) { assert.equal(this, native); this.listener = listener; },
    uploadSmartPttSlice(id, slice) { assert.equal(this, native); this.calls.push({ id, slice }); },
    removeKernelBdhUploadListener(listener) {
      assert.equal(this, native); assert.equal(listener, this.listener); this.removed++;
    },
  };
  const bridge = createBridge({ socketPath, token, adminToken, dataDirectory: tasks, ...options });
  t.after(() => { bridge.stop(); fs.rmSync(root, { recursive: true, force: true }); });
  await bridge.ready;
  bridge.setService(native);
  return { root, tasks, socketPath, native, bridge };
}
async function client(socketPath, value) {
  const socket = net.createConnection(socketPath);
  const events = new EventEmitter();
  const messages = [];
  let buffer = '';
  let closed = false;
  socket.on('error', () => {});
  socket.on('data', bytes => {
    buffer += bytes.toString();
    for (;;) {
      const index = buffer.indexOf('\n'); if (index < 0) break;
      const message = JSON.parse(buffer.slice(0, index)); buffer = buffer.slice(index + 1);
      messages.push(message); events.emit('message', message);
    }
  });
  const done = new Promise(resolve => socket.once('close', () => { closed = true; resolve(messages); }));
  await once(socket, 'connect');
  socket.write(Buffer.isBuffer(value) ? value : JSON.stringify(value) + '\n');
  return { socket, messages, done, async next(type) {
    for (;;) {
      const match = messages.find(message => message.type === type); if (match) return match;
      if (closed) throw new Error('client_closed_before_' + type);
      await once(events, 'message');
    }
  } };
}
test('native receiver, exact arguments, task isolation, one final and owned cleanup', async t => {
  const f = await fixture(t);
  const c = await client(f.socketPath, request()); await c.next('accepted');
  const { id, slice } = f.native.calls[0];
  assert.match(id, /^[0-9a-f]{30}$/);
  assert.deepEqual({ ...slice, filePath: 'owned' }, { chatType: 7, sliceIndex: 0, filePath: 'owned', voiceOffset: 0, isEnd: true, removePeriod: false });
  assert.ok(slice.filePath.startsWith(f.tasks + path.sep));
  assert.deepEqual(fs.readFileSync(slice.filePath), silk);
  assert.equal(fs.statSync(slice.filePath).mode & 0o777, 0o600);
  f.native.listener.onSmartPttUpdateText('wrong', 'secret unrelated task', true);
  f.native.listener.onSmartPttUpdateText(id, '第一', true);
  f.native.listener.onSmartPttUpdateText(id, '句', false);
  f.native.listener.onSmartPttTranslateFinished('wrong', 0, '');
  f.native.listener.onSmartPttTranslateFinished(id, 0, '');
  f.native.listener.onSmartPttTranslateFinished(id, 0, '');
  f.native.listener.onSmartPttUpdateText(id, 'late', true);
  const messages = await c.done;
  assert.deepEqual(messages.filter(x => x.type === 'final'), [{ type: 'final', text: '第一句' }]);
  assert.equal(JSON.stringify(messages).includes('secret'), false);
  assert.equal(f.native.removed, 1); assert.deepEqual(fs.readdirSync(f.tasks), []);
  assert.deepEqual(f.bridge.snapshot(), { ready: true, busy: false, cleanup_failed: false, submitted: 1, completed: 1, failed: 0, abandoned: 0 });
});
test('not ready and incomplete service never invokes native upload', async t => {
  const f = await fixture(t); f.bridge.setService({ uploadSmartPttSlice() { throw new Error('must not call'); } });
  assert.deepEqual(await (await client(f.socketPath, request())).done, [{ type: 'error', code: 'qq_not_ready' }]);
  assert.deepEqual(fs.readdirSync(f.tasks), []);
});
test('separate admin credential and safe status', async t => {
  const controls = []; const f = await fixture(t, { onControl: op => { controls.push(op); return 'ok'; } });
  assert.deepEqual(await (await client(f.socketPath, { op: 'login', token })).done, []);
  assert.deepEqual(controls, []);
  assert.deepEqual(await (await client(f.socketPath, { op: 'login', token: adminToken })).done, [{ type: 'control', result: 'ok' }]);
  assert.deepEqual(controls, ['login']);
  const status = await (await client(f.socketPath, { op: 'status', token })).done;
  assert.equal(status[0].ready, true); assert.equal(JSON.stringify(status).includes(token), false);
  assert.equal(fs.statSync(f.socketPath).mode & 0o777, 0o600);
});
test('wrong token and unknown fields cause no upload or temporary audio', async t => {
  const f = await fixture(t);
  assert.deepEqual(await (await client(f.socketPath, { ...request(), token: 'c'.repeat(64) })).done, []);
  assert.equal((await (await client(f.socketPath, { ...request(), filePath: '/untrusted' })).done)[0].code, 'unsupported_operation');
  assert.deepEqual(f.native.calls, []); assert.deepEqual(fs.readdirSync(f.tasks), []);
});
test('malformed UTF8, multiple lines and oversized messages cause no upload', async t => {
  const f = await fixture(t);
  for (const value of [Buffer.from([0xff, 10]), Buffer.from('{}\n{}\n'), Buffer.alloc(MAX_REQUEST + 1, 65)]) {
    const c = await client(f.socketPath, value); await c.done;
  }
  assert.deepEqual(f.native.calls, []); assert.deepEqual(fs.readdirSync(f.tasks), []);
});
test('reject invalid framing and more than 180 seconds of SILK records', async t => {
  const f = await fixture(t);
  for (const data of [Buffer.from('bad'), Buffer.concat([Buffer.from('\x02#!SILK_V3'), ...Array.from({ length: 9001 }, () => Buffer.from([1, 0, 9]))])]) {
    const c = await client(f.socketPath, { ...request(), silk_base64: data.toString('base64') });
    assert.equal((await c.done)[0].code, 'invalid_silk');
  }
  assert.equal(f.native.calls.length, 0);
});
test('native failure cannot become successful text or expose native error message', async t => {
  const f = await fixture(t); const c = await client(f.socketPath, request()); await c.next('accepted');
  const { id } = f.native.calls[0];
  f.native.listener.onSmartPttUpdateText(id, 'unfinished', false);
  f.native.listener.onSmartPttTranslateFinished(id, 503, 'private server/account detail');
  assert.deepEqual((await c.done).at(-1), { type: 'error', code: 'qq_smartptt_error', native_error_code: 503 });
  assert.equal(f.native.removed, 1); assert.deepEqual(fs.readdirSync(f.tasks), []);
});
test('disconnect retains task custody and rejects another upload until native completion', async t => {
  const f = await fixture(t); const c = await client(f.socketPath, request()); await c.next('accepted');
  c.socket.destroy(); await c.done;
  const busy = await client(f.socketPath, request());
  assert.deepEqual(await busy.done, [{ type: 'error', code: 'qq_busy' }]);
  assert.equal(f.bridge.snapshot().abandoned, 1); assert.equal(fs.readdirSync(f.tasks).length, 1);
  const { id } = f.native.calls[0]; f.native.listener.onSmartPttTranslateFinished(id, 0, '');
  assert.equal(f.bridge.snapshot().busy, false); assert.equal(f.native.removed, 1);
  assert.deepEqual(fs.readdirSync(f.tasks), []);
});
test('deadline and shutdown clean native listener and owned temporary input', async t => {
  const f = await fixture(t, { timeoutMs: 30 });
  const c = await client(f.socketPath, request()); await c.next('accepted');
  assert.equal((await c.done).at(-1).code, 'qq_smartptt_timeout');
  assert.equal(f.native.removed, 1); assert.deepEqual(fs.readdirSync(f.tasks), []);
  const second = await client(f.socketPath, request()); await second.next('accepted');
  f.bridge.stop(); await second.done;
  assert.equal(f.native.removed, 2); assert.deepEqual(fs.readdirSync(f.tasks), []);
  assert.equal(fs.existsSync(f.socketPath), false);
});
test('reentrant completion during listener registration prevents a later upload', async t => {
  const f = await fixture(t);
  f.native.addKernelBdhUploadListener = function(listener) {
    this.listener = listener;
    // Derive the generated task ID from a captured pre-submit journal.
    listener.onSmartPttTranslateFinished(this.pendingId, 0, '');
  };
  f.bridge.stop();
  const socketPath = path.join(f.root, 'second.sock');
  const bridge = createBridge({ socketPath, token, adminToken, dataDirectory: f.tasks,
    beforeSubmit({ task_id }) { f.native.pendingId = task_id; } });
  t.after(() => bridge.stop()); await bridge.ready; bridge.setService(f.native);
  const c = await client(socketPath, request());
  assert.deepEqual(await c.done, [{ type: 'final', text: '' }]);
  assert.equal(f.native.calls.length, 0); assert.equal(f.native.removed, 1);
});
test('failed durable pre-submit journal prevents all native calls', async t => {
  const f = await fixture(t, { beforeSubmit() { throw new Error('disk full'); } });
  const c = await client(f.socketPath, request());
  assert.equal((await c.done)[0].code, 'qq_smartptt_call_failed');
  assert.equal(f.native.listener, null); assert.equal(f.native.calls.length, 0);
  assert.deepEqual(fs.readdirSync(f.tasks), []);
});
test('native cleanup failure latches unavailable and never launches another task', async t => {
  const f = await fixture(t);
  f.native.removeKernelBdhUploadListener = () => { throw new Error('cleanup failed'); };
  const c = await client(f.socketPath, request()); await c.next('accepted');
  f.native.listener.onSmartPttTranslateFinished(f.native.calls[0].id, 0, ''); await c.done;
  assert.equal(f.bridge.snapshot().cleanup_failed, true);
  assert.equal((await (await client(f.socketPath, request())).done)[0].code, 'qq_cleanup_failed');
  assert.equal(f.native.calls.length, 1); assert.deepEqual(fs.readdirSync(f.tasks), []);
});
test('bounded native callback output fails closed and removes listener', async t => {
  const f = await fixture(t); const c = await client(f.socketPath, request()); await c.next('accepted');
  f.native.listener.onSmartPttUpdateText(f.native.calls[0].id, 'x'.repeat(600000), false);
  assert.equal((await c.done).at(-1).code, 'qq_response_limit'); assert.equal(f.native.removed, 1);
});
test('existing socket path and non-private storage are refused', async t => {
  const f = await fixture(t);
  assert.throws(() => createBridge({ socketPath: f.socketPath, token, adminToken, dataDirectory: f.tasks }), /already_exists/);
  fs.chmodSync(f.tasks, 0o755);
  assert.throws(() => createBridge({ socketPath: path.join(f.root, 'other.sock'), token, adminToken, dataDirectory: f.tasks }), /unsafe_bridge_directory/);
});
