#!/usr/bin/env python3
"""Inspect QQ or prepare a capability-only bootstrap. Never installs or launches QQ."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import plistlib
import stat
import struct
import sys
from string import Template

MAX_JSON = 32_768
MAX_HEADER = 2_000_000
SOURCE = Path(__file__).resolve().parent


def read_regular(path, limit):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
            raise ValueError('unsupported_file')
        with os.fdopen(descriptor, 'rb', closefd=False) as stream:
            data = stream.read(limit + 1)
        if len(data) > limit:
            raise ValueError('file_limit')
        return data
    finally:
        os.close(descriptor)


def archive_entry(archive, relative):
    descriptor = os.open(archive, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode):
            raise ValueError('unsupported_archive')
        with os.fdopen(descriptor, 'rb', closefd=False) as stream:
            prefix = stream.read(16)
            if len(prefix) != 16:
                raise ValueError('invalid_archive_header')
            first, header_size, pickle_size, json_size = struct.unpack('<4I', prefix)
            if (first != 4 or not 8 <= header_size <= MAX_HEADER
                    or pickle_size + 4 != header_size or json_size > pickle_size - 4):
                raise ValueError('invalid_archive_header')
            encoded = stream.read(json_size)
            if len(encoded) != json_size:
                raise ValueError('invalid_archive_header')
            node = json.loads(encoded)
            for part in relative.parts:
                node = node['files'][part]
            if node.get('unpacked') or 'link' in node:
                raise ValueError('unsupported_archive_entry')
            size, offset = int(node['size']), int(node['offset'])
            start = 8 + header_size + offset
            if size < 0 or size > MAX_JSON or offset < 0 or start + size > info.st_size:
                raise ValueError('invalid_archive_entry')
            stream.seek(start)
            data = stream.read(size)
            if len(data) != size:
                raise ValueError('invalid_archive_entry')
            return data
    finally:
        os.close(descriptor)


def inspect(app):
    if app.is_symlink():
        raise ValueError('linked_app')
    app = app.resolve(strict=True)
    resources = app / 'Contents' / 'Resources' / 'app'
    if resources.resolve(strict=True) != resources:
        raise ValueError('linked_resources')
    package_bytes = read_regular(resources / 'package.json', MAX_JSON)
    package = json.loads(package_bytes)
    if not isinstance(package, dict):
        raise ValueError('invalid_package')
    entry = package.get('main')
    if not isinstance(entry, str) or '\\' in entry:
        raise ValueError('invalid_main')
    relative = PurePosixPath(entry)
    if relative.is_absolute() or '..' in relative.parts or not relative.parts:
        raise ValueError('invalid_main')
    if relative.parts[0].endswith('.asar'):
        if len(relative.parts) < 2:
            raise ValueError('invalid_main')
        entry_bytes = archive_entry(resources / relative.parts[0], PurePosixPath(*relative.parts[1:]))
    else:
        target = resources.joinpath(*relative.parts)
        if target.resolve(strict=True) != target:
            raise ValueError('linked_main')
        entry_bytes = read_regular(target, MAX_JSON)
    info = plistlib.loads(read_regular(app / 'Contents' / 'Info.plist', 1_000_000))
    result = {
        'schema_version': 1,
        'diagnostic_only': True,
        'app': str(app),
        'qq_version': package.get('version'),
        'build': package.get('buildVersion'),
        'app_store': package.get('isAppStore') is True,
        'bytecode_shell': package.get('isByteCodeShell') is True,
        'main': entry,
        'package_sha256': hashlib.sha256(package_bytes).hexdigest(),
        'entry_sha256': hashlib.sha256(entry_bytes).hexdigest(),
        'bundle_executable': info.get('CFBundleExecutable'),
        'native_wrapper_present': (resources / 'wrapper.node').is_file(),
        'runtime_verified': False,
        'smartptt_verified': False,
    }
    return result, package_bytes, package


def write_new(path, data):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(descriptor, 'wb', closefd=False) as stream:
            stream.write(data)
    finally:
        os.close(descriptor)


def prepare(app, output):
    inspection, original, package = inspect(app)
    output = output.absolute()
    if output.parent.resolve(strict=True) != output.parent:
        raise ValueError('linked_output_parent')
    if output == Path(inspection['app']) or Path(inspection['app']) in output.parents:
        raise ValueError('output_inside_app')
    # A fresh private directory only; nothing under the QQ bundle is written.
    output.mkdir(mode=0o700)
    staged = output / 'voiceport-qq-diagnostic'
    staged.mkdir(mode=0o700)
    for name in ('diagnostic.cjs', 'report.cjs'):
        write_new(staged / name, read_regular(SOURCE / name, 65_536))
    bootstrap = Template('''\
'use strict';
const path = require('node:path');
let diagnostic;
try {
  const { app } = require('electron');
  const { createReporter } = require('./report.cjs');
  const reporter = createReporter({
    baseDirectory: app.getPath('temp'),
    qqVersion: $qq_version,
  });
  diagnostic = require('./diagnostic.cjs').install({
    publish: reporter.publish,
    cachedModules: Object.values(require.cache),
  });
  process.stderr.write('VoicePort QQ diagnostic report: ' + reporter.reportPath + '\\n');
  const timer = setTimeout(() => {
    if (!diagnostic.snapshot().observed.ready) diagnostic.timeout();
    diagnostic.stop();
  }, 90000);
  timer.unref();
} catch {
  if (diagnostic) diagnostic.stop();
  process.stderr.write('VoicePort QQ diagnostic initialization failed\\n');
}
// Preserve main-module guards when launched as the application's entry point.
// A caller requiring this bootstrap retains ordinary dependency semantics.
const originalEntry = path.resolve(__dirname, '..', $original_entry);
if (require.main === module) {
  process.argv[1] = originalEntry;
  require('node:module')._load(originalEntry, null, true);
} else {
  require(originalEntry);
}
''').substitute(qq_version=json.dumps(inspection['qq_version']),
                original_entry=json.dumps(inspection['main']))
    write_new(staged / 'bootstrap.cjs', bootstrap.encode())
    proposed = dict(package, main='./voiceport-qq-diagnostic/bootstrap.cjs')
    write_new(output / 'original-package.json', original)
    proposed_bytes = (json.dumps(proposed, ensure_ascii=False, indent=2) + '\n').encode()
    write_new(output / 'proposed-package.json', proposed_bytes)
    plan = {
        **inspection,
        'installed': False,
        'staging_directory': str(output),
        'proposed_main': proposed['main'],
        'proposed_package_sha256': hashlib.sha256(proposed_bytes).hexdigest(),
        'staged_files_sha256': {
            name: hashlib.sha256(read_regular(staged / name, 65_536)).hexdigest()
            for name in ('diagnostic.cjs', 'report.cjs', 'bootstrap.cjs')
        },
        'runtime_timeout_seconds': 90,
        'writes_on_install': [
            'Contents/Resources/app/voiceport-qq-diagnostic/diagnostic.cjs',
            'Contents/Resources/app/voiceport-qq-diagnostic/report.cjs',
            'Contents/Resources/app/voiceport-qq-diagnostic/bootstrap.cjs',
            'Contents/Resources/app/package.json',
        ],
        'requires_qq_restart': True,
        'signing_and_sandbox_verified': False,
        'probe_invokes': ['getBdhUploadService (only after observed readiness)'],
        'probe_does_not_invoke': [
            'session creation', 'login', 'message queries', 'listener registration',
            'recording', 'uploadSmartPttSlice',
        ],
    }
    write_new(output / 'plan.json', (json.dumps(plan, indent=2, ensure_ascii=False) + '\n').encode())
    return plan


def read_report(path):
    info = path.lstat()
    parent = path.parent.lstat()
    if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
            or stat.S_IMODE(info.st_mode) & 0o077
            or not stat.S_ISDIR(parent.st_mode) or parent.st_uid != os.getuid()
            or stat.S_IMODE(parent.st_mode) & 0o077):
        raise ValueError('report_permissions')
    report = json.loads(read_regular(path, 16_384))
    if (not isinstance(report, dict) or report.get('schema_version') != 1
            or report.get('diagnostic_only') is not True):
        raise ValueError('invalid_report')
    # Project only known booleans; never echo untrusted report text or unknown fields.
    observed = report.get('observed', {})
    capabilities = report.get('capabilities', {})
    observed_keys = ('wrapper_loaded', 'session_created', 'init_observed', 'ready', 'bdh_service')
    capability_keys = ('getBdhUploadService', 'uploadSmartPttSlice',
                       'addKernelBdhUploadListener', 'removeKernelBdhUploadListener')
    if (not isinstance(observed, dict) or not isinstance(capabilities, dict)
            or any(type(observed.get(key)) is not bool for key in observed_keys)
            or any(capabilities.get(key) is not None and type(capabilities[key]) is not bool
                   for key in capability_keys)):
        raise ValueError('invalid_report')
    return {
        'schema_version': 1, 'diagnostic_only': True,
        'observed': {key: observed[key] for key in observed_keys},
        'capabilities': {key: capabilities.get(key) for key in capability_keys},
        'transcription_verified': False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    for command in ('inspect', 'prepare'):
        sub = commands.add_parser(command)
        sub.add_argument('--qq', type=Path, default=Path('/Applications/QQ.app'))
        if command == 'prepare':
            sub.add_argument('--output', type=Path, required=True)
    sub = commands.add_parser('status')
    sub.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == 'inspect':
            result = inspect(args.qq)[0]
        elif args.command == 'prepare':
            result = prepare(args.qq, args.output)
        else:
            result = read_report(args.report)
    except (OSError, ValueError, KeyError, TypeError, struct.error, plistlib.InvalidFileException):
        print(json.dumps({'status': 'error', 'code': 'diagnostic_operation_failed'}))
        return 1
    print(json.dumps({'status': 'ok', 'data': result}, ensure_ascii=False, indent=2))
    return 0


if __name__ == '__main__':
    sys.exit(main())
