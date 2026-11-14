'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { install } = require('../../scripts/qq_bridge/diagnostic.cjs');
const { createReporter } = require('../../scripts/qq_bridge/report.cjs');

function fixture(exports) {
  const events = [];
  const nativeReturn = {};
  const processObject = {
    dlopen(module, filename, flag) {
      assert.equal(this, processObject);
      assert.equal(flag, 42);
      module.exports = exports;
      return nativeReturn;
    },
  };
  const original = processObject.dlopen;
  const control = install({ processObject, publish: event => events.push(event) });
  const module = {};
  function load(filename = '/synthetic/wrapper.node') {
    assert.equal(processObject.dlopen(module, filename, 42), nativeReturn);
    return module.exports;
  }
  return { processObject, original, control, module, load, events };
}

function nativeFixture() {
  const calls = { create: 0, init: 0, getService: 0, callback: 0, forbidden: 0 };
  const credentials = { token: 'PRIVATE_LOGIN_TOKEN', account: 'PRIVATE_ACCOUNT' };
  const callbackThis = {};
  const callbackResult = {};
  const initResult = {};
  const service = Object.fromEntries([
    'uploadSmartPttSlice', 'addKernelBdhUploadListener', 'removeKernelBdhUploadListener',
  ].map(name => [name, () => { calls.forbidden++; throw new Error(name); }]));
  const listener = {
    onOpentelemetryInit(argument) {
      assert.equal(this, callbackThis);
      assert.equal(argument.is_init, true);
      calls.callback++;
      return callbackResult;
    },
  };
  const session = {
    init(first, actualListener, last) {
      assert.equal(this, session);
      assert.equal(first, credentials);
      assert.equal(last, 'PRIVATE_ARGUMENT');
      calls.init++;
      assert.equal(actualListener.onOpentelemetryInit.call(callbackThis, { is_init: true }), callbackResult);
      return initResult;
    },
    getBdhUploadService() {
      assert.equal(this, session);
      assert.equal(calls.callback, 1, 'QQ callback runs before observation');
      calls.getService++;
      return service;
    },
  };
  const factory = {
    create(arg) {
      assert.equal(this, factory);
      assert.equal(arg, credentials);
      calls.create++;
      return session;
    },
  };
  return { calls, credentials, listener, session, factory, initResult };
}

test('loading and capability observation never create a session or invoke uploads/listeners', () => {
  const native = nativeFixture();
  const f = fixture({ NodeIQQNTWrapperSession: native.factory });
  const exports = f.load();
  assert.equal(native.calls.create, 0);
  assert.equal(native.calls.getService, 0);
  const session = exports.NodeIQQNTWrapperSession.create(native.credentials);
  assert.equal(session, native.session);
  assert.equal(native.calls.getService, 0, 'no service calls before init readiness');
  assert.equal(session.init(native.credentials, native.listener, 'PRIVATE_ARGUMENT'), native.initResult);
  assert.equal(native.calls.getService, 1);
  assert.equal(native.calls.forbidden, 0);
  assert.equal(f.control.snapshot().stage, 'service_observed');
  assert.ok(Object.values(f.control.snapshot().capabilities).every(value => value === true));
  const log = JSON.stringify(f.events);
  for (const secret of ['PRIVATE_LOGIN_TOKEN', 'PRIVATE_ACCOUNT', 'PRIVATE_ARGUMENT']) {
    assert.equal(log.includes(secret), false);
  }
  f.control.stop();
});

test('constructor and inherited constructors preserve instance/prototype identity and factory receivers', () => {
  const marker = {};
  class Session {
    constructor(value) { assert.equal(value, marker); this.value = value; }
    init() { assert.equal(this.value, marker); return marker; }
    static create(value) { assert.equal(this, Session); return new Session(value); }
    static getNTWrapperSession(value) { assert.equal(this, Session); return new Session(value); }
  }
  const f = fixture({ NodeIQQNTWrapperSession: Session });
  const observed = f.load().NodeIQQNTWrapperSession;
  const first = new observed(marker);
  assert.ok(first instanceof Session);
  assert.equal(Object.getPrototypeOf(first), Session.prototype);
  assert.equal(first.init(), marker);
  assert.ok(observed.create(marker) instanceof Session);
  assert.ok(observed.getNTWrapperSession(marker) instanceof Session);
  class Derived extends observed {}
  const derived = new Derived(marker);
  assert.ok(derived instanceof Derived);
  assert.ok(derived instanceof Session);
  f.control.stop();
});

