'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { install } = require('../../scripts/qq_bridge/diagnostic.cjs');
function fixture(t, options = {}) {
  const events = []; const order = [];
  const service = { uploadSmartPttSlice() { throw new Error('unexpected upload'); },
    addKernelBdhUploadListener() { throw new Error('unexpected listener'); },
    removeKernelBdhUploadListener() { throw new Error('unexpected cleanup'); } };
  const session = { init(listener) { assert.equal(this, session); this.listener = listener; return 17; },
    getBdhUploadService() { assert.equal(this, session); order.push('getter'); return service; } };
  const factory = { create() { assert.equal(this, factory); return session; } };
  const processObject = { dlopen(module) { module.exports = { NodeIQQNTWrapperSession: factory }; return 19; } };
  const original = processObject.dlopen;
  const probe = install({ processObject, publish: value => events.push(value), ...options });
  t.after(() => probe.stop());
  const module = {}; assert.equal(processObject.dlopen(module, '/synthetic/wrapper.node'), 19);
  return { service, session, order, events, probe, original, processObject, factory: module.exports.NodeIQQNTWrapperSession };
}
test('private callbacks receive QQ-owned handles only after exact successful readiness', t => {
  const captures = []; const ready = [];
  const f = fixture(t, { onSession: x => captures.push(x), onReady: x => { f.order.push('ready'); ready.push(x); } });
  assert.deepEqual(captures, []); assert.deepEqual(ready, []);
  assert.equal(f.factory.create(), f.session); assert.equal(f.factory.create(), f.session);
  assert.deepEqual(captures, [{ session: f.session }]);
  const result = {};
  const receiver = {};
  const listener = { onOpentelemetryInit(value) { assert.equal(this, receiver); f.order.push('original'); return result; } };
  assert.equal(f.session.init(listener), 17);
  for (const value of [null, {}, { is_init: false }, { is_init: 1 }]) {
    assert.equal(listener.onOpentelemetryInit.call(receiver, value), result);
  }
  assert.deepEqual(ready, []);
  assert.equal(listener.onOpentelemetryInit.call(receiver, { is_init: true }), result);
  assert.deepEqual(f.order.slice(-3), ['original', 'getter', 'ready']);
  assert.deepEqual(ready, [{ session: f.session, service: f.service }]);
  f.session.toJSON = f.service.toJSON = () => { throw new Error('must not serialize handles'); };
  assert.doesNotThrow(() => JSON.stringify(f.probe.snapshot()));
  assert.doesNotThrow(() => JSON.stringify(f.events));
});
test('throwing original callback never establishes readiness; private callback failure preserves QQ', t => {
  const expected = new Error('original failure'); let ready = 0;
  const f = fixture(t, { onSession() { throw new Error('private'); }, onReady() { ready++; throw new Error('private'); } });
  f.factory.create();
  let throwOriginal = true;
  const listener = { onOpentelemetryInit() { if (throwOriginal) throw expected; return 21; } };
  f.session.init(listener);
  assert.throws(() => listener.onOpentelemetryInit({ is_init: true }), error => error === expected);
  assert.equal(ready, 0); throwOriginal = false;
  assert.equal(listener.onOpentelemetryInit({ is_init: true }), 21); assert.equal(ready, 1);
  f.probe.stop(); assert.equal(f.processObject.dlopen, f.original);
  listener.onOpentelemetryInit({ is_init: true }); assert.equal(ready, 1);
});
test('async private callback rejection is contained and unavailable service stays unavailable', async t => {
  const f = fixture(t, { async onSession() { throw new Error('private'); }, async onReady() { throw new Error('private'); } });
  f.factory.create(); const listener = { onOpentelemetryInit() { return 23; } }; f.session.init(listener);
  assert.equal(listener.onOpentelemetryInit({ is_init: true }), 23);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(f.probe.snapshot().observed.bdh_service, true);
  const g = fixture(t, { onReady() { throw new Error('must not run'); } });
  g.session.getBdhUploadService = () => null; g.factory.create();
  const other = { onOpentelemetryInit() {} }; g.session.init(other); other.onOpentelemetryInit({ is_init: true });
  assert.equal(g.probe.snapshot().observed.bdh_service, false);
});
