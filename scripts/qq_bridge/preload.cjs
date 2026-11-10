'use strict';

// Optional NODE_OPTIONS preload. QQ's own launcher still runs normally.
// Place this and its two companion modules in a new private directory first.
let diagnostic;
try {
  const { createReporter } = require('./report.cjs');
  const reporter = createReporter({ baseDirectory: __dirname });
  diagnostic = require('./diagnostic.cjs').install({
    publish: reporter.publish,
    cachedModules: Object.values(require.cache),
  });
  process.stderr.write('VoicePort QQ diagnostic report: ' + reporter.reportPath + '\n');
  const timer = setTimeout(() => {
    if (!diagnostic.snapshot().observed.ready) diagnostic.timeout();
    diagnostic.stop();
  }, 90000);
  timer.unref();
} catch {
  if (diagnostic) diagnostic.stop();
  process.stderr.write('VoicePort QQ diagnostic initialization failed\n');
}
