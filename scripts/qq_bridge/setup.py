#!/usr/bin/env python3
"""Prepare/start/control an owned QQ copy; never modifies the original installation."""
import argparse
import base64
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path
import plistlib
import secrets
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request

HERE = Path(__file__).resolve().parent
QQ_APP = Path('/Applications/QQ.app')
PACKAGE_URL = 'https://registry.npmjs.org/silk-wasm/-/silk-wasm-3.7.1.tgz'
INTEGRITY = 'mXPwLRtZxrYV3TZx41jMAeKc80wvmyrcXIcs8HctFxK15Ahz2OJQENYhNgEPeCEOdI6Mbx1NxQsqxzwc3DKerw=='
ORIGINAL_FILES = {
    'Contents/Info.plist': '1e242c1445cbbaa5bb886737f7be87aa36a76f9f908e717fa5da11b0749bb8da',
    'Contents/Resources/app/package.json': 'fa969cb90248aa2b02bcb9b687afafa02336413c4768233ab7389008af40c42e',
}
RUNTIME_FILES = ['runtime_bootstrap.cjs', 'bridge.cjs', 'diagnostic.cjs', 'smartptt_protocol.cjs', 'stream_framing.cjs']
ADAPTER_FILES = ['adapter.cjs', 'codec.cjs', 'stream_framing.cjs', 'package.json']


def require(condition, code):
    if not condition:
        raise RuntimeError(code)


def write_json(target, value):
    fd, name = tempfile.mkstemp(prefix='.' + target.name, dir=target.parent)
    try:
        with os.fdopen(fd, 'w') as handle:
            json.dump(value, handle, ensure_ascii=False, indent=2)
            handle.write('\n')
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(name, target)
        directory = os.open(target.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def private_directory(target):
    info = target.lstat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and not info.st_mode & 0o077,
            'unsafe_owned_directory')


def fingerprint(app):
    return {relative: hashlib.sha256((app / relative).read_bytes()).hexdigest() for relative in ORIGINAL_FILES}


def signature(app):
    result = subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict', str(app)],
                            capture_output=True, timeout=40)
    require(result.returncode == 0, 'app_signature_invalid')


def materialize_codec(destination, archive=None):
    if archive:
        require(archive.is_file() and archive.stat().st_size <= 2_000_000, 'invalid_encoder_archive')
        data = archive.read_bytes()
    else:
        # Fixed public package; never sends machine/project identity or auth data.
        with urllib.request.urlopen(PACKAGE_URL, timeout=25) as response:
            data = response.read(2_000_001)
    require(len(data) <= 2_000_000 and base64.b64encode(hashlib.sha512(data).digest()).decode() == INTEGRITY,
            'encoder_integrity_mismatch')
    wanted = {'package/package.json', 'package/LICENSE', 'package/lib/index.cjs', 'package/lib/silk.wasm'}
    contents = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive_file:
        for entry in archive_file.getmembers():
            require(entry.isfile() and entry.name.startswith('package/') and '..' not in Path(entry.name).parts
                    and entry.size <= 1_000_000, 'unsafe_encoder_archive')
            if entry.name in wanted:
                require(entry.name not in contents, 'duplicate_encoder_file')
                contents[entry.name] = archive_file.extractfile(entry).read()
    require(set(contents) == wanted, 'encoder_package_incomplete')
    metadata = json.loads(contents['package/package.json'])
    require(metadata['name'] == 'silk-wasm' and metadata['version'] == '3.7.1', 'encoder_version_mismatch')
    for name, content in contents.items():
        target = destination / Path(name).relative_to('package')
        target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        target.write_bytes(content)
        target.chmod(0o600)


