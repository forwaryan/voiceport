'use strict';
// Explicitly installed in a separate QQ copy. No account/IPC payload logging.
const fs = require('node:fs');
const path = require('node:path');
const electron = require('electron');
const { app, BrowserWindow } = electron;
const config = JSON.parse(fs.readFileSync(path.join(__dirname, 'runtime-config.json'), 'utf8'));
if (process.platform !== 'darwin' || process.type !== 'browser' || app.isReady() ||
    path.join(config.appDirectory, 'voiceport-qq-bridge') !== __dirname ||
    config.originalMain !== './application.asar/app_launcher/index.js') throw new Error('invalid_qq_bridge_entry');
const { createBridge } = require('./bridge.cjs');
const { install } = require('./diagnostic.cjs');
for (const name of ['temp', 'logs']) fs.mkdirSync(path.join(config.runtime, name), { recursive: true, mode: 0o700 });
app.setPath('appData', path.dirname(config.profile));
app.setPath('userData', config.profile);
app.setPath('sessionData', config.profile);
app.setPath('temp', path.join(config.runtime, 'temp'));
app.setAppLogsPath(path.join(config.runtime, 'logs'));
for (const name of ['setLoginItemSettings', 'setAsDefaultProtocolClient', 'removeAsDefaultProtocolClient']) {
  app[name] = () => false;
}
let observer;
let bridge;
let loginAttempted = false;
const state = { pid: process.pid, qq_version: config.qqVersion, app_ready: false,
  observer: null, bridge: null, login_action_attempted: false };
function save() {
  const target = path.join(config.runtime, 'status.json');
  const tmp = target + '.tmp';
  fs.writeFileSync(tmp, JSON.stringify(state) + '\n', { mode: 0o600 });
  fs.renameSync(tmp, target);
}
async function login() {
  if (bridge.snapshot().ready) return 'already_ready';
  if (loginAttempted) return 'login_already_attempted';
  const windows = BrowserWindow.getAllWindows().filter(window => {
    if (window.isDestroyed()) return false;
    try { return /\/renderer\/(?:login|newLogin)\.html$/.test(new URL(window.webContents.getURL()).pathname); }
    catch { return false; }
  });
  if (windows.length !== 1) return 'login_window_unavailable';
  loginAttempted = true;
  state.login_action_attempted = true;
  save();
  // A fixed normal UI action; no routes, auth fields or native init are supplied.
  const result = await windows[0].webContents.executeJavaScript(`(() => {
    const buttons = Array.from(document.querySelectorAll('button,[role="button"]'))
      .filter(button => button.textContent.trim() === '登录' && button.getClientRects().length > 0);
    if (buttons.length !== 1 || buttons[0].disabled) return 'login_required';
    buttons[0].click();
    return 'clicked';
  })()`, true);
  return result === 'clicked' ? 'clicked' : 'login_required';
}
bridge = createBridge({ socketPath: config.socketPath, token: config.pairingToken,
  adminToken: config.adminToken, dataDirectory: config.tasksDirectory,
  beforeSubmit(metadata) {
    const fd = fs.openSync(path.join(config.runtime, 'native-attempts.jsonl'), 'a', 0o600);
    try {
      fs.writeFileSync(fd, JSON.stringify({ event: 'native_task_attempt', ...metadata,
        at: new Date().toISOString() }) + '\n');
      fs.fsyncSync(fd);
    } finally { fs.closeSync(fd); }
  },
  onState(value) { state.bridge = value; save(); },
  async onControl(op) {
    if (op === 'login') return login();
    if (op === 'shutdown') { setTimeout(() => app.quit(), 50); return 'stopping'; }
    throw new Error('unsupported_control');
  },
});
bridge.ready.catch(() => { app.quit(); });
observer = install({ cachedModules: Object.values(require.cache),
  publish(value) { state.observer = value; save(); },
  onReady({ service }) { bridge.setService(service); },
});
app.whenReady().then(() => { state.app_ready = true; save(); });
app.once('before-quit', () => { bridge.stop(); observer.stop(); });
process.once('SIGTERM', () => app.quit());
save();
// Preserve QQ's normal windows, authentication, network and exception policy.
require(path.join(config.appDirectory, config.originalMain));
