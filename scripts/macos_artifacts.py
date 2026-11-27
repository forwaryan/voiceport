#!/usr/bin/env python3
"""Verify a final VoicePort macOS ZIP and its declared attachments."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import unicodedata
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_bundle as bundle
from distribution_signing import validate as verify_developer_id, verify_requirement

MANIFEST = 'artifact-manifest.json'
TEST_ARCHIVE = 'VoicePort-Test-macos-arm64.zip'
MAX_ZIP = 64 * 1024 * 1024
MAX_EXPANSION = 256 * 1024 * 1024
MAX_ENTRIES = 16_384
MAX_MANIFEST = 256 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def valid_sha256(value):
    return isinstance(value, str) and re.fullmatch(r'[0-9a-f]{64}', value) is not None


def attachment_name(name):
    require(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*', name) and name != MANIFEST,
            f'invalid attachment name: {name!r}')
    return name


def exact_directory(directory, names):
    require(stat.S_ISDIR(directory.lstat().st_mode), 'expected a real attachment directory')
    require({p.name for p in directory.iterdir()} == set(names),
            'attachment directory has missing or unexpected entries')


def read_file(path, limit=None):
    """Open only regular files, without following a link at the attachment name."""
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), 'rb') as source:
        before = os.fstat(source.fileno())
        require(stat.S_ISREG(before.st_mode), f'expected regular file: {path.name}')
        if limit is None:
            result = {'bytes': before.st_size, 'sha256': hashlib.file_digest(source, 'sha256').hexdigest()}
        else:
            require(before.st_size <= limit, f'file exceeds size limit: {path.name}')
            result = source.read(limit + 1)
            require(len(result) == before.st_size, f'file size changed: {path.name}')
        after = os.fstat(source.fileno())
        require((before.st_size, before.st_mtime_ns, before.st_ctime_ns)
                == (after.st_size, after.st_mtime_ns, after.st_ctime_ns), f'file changed: {path.name}')
    return result


def write_manifest(directory, archive, attachments):
    names = [attachment_name(name) for name in [archive, *attachments]]
    require(len({name.casefold() for name in names}) == len(names), 'duplicate attachment name')
    exact_directory(directory, names)
    manifest = {'schema_version': 1, 'archive': archive,
                'files': {name: read_file(directory / name) for name in names}}
    data = (json.dumps(manifest, indent=2, sort_keys=True) + '\n').encode()
    require(len(data) <= MAX_MANIFEST, 'attachment manifest exceeds size limit')
    with (directory / MANIFEST).open('xb') as target:
        target.write(data)
    return sha256(data)


def unique_object(pairs):
    result = dict(pairs)
    require(len(result) == len(pairs), 'duplicate JSON key in attachment manifest')
    return result


def read_manifest(directory, digest):
    require(valid_sha256(digest), 'expected a lowercase SHA-256 for the trusted attachment manifest')
    require(stat.S_ISDIR(directory.lstat().st_mode), 'expected a real attachment directory')
    data = read_file(directory / MANIFEST, MAX_MANIFEST)
    require(sha256(data) == digest, 'attachment manifest SHA-256 mismatch')
    value = json.loads(data, object_pairs_hook=unique_object)
    require(isinstance(value, dict) and set(value) == {'schema_version', 'archive', 'files'}
            and type(value['schema_version']) is int and value['schema_version'] == 1
            and isinstance(value['archive'], str) and isinstance(value['files'], dict),
            'invalid attachment manifest schema')
    archive, files = value['archive'], value['files']
    require(archive in files, 'manifest does not list the final ZIP')
    require(len({name.casefold() for name in files}) == len(files), 'duplicate attachment name')
    for name, record in files.items():
        attachment_name(name)
        require(isinstance(record, dict) and set(record) == {'bytes', 'sha256'}
                and type(record['bytes']) is int and record['bytes'] >= 0
                and valid_sha256(record['sha256']), f'invalid manifest record: {name}')
    exact_directory(directory, [MANIFEST, *files])
    return archive, files


def extract_zip(data, directory, root):
    """Extract only the application tree. Check but do not restore AppleDouble metadata."""
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        entries = archive.infolist()
        require(0 < len(entries) <= MAX_ENTRIES, 'invalid ZIP entry count')
        require(sum(entry.file_size for entry in entries) <= MAX_EXPANSION, 'ZIP expansion exceeds size limit')
        seen = {}
        members = {}
        metadata = []
        directory_modes = []
        for entry in entries:
            name = entry.orig_filename
            parts = name.removesuffix('/').split('/')
            require(len(parts) <= 64 and not any(part in ('', '.', '..') or any(c in part for c in '\\:\0')
                                                 for part in parts), f'invalid ZIP member path: {name!r}')
            is_directory = entry.is_dir()
            mode = entry.external_attr >> 16
            kind = stat.S_IFMT(mode)
            require(kind in (0, stat.S_IFDIR if is_directory else stat.S_IFREG)
                    and not mode & 0o7000 and not entry.flag_bits & 1
                    and entry.compress_type in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED)
                    and (not is_directory or entry.file_size == 0),
                    f'unsafe ZIP member: {name!r}')
            # Reject case and Unicode aliases on any path component, including
            # parents which are implicit in the central directory.
            for index in range(1, len(parts) + 1):
                path = '/'.join(parts[:index])
                key = unicodedata.normalize('NFD', path.casefold())
                require(seen.setdefault(key, path) == path, f'aliased ZIP member: {name!r}')
            path = '/'.join(parts)
            require(path not in members, f'duplicate ZIP member: {name!r}')
            members[path] = is_directory
            if parts[0] == '__MACOSX':
                metadata.append((parts[1:], is_directory))
            else:
                require(parts[0] == root and (len(parts) > 1 or is_directory),
                        f'unexpected ZIP root: {name!r}')
            with archive.open(entry) as source:
                if parts[0] == '__MACOSX':
                    if not is_directory:
                        require(source.read(4) == b'\x00\x05\x16\x07', 'invalid AppleDouble metadata')
                        while source.read(65536):
                            pass
                    continue
                target = directory.joinpath(*parts)
                # ZIP creator systems 3 and 19 encode Unix and macOS modes.
                has_unix_mode = entry.create_system in (3, 19)
                permissions = stat.S_IMODE(mode)
                if is_directory:
                    source.read(1)  # Verify the local header and CRC as for files.
                    if has_unix_mode:
                        require(permissions & 0o500 == 0o500,
                                f'ZIP directory must be readable and searchable by its owner: {name!r}')
                        directory_modes.append((target, permissions))
                    target.mkdir(parents=True, exist_ok=True)
                else:
                    target.parent.mkdir(parents=True, exist_ok=True)
                    with target.open('xb') as output:
                        shutil.copyfileobj(source, output, length=65536)
                    require(target.stat().st_size == entry.file_size, f'ZIP member size mismatch: {name!r}')
                    target.chmod(permissions if has_unix_mode else 0o644)
        app = directory / root
        require(app.is_dir(), 'ZIP lacks the expected application root')
        for parts, is_directory in metadata:
            if is_directory:
                require(not parts or (parts[0] == root and directory.joinpath(*parts).is_dir()),
                        'unexpected AppleDouble directory')
            else:
                require(bool(parts) and parts[-1].startswith('._'), 'unexpected AppleDouble file')
                actual = [*parts[:-1], parts[-1][2:]]
                require(actual[0] == root and directory.joinpath(*actual).exists(), 'orphan AppleDouble file')
        # Apply modes after extracting all children, including read-only directories.
        for target, permissions in directory_modes:
            target.chmod(permissions)
    return app


def codesign(*args):
    return subprocess.run(['/usr/bin/codesign', *args], check=True, capture_output=True,
                          text=True, timeout=30).stderr.splitlines()


def verify_app(app, *, version, build, signature, certificate=None, team_id=None):
    test = signature == 'test'
    _, cli = bundle.validate_layout(app, test=test, stapled=signature == 'distribution')
    info = plistlib.loads(read_file(app / 'Contents/Info.plist', 65536))
    require(isinstance(info, dict), 'invalid bundle Info.plist')
    for key, expected in {'CFBundleIdentifier': bundle.TEST_ID if test else bundle.APP_ID,
                          'CFBundleName': bundle.TEST_NAME if test else bundle.APP_NAME,
                          'CFBundleDisplayName': bundle.TEST_NAME if test else bundle.APP_NAME,
                          'CFBundleExecutable': bundle.TEST_MAIN if test else bundle.MAIN,
                          'CFBundlePackageType': 'APPL',
                          'CFBundleShortVersionString': version, 'CFBundleVersion': build}.items():
        require(info.get(key) == expected, f'bundle {key} mismatch')
    if signature == 'distribution':
        verify_developer_id(app, team_id)
    else:
        codesign('--verify', '--deep', '--strict', app)
        cli_id = bundle.TEST_ID + '.cli' if test else bundle.CLI
        for binary, identifier in [(app, info['CFBundleIdentifier']), (cli, cli_id)]:
            codesign('--verify', '--strict', binary)
            fields = codesign('--display', '--verbose=4', binary)
            require(f'Identifier={identifier}' in fields, f'unexpected signature identifier: {binary.name}')
            if test:
                require('Signature=adhoc' in fields, f'expected ad-hoc test signature: {binary.name}')
            else:
                verify_requirement(binary, f'certificate leaf = H"{certificate}"')


def validate_identity_options(version, build, signature, certificate, team_id):
    require(re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', version),
            'expected a release version X.Y.Z')
    require(re.fullmatch(r'[1-9][0-9]*', build) and int(build) < 2**64, 'expected a positive numeric build')
    require((signature == 'preview') == (certificate is not None)
            and (signature == 'distribution') == (team_id is not None),
            'preview requires only --certificate-sha1; distribution requires only --team-id')
    if certificate is not None:
        require(re.fullmatch(r'[0-9A-Fa-f]{40}', certificate), 'expected a 40-digit certificate SHA-1 fingerprint')


def verify(directory, digest, *, version, build, signature, certificate=None, team_id=None):
    validate_identity_options(version, build, signature, certificate, team_id)
    archive, files = read_manifest(directory, digest)
    allowed = {TEST_ARCHIVE} if signature == 'test' else {
        'VoicePort-macos-arm64.zip', f'VoicePort-v{version}-macos-arm64.zip',
    }
    require(archive in allowed, 'unexpected final ZIP name')
    require(0 < files[archive]['bytes'] <= MAX_ZIP, 'invalid final ZIP size')
    data = read_file(directory / archive, MAX_ZIP)
    for name, record in files.items():
        actual = {'bytes': len(data), 'sha256': sha256(data)} if name == archive else read_file(directory / name)
        require(actual['bytes'] == record['bytes'], f'attachment size mismatch: {name}')
        require(actual['sha256'] == record['sha256'], f'attachment SHA-256 mismatch: {name}')
    root = (bundle.TEST_NAME if signature == 'test' else bundle.APP_NAME) + '.app'
    with tempfile.TemporaryDirectory(prefix='voiceport-artifact-check-') as temp:
        app = extract_zip(data, Path(temp), root)
        verify_app(app, version=version, build=build, signature=signature, certificate=certificate, team_id=team_id)
    return {'status': 'verified', 'manifest_sha256': digest, 'archive': archive, 'files': files,
            'version': version, 'build': build, 'signature': signature}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    manifest = commands.add_parser('manifest', help='record an explicit attachment set in a clean directory')
    manifest.add_argument('directory', type=Path)
    manifest.add_argument('--archive', required=True)
    manifest.add_argument('--attachment', action='append', default=[])
    verification = commands.add_parser('verify', help='verify a final ZIP and every declared attachment')
    verification.add_argument('directory', type=Path)
    verification.add_argument('--manifest-sha256', required=True)
    verification.add_argument('--version', required=True)
    verification.add_argument('--build', required=True)
    verification.add_argument('--signature', choices=['test', 'preview', 'distribution'], required=True)
    verification.add_argument('--certificate-sha1')
    verification.add_argument('--team-id')
    args = parser.parse_args()
    try:
        if args.command == 'manifest':
            digest = write_manifest(args.directory, args.archive, args.attachment)
            report = {'status': 'recorded', 'manifest_sha256': digest}
        else:
            report = verify(args.directory, args.manifest_sha256, version=args.version, build=args.build,
                            signature=args.signature, certificate=args.certificate_sha1, team_id=args.team_id)
        print(json.dumps(report, sort_keys=True))
    except (OSError, ValueError, AssertionError, zipfile.BadZipFile, subprocess.SubprocessError) as error:
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr.strip(), file=sys.stderr)
        print(f'macOS artifact check failed: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