def prepare(args):
    output = args.output
    require(sys.platform == 'darwin' and not output.exists(), 'prepare_requires_new_macos_directory')
    source = args.qq_app.resolve(strict=True)
    require(source == QQ_APP, 'unsupported_qq_installation')
    require(not output.resolve().is_relative_to(source), 'output_must_be_outside_qq_installation')
    require(fingerprint(source) == ORIGINAL_FILES, 'unsupported_qq_version_expected_7_0_1_52892')
    signature(source)
    original_package = json.loads((source / 'Contents/Resources/app/package.json').read_bytes())
    require(original_package['main'] == './application.asar/app_launcher/index.js', 'unsupported_qq_entry')
    node = Path(args.node or shutil.which('node') or '').resolve()
    require(node.is_file() and os.access(node, os.X_OK), 'node_executable_required')
    version = subprocess.run([str(node), '--version'], capture_output=True, text=True, check=True, timeout=5).stdout.strip()
    require(int(version.lstrip('v').split('.')[0]) >= 22, 'node_22_or_newer_required')
    sources = {name: (HERE / name).read_bytes() for name in set(RUNTIME_FILES + ADAPTER_FILES)}
    output.mkdir(parents=True, mode=0o700)
    private_directory(output)
    runtime = Path(tempfile.mkdtemp(prefix='vpqq-', dir='/private/tmp'))
    runtime.chmod(0o700)
    tasks = runtime / 'tasks'
    tasks.mkdir(mode=0o700)
    app = output / 'QQ Bridge.app'
    journal = {'operation': 'prepare', 'stage': 'copy_attempt', 'source': str(source), 'app': str(app),
               'runtime': str(runtime), 'original_files': ORIGINAL_FILES, 'qq_version': '7.0.1-52892'}
    write_json(output / 'preparation.json', journal)
    subprocess.run(['/bin/cp', '-cR', str(source), str(app)], check=True, timeout=120)
    require((source / 'Contents/Info.plist').stat().st_ino != (app / 'Contents/Info.plist').stat().st_ino,
            'copy_must_have_separate_inodes')
    # Read every target before changing copied metadata. QQ's helper lookup needs CFBundleName=QQ.
    namespace = 'local.voiceport.qqbridge.' + secrets.token_hex(5)
    rows = []
    for index, bundle in enumerate([app] + sorted(app.rglob('*.app'))):
        target = bundle / 'Contents/Info.plist'
        if not target.is_file():
            continue
        require(not target.is_symlink() and target.resolve().is_relative_to(app), 'copy_metadata_escapes_app')
        info = plistlib.loads(target.read_bytes())
        old_id = info.get('CFBundleIdentifier', '')
        if bundle == app:
            new_id = namespace
            info['CFBundleName'] = 'QQ'
            info['CFBundleDisplayName'] = 'QQ VoicePort Bridge'
        elif bundle.parent == app / 'Contents/Frameworks' and old_id.startswith('com.tencent.qq.helper'):
            new_id = namespace + old_id[len('com.tencent.qq'):]
        else:
            new_id = namespace + '.child' + str(index)
        info['CFBundleIdentifier'] = new_id
        info.pop('CFBundleURLTypes', None)
        info.pop('NSServices', None)
        rows.append((bundle, target, info))
    journal['stage'] = 'owned_copy_edit_attempt'
    write_json(output / 'preparation.json', journal)
    for _, target, info in rows:
        target.write_bytes(plistlib.dumps(info, fmt=plistlib.FMT_BINARY))
    app_directory = app / 'Contents/Resources/app'
    code = app_directory / 'voiceport-qq-bridge'
    code.mkdir(mode=0o700)
    for name in RUNTIME_FILES:
        (code / name).write_bytes(sources[name])
    pairing_token, admin_token = secrets.token_hex(32), secrets.token_hex(32)
    config = {'appDirectory': str(app_directory), 'originalMain': original_package['main'],
              'runtime': str(runtime), 'profile': str(Path.home() / 'Library/Containers/com.tencent.qq/Data/Library/Application Support/QQ'),
              'socketPath': str(runtime / 'bridge.sock'), 'tasksDirectory': str(tasks),
              'pairingToken': pairing_token, 'adminToken': admin_token, 'qqVersion': '7.0.1-52892'}
    write_json(code / 'runtime-config.json', config)
    original_package['main'] = './voiceport-qq-bridge/runtime_bootstrap.cjs'
    write_json(app_directory / 'package.json', original_package)
    adapter = output / 'adapter'
    adapter.mkdir(mode=0o700)
    for name in ADAPTER_FILES:
        (adapter / name).write_bytes(sources[name])
    materialize_codec(adapter / 'node_modules/silk-wasm', args.encoder_tgz)
    entitlements = output / 'copy-entitlements.plist'
    entitlements.write_bytes(plistlib.dumps({'com.apple.security.cs.allow-jit': True,
        'com.apple.security.cs.allow-unsigned-executable-memory': True,
        'com.apple.security.cs.disable-library-validation': True}))
    signed = []
    for bundle, _, _ in sorted(rows, key=lambda row: len(row[0].parts), reverse=True):
        journal.update(stage='owned_copy_sign_attempt', signing_bundle=str(bundle.relative_to(app)))
        write_json(output / 'preparation.json', journal)
        result = subprocess.run(['/usr/bin/codesign', '--force', '--sign', '-', '--options', 'runtime',
                                 '--entitlements', str(entitlements), str(bundle)], capture_output=True, timeout=40)
        require(result.returncode == 0, 'owned_copy_sign_failed')
        signed.append(str(bundle.relative_to(app)))
    signature(app)
    require(fingerprint(source) == ORIGINAL_FILES, 'original_installation_changed')
    signature(source)
    manifest = {'schema_version': 1, 'protocol_version': 1, 'id': 'qq.smartptt', 'version': '1.0.0',
        'name': 'QQ SmartPtt', 'origin': '本机 QQ 7.0.1-52892 独立副本；联网转写由 QQ 提供', 'adapter': 'jsonl',
        'capabilities': {'batch': True, 'streaming': False, 'system_preview': False},
        'fields': [{'id': 'socket_path', 'label': '本机桥接 Socket 路径', 'kind': 'string', 'required': True},
                   {'id': 'pairing_token', 'label': '本机桥接配对密钥', 'kind': 'secret', 'required': True}],
        'program': {'executable': str(node), 'args': [str(adapter / 'adapter.cjs')]}}
    write_json(output / 'qq-smartptt.json', manifest)
    write_json(output / 'client-config.json', {'socket_path': config['socketPath'], 'pairing_token': pairing_token})
    write_json(output / 'installation.json', {'app': str(app), 'original_app': str(source), 'runtime': str(runtime),
        'config': str(code / 'runtime-config.json'), 'node': str(node), 'manifest': str(output / 'qq-smartptt.json')})
    (output / 'manage.py').write_bytes(Path(__file__).read_bytes())
    (output / 'manage.py').chmod(0o600)
    journal.update(stage='prepared', signed_bundles=signed, original_files_unchanged=True)
    write_json(output / 'preparation.json', journal)
    return {'status': 'prepared', 'manifest': str(output / 'qq-smartptt.json'), 'started': False}


