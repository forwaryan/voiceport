#!/usr/bin/env python3
"""Install a verified bundle at one stable path, preserving the previous bundle."""
import argparse
import fcntl
import json
import os
import pathlib
import plistlib
import shutil
import stat
import subprocess
import tempfile
import time

BUNDLE_ID = 'io.github.forwaryan.voiceport'


def real_path(path):
    path = pathlib.Path(os.path.abspath(path))
    for ancestor in [*reversed(path.parents), path]:
        if ancestor.is_symlink():
            raise ValueError(f'linked install path: {ancestor}')
    return path


def bundle_info(app):
    from check_bundle import real_tree
    real_tree(app)
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    if info.get('CFBundleIdentifier') != BUNDLE_ID or info.get('CFBundleExecutable') != 'voiceport-app':
        raise ValueError('expected VoicePort bundle identity and executable')
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True, capture_output=True)
    identity = subprocess.run(['codesign', '-dv', str(app)], check=True, capture_output=True, text=True).stderr
    if f'Identifier={BUNDLE_ID}' not in identity.splitlines():
        raise ValueError('signature identifier does not match VoicePort')
    return info


def signing_continues(previous, candidate):
    result = subprocess.run(['codesign', '-d', '-r-', str(previous)], check=True, capture_output=True, text=True)
    lines = [line.removeprefix('# ').strip() for line in (result.stdout + '\n' + result.stderr).splitlines()]
    requirement = next((line.partition('designated => ')[2] for line in lines
                        if line.startswith('designated => ')), '')
    if not requirement:
        raise ValueError('could not read the installed designated requirement')
    return subprocess.run(['codesign', '--verify', '--strict', '-R=' + requirement, str(candidate)],
                          capture_output=True).returncode == 0


def running_status(cli):
    result = subprocess.run([str(cli), '--json', 'app', 'status'], capture_output=True, text=True, timeout=10)
    response = json.loads(result.stdout)
    if result.returncode == 3 and response.get('error', {}).get('code') == 'app_not_running':
        return None
    if result.returncode != 0 or response.get('status') != 'ok':
        raise ValueError('could not establish running App status; no files changed')
    return response['data']


def stop_idle(cli, allow_quit):
    current = running_status(cli)
    processes = subprocess.check_output(['ps', '-axo', 'pid=,comm='], text=True)
    pids = {int(line.strip().split(None, 1)[0]) for line in processes.splitlines()
            if line.strip().endswith('/Contents/MacOS/voiceport-app')}
    known_pid = current['pid'] if current else None
    if pids - ({known_pid} if known_pid else set()):
        raise ValueError('another VoicePort process has no matching IPC status; close it before installing')
    if current is None:
        return
    if current['busy'] or current['pending'] or current['operation'] is not None or current.get('discovering'):
        raise ValueError('VoicePort has unfinished work; finish it before installing')
    if current.get('state') != 'idle':
        raise ValueError('VoicePort may retain a result or failed recording; save it and quit the App before installing')
    bundle_info(real_path(current['bundle_path']))
    if not allow_quit:
        raise ValueError('VoicePort is running; quit it or use --quit-idle')
    result = subprocess.run([str(cli), '--json', 'app', 'quit'], capture_output=True, text=True, timeout=10)
    response = json.loads(result.stdout)
    if result.returncode != 0 or response.get('data', {}).get('quitting') is not True:
        raise ValueError('graceful quit was not confirmed; no files changed')
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            os.kill(known_pid, 0)
        except ProcessLookupError:
            return
        time.sleep(0.1)
    raise ValueError('App has not exited; no files changed')


def install(source, destination, *, quit_idle=False, allow_signing_change=False):
    source, destination = real_path(source), real_path(destination)
    if destination.name != 'VoicePort.app':
        raise ValueError('use the stable application name VoicePort.app')
    if source == destination or source in destination.parents or destination in source.parents:
        raise ValueError('source and destination must be separate bundles')
    bundle_info(source)
    if destination.exists():
        bundle_info(destination)
        if not signing_continues(destination, source) and not allow_signing_change:
            raise ValueError('update does not satisfy the installed signing requirement; use the same signing certificate '
                             'or explicitly use --allow-signing-change and approve macOS permissions again')
    destination.parent.mkdir(parents=True, exist_ok=True)
    if not stat.S_ISDIR(destination.parent.lstat().st_mode):
        raise ValueError('expected real installation directory')
    # Share the kernel lock with the in-app updater. A crash releases the lock.
    lock_fd = os.open(destination.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    backup = None
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if os.path.lexists(destination.parent / '.voiceport-install.lock'):
            raise ValueError('an older installer lock exists; verify its owner before recovery')
        with tempfile.TemporaryDirectory(prefix='.voiceport-update-', dir=destination.parent) as temp:
            staged = pathlib.Path(temp) / 'VoicePort.app'
            shutil.copytree(source, staged)
            bundle_info(staged)
            stop_idle(source / 'Contents/MacOS/voiceport', quit_idle)
            # Recheck after staging and graceful quit, before replacing anything.
            if destination.exists():
                bundle_info(destination)
                if not signing_continues(destination, staged) and not allow_signing_change:
                    raise ValueError('installed signing identity changed during staging')
                backup_dir = pathlib.Path(tempfile.mkdtemp(prefix='.voiceport-backup-', dir=destination.parent))
                backup = backup_dir / 'VoicePort.app'
                destination.rename(backup)
            try:
                staged.rename(destination)
            except OSError:
                if backup is not None and not destination.exists():
                    backup.rename(destination)
                raise
        return {'installed': str(destination), 'backup': str(backup) if backup else None,
                'signing_change_allowed': allow_signing_change}
    finally:
        os.close(lock_fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=pathlib.Path)
    parser.add_argument('--destination', type=pathlib.Path, default=pathlib.Path.home() / 'Applications/VoicePort.app')
    parser.add_argument('--quit-idle', action='store_true', help='gracefully quit only a confirmed idle VoicePort')
    parser.add_argument('--allow-signing-change', action='store_true', help='explicit migration; macOS permissions may need approval')
    args = parser.parse_args()
    try:
        print(json.dumps(install(args.source, args.destination, quit_idle=args.quit_idle,
                                 allow_signing_change=args.allow_signing_change)))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f'Install stopped: {error}\n')


if __name__ == '__main__':
    import sys
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    main()
