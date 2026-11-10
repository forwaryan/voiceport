'use strict';
// Capability-only CDP probe. Never reads page text or invokes QQ IPC methods.
const MAX_BYTES = 65536;
const FLAGS = ['electron', 'ipcSend', 'ipcInvoke', 'ipcOn',
  'directIpcSend', 'directIpcInvoke', 'directIpcOn', 'ipcImpl', 'waitForWindowTypeEnsure',
  'require', 'nativeBridge', 'qqnt', 'qqntApi', 'preloadApi', 'audioSDK', 'audioSDKApi', 'proxyInvoke'];
const TYPES = new Set(['undefined', 'object', 'function', 'string', 'boolean', 'number', 'symbol', 'bigint']);
const EXPRESSION = `(() => {
  const e = globalThis.electron;
  return {
    electron: typeof e,
    ipcSend: typeof e?.ipcRenderer?.send,
    ipcInvoke: typeof e?.ipcRenderer?.invoke,
    ipcOn: typeof e?.ipcRenderer?.on,
    directIpcSend: typeof globalThis.ipcRenderer?.send,
    directIpcInvoke: typeof globalThis.ipcRenderer?.invoke,
    directIpcOn: typeof globalThis.ipcRenderer?.on,
    ipcImpl: typeof globalThis.ipcImpl,
    waitForWindowTypeEnsure: typeof globalThis.waitForWindowTypeEnsure,
    require: typeof globalThis.require,
    nativeBridge: typeof globalThis.NativeBridge,
    qqnt: typeof globalThis.QQNT,
    qqntApi: typeof globalThis.qqntApi,
    preloadApi: typeof globalThis.preloadApi,
    audioSDK: typeof globalThis.audioSDK,
    audioSDKApi: typeof globalThis.audioSDKApi,
    proxyInvoke: typeof globalThis.proxyInvoke
  };
})()`;

function validPort(port) {
  return Number.isInteger(port) && port >= 1024 && port <= 65535;
}

function applicationTarget(item, port) {
  if (!item || item.type !== 'page' || typeof item.url !== 'string' ||
      typeof item.webSocketDebuggerUrl !== 'string') return null;
  try {
    const page = new URL(item.url);
    if (page.username || page.password) return null;
    const localApp = ['app:', 'qqnt:'].includes(page.protocol) ||
      (page.protocol === 'file:' && !page.hostname &&
       page.pathname.startsWith('/Applications/QQ.app/Contents/Resources/'));
    if (!localApp) return null;
    const socket = new URL(item.webSocketDebuggerUrl);
    if (socket.protocol !== 'ws:' || socket.hostname !== '127.0.0.1' ||
        Number(socket.port) !== port || socket.username || socket.password ||
        socket.search || socket.hash || !/^\/devtools\/page\/[A-Za-z0-9_-]+$/.test(socket.pathname)) return null;
    return socket.href;
  } catch { return null; }
}

async function boundedJson(response) {
  if (!response.ok || !response.body) throw new Error('endpoint_failed');
  const reader = response.body.getReader();
  let size = 0;
  const chunks = [];
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > MAX_BYTES) throw new Error('endpoint_too_large');
      chunks.push(Buffer.from(value));
    }
    return JSON.parse(Buffer.concat(chunks).toString('utf8'));
  } finally {
    await reader.cancel().catch(() => {});
  }
}

function evaluate(address, timeoutMs) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(address);
    let settled = false;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.close();
      if (error) reject(error); else resolve(value);
    };
    const timer = setTimeout(() => finish(new Error('protocol_timeout')), timeoutMs);
    socket.addEventListener('error', () => finish(new Error('socket_failed')));
    socket.addEventListener('close', () => {
      if (!settled) finish(new Error('socket_closed'));
    });
    socket.addEventListener('open', () => socket.send(JSON.stringify({
      id: 1, method: 'Runtime.evaluate', params: {
        expression: EXPRESSION, returnByValue: true, silent: true,
        throwOnSideEffect: true, timeout: Math.min(timeoutMs, 2000),
      },
    })));
    socket.addEventListener('message', event => {
      if (typeof event.data !== 'string' || Buffer.byteLength(event.data) > MAX_BYTES) {
        finish(new Error('protocol_too_large'));
        return;
      }
      let message;
      try { message = JSON.parse(event.data); } catch {
        finish(new Error('protocol_invalid')); return;
      }
      if (!message || typeof message !== 'object' || Array.isArray(message)) {
        finish(new Error('protocol_invalid')); return;
      }
      // Ignore unsolicited protocol events; never store their payloads.
      if (message.id !== 1) return;
      const value = message.result?.result?.value;
      if (message.error || message.result?.exceptionDetails || !value || typeof value !== 'object') {
        finish(new Error('evaluation_failed')); return;
      }
      const flags = {};
      for (const key of FLAGS) {
        if (!TYPES.has(value[key])) { finish(new Error('unexpected_result')); return; }
        flags[key] = value[key];
      }
      finish(null, flags);
    });
  });
}

async function probe(port) {
  if (!validPort(port)) throw new TypeError('invalid_port');
  const base = { diagnostic_only: true, transcription_verified: false, available: false, evaluated: false };
  let available = false;
  let pageCount = 0;
  let localCount = 0;
  try {
    const response = await fetch(`http://127.0.0.1:${port}/json/list`, {
      redirect: 'error', signal: AbortSignal.timeout(2500),
    });
    const items = await boundedJson(response);
    if (!Array.isArray(items)) throw new Error('endpoint_invalid');
    const pages = items.filter(item => item?.type === 'page');
    const targets = [...new Set(items.map(item => applicationTarget(item, port)).filter(Boolean))];
    available = true;
    pageCount = pages.length;
    localCount = targets.length;
    if (targets.length === 0 || targets.length > 4) return {
      ...base, available: true, code: 'no_bounded_local_application_targets',
      page_count: pages.length, local_application_count: targets.length,
    };
    const capabilities = [];
    for (const target of targets) capabilities.push(await evaluate(target, 2500));
    return { ...base, available: true, evaluated: true, page_count: pages.length,
      local_application_count: targets.length, capabilities };
  } catch {
    return { ...base, available, page_count: pageCount, local_application_count: localCount,
      code: 'renderer_probe_failed' };
  }
}

module.exports = { applicationTarget, boundedJson, probe };
if (require.main === module) {
  const port = Number(process.argv[2]);
  if (process.argv.length !== 3 || !validPort(port)) process.exitCode = 2;
  else {
    const deadline = setTimeout(() => {
      process.stdout.write(JSON.stringify({ diagnostic_only: true, transcription_verified: false,
        available: false, evaluated: false, code: 'renderer_deadline' }) + '\n');
      process.exit(1);
    }, 16000);
    probe(port).then(result => {
      clearTimeout(deadline);
      process.stdout.write(JSON.stringify(result) + '\n', () => process.exit(result.evaluated ? 0 : 1));
    }).catch(() => { clearTimeout(deadline); process.exitCode = 1; });
  }
}