def load(output):
    private_directory(output)
    plan = json.loads((output / 'installation.json').read_bytes())
    require('superseded_by' not in plan, 'installation_superseded_use_relocated_directory')
    config = json.loads(Path(plan['config']).read_bytes())
    if Path(plan['runtime']).exists():
        private_directory(Path(plan['runtime']))
    return plan, config


def processes(plan):
    prefixes = [plan['app'] + '/', plan['original_app'] + '/']
    result = subprocess.run(['/bin/ps', '-ww', '-axo', 'pid=,ppid=,comm='], capture_output=True, text=True, check=True, timeout=3)
    rows = [line.strip().split(None, 2) for line in result.stdout.splitlines()]
    return [{'pid': int(pid), 'parent': int(parent), 'executable': exe} for pid, parent, exe in rows
            if any(exe.startswith(prefix) for prefix in prefixes)]


def identity(pid):
    result = subprocess.run(['/bin/ps', '-ww', '-p', str(pid), '-o', 'comm=,lstart='],
                            capture_output=True, text=True, timeout=3, env={**os.environ, 'LC_ALL': 'C'})
    return result.stdout.strip() if result.returncode == 0 else None


def wait_until(predicate, seconds):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.1)
    return bool(predicate())


def journal(output, event, **details):
    with (output / 'lifecycle.jsonl').open('a', encoding='utf8') as handle:
        os.chmod(handle.name, 0o600)
        handle.write(json.dumps({'event': event, 'at': time.time(), **details}) + '\n')
        handle.flush()
        os.fsync(handle.fileno())


