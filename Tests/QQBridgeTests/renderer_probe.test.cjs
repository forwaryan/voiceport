'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const vm = require('node:vm');
const { once } = require('node:events');
const { applicationTarget, boundedJson, probe } = require('../../scripts/qq_bridge/renderer_probe.cjs');

function target(port, changes = {}) {
  return { type: 'page', url: 'app://./index.html',
    webSocketDebuggerUrl: `ws://127.0.0.1:${port}/devtools/page/fixture`, ...changes };
}

async function endpoint(t, items) {
  const server = http.createServer((_req, res) => {
    res.setHeader('Content-Type', 'application/json');
    res.end(JSON.stringify(items(server.address().port)));
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(() => new Promise(resolve => { server.close(resolve); server.closeAllConnections(); }));
  return server.address().port;
}

function socketFixture(t, respond) {
  const previous = globalThis.WebSocket;
  const calls = [];
  class Socket extends EventTarget {
    constructor(address) {
      super(); calls.push({ address });
      queueMicrotask(() => this.dispatchEvent(new Event('open')));
    }
    send(text) {
      const request = JSON.parse(text);
      calls.at(-1).request = request;
      const result = respond(request);
      queueMicrotask(() => this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(result) })));
    }
    close() { calls.at(-1).closed = true; }
  }
  globalThis.WebSocket = Socket;
  t.after(() => { globalThis.WebSocket = previous; });
  return calls;
}

const flags = { electron: 'object', ipcSend: 'function', ipcInvoke: 'function', ipcOn: 'function',
  directIpcSend: 'undefined', directIpcInvoke: 'undefined', directIpcOn: 'undefined',
  windowIpc: 'undefined', windowIpcSend: 'undefined', windowIpcInvoke: 'undefined', windowIpcOn: 'undefined',
  ipcImpl: 'undefined', waitForWindowTypeEnsure: 'undefined',
  require: 'undefined', nativeBridge: 'undefined', qqnt: 'undefined', qqntApi: 'undefined',
  preloadApi: 'undefined', audioSDK: 'undefined', audioSDKApi: 'undefined', proxyInvoke: 'undefined' };

test('only local app pages and exact loopback debugger endpoints are eligible', () => {
  assert.ok(applicationTarget(target(45678), 45678));
  assert.ok(applicationTarget(target(45678, { url: 'file:///Applications/QQ.app/Contents/Resources/app/index.html' }), 45678));
  for (const changes of [
    { type: 'service_worker' }, { url: 'https://example.invalid/chat?secret=PRIVATE' },
    { url: 'file:///Users/private/chat.html' }, { url: 'app://user:PRIVATE@localhost/page' },
    { webSocketDebuggerUrl: 'ws://example.invalid:45678/devtools/page/fixture' },
    { webSocketDebuggerUrl: 'ws://127.0.0.1:45679/devtools/page/fixture' },
    { webSocketDebuggerUrl: 'ws://127.0.0.1:45678/devtools/browser/fixture' },
    { webSocketDebuggerUrl: 'ws://127.0.0.1:45678/devtools/page/fixture?secret=PRIVATE' },
    { webSocketDebuggerUrl: 'ws://user:PRIVATE@127.0.0.1:45678/devtools/page/fixture' },
  ]) assert.equal(applicationTarget(target(45678, changes), 45678), null);
});

test('HTTP body limit applies while streaming, before parsing the complete body', async () => {
  let cancelled = false;
  const stream = new ReadableStream({
    pull(controller) { controller.enqueue(new Uint8Array(40000)); },
    cancel() { cancelled = true; },
  });
  await assert.rejects(boundedJson(new Response(stream)), /endpoint_too_large/);
  assert.equal(cancelled, true);
});

test('fixed expression reads capability types without invoking IPC or reading content', async t => {
  const calls = socketFixture(t, request => {
    assert.equal(request.method, 'Runtime.evaluate');
    assert.equal(request.params.throwOnSideEffect, true);
    const forbidden = () => { throw new Error('IPC invoked'); };
    const sandbox = { electron: { ipcRenderer: { send: forbidden, invoke: forbidden, on: forbidden } } };
    Object.defineProperty(sandbox, 'document', { get() { throw new Error('page content accessed'); } });
    const value = vm.runInNewContext(request.params.expression, sandbox);
    return { id: request.id, result: { result: { value: { ...value, secret: 'PRIVATE_TOKEN' } } } };
  });
  const port = await endpoint(t, port => [target(port)]);
  const result = await probe(port);
  assert.equal(result.evaluated, true);
  assert.deepEqual(result.capabilities, [flags]);
  assert.equal(result.transcription_verified, false);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].closed, true);
  assert.ok(!JSON.stringify(result).includes('PRIVATE'));
});