test('immutable native factories remain callable and observable without changing their descriptors', () => {
  for (const callable of [false, true]) {
    const marker = {};
    let factory;
    class Session {
      static #marker = marker;
      constructor(value) { assert.equal(value, marker); this.value = value; }
      init() { return this.value; }
      static create(value) { assert.equal(this.#marker, marker); return new Session(value); }
      static getNTWrapperSession(value) { return this.create(value); }
    }
    factory = callable ? Session : {
      create(value) { assert.equal(this, factory); return new Session(value); },
      getNTWrapperSession(value) { assert.equal(this, factory); return this.create(value); },
    };
    Object.freeze(factory);
    const descriptors = Object.getOwnPropertyDescriptors(factory);
    const f = fixture({ NodeIQQNTWrapperSession: factory });
    const observed = f.load().NodeIQQNTWrapperSession;
    assert.equal(f.control.snapshot().observed.session_created, false);
    assert.notEqual(observed, factory);
    for (const name of ['create', 'getNTWrapperSession']) {
      const session = observed[name](marker);
      assert.ok(session instanceof Session);
      assert.equal(session.init(), marker);
    }
    if (callable) {
      assert.equal(Object.getPrototypeOf(new observed(marker)), Session.prototype);
      class Derived extends observed {}
      const derived = new Derived(marker);
      assert.ok(derived instanceof Derived);
      assert.ok(derived instanceof Session);
    }
    assert.equal(f.control.snapshot().observed.session_created, true);
    assert.deepEqual(Object.getOwnPropertyDescriptors(factory), descriptors);
    f.control.stop();
    const snapshot = f.control.snapshot();
    for (const name of ['create', 'getNTWrapperSession']) {
      const session = observed[name](marker);
      assert.equal(session.init(), marker);
      assert.equal(Object.hasOwn(session, 'init'), false);
    }
    assert.deepEqual(f.control.snapshot(), snapshot);
    assert.equal(f.module.exports.NodeIQQNTWrapperSession, factory);
    assert.deepEqual(Object.getOwnPropertyDescriptors(factory), descriptors);
  }
});

test('an immutable factory export reports the observation limit without changing native behavior', () => {
  const session = { init() {} };
  const factory = Object.freeze({ create() { assert.equal(this, factory); return session; } });
  const exports = Object.freeze({ NodeIQQNTWrapperSession: factory });
  const f = fixture(exports);
  assert.equal(f.load(), exports);
  assert.equal(exports.NodeIQQNTWrapperSession.create(), session);
  assert.equal(f.control.snapshot().observed.session_created, false);
  assert.ok(f.control.snapshot().notes.includes('readonly_session_export'));
  f.control.stop();
});

test('callback throws and native dlopen throws retain the exact original error', () => {
  const originalError = new Error('PRIVATE_CALLBACK_ERROR');
  const listener = { onOpentelemetryInit() { throw originalError; } };
  const session = { init(arg) { arg.onOpentelemetryInit({ is_init: true }); } };
  const f = fixture({ NodeIQQNTWrapperSession: { create: () => session } });
  f.load().NodeIQQNTWrapperSession.create();
  assert.throws(() => session.init(listener), error => error === originalError);
  assert.equal(f.control.snapshot().observed.ready, false);
  assert.equal(JSON.stringify(f.events).includes('PRIVATE_CALLBACK_ERROR'), false);
  f.control.stop();
  const processObject = { dlopen() { throw originalError; } };
  const control = install({ processObject });
  assert.throws(() => processObject.dlopen({}, 'wrapper.node'), error => error === originalError);
  assert.equal(control.snapshot().observed.wrapper_loaded, false);
  control.stop();
});

test('read-only listeners and reporting failures leave QQ calls usable', () => {
  let count = 0;
  const listener = Object.freeze({ onOpentelemetryInit() { count++; } });
  const session = { init(arg) { arg.onOpentelemetryInit({ is_init: true }); return 7; } };
  const f = fixture({ NodeIQQNTWrapperSession: { create: () => session } });
  f.load().NodeIQQNTWrapperSession.create();
  assert.equal(session.init(listener), 7);
  assert.equal(count, 1);
  assert.ok(f.control.snapshot().notes.includes('readonly_init_listener'));
  assert.equal(f.control.snapshot().observed.ready, false);
  f.control.stop();
  const processObject = { dlopen() { return 8; } };
  const control = install({ processObject, publish() { throw new Error('disk full'); } });
  assert.equal(processObject.dlopen({}, 'unrelated.node'), 8);
  control.stop();
});

test('instrumentation getter failures and unrelated native modules do not change native results', () => {
  const exports = {};
  Object.defineProperty(exports, 'NodeIQQNTWrapperSession', {
    get() { throw new Error('PRIVATE_NATIVE_ERROR'); },
  });
  const f = fixture(exports);
  f.load('unrelated.node');
  assert.equal(f.control.snapshot().observed.wrapper_loaded, false);
  f.load();
  assert.ok(f.control.snapshot().notes.includes('observation_failed'));
  assert.equal(JSON.stringify(f.events).includes('PRIVATE_NATIVE_ERROR'), false);
  f.control.stop();
});

test('factory getter failures leave other methods usable with original receivers and exceptions', () => {
  for (const callable of [false, true]) {
    for (const failingName of ['create', 'getNTWrapperSession']) {
      const workingName = failingName === 'create' ? 'getNTWrapperSession' : 'create';
      const factory = callable ? function Session() {} : {};
      const getterError = new Error('PRIVATE_FACTORY_GETTER_ERROR');
      const callError = new Error('PRIVATE_FACTORY_CALL_ERROR');
      const marker = {};
      const session = { init() {} };
      let expectedReceiver = factory;
      let calls = 0;
      Object.defineProperty(factory, failingName, {
        configurable: true, get() { throw getterError; },
      });
      Object.defineProperty(factory, workingName, {
        configurable: true, writable: true,
        value(argument) {
          assert.equal(this, expectedReceiver);
          calls++;
          if (argument === callError) throw callError;
          assert.equal(argument, marker);
          return session;
        },
      });
      const descriptors = Object.getOwnPropertyDescriptors(factory);
      const f = fixture({ NodeIQQNTWrapperSession: factory });
      const observed = f.load().NodeIQQNTWrapperSession;
      assert.ok(f.control.snapshot().notes.includes('observation_failed'));
      assert.equal(observed[workingName](marker), session);
      assert.throws(() => observed[workingName](callError), error => error === callError);
      expectedReceiver = {};
      assert.equal(observed[workingName].call(expectedReceiver, marker), session);
      assert.equal(calls, 3);
      assert.throws(() => observed[failingName], error => error === getterError);
      assert.equal(JSON.stringify(f.events).includes('PRIVATE_FACTORY_'), false);
      f.control.stop();
      assert.deepEqual(Object.getOwnPropertyDescriptors(factory), descriptors);
    }
  }
});

test('retained constructor proxies preserve factory calls after stop without observing new sessions', () => {
  const marker = {};
  const originalError = new Error('PRIVATE_FACTORY_ERROR');
  class Session {
    static #marker = marker;
    constructor(value) { assert.equal(value, marker); this.value = value; }
    init() { return this.value; }
    static create(value) {
      assert.equal(this.#marker, marker);
      if (value === originalError) throw originalError;
      return new this(value);
    }
    static getNTWrapperSession(value) { return this.create(value); }
  }
  const create = Object.getOwnPropertyDescriptor(Session, 'create');
  const get = Object.getOwnPropertyDescriptor(Session, 'getNTWrapperSession');
  const f = fixture({ NodeIQQNTWrapperSession: Session });
  const observed = f.load().NodeIQQNTWrapperSession;
  const retainedCreate = observed.create;
  const retainedGet = observed.getNTWrapperSession;
  assert.equal(observed.create(marker).value, marker);
  f.control.stop();
  assert.equal(f.module.exports.NodeIQQNTWrapperSession, Session);
  assert.deepEqual(Object.getOwnPropertyDescriptor(Session, 'create'), create);
  assert.deepEqual(Object.getOwnPropertyDescriptor(Session, 'getNTWrapperSession'), get);
  const snapshot = f.control.snapshot();
  const eventCount = f.events.length;
  assert.equal(observed.create, retainedCreate);
  assert.equal(observed.getNTWrapperSession, retainedGet);
  for (const name of ['create', 'getNTWrapperSession']) {
    const session = observed[name](marker);
    assert.ok(session instanceof Session);
    assert.equal(session.init(), marker);
    assert.equal(Object.hasOwn(session, 'init'), false, 'stopped observer does not wrap sessions');
    assert.throws(() => observed[name](originalError), error => error === originalError);
  }
  assert.equal(retainedCreate.call(observed, marker).value, marker);
  assert.equal(retainedGet.call(observed, marker).value, marker);
  assert.equal(Object.getPrototypeOf(new observed(marker)), Session.prototype);
  class Derived extends observed {}
  assert.ok(new Derived(marker) instanceof Derived);
  assert.deepEqual(f.control.snapshot(), snapshot);
  assert.equal(f.events.length, eventCount);
  const later = function(value) { assert.equal(this, observed); return value; };
  Session.create = later;
  assert.equal(observed.create, later, 'retained proxy does not replace a later hook');
  assert.equal(observed.create(marker), marker);
  Object.defineProperty(Session, 'getNTWrapperSession', { ...get, configurable: false, writable: false });
  assert.equal(observed.getNTWrapperSession, get.value, 'immutable properties retain their exact value');
});

test('listeners shared by distinct sessions never establish readiness for either session', () => {
  const marker = {};
  const callbackThis = {};
  const originalError = new Error('PRIVATE_SHARED_CALLBACK_ERROR');
  let callbackCalls = 0;
  const serviceCalls = [0, 0];
  const listener = {
    onOpentelemetryInit(argument, trailing) {
      assert.equal(this, callbackThis);
      assert.equal(trailing, marker);
      callbackCalls++;
      if (argument.fail) throw originalError;
      return marker;
    },
  };
  const callback = Object.getOwnPropertyDescriptor(listener, 'onOpentelemetryInit');
  const sessions = serviceCalls.map((_count, index) => ({
    init(actualListener) { assert.equal(this, sessions[index]); assert.equal(actualListener, listener); return marker; },
    getBdhUploadService() { serviceCalls[index]++; return {}; },
  }));
  let index = 0;
  const f = fixture({ NodeIQQNTWrapperSession: { create() { return sessions[index++]; } } });
  const factory = f.load().NodeIQQNTWrapperSession;
  assert.equal(factory.create().init(listener), marker);
  assert.equal(factory.create().init(listener), marker);
  const retainedCallback = listener.onOpentelemetryInit;
  for (let count = 0; count < 2; count++) {
    assert.equal(listener.onOpentelemetryInit.call(callbackThis, { is_init: true }, marker), marker);
  }
  assert.throws(() => listener.onOpentelemetryInit.call(callbackThis, { is_init: true, fail: true }, marker),
    error => error === originalError);
  assert.equal(callbackCalls, 3);
  assert.deepEqual(serviceCalls, [0, 0]);
  const snapshot = f.control.snapshot();
  assert.equal(snapshot.observed.ready, false);
  assert.equal(snapshot.observed.bdh_service, false);
  assert.ok(Object.values(snapshot.capabilities).every(value => value === null));
  assert.ok(snapshot.notes.includes('shared_init_listener'));
  assert.equal(JSON.stringify(f.events).includes('PRIVATE_SHARED_CALLBACK_ERROR'), false);
  f.control.stop();
  assert.deepEqual(Object.getOwnPropertyDescriptor(listener, 'onOpentelemetryInit'), callback);
  assert.equal(retainedCallback.call(callbackThis, { is_init: true }, marker), marker);
  assert.deepEqual(serviceCalls, [0, 0]);
  assert.deepEqual(f.control.snapshot(), snapshot);
});

test('repeated registration of a listener with one session still observes readiness', () => {
  let serviceCalls = 0;
  const listener = { onOpentelemetryInit() {} };
  const session = {
    init(actualListener) { assert.equal(actualListener, listener); },
    getBdhUploadService() { serviceCalls++; return {}; },
  };
  const f = fixture({ NodeIQQNTWrapperSession: { create() { return session; } } });
  f.load().NodeIQQNTWrapperSession.create();
  session.init(listener);
  session.init(listener);
  listener.onOpentelemetryInit({ is_init: true });
  assert.equal(serviceCalls, 1);
  assert.equal(f.control.snapshot().observed.ready, true);
  assert.equal(f.control.snapshot().notes.includes('shared_init_listener'), false);
  f.control.stop();
});

test('missing methods and failed service getters do not report working transcription', () => {
  const listener = { onOpentelemetryInit() {} };
  const session = {
    init(arg) { arg.onOpentelemetryInit({ is_init: true }); },
    getBdhUploadService() { throw new Error('PRIVATE_SERVICE_ERROR'); },
  };
  const f = fixture({ NodeIQQNTWrapperSession: { create: () => session } });
  f.load().NodeIQQNTWrapperSession.create();
  session.init(listener);
  assert.equal(f.control.snapshot().observed.ready, true);
  assert.equal(f.control.snapshot().observed.bdh_service, false);
  assert.equal(f.control.snapshot().capabilities.uploadSmartPttSlice, null);
  assert.ok(f.control.snapshot().notes.includes('bdh_getter_failed'));
  assert.equal(JSON.stringify(f.events).includes('PRIVATE_SERVICE_ERROR'), false);
  f.control.stop();
});

test('stop restores original descriptors and does not overwrite a later hook', () => {
  const native = nativeFixture();
  const create = Object.getOwnPropertyDescriptor(native.factory, 'create');
  const init = Object.getOwnPropertyDescriptor(native.session, 'init');
  const callback = Object.getOwnPropertyDescriptor(native.listener, 'onOpentelemetryInit');
  const f = fixture({ NodeIQQNTWrapperSession: native.factory });
  assert.equal(install({ processObject: f.processObject }), f.control);
  f.load().NodeIQQNTWrapperSession.create(native.credentials);
  native.session.init(native.credentials, native.listener, 'PRIVATE_ARGUMENT');
  f.control.stop();
  f.control.stop();
  assert.equal(f.processObject.dlopen, f.original);
  assert.deepEqual(Object.getOwnPropertyDescriptor(native.factory, 'create'), create);
  assert.deepEqual(Object.getOwnPropertyDescriptor(native.session, 'init'), init);
  assert.deepEqual(Object.getOwnPropertyDescriptor(native.listener, 'onOpentelemetryInit'), callback);
  const other = fixture({});
  const later = () => 1;
  other.processObject.dlopen = later;
  other.control.stop();
  assert.equal(other.processObject.dlopen, later);
});

test('a preloaded wrapper is observed without inventing a session or readiness', () => {
  let calls = 0;
  const session = { init() {} };
  const factory = { create() { calls++; return session; } };
  const module = { filename: '/synthetic/wrapper.node', exports: { NodeIQQNTWrapperSession: factory } };
  const processObject = { dlopen() {} };
  const control = install({ processObject, cachedModules: [module] });
  assert.equal(calls, 0);
  assert.equal(control.snapshot().observed.wrapper_loaded, true);
  assert.equal(control.snapshot().observed.session_created, false);
  assert.equal(control.snapshot().observed.ready, false);
  assert.ok(control.snapshot().notes.includes('session_may_precede_probe'));
  factory.create();
  assert.equal(calls, 1);
  assert.equal(control.snapshot().observed.session_created, true);
  control.stop();
});

test('timeout stays explicit and private reports retain only the latest snapshot', () => {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'voiceport-qq-test-'));
  try {
    const reporter = createReporter({ baseDirectory: base, qqVersion: '7.0.1-52892' });
    const processObject = { dlopen() {} };
    const control = install({ processObject, publish: reporter.publish });
    control.timeout();
    control.stop();
    const report = JSON.parse(fs.readFileSync(reporter.reportPath, 'utf8'));
    assert.equal(report.stage, 'timed_out');
    assert.ok(report.notes.includes('diagnostic_timeout'));
    assert.equal(report.observed.ready, false);
    assert.equal(report.runtime.qq_version, '7.0.1-52892');
    assert.equal(fs.statSync(reporter.reportPath).mode & 0o777, 0o600);
    assert.equal(fs.statSync(path.dirname(reporter.reportPath)).mode & 0o777, 0o700);
    assert.deepEqual(fs.readdirSync(path.dirname(reporter.reportPath)), ['report.json']);
  } finally { fs.rmSync(base, { recursive: true }); }
});
