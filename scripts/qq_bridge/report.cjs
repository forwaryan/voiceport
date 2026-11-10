'use strict';

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

function createReporter({ baseDirectory = os.tmpdir(), qqVersion = 'unknown' } = {}) {
  const directory = fs.mkdtempSync(path.join(baseDirectory, 'voiceport-qq-diagnostic-'));
  fs.chmodSync(directory, 0o700);
  const reportPath = path.join(directory, 'report.json');
  let sequence = 0;
  const runtime = {
    platform: process.platform,
    arch: process.arch,
    node: process.versions.node,
    electron: process.versions.electron || null,
    qq_version: /^[0-9]+(?:[.\-][0-9]+){1,5}$/.test(qqVersion) ? qqVersion : 'unknown',
  };
  return {
    reportPath,
    publish(snapshot) {
      const bytes = Buffer.from(JSON.stringify({ ...snapshot, runtime }) + '\n');
      if (bytes.length > 16384) throw new RangeError('report_limit');
      const temporary = path.join(directory, `next-${sequence++}.json`);
      let created = false;
      try {
        const descriptor = fs.openSync(temporary, 'wx', 0o600);
        created = true;
        try { fs.writeFileSync(descriptor, bytes); }
        finally { fs.closeSync(descriptor); }
        fs.renameSync(temporary, reportPath);
      } catch (error) {
        if (created) { try { fs.unlinkSync(temporary); } catch {} }
        throw error;
      }
    },
  };
}

module.exports = { createReporter };