def control(config, op):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(5)
        client.connect(config['socketPath'])
        client.sendall((json.dumps({'op': op, 'token': config['adminToken']}) + '\n').encode())
        data = bytearray()
        while b'\n' not in data:
            chunk = client.recv(4096)
            require(chunk and len(data) + len(chunk) <= 10000, 'invalid_control_response')
            data.extend(chunk)
        return json.loads(data)


def start(args, plan, config):
    output = args.output
    app = Path(plan['app'])
    require(fingerprint(Path(plan['original_app'])) == ORIGINAL_FILES, 'original_installation_changed')
    signature(app)
    rows = processes(plan)
    require(not any(row['executable'].startswith(str(app) + '/') for row in rows), 'bridge_already_running')
    # A previous unresolved launch must be diagnosed through status/stop, never repeated.
    state_file = output / 'launch.json'
    if state_file.exists():
        old = json.loads(state_file.read_bytes())
        require(old.get('stage') == 'stopped', 'previous_launch_requires_status_and_stop')
    runtime = Path(plan['runtime'])
    require(str(runtime).startswith('/private/tmp/vpqq-') and runtime.parent == Path('/private/tmp'), 'invalid_runtime_directory')
    if not runtime.exists():
        runtime.mkdir(mode=0o700)
    private_directory(runtime)
    tasks = Path(config['tasksDirectory'])
    require(tasks == runtime / 'tasks', 'invalid_task_directory')
    if not tasks.exists():
        tasks.mkdir(mode=0o700)
    private_directory(tasks)
    original_exe = str(Path(plan['original_app']) / 'Contents/MacOS/QQ')
    mains = [row for row in rows if row['executable'] == original_exe]
    require(len(mains) <= 1 and (not mains or args.replace_qq), 'quit_original_qq_or_use_replace_qq')
    for row in mains:
        before = identity(row['pid'])
        journal(output, 'original_main_term_attempt', **row, identity=before)
        require(identity(row['pid']) == before, 'original_process_identity_changed')
        os.kill(row['pid'], signal.SIGTERM)
        require(wait_until(lambda: identity(row['pid']) != before, 10), 'original_qq_did_not_exit')
    require(not any(row['executable'].startswith(plan['original_app'] + '/Contents/Frameworks/QQ Helper')
                    or row['executable'] == original_exe for row in processes(plan)), 'original_qq_helper_still_running')
    require(not Path(config['socketPath']).exists(), 'stale_socket_requires_stop')
    runtime = Path(plan['runtime'])
    env_temp = runtime / 'env-temp'
    env_temp.mkdir(mode=0o700, exist_ok=True)
    state = {'stage': 'launch_attempt', 'app': str(app), 'original_was_running': bool(mains), 'at': time.time()}
    write_json(state_file, state)
    journal(output, 'owned_main_launch_attempt', executable=str(app / 'Contents/MacOS/QQ'))
    env = {'HOME': str(Path.home()), 'TMPDIR': str(env_temp) + '/', 'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'LANG': 'en_US.UTF-8'}
    child = subprocess.Popen([str(app / 'Contents/MacOS/QQ')], cwd=runtime, env=env,
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             start_new_session=True)
    state.update(stage='running', pid=child.pid, identity=identity(child.pid))
    write_json(state_file, state)
    require(wait_until(lambda: Path(config['socketPath']).exists() or child.poll() is not None, 12), 'bridge_socket_not_created')
    require(child.poll() is None, 'bridge_app_exited_inspect_status')
    return {'status': 'running', 'pid': child.pid, 'bridge': control(config, 'status')}


def stop(args, plan, config):
    output = args.output
    journal(output, 'owned_stop_attempt')
    prefix = plan['app'] + '/'
    owned = [row for row in processes(plan) if row['executable'].startswith(prefix)]
    if Path(config['socketPath']).exists() and owned:
        journal(output, 'authenticated_shutdown_attempt')
        try:
            control(config, 'shutdown')
        except (OSError, RuntimeError, json.JSONDecodeError):
            pass
        wait_until(lambda: not any(row['executable'].startswith(prefix) for row in processes(plan)), 8)
    for row in processes(plan):
        if not row['executable'].startswith(prefix):
            continue
        before = identity(row['pid'])
        journal(output, 'owned_term_attempt', **row, identity=before)
        if identity(row['pid']) == before:
            os.kill(row['pid'], signal.SIGTERM)
    require(wait_until(lambda: not any(row['executable'].startswith(prefix) for row in processes(plan)), 5),
            'owned_process_still_running_no_restore')
    socket_path = Path(config['socketPath'])
    if socket_path.exists():
        info = socket_path.lstat()
        require(stat.S_ISSOCK(info.st_mode) and info.st_uid == os.getuid(), 'unexpected_socket_path')
        journal(output, 'dead_owned_socket_unlink_attempt', inode=info.st_ino)
        socket_path.unlink()
    # A dead QQ process can leave only its private task files; no process can read them now.
    tasks = Path(config['tasksDirectory'])
    if tasks.exists():
        private_directory(tasks)
    for directory in tasks.iterdir() if tasks.exists() else []:
        require(directory.name.startswith('task-') and directory.is_dir() and not directory.is_symlink(), 'unexpected_task_storage')
        files = list(directory.iterdir())
        require(all(item.name == 'input.silk' and item.is_file() and not item.is_symlink() for item in files), 'unexpected_task_file')
        journal(output, 'dead_owned_task_cleanup_attempt', directory=directory.name)
        for item in files:
            item.unlink()
        directory.rmdir()
    write_json(output / 'launch.json', {'stage': 'stopped', 'at': time.time()})
    require(fingerprint(Path(plan['original_app'])) == ORIGINAL_FILES, 'original_installation_changed')
    if args.restore_qq:
        original_exe = str(Path(plan['original_app']) / 'Contents/MacOS/QQ')
        if not any(row['executable'] == original_exe for row in processes(plan)):
            journal(output, 'original_restore_attempt')
            subprocess.run(['/usr/bin/open', '-g', plan['original_app']], check=True, timeout=10)
            require(wait_until(lambda: any(row['executable'] == original_exe for row in processes(plan)), 8), 'original_restore_not_observed')
    return {'status': 'stopped', 'remaining_owned_processes': [], 'original_files_unchanged': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    commands = parser.add_subparsers(dest='command', required=True)
    create = commands.add_parser('prepare')
    create.add_argument('--qq-app', type=Path, default=Path('/Applications/QQ.app'))
    create.add_argument('--node')
    create.add_argument('--encoder-tgz', type=Path)
    launch = commands.add_parser('start')
    launch.add_argument('--replace-qq', action='store_true', help='Normally quit original QQ first; shares QQ-managed profile')
    commands.add_parser('status')
    commands.add_parser('login')
    end = commands.add_parser('stop')
    end.add_argument('--restore-qq', action='store_true')
    args = parser.parse_args()
    require(args.output.is_absolute() and args.output.resolve() == args.output, 'absolute_canonical_output_required')
    if args.command == 'prepare':
        return prepare(args)
    plan, config = load(args.output)
    with (args.output / 'control.lock').open('a') as lock:
        os.chmod(lock.name, 0o600)
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        if args.command == 'start':
            return start(args, plan, config)
        if args.command == 'stop':
            return stop(args, plan, config)
        if args.command == 'login':
            journal(args.output, 'normal_login_button_attempt')
            return control(config, 'login')
        owned = [row for row in processes(plan) if row['executable'].startswith(plan['app'] + '/')]
        return {'processes': owned, 'bridge': control(config, 'status') if owned else None}


if __name__ == '__main__':
    try:
        print(json.dumps(main(), ensure_ascii=False))
    except Exception as error:
        # No exception details containing paths, auth tokens or target payloads.
        code = str(error) if type(error) is RuntimeError else type(error).__name__
        print(json.dumps({'status': 'error', 'code': code}), file=sys.stderr)
        sys.exit(1)
