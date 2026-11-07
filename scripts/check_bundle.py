#!/usr/bin/env python3
"""Check Rust app files and signature correspondence without launching the app."""
import argparse
import hashlib
import json
import pathlib
import plistlib
import shutil
import stat
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from license_materials import check

ROOT = pathlib.Path(__file__).resolve().parents[1]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def real_tree(path):
    """Reject linked roots, linked descendants and special files before reading."""
    mode = path.lstat().st_mode
    assert stat.S_ISDIR(mode), 'expected real bundle directory: ' + str(path)
    for child in path.iterdir():
        mode = child.lstat().st_mode
        assert stat.S_ISREG(mode) or stat.S_ISDIR(mode), 'unexpected bundle file type: ' + str(child)
        if stat.S_ISDIR(mode):
            real_tree(child)


def validate(app, target):
    app, target = pathlib.Path(app), pathlib.Path(target)
    real_tree(app)
    contents = app / 'Contents'
    assert {p.name for p in app.iterdir()} == {'Contents'}, 'unexpected bundle root'
    assert {p.name for p in contents.iterdir()} == {'Info.plist', 'MacOS', 'Resources', '_CodeSignature'}, 'unexpected bundle contents (no helpers are shipped)'
    assert {p.name for p in (contents / 'MacOS').iterdir()} == {'VoicePort'}, 'unexpected app executable'
    assert {p.name for p in (contents / 'Resources').iterdir()} == {'THIRD_PARTY_NOTICES.md', 'third-party'}, 'unexpected app resources'
    main = contents / 'MacOS/VoicePort'
    assert main.is_file() and stat.S_IMODE(main.stat().st_mode) == 0o755, 'main must be executable mode 0755'
    assert stat.S_ISREG(target.lstat().st_mode) and target.stat().st_mode & 0o111, 'expected executable Rust macOS product'
    assert (contents / 'Info.plist').read_bytes() == (ROOT / 'resources/Info.plist').read_bytes(), 'unexpected bundle Info'
    assert plistlib.loads((contents / 'Info.plist').read_bytes())['CFBundleExecutable'] == 'VoicePort'
    for binary in [main, target]:
        assert subprocess.check_output(['lipo', '-archs', str(binary)], text=True).strip() == 'arm64', 'unexpected architecture'
    assert (contents / 'Resources/THIRD_PARTY_NOTICES.md').read_bytes() == (ROOT / 'THIRD_PARTY_NOTICES.md').read_bytes(), 'unexpected third-party notices'
    materials = check(contents / 'Resources/third-party')
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
    with tempfile.TemporaryDirectory(prefix='voiceport-main-proof-') as temp:
        twin = pathlib.Path(temp) / app.name
        shutil.copytree(app, twin)
        shutil.copyfile(target, twin / 'Contents/MacOS/VoicePort')
        (twin / 'Contents/MacOS/VoicePort').chmod(0o755)
        subprocess.run(['codesign', '--force', '--sign', '-', str(twin)], check=True, capture_output=True)
        assert sha(twin / 'Contents/MacOS/VoicePort') == sha(main), 'main does not match this Rust voiceport-macos build plus bundle signature'
    return {
        'main_sha256': sha(main),
        'cargo_product_sha256': sha(target),
        'main_signed_copy_matches': True,
        'signed_copy_matches': True,
        'license_packages': len(materials['packages']),
    }


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', type=pathlib.Path)
    parser.add_argument('target', type=pathlib.Path, help='current Cargo release/voiceport-macos product')
    args = parser.parse_args()
    print(json.dumps(validate(args.app, args.target)))
