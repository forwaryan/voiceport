'use strict';
// QQ-process transport. Authentication/init stay with QQ; clients supply SILK bytes.
const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const crypto = require('node:crypto');
const { inspectStreamChunks } = require('./stream_framing.cjs');
const { buildSmartPttRequest, createTranscriptState } = require('./smartptt_protocol.cjs');
const MAX_REQUEST = 4000000;
const MAX_SILK = 2 * 1024 * 1024;
const MAX_RESPONSE = 1000000;
const METHODS = ['uploadSmartPttSlice', 'addKernelBdhUploadListener', 'removeKernelBdhUploadListener'];

function createBridge({ socketPath, token, adminToken, dataDirectory, onControl,
  onState = () => {}, beforeSubmit = () => {}, timeoutMs = 70000 } = {}) {
  if (typeof socketPath !== 'string' || !path.isAbsolute(socketPath) ||
      Buffer.byteLength(socketPath) > 100 || socketPath.includes('\0') ||
      !/^[a-f0-9]{64}$/.test(token || '') || !/^[a-f0-9]{64}$/.test(adminToken || '') ||
      token === adminToken || typeof dataDirectory !== 'string' || !path.isAbsolute(dataDirectory) ||
      typeof beforeSubmit !== 'function' || !Number.isInteger(timeoutMs) || timeoutMs <= 0 || timeoutMs > 70000) {
    throw new Error('invalid_bridge_configuration');
  }
  const directory = fs.lstatSync(dataDirectory);
  const parent = fs.lstatSync(path.dirname(socketPath));
  for (const stat of [directory, parent]) {
    if (!stat.isDirectory() || stat.isSymbolicLink() || (stat.mode & 0o077) !== 0 ||
        (process.getuid && stat.uid !== process.getuid())) throw new Error('unsafe_bridge_directory');
  }
  if (fs.existsSync(socketPath)) throw new Error('bridge_socket_already_exists');
  let service = null;
  let current = null;
  let stopped = false;
  let socketInode = null;
  let cleanupFailed = false;
  const connections = new Set();
  const counts = { submitted: 0, completed: 0, failed: 0, abandoned: 0 };
  function snapshot() { return { ready: !stopped && !cleanupFailed && !!service,
    busy: !!current, cleanup_failed: cleanupFailed, ...counts }; }
  function publish() { try { onState(snapshot()); } catch {} }
  function matches(value, expected) {
    return typeof value === 'string' && /^[a-f0-9]{64}$/.test(value) &&
      crypto.timingSafeEqual(Buffer.from(value), Buffer.from(expected));
  }
  function send(socket, value, end = false) {
    if (socket.destroyed || socket.writableEnded) return;
    const line = JSON.stringify(value) + '\n';
    if (end) socket.end(line); else socket.write(line);
  }
  function reject(socket, code) { send(socket, { type: 'error', code }, true); }
  function cleanup(task) {
    clearTimeout(task.timer);
    if (task.listenerAttempted) {
      try { Reflect.apply(task.service.removeKernelBdhUploadListener, task.service, [task.listener]); }
      catch { cleanupFailed = true; }
    }
    try { fs.unlinkSync(task.file); fs.rmdirSync(task.directory); }
    catch { cleanupFailed = true; }
  }
  function finish(task, result) {
    if (task.closed) return;
    task.closed = true;
    cleanup(task);
    if (current === task) current = null;
    if (result.type === 'final') counts.completed++; else counts.failed++;
    if (!task.abandoned) send(task.socket, result, true);
    publish();
  }
  function transcribe(socket, request) {
    if (!service || stopped) return reject(socket, 'qq_not_ready');
    if (cleanupFailed) return reject(socket, 'qq_cleanup_failed');
    if (current) return reject(socket, 'qq_busy');
    let data;
    try {
      const encoded = request.silk_base64;
      if (typeof encoded !== 'string' || encoded.length > Math.ceil(MAX_SILK / 3) * 4 ||
          !/^[A-Za-z0-9+/]+={0,2}$/.test(encoded)) throw new Error();
      data = Buffer.from(encoded, 'base64');
      if (data.length > MAX_SILK || data.toString('base64') !== encoded) throw new Error();
      const info = inspectStreamChunks([data]);
      if (info.frame_count > 180 * 50) throw new Error();
    } catch { return reject(socket, 'invalid_silk'); }
    let taskDirectory;
    let file;
    try {
      taskDirectory = fs.mkdtempSync(path.join(dataDirectory, 'task-'));
      fs.chmodSync(taskDirectory, 0o700);
      file = path.join(taskDirectory, 'input.silk');
      fs.writeFileSync(file, data, { flag: 'wx', mode: 0o600 });
    } catch {
      if (taskDirectory) {
        try { if (file && fs.existsSync(file)) fs.unlinkSync(file); fs.rmdirSync(taskDirectory); } catch { cleanupFailed = true; }
      }
      publish();
      return reject(socket, 'qq_audio_storage_failed');
    }
    const id = crypto.randomBytes(15).toString('hex');
    const transcript = createTranscriptState(id);
    const task = { socket, service, directory: taskDirectory, file, listener: null,
      listenerAttempted: false, timer: null, closed: false, abandoned: false, responseBytes: 0 };
    current = task;
    task.listener = {
      onSmartPttUpdateText(callbackId, text, segFinished) {
        if (task.closed || callbackId !== id) return;
        try {
          const value = transcript.update(callbackId, text, segFinished);
          if (value.status !== 'partial' || task.abandoned) return;
          const partial = { type: 'partial', text: value.text };
          task.responseBytes += Buffer.byteLength(JSON.stringify(partial)) + 1;
          if (task.responseBytes > MAX_RESPONSE / 2) return finish(task, { type: 'error', code: 'qq_response_limit' });
          send(socket, partial);
        } catch { finish(task, { type: 'error', code: 'qq_invalid_callback' }); }
      },
      onSmartPttTranslateFinished(callbackId, code, message) {
        if (task.closed || callbackId !== id) return;
        try {
          const value = transcript.finish(callbackId, code, message);
          if (value.status === 'failed') return finish(task, { type: 'error', code: 'qq_smartptt_error', native_error_code: value.errorCode });
          if (value.status !== 'finished') return;
          const result = { type: 'final', text: value.text };
          if (task.responseBytes + Buffer.byteLength(JSON.stringify(result)) + 1 > MAX_RESPONSE) {
            return finish(task, { type: 'error', code: 'qq_response_limit' });
          }
          finish(task, result);
        } catch { finish(task, { type: 'error', code: 'qq_invalid_callback' }); }
      },
    };
    // The native API has no verified cancellation method. A disconnected caller
    // receives nothing; retain custody until completion/deadline, then clean up.
    socket.once('close', () => {
      if (!task.closed) { task.abandoned = true; counts.abandoned++; publish(); }
    });
    task.timer = setTimeout(() => finish(task, { type: 'error', code: 'qq_smartptt_timeout' }), timeoutMs);
    try {
      beforeSubmit({ task_id: id, silk_bytes: data.length });
      task.listenerAttempted = true;
      Reflect.apply(task.service.addKernelBdhUploadListener, task.service, [task.listener]);
      if (task.closed) return; // A native listener may synchronously finish the task.
      const nativeRequest = buildSmartPttRequest(id, { filePath: file, chunkIndex: 0, chunkOffsetBytes: 0, isLast: 1 });
      counts.submitted++;
      publish(); // Journal the attempt before calling the native upload.
      Reflect.apply(task.service.uploadSmartPttSlice, task.service, [nativeRequest.taskId, nativeRequest.sliceParam]);
      if (!task.closed) send(socket, { type: 'accepted' });
    } catch { finish(task, { type: 'error', code: 'qq_smartptt_call_failed' }); }
  }
  const server = net.createServer(socket => {
    connections.add(socket);
    socket.setTimeout(10000, () => socket.destroy());
    socket.on('error', () => {});
    socket.once('close', () => connections.delete(socket));
    let chunks = [];
    let size = 0;
    let received = false;
    socket.on('data', chunk => {
      if (received) return socket.destroy();
      size += chunk.length;
      if (size > MAX_REQUEST) return socket.destroy();
      chunks.push(chunk);
      if (!chunk.includes(10)) return;
      received = true;
      const bytes = Buffer.concat(chunks, size);
      chunks = [];
      if (bytes.indexOf(10) !== bytes.length - 1) return reject(socket, 'invalid_request');
      let request;
      try { request = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)); }
      catch { return reject(socket, 'invalid_request'); }
      if (!request || typeof request !== 'object' || Array.isArray(request)) return reject(socket, 'invalid_request');
      if (request.op === 'login' || request.op === 'shutdown') {
        if (!matches(request.token, adminToken)) return socket.destroy();
        if (Object.keys(request).some(key => !['op', 'token'].includes(key)) || typeof onControl !== 'function') return reject(socket, 'unsupported_operation');
        Promise.resolve().then(() => onControl(request.op)).then(
          result => send(socket, { type: 'control', result }, true),
          () => reject(socket, 'qq_control_failed'));
        return;
      }
      if (!matches(request.token, token) && !matches(request.token, adminToken)) return socket.destroy();
      if (request.op === 'status' && Object.keys(request).every(key => ['op', 'token'].includes(key))) {
        return send(socket, { type: 'status', ...snapshot() }, true);
      }
      if (request.op !== 'transcribe' || Object.keys(request).some(key => !['op', 'token', 'silk_base64'].includes(key))) return reject(socket, 'unsupported_operation');
      socket.setTimeout(timeoutMs + 5000, () => socket.destroy());
      transcribe(socket, request);
    });
  });
  server.maxConnections = 16;
  function stop() {
    if (stopped) return;
    stopped = true;
    if (current) finish(current, { type: 'error', code: 'qq_bridge_stopped' });
    for (const socket of connections) socket.destroy();
    server.close(() => {});
    try {
      const stat = fs.lstatSync(socketPath);
      if (stat.isSocket() && stat.ino === socketInode) fs.unlinkSync(socketPath);
    } catch {}
    publish();
  }
  const ready = new Promise((resolve, rejectReady) => {
    server.on('error', error => { stop(); rejectReady(error); });
    server.listen(socketPath, () => {
      try {
        const stat = fs.lstatSync(socketPath);
        if (!stat.isSocket()) throw new Error('bridge_socket_replaced');
        socketInode = stat.ino;
        if (stopped) {
          server.close(() => {});
          fs.unlinkSync(socketPath);
          rejectReady(new Error('bridge_stopped'));
          return;
        }
        fs.chmodSync(socketPath, 0o600);
        publish();
        resolve();
      } catch (error) { stop(); rejectReady(error); }
    });
  });
  return { ready, snapshot, stop, setService(value) {
    if (stopped) return;
    service = value && METHODS.every(name => typeof value[name] === 'function') ? value : null;
    publish();
  } };
}
module.exports = { createBridge, MAX_REQUEST, MAX_SILK };
