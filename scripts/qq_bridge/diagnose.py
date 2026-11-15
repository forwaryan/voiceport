#!/usr/bin/env python3
"""Inspect QQ or prepare a capability-only bootstrap. Never installs or launches QQ."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import plistlib
import re
import selectors
import stat
import struct
import subprocess
import sys
import time
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
                if not isinstance(node, dict) or not isinstance(node.get('files'), dict):
                    raise ValueError('invalid_archive_entry')
                node = node['files'][part]
            if not isinstance(node, dict) or node.get('unpacked') or 'link' in node:
                raise ValueError('unsupported_archive_entry')
            size, encoded_offset = node.get('size'), node.get('offset')
            if (type(size) is not int or not isinstance(encoded_offset, str)
                    or not re.fullmatch(r'[0-9]{1,20}', encoded_offset)):
                raise ValueError('invalid_archive_entry')
            offset = int(encoded_offset)
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


def read_bundle_file(app, relative, limit):
    target = app / relative
    if target.resolve(strict=True) != target:
        raise ValueError('linked_bundle_metadata')
    return read_regular(target, limit)


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
    info = plistlib.loads(read_bundle_file(app, 'Contents/Info.plist', 1_000_000))
    if not isinstance(info, dict):
        raise ValueError('invalid_bundle_info')
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


# Historical interoperability evidence, not a portable installation allowlist.
KNOWN_COPY_BUILD = {
    'version': '7.0.1-52892',
    'package_sha256': 'fa969cb90248aa2b02bcb9b687afafa02336413c4768233ab7389008af40c42e',
    'info_sha256': '1e242c1445cbbaa5bb886737f7be87aa36a76f9f908e717fa5da11b0749bb8da',
}
# A leading '=' makes codesign parse text instead of treating it as a file.
# MAS apps use Apple's shared leaf certificate; the authenticated CodeDirectory
# TeamIdentifier is checked separately rather than requiring Tencent's leaf OU.
QQ_SIGNATURE_REQUIREMENT = '=anchor apple generic and identifier "com.tencent.qq"'
MAX_VERIFIER_OUTPUT = 1_000_000
METADATA_ERRORS = (OSError, ValueError, KeyError, TypeError, OverflowError,
                   RecursionError, struct.error, plistlib.InvalidFileException)


def run_codesign(args, *, capture_output=True, timeout, check=False):
    """Capture both verifier pipes with byte and time bounds, without shell code."""
    if not capture_output or check:
        raise ValueError('unsupported_verifier_options')
    with subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, bufsize=0, cwd='/',
                          env={'LANG': 'C', 'LC_ALL': 'C', 'PATH': '/usr/bin:/bin'}) as child:
        deadline = time.monotonic() + timeout
        output = [bytearray(), bytearray()]
        try:
            with selectors.DefaultSelector() as selector:
                for index, pipe in enumerate((child.stdout, child.stderr)):
                    os.set_blocking(pipe.fileno(), False)
                    selector.register(pipe, selectors.EVENT_READ, index)
                while selector.get_map():
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise subprocess.TimeoutExpired(args, timeout)
                    for key, _ in selector.select(remaining):
                        try:
                            chunk = os.read(key.fd, 8192)
                        except BlockingIOError:
                            continue
                        if not chunk:
                            selector.unregister(key.fileobj)
                        elif len(output[key.data]) + len(chunk) > MAX_VERIFIER_OUTPUT:
                            raise ValueError('verifier_output_limit')
                        else:
                            output[key.data].extend(chunk)
            returncode = child.wait(timeout=max(0, deadline - time.monotonic()))
            return subprocess.CompletedProcess(args, returncode, bytes(output[0]), bytes(output[1]))
        finally:
            if child.poll() is None:
                child.kill()
            child.wait()


def resource_sealed(seal, name):
    for key in ('files', 'files2'):
        entry = seal.get(key, {}).get(name)
        if isinstance(entry, bytes) and len(entry) == 20:
            return True
        if isinstance(entry, dict) and entry.get('omit') is not True:
            if any(isinstance(entry.get(algorithm), bytes) and len(entry[algorithm]) == size
                   for algorithm, size in (('hash', 20), ('hash2', 32))):
                return True
    return False


def doctor(app=None, *, applications_directories=None, system=None, run=run_codesign):
    """Read installation prerequisites; never opens QQ, user data or a socket."""
    result = {
        'schema_version': 1,
        'diagnostic_only': True,
        'platform': system or sys.platform,
        'architecture': platform.machine(),
        'connection_status': 'not_tested',
        'transcription_verified': False,
        'automatic_setup_supported': False,
        'checks': [],
        'blockers': [],
    }

    def block(code, message):
        result['blockers'].append({'code': code, 'message': message})
        return result

    if result['platform'] != 'darwin':
        return block('platform_unverified', '此平台尚无经过验证的 QQ 接入路线。')
    try:
        if app is None:
            directories = applications_directories if applications_directories is not None else (
                Path('/Applications'), Path.home() / 'Applications',
            )
            # No recursive search through account data, Downloads or arbitrary app code.
            candidates = list(dict.fromkeys(Path(root) / 'QQ.app' for root in directories
                                            if os.path.lexists(Path(root) / 'QQ.app')))
            if not candidates:
                return block('qq_not_found', '未在系统或用户的 Applications 目录找到 QQ；可用 --qq 指定安装位置。')
            if len(candidates) != 1:
                result['candidates'] = [str(path) for path in candidates]
                return block('multiple_qq_installations', '发现多个 QQ 安装；请用 --qq 指定要检查的安装。')
            app = candidates[0]
        inspection = inspect(Path(app).absolute())[0]
        app = Path(inspection['app'])
        info_bytes = read_bundle_file(app, 'Contents/Info.plist', 1_000_000)
        info = plistlib.loads(info_bytes)
        if not isinstance(info, dict) or info.get('CFBundleIdentifier') != 'com.tencent.qq':
            raise ValueError('not_official_qq_bundle')
    except METADATA_ERRORS:
        return block('qq_installation_unreadable', '无法安全读取 QQ 安装元数据；请检查安装是否完整。')
    result['app'] = str(app)
    version = inspection['qq_version']
    result['qq_version'] = (version if isinstance(version, str) and len(version) <= 64
                            and re.fullmatch(r'[0-9.]+-[0-9]+', version) else None)
    result['app_store'] = inspection['app_store']
    result['checks'].append({'id': 'installation_metadata', 'status': 'passed'})
    try:
        # Reject linked signature metadata before asking the verifier to read it.
        seal = plistlib.loads(read_bundle_file(app, 'Contents/_CodeSignature/CodeResources', 8_000_000))
        if not isinstance(seal, dict) or any(not isinstance(seal.get(k, {}), dict) for k in ('files', 'files2')):
            raise ValueError('signature_resources_unreadable')
        # Run only Apple's fixed verifier, never a binary from the inspected app.
        verified = run(['/usr/bin/codesign', '--verify', '--deep', '--strict',
                        '-R', QQ_SIGNATURE_REQUIREMENT, str(app)],
                       capture_output=True, timeout=30, check=False)
        if verified.returncode != 0:
            return block('official_signature_invalid', 'QQ 未通过官方签名及完整性校验；不会自动改包或重新签名。')
        displayed = run(['/usr/bin/codesign', '-d', '--verbose=4', '--entitlements', ':-', str(app)],
                        capture_output=True, timeout=10, check=False)
        if (displayed.returncode != 0
                or len(displayed.stdout) > MAX_VERIFIER_OUTPUT
                or len(displayed.stderr) > MAX_VERIFIER_OUTPUT):
            raise ValueError('entitlements_unreadable')
        teams = re.findall(rb'^TeamIdentifier=([^\r\n]+)', displayed.stderr, re.MULTILINE)
        if teams != [b'FN2V63AD2J']:
            return block('official_signature_invalid', 'QQ 签名的团队标识与腾讯不符；不会自动改包或重新签名。')
        result['checks'].append({'id': 'official_signature', 'status': 'passed'})
        entitlements = plistlib.loads(displayed.stdout)
        if (not isinstance(entitlements, dict)
                or type(entitlements.get('com.apple.security.app-sandbox', False)) is not bool):
            raise ValueError('entitlements_unreadable')
    except METADATA_ERRORS + (subprocess.SubprocessError,):
        return block('signature_inspection_unavailable', '无法完成签名与沙盒检查；不会根据文件存在就判为可连接。')
    relative = PurePosixPath(inspection['main'])
    entry_resource = (PurePosixPath(relative.parts[0]) if relative.parts[0].endswith('.asar')
                      else relative)
    sealed = all(resource_sealed(seal, str(PurePosixPath('Resources/app') / name))
                 for name in (PurePosixPath('package.json'), entry_resource))
    result['sandbox_enabled'] = entitlements.get('com.apple.security.app-sandbox') is True
    result['entry_resources_sealed'] = sealed
    result['historical_copy_build_match'] = (
        version == KNOWN_COPY_BUILD['version']
        and inspection['package_sha256'] == KNOWN_COPY_BUILD['package_sha256']
        and hashlib.sha256(info_bytes).hexdigest() == KNOWN_COPY_BUILD['info_sha256']
    )
    result['checks'].append({'id': 'signing_constraints', 'status': 'inspected'})
    if not result['sandbox_enabled']:
        block('sandbox_disabled', '此 QQ 安装未启用原有 App Sandbox；其配置不能作为已验证的普通 QQ 接入前提。')
    if not sealed:
        block('entry_resources_unsealed', 'QQ 配置或实际启动入口未受资源签名封存；不能确认接入前提完整。')
    if not result['historical_copy_build_match']:
        block('qq_build_unverified', '此 QQ 构建尚未完成真实桥接兼容性验证；不能只按版本号放行安装。')
    return block('signature_preserving_route_unverified',
                 '尚无经过验证的普通 QQ 接入路线能保留官方签名及原有沙盒。历史独立副本的成功不代表此安装已连通；不会自动重跑改包安装。')


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
    sub = commands.add_parser('doctor', help='Read-only QQ installation and connection prerequisites')
    sub.add_argument('--qq', type=Path, help='Explicit installation; otherwise discover Applications/QQ.app')
    sub = commands.add_parser('status')
    sub.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == 'doctor':
            result = doctor(args.qq)
        elif args.command == 'inspect':
            result = inspect(args.qq)[0]
        elif args.command == 'prepare':
            result = prepare(args.qq, args.output)
        else:
            result = read_report(args.report)
    except METADATA_ERRORS:
        print(json.dumps({'status': 'error', 'code': 'diagnostic_operation_failed'}))
        return 1
    print(json.dumps({'status': 'ok', 'data': result}, ensure_ascii=False, indent=2))
    return 0


if __name__ == '__main__':
    sys.exit(main())