test('QQ preload globals are detected when the Electron namespace is absent', async t => {
  socketFixture(t, request => {
    const forbidden = () => { throw new Error('preload method invoked'); };
    const sandbox = {
      ipcRenderer: { send: forbidden, invoke: forbidden, on: forbidden },
      ipcImpl: {}, waitForWindowTypeEnsure: forbidden,
    };
    Object.defineProperty(sandbox, 'document', { get() { throw new Error('page content accessed'); } });
    const value = vm.runInNewContext(request.params.expression, sandbox);
    return { id: request.id, result: { result: { value } } };
  });
  const port = await endpoint(t, port => [target(port)]);
  const result = await probe(port);
  assert.equal(result.evaluated, true);
  assert.deepEqual(result.capabilities, [{ ...flags,
    electron: 'undefined', ipcSend: 'undefined', ipcInvoke: 'undefined', ipcOn: 'undefined',
    directIpcSend: 'function', directIpcInvoke: 'function', directIpcOn: 'function',
    ipcImpl: 'object', waitForWindowTypeEnsure: 'function',
  }]);
  assert.equal(result.transcription_verified, false);
});

test('screenshot IPC bridge is detected without Electron or direct ipcRenderer globals', async t => {
  socketFixture(t, request => {
    const forbidden = () => { throw new Error('screenshot IPC invoked'); };
    const sandbox = { ipc: { send: forbidden, invoke: forbidden, on: forbidden } };
    Object.defineProperty(sandbox, 'document', { get() { throw new Error('page content accessed'); } });
    Object.defineProperty(sandbox.ipc, 'account', { get() { throw new Error('unrelated bridge field accessed'); } });
    const value = vm.runInNewContext(request.params.expression, sandbox);
    return { id: request.id, result: { result: { value } } };
  });
  const port = await endpoint(t, port => [target(port, {
    url: 'file:///Applications/QQ.app/Contents/Resources/app/renderer/screenshot.html',
  })]);
  const result = await probe(port);
  assert.equal(result.evaluated, true);
  assert.equal(result.capabilities[0].windowIpc, 'object');
  assert.equal(result.capabilities[0].windowIpcSend, 'function');
  assert.equal(result.capabilities[0].windowIpcInvoke, 'function');
  assert.equal(result.capabilities[0].windowIpcOn, 'function');
  assert.equal(result.capabilities[0].electron, 'undefined');
  assert.equal(result.capabilities[0].directIpcSend, 'undefined');
  assert.equal(result.transcription_verified, false);
});

test('external page listings never open a debugger socket or return titles and URLs', async t => {
  const calls = socketFixture(t, () => { throw new Error('unexpected evaluation'); });
  const port = await endpoint(t, port => [target(port, {
    url: 'https://example.invalid/PRIVATE_CHAT', title: 'PRIVATE_NAME',
  })]);
  const result = await probe(port);
  assert.equal(result.available, true);
  assert.equal(result.evaluated, false);
  assert.equal(calls.length, 0);
  assert.ok(!JSON.stringify(result).includes('PRIVATE'));
});

test('untrusted evaluation values are rejected without leaking error text', async t => {
  const calls = socketFixture(t, request => ({ id: request.id, result: {
    result: { value: { ...flags, ipcSend: 'PRIVATE_TOKEN' } },
  } }));
  const port = await endpoint(t, port => [target(port)]);
  const result = await probe(port);
  assert.equal(result.available, true, 'a rejected evaluation does not erase an observed endpoint');
  assert.equal(result.evaluated, false);
  assert.ok(!JSON.stringify(result).includes('PRIVATE'));
  assert.equal(calls[0].closed, true);
});

test('top-level null protocol messages produce a structured failure and close the socket', async t => {
  const calls = socketFixture(t, () => null);
  const port = await endpoint(t, port => [target(port)]);
  const result = await probe(port);
  assert.equal(result.available, true);
  assert.equal(result.evaluated, false);
  assert.equal(result.code, 'renderer_probe_failed');
  assert.equal(calls[0].closed, true);
});

test('protocol exception payloads and too many local targets never escape the report', async t => {
  const calls = socketFixture(t, request => ({ id: request.id,
    result: { exceptionDetails: { text: 'PRIVATE_EXCEPTION' } } }));
  const port = await endpoint(t, port => [target(port)]);
  assert.equal((await probe(port)).evaluated, false);
  const manyPort = await endpoint(t, port => Array.from({ length: 5 }, (_, i) =>
    target(port, { webSocketDebuggerUrl: `ws://127.0.0.1:${port}/devtools/page/fixture${i}` })));
  const result = await probe(manyPort);
  assert.equal(result.evaluated, false);
  assert.equal(result.local_application_count, 5);
  assert.equal(calls.length, 1);
  assert.ok(!JSON.stringify(result).includes('PRIVATE'));
});
