#!/usr/bin/env python3
"""Prepare a Developer ID package, submit it once, and resume notarization by ID."""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent))
from distribution_signing import validate as validate_distribution
from signing_identity import resolve_identity, validate_team_id

ROOT = Path(__file__).resolve().parents[1]


def sha(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def write_new(path, value):
    with path.open('x', encoding='utf-8') as output:
        json.dump(value, output, indent=2)
        output.write('\n')
        output.flush()
        os.fsync(output.fileno())


def read_json(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError(f'expected regular release record: {path}')
    return json.loads(path.read_text(encoding='utf-8'))


def real_path(path):
    path = Path(os.path.abspath(Path(path).expanduser()))
    for part in [*reversed(path.parents), path]:
        if part.is_symlink():
            raise ValueError(f'release path must not contain links: {part}')
    return path


def record(folder, event):
    path = folder / 'journal.jsonl'
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, 'a', encoding='utf-8') as output:
        output.write(json.dumps(event) + '\n')
        output.flush()
        os.fsync(output.fileno())


def run(folder, stage, argv, *, timeout=120, env=None):
    operation = str(uuid.uuid4())
    record(folder, {'operation': operation, 'stage': stage, 'state': 'intent', 'argv': list(map(str, argv))})
    try:
        result = subprocess.run(list(map(str, argv)), cwd=ROOT, env=env,
                                capture_output=True, text=True, timeout=timeout)
    except (OSError, subprocess.SubprocessError) as error:
        record(folder, {'operation': operation, 'stage': stage, 'state': 'unknown', 'error': type(error).__name__})
        raise ValueError(f'{stage} result unknown; inspect journal before recovery; do not repeat submission') from error
    receipt = f'{stage}-{operation}.json'
    write_new(folder / receipt, {'returncode': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr})
    record(folder, {'operation': operation, 'stage': stage,
                    'state': 'completed' if result.returncode == 0 else 'failed', 'receipt': receipt})
    if result.returncode:
        raise ValueError(f'{stage} failed (exit {result.returncode}); details: {folder / receipt}')
    return result.stdout


@contextmanager
def lock(folder):
    path = folder / '.release-lock'
    with path.open('x') as output:
        output.write(str(os.getpid()))
        output.flush()
        os.fsync(output.fileno())
    try:
        yield
    finally:
        path.unlink()


def prepare(folder, team_id, skip_build):
    validate_team_id(team_id)
    # Credential and destination failures precede any build or output mutation.
    identity = resolve_identity(os.environ.get('VOICEPORT_CODESIGN_IDENTITY'),
                                Path.home() / '.config/voiceport/signing.json',
                                distribution=True, team_id=team_id)
    if folder.exists():
        raise ValueError('release output already exists; use resume for an existing submission')
    folder.mkdir(parents=True, exist_ok=False)
    with lock(folder):
        environment = dict(os.environ, VOICEPORT_CODESIGN_IDENTITY=identity,
                           VOICEPORT_TEAM_ID=team_id, VOICEPORT_APP_DIRECTORY=str(folder / 'VoicePort.app'))
        command = ['bash', ROOT / 'scripts/package_app.sh', '--distribution']
        if skip_build:
            command.append('--skip-build')
        run(folder, 'package', command, timeout=1800, env=environment)
        app = folder / 'VoicePort.app'
        validate_distribution(app, team_id)
        candidate = folder / 'notarization-upload.zip'
        run(folder, 'archive-submission', ['/usr/bin/ditto', '-c', '-k', '--sequesterRsrc', '--keepParent', app, candidate])
        info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
        write_new(folder / 'prepared.json', {
            'schema_version': 1, 'team_id': team_id, 'identity': identity,
            'version': info['CFBundleShortVersionString'], 'build': info['CFBundleVersion'],
            'archive_sha256': sha(candidate),
            'main_sha256': sha(app / 'Contents/MacOS/voiceport-app'),
            'cli_sha256': sha(app / 'Contents/MacOS/voiceport'),
        })
        record(folder, {'stage': 'prepared', 'state': 'completed'})


def check_prepared(folder):
    prepared = read_json(folder / 'prepared.json')
    if prepared.get('schema_version') != 1:
        raise ValueError('unsupported prepared release schema')
    validate_team_id(prepared.get('team_id'))
    candidate = real_path(folder / 'notarization-upload.zip')
    if not candidate.is_file() or sha(candidate) != prepared['archive_sha256']:
        raise ValueError('notarization archive changed after preparation')
    app = folder / 'VoicePort.app'
    validate_distribution(app, prepared['team_id'])
    for name, field in [('voiceport-app', 'main_sha256'), ('voiceport', 'cli_sha256')]:
        if sha(app / 'Contents/MacOS' / name) != prepared[field]:
            raise ValueError('signed executables changed after preparation')
    return prepared


def credentials(profile, keychain):
    if not profile or not profile.strip():
        raise ValueError('a Keychain notarization profile is required')
    result = ['--keychain-profile', profile]
    if keychain:
        result += ['--keychain', str(real_path(keychain))]
    return result


def submission_id(value):
    identifier = value.get('id')
    if not isinstance(identifier, str) or str(uuid.UUID(identifier)) != identifier.lower():
        raise ValueError('notarytool did not return a valid submission UUID; inspect original receipt, do not resubmit')
    return identifier


def submit(folder, profile, keychain):
    with lock(folder):
        check_prepared(folder)
        auth = credentials(profile, keychain)
        # Persisted intent fences duplicate uploads even after a crash or lost reply.
        if (folder / 'submit-intent.json').exists():
            raise ValueError('submission already attempted; use resume with the saved ID, never submit again')
        write_new(folder / 'submit-intent.json', {'state': 'intent', 'archive_sha256': sha(folder / 'notarization-upload.zip')})
        output = run(folder, 'notary-submit', ['/usr/bin/xcrun', 'notarytool', 'submit',
                     folder / 'notarization-upload.zip', *auth, '--output-format', 'json'], timeout=600)
        result = json.loads(output)
        identifier = submission_id(result)
        write_new(folder / 'submission.json', {'id': identifier})
        complete(folder, identifier, auth)


def recover_id(folder):
    if (folder / 'submission.json').exists():
        return submission_id(read_json(folder / 'submission.json'))
    # Recover only our original successful upload receipt, never a fresh upload.
    journal = folder / 'journal.jsonl'
    if journal.is_symlink():
        raise ValueError('linked release journal')
    for line in journal.read_text().splitlines():
        event = json.loads(line)
        if event.get('stage') != 'notary-submit':
            continue
        if event.get('state') == 'completed':
            receipt = event.get('receipt', '')
        elif event.get('state') == 'intent':
            # A crash can happen after fsync(receipt), before fsync(completed).
            operation = str(uuid.UUID(event['operation']))
            receipt = f'notary-submit-{operation}.json'
            if not (folder / receipt).exists():
                continue
        else:
            continue
        if not receipt or Path(receipt).name != receipt:
            raise ValueError('invalid release receipt path')
        result = read_json(folder / receipt)
        if result['returncode'] != 0:
            continue
        identifier = submission_id(json.loads(result['stdout']))
        write_new(folder / 'submission.json', {'id': identifier})
        return identifier
    raise ValueError('no confirmed submission ID; inspect the original journal and Apple notarization history; do not resubmit')


def complete(folder, identifier, auth):
    info = json.loads(run(folder, 'notary-info', ['/usr/bin/xcrun', 'notarytool', 'info', identifier,
                                               *auth, '--output-format', 'json']))
    if submission_id(info) != identifier:
        raise ValueError('notarization response has the wrong submission ID')
    if info.get('status') == 'In Progress':
        info = json.loads(run(folder, 'notary-wait', ['/usr/bin/xcrun', 'notarytool', 'wait', identifier,
                         *auth, '--timeout', '20m', '--output-format', 'json'], timeout=1250))
    if submission_id(info) != identifier or info.get('status') != 'Accepted':
        raise ValueError(f'notarization is {info.get("status", "unknown")}; no distributable archive created')
    prepared = check_prepared(folder)
    app = folder / 'VoicePort.app'
    # Stapling is local/idempotent; an interrupted run may already have attached it.
    run(folder, 'staple', ['/usr/bin/xcrun', 'stapler', 'staple', app])
    run(folder, 'validate-ticket', ['/usr/bin/xcrun', 'stapler', 'validate', app])
    run(folder, 'gatekeeper', ['/usr/sbin/spctl', '--assess', '--type', 'execute', '--verbose=4', app])
    check_prepared(folder)
    # Fixed output name avoids using untrusted version strings as paths.
    archive = real_path(folder / 'VoicePort-macos-arm64.zip')
    if archive.exists():
        raise ValueError('final archive already exists; inspect it instead of overwriting')
    run(folder, 'archive-final', ['/usr/bin/ditto', '-c', '-k', '--sequesterRsrc', '--keepParent', app, archive])
    write_new(folder / 'release.json', {**prepared, 'notarization_id': identifier, 'notarization_status': 'Accepted',
              'stapled': True, 'gatekeeper_accepted': True, 'release_archive': archive.name,
              'release_sha256': sha(archive)})
    record(folder, {'stage': 'release', 'state': 'completed', 'sha256': sha(archive)})


def resume(folder, profile, keychain):
    with lock(folder):
        check_prepared(folder)
        if (folder / 'release.json').exists():
            result = read_json(folder / 'release.json')
            if (result.get('notarization_status') != 'Accepted' or result.get('release_archive') != 'VoicePort-macos-arm64.zip'
                    or sha(real_path(folder / result['release_archive'])) != result.get('release_sha256')):
                raise ValueError('completed release archive does not match its receipt')
            return
        complete(folder, recover_id(folder), credentials(profile, keychain))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    prepare_parser = commands.add_parser('prepare', help='local signing and upload archive preparation')
    prepare_parser.add_argument('output', type=Path)
    prepare_parser.add_argument('--team-id', required=True)
    prepare_parser.add_argument('--skip-build', action='store_true')
    for command in ['submit', 'resume']:
        child = commands.add_parser(command, help='submit once' if command == 'submit' else 'check saved submission without uploading again')
        child.add_argument('output', type=Path)
        child.add_argument('--notary-profile', required=True)
        child.add_argument('--keychain', type=Path)
    args = parser.parse_args()
    try:
        folder = real_path(args.output)
        if args.command == 'prepare':
            prepare(folder, args.team_id, args.skip_build)
        elif args.command == 'submit':
            submit(folder, args.notary_profile, args.keychain)
        else:
            resume(folder, args.notary_profile, args.keychain)
        print(json.dumps({'status': 'prepared' if args.command == 'prepare' else 'ready', 'output': str(folder)}))
    except (OSError, ValueError, AssertionError, KeyError, subprocess.SubprocessError) as error:
        print(f'Release failed: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
