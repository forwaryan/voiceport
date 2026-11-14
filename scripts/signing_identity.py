#!/usr/bin/env python3
"""Resolve one packaging identity without changing Keychain or saved settings."""
import json
import os
import pathlib
import re
import stat
import subprocess
import sys

FINGERPRINT = re.compile(r'[0-9A-Fa-f]{40}')
IDENTITY_LINE = re.compile(r'\s*\d+\) ([0-9A-Fa-f]{40}) "(.*)"\s*')


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'duplicate config field: {key}')
        result[key] = value
    return result


def _saved_identity(path):
    # Reject links in the config path, including a dangling leaf or parent link.
    for part in [*reversed(path.parents), path]:
        try:
            info = part.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(info.st_mode):
            raise ValueError(f'signing config path must not contain a symlink: {part}')
    try:
        info = path.lstat()
    except FileNotFoundError:
        return None
    if not stat.S_ISREG(info.st_mode):
        raise ValueError(f'signing config must be a regular file: {path}')
    # Do not follow a leaf replaced by a symlink between validation and open.
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, encoding='utf-8') as source:
        opened = os.fstat(source.fileno())
        if not stat.S_ISREG(opened.st_mode) or (opened.st_dev, opened.st_ino) != (info.st_dev, info.st_ino):
            raise ValueError(f'signing config changed while opening: {path}')
        content = source.read(16385)
    if len(content) > 16384:
        raise ValueError(f'signing config exceeds 16 KiB: {path}')
    config = json.loads(content, object_pairs_hook=_unique_object)
    if (not isinstance(config, dict) or set(config) != {'schema_version', 'identity'}
            or type(config['schema_version']) is not int or config['schema_version'] != 1
            or not isinstance(config['identity'], str)
            or not FINGERPRINT.fullmatch(config['identity'])):
        raise ValueError(f'expected signing config with schema_version: 1 and identity: 40 hex SHA1: {path}')
    return config['identity'].upper()


TEAM_ID = re.compile(r'[A-Z0-9]{10}')


def validate_team_id(team_id):
    if not isinstance(team_id, str) or not TEAM_ID.fullmatch(team_id):
        raise ValueError('distribution signing requires a 10-character Apple Team ID')
    return team_id


def resolve_identity(explicit, config_path, *, distribution=False, team_id=None):
    """Return a verified SHA1, or '-' only when no persistent identity is saved.

    Explicit certificate names remain supported, but must match exactly one
    valid identity. Config validation always runs, even with an override.
    """
    if distribution:
        validate_team_id(team_id)
    config_path = pathlib.Path(config_path)
    try:
        saved = _saved_identity(config_path)
    except (OSError, ValueError, UnicodeError) as error:
        raise ValueError(f'cannot use signing config {config_path}: {error}') from error
    selected = explicit if explicit is not None else saved
    if selected is None or selected == '-':
        if distribution:
            raise ValueError('distribution requires a Developer ID Application identity; ad-hoc signing is forbidden')
        if saved is not None:
            raise ValueError('VOICEPORT_CODESIGN_IDENTITY=- is forbidden while a persistent signing identity is configured')
        print('Using ad-hoc signing; macOS permissions may require approval after updates.', file=sys.stderr)
        return '-'
    if not isinstance(selected, str) or not selected.strip():
        raise ValueError('VOICEPORT_CODESIGN_IDENTITY must not be empty')
    try:
        result = subprocess.run(['/usr/bin/security', 'find-identity', '-v', '-p', 'codesigning'],
                                check=True, capture_output=True, text=True, timeout=15)
    except (OSError, subprocess.SubprocessError) as error:
        raise ValueError('could not list valid signing identities; refusing ad-hoc fallback') from error
    identities = [match.groups() for line in result.stdout.splitlines()
                  if (match := IDENTITY_LINE.fullmatch(line))]
    if FINGERPRINT.fullmatch(selected):
        matches = {fingerprint.upper() for fingerprint, _ in identities if fingerprint.upper() == selected.upper()}
    else:
        matches = {fingerprint.upper() for fingerprint, name in identities if name == selected}
    if len(matches) != 1:
        raise ValueError('selected signing identity is unavailable or ambiguous in security find-identity; '
                         'refusing ad-hoc fallback')
    fingerprint = matches.pop()
    if distribution:
        names = {name for value, name in identities if value.upper() == fingerprint}
        if len(names) != 1 or not re.fullmatch(r'Developer ID Application: .+ \(' + re.escape(team_id) + r'\)', names.pop()):
            raise ValueError('distribution identity must be Developer ID Application for the expected Team ID')
    return fingerprint


def main():
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--distribution', action='store_true')
    args = parser.parse_args()
    try:
        identity = resolve_identity(os.environ.get('VOICEPORT_CODESIGN_IDENTITY'),
                                    pathlib.Path.home() / '.config/voiceport/signing.json',
                                    distribution=args.distribution,
                                    team_id=os.environ.get('VOICEPORT_TEAM_ID'))
    except (OSError, ValueError) as error:
        print(f'Signing identity error: {error}', file=sys.stderr)
        return 1
    print(identity)
    return 0


if __name__ == '__main__':
    sys.exit(main())
