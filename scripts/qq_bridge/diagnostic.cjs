'use strict';

// Capability observer only. Never creates a session or invokes transcription.
const path = require('node:path');
const active = new WeakMap();
const SERVICE_METHODS = Object.freeze([
  'uploadSmartPttSlice', 'addKernelBdhUploadListener',
  'removeKernelBdhUploadListener',
]);

function install({ processObject = process, publish = () => {}, cachedModules = [] } = {}) {
  if (active.has(processObject)) return active.get(processObject);
  const restorers = [];
  const factories = new WeakSet();
  const sessions = new WeakSet();
  const callbacks = new WeakMap();
  let stopped = false;
  const state = {
    schema_version: 1,
    diagnostic_only: true,
    stage: 'installed',
    observed: {
      wrapper_loaded: false, session_created: false,
      init_observed: false, ready: false, bdh_service: false,
    },
    capabilities: {
      getBdhUploadService: null,
      uploadSmartPttSlice: null,
      addKernelBdhUploadListener: null,
      removeKernelBdhUploadListener: null,
    },
    notes: [],
  };
  const snapshot = () => JSON.parse(JSON.stringify(state));
  function emit() {
    if (!stopped) {
      try { publish(snapshot()); } catch { /* Reporting must not break QQ. */ }
    }
  }
  function note(code) {
    if (!state.notes.includes(code) && state.notes.length < 16) {
      state.notes.push(code);
      emit();
    }
  }
  function observe(action) {
    if (stopped) return;
    try { action(); } catch { note('observation_failed'); }
  }
  function replace(object, key, value) {
    const previous = Object.getOwnPropertyDescriptor(object, key);
    if (previous && (!('value' in previous) ||
        (!previous.configurable && !previous.writable))) return false;
    if (!previous && !Object.isExtensible(object)) return false;
    Object.defineProperty(object, key, previous
      ? { ...previous, value }
      : { value, configurable: true, writable: true, enumerable: false });
    restorers.push(() => {
      const current = Object.getOwnPropertyDescriptor(object, key);
      if (!current || current.value !== value) return;
      if (previous) Object.defineProperty(object, key, previous);
      else delete object[key];
    });
    return true;
  }
  function checkService(session) {
    state.observed.ready = true;
    state.stage = 'session_ready';
    const getter = session.getBdhUploadService;
    state.capabilities.getBdhUploadService = typeof getter === 'function';
    if (typeof getter !== 'function') {
      note('bdh_getter_missing');
      emit();
      return;
    }
    // Only the service getter is called; no listeners or upload methods are called.
    let service;
    try { service = Reflect.apply(getter, session, []); }
    catch { note('bdh_getter_failed'); emit(); return; }
    if (!service || typeof service !== 'object' || typeof service.then === 'function') {
      note('bdh_service_unavailable');
      emit();
      return;
    }
    state.observed.bdh_service = true;
    for (const name of SERVICE_METHODS) {
      state.capabilities[name] = typeof service[name] === 'function';
    }
    state.stage = 'service_observed';
    emit();
  }
  function observeListener(listener, session) {
    if (!listener || (typeof listener !== 'object' && typeof listener !== 'function')) return;
    const previous = callbacks.get(listener);
    if (previous) {
      if (previous.session !== session) {
        previous.shared = true;
        note('shared_init_listener');
      }
      return;
    }
    const original = listener.onOpentelemetryInit;
    if (typeof original !== 'function') return;
    const callback = { session, shared: false };
    function onOpentelemetryInit(...args) {
      const result = Reflect.apply(original, this, args);
      observe(() => {
        // A reused listener does not identify which session became ready.
        if (!callback.shared && args[0] && args[0].is_init === true) checkService(session);
      });
      return result;
    }
    if (replace(listener, 'onOpentelemetryInit', onOpentelemetryInit)) {
      callbacks.set(listener, callback);
    } else note('readonly_init_listener');
  }
  function capture(session) {
    if (!session || (typeof session !== 'object' && typeof session !== 'function')) return;
    if (sessions.has(session)) return;
    sessions.add(session);
    state.observed.session_created = true;
    state.stage = 'session_observed';
    const original = session.init;
    if (typeof original !== 'function') {
      note('session_init_missing');
      emit();
      return;
    }
    function init(...args) {
      observe(() => {
        state.observed.init_observed = true;
        state.stage = 'session_initializing';
        // Detect only the known readiness callback; never enumerate login arguments.
        for (const arg of args) observeListener(arg, session);
        emit();
      });
      return Reflect.apply(original, this, args);
    }
    if (!replace(session, 'init', init)) note('readonly_session_init');
    emit();
  }
  function wrapFactory(factory) {
    if (!factory || (typeof factory !== 'object' && typeof factory !== 'function')) return factory;
    if (factories.has(factory)) return factory;
    factories.add(factory);
    let proxy = factory;
    const methods = new Map();
    const readonly = new Map();
    for (const name of ['create', 'getNTWrapperSession']) {
      observe(() => {
        const original = factory[name];
        if (typeof original !== 'function') return;
        function getSession(...args) {
          const result = Reflect.apply(original, this === proxy ? factory : this, args);
          observe(() => capture(result));
          return result;
        }
        if (replace(factory, name, getSession)) {
          methods.set(name, { original, wrapped: getSession });
        } else {
          readonly.set(name, { original, wrapped: getSession });
          note('readonly_session_factory');
        }
      });
    }
    if (readonly.size) {
      // A proxy over a native object's immutable method cannot return a wrapper:
      // that violates Proxy invariants. Use an independent, extensible target.
      // No native descriptor changes; all calls still use the original receiver.
      const target = typeof factory === 'function'
        ? function() {}.bind(null) : Object.create(Object.getPrototypeOf(factory));
      proxy = new Proxy(target, {
        get(_target, key) {
          const value = Reflect.get(factory, key, factory);
          const fixed = readonly.get(key);
          if (fixed && value === fixed.original) return fixed.wrapped;
          const method = methods.get(key);
          if (stopped && method && value === method.original) return method.wrapped;
          return value;
        },
        construct(_target, args, newTarget) {
          const result = Reflect.construct(factory, args, newTarget === proxy ? factory : newTarget);
          observe(() => capture(result));
          return result;
        },
        apply(_target, receiver, args) {
          const result = Reflect.apply(factory, receiver === proxy ? factory : receiver, args);
          observe(() => capture(result));
          return result;
        },
      });
      factories.add(proxy);
      note('readonly_factory_facade');
      return proxy;
    }
    if (typeof factory === 'function') {
      proxy = new Proxy(factory, {
        get(target, key, receiver) {
          const value = Reflect.get(target, key, receiver);
          const method = methods.get(key);
          if (!stopped || !method || value !== method.original) return value;
          const descriptor = Object.getOwnPropertyDescriptor(target, key);
          if (descriptor && (!('value' in descriptor) ||
              (!descriptor.configurable && !descriptor.writable))) return value;
          // Retained proxies still normalize factory receivers after restoration.
          // Later hooks and immutable properties keep their exact current value.
          return method.wrapped;
        },
        construct(target, args, newTarget) {
          const result = Reflect.construct(target, args, newTarget === proxy ? target : newTarget);
          observe(() => capture(result));
          return result;
        },
        apply(target, receiver, args) {
          const result = Reflect.apply(target, receiver, args);
          observe(() => capture(result));
          return result;
        },
      });
      factories.add(proxy);
    }
    return proxy;
  }
  function inspect(exports) {
    state.observed.wrapper_loaded = true;
    state.stage = 'wrapper_observed';
    const factory = exports && exports.NodeIQQNTWrapperSession;
    if (!factory) { note('session_factory_missing'); emit(); return; }
    const wrapped = wrapFactory(factory);
    if (wrapped !== factory && !replace(exports, 'NodeIQQNTWrapperSession', wrapped)) {
      note('readonly_session_export');
    }
    emit();
  }
  const originalDlopen = processObject.dlopen;
  if (typeof originalDlopen !== 'function') throw new TypeError('dlopen_unavailable');
  function dlopen(...args) {
    const result = Reflect.apply(originalDlopen, this, args);
    const [module, filename] = args;
    observe(() => {
      if (typeof filename === 'string' && path.basename(filename) === 'wrapper.node') {
        inspect(module.exports);
      }
    });
    return result;
  }
  if (!replace(processObject, 'dlopen', dlopen)) throw new TypeError('dlopen_readonly');
  const control = {
    snapshot,
    timeout() { observe(() => { state.stage = 'timed_out'; note('diagnostic_timeout'); }); },
    stop() {
      if (stopped) return;
      stopped = true;
      for (const restore of restorers.reverse()) {
        try { restore(); } catch { /* Do not overwrite hooks installed after ours. */ }
      }
      active.delete(processObject);
    },
  };
  active.set(processObject, control);
  observe(() => {
    for (const module of cachedModules) {
      if (module && typeof module.filename === 'string' &&
          path.basename(module.filename) === 'wrapper.node') {
        note('wrapper_already_cached');
        inspect(module.exports);
        // Existing sessions cannot be recovered without calling unknown factories.
        note('session_may_precede_probe');
      }
    }
  });
  emit();
  return control;
}

module.exports = { install };
