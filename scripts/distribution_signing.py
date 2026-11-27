#!/usr/bin/env python3
"""Verify Developer ID, Team ID, hardened runtime and entitlements without launching."""
import argparse
import json
from pathlib import Path
import plistlib
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check_bundle import real_tree
from signing_identity import validate_team_id

BUNDLE_ID = 'io.github.forwaryan.voiceport'
CLI_ENTITLEMENTS = {'com.apple.security.cs.disable-library-validation': True}
APP_ENTITLEMENTS = {**CLI_ENTITLEMENTS, 'com.apple.security.device.audio-input': True}


def require_metadata(metadata, *, team_id, identifier):
    fields = metadata.splitlines()
    if f'TeamIdentifier={team_id}' not in fields or f'Identifier={identifier}' not in fields:
        raise ValueError('distribution signature has an unexpected Team ID or identifier')
    if not any(re.fullmatch(r'Authority=Developer ID Application: .+ \(' + re.escape(team_id) + r'\)', line)
               for line in fields):
        raise ValueError('distribution signature must use Developer ID Application')
    if not re.search(r'flags=0x[0-9a-fA-F]+\([^)]*\bruntime\b[^)]*\)', metadata):
        raise ValueError('distribution signature is missing hardened runtime')
    if not any(line.startswith('Timestamp=') and line != 'Timestamp=none' and line[10:].strip()
               for line in fields):
        raise ValueError('distribution signature is missing a secure timestamp')


def verify_requirement(binary, requirement):
    subprocess.run(['/usr/bin/codesign', '--verify', '--strict', f'-R={requirement}', str(binary)],
                   check=True, capture_output=True, text=True, timeout=30)


def validate(app, team_id):
    validate_team_id(team_id)
    app = Path(app)
    real_tree(app)
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    if info.get('CFBundleIdentifier') != BUNDLE_ID or info.get('CFBundleExecutable') != 'voiceport-app':
        raise ValueError('unexpected distribution bundle identity')
    requirement = ('anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists '
                   f'and certificate leaf[subject.OU] = "{team_id}"')
    for binary, identifier, expected in [
        (app, BUNDLE_ID, APP_ENTITLEMENTS),
        (app / 'Contents/MacOS/voiceport', BUNDLE_ID + '.cli', CLI_ENTITLEMENTS),
    ]:
        verify_requirement(binary, requirement)
        result = subprocess.run(['/usr/bin/codesign', '--display', '--verbose=4', str(binary)],
                                check=True, capture_output=True, text=True, timeout=30)
        require_metadata(result.stderr, team_id=team_id, identifier=identifier)
        entitlements = subprocess.run(['/usr/bin/codesign', '--display', '--entitlements', ':-', str(binary)],
                                     check=True, capture_output=True, timeout=30).stdout
        actual = plistlib.loads(entitlements) if entitlements.strip() else {}
        if actual != expected or any(type(value) is not bool for value in actual.values()):
            raise ValueError(f'unexpected distribution entitlements: {binary.name}')
    subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict', str(app)],
                   check=True, capture_output=True, text=True, timeout=30)
    return {'bundle_id': BUNDLE_ID, 'team_id': team_id, 'developer_id': True,
            'hardened_runtime': True, 'secure_timestamp': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', type=Path)
    parser.add_argument('--team-id', required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(validate(args.app, args.team_id)))
    except (OSError, ValueError, AssertionError, subprocess.SubprocessError) as error:
        print(f'Distribution signature check failed: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
