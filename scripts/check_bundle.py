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
APP_NAME = 'VoicePort'
APP_ID = 'io.github.forwaryan.voiceport'
MAIN = 'voiceport-app'
CLI = 'voiceport'
TEST_NAME = 'VoicePort Test'
TEST_ID = APP_ID + '.test'
TEST_MAIN = 'voiceport-test-app'
TEST_CLI = 'voiceport-test'


def expected_info(test):
    source = (ROOT / 'resources/Info.plist').read_bytes()
    if not test:
        return source
    info = plistlib.loads(source)
    info.update(CFBundleName=TEST_NAME, CFBundleDisplayName=TEST_NAME,
                CFBundleIdentifier=TEST_ID, CFBundleExecutable=TEST_MAIN)
    return plistlib.dumps(info)


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


def validate_layout(app, *, test=False, stapled=False):
    real_tree(app)
    contents = app / 'Contents'
    assert {p.name for p in app.iterdir()} == {'Contents'}, 'unexpected bundle root'
    expected = {'Info.plist', 'MacOS', 'Resources', '_CodeSignature'}
    entries = {p.name for p in contents.iterdir()}
    assert entries == expected or (stapled and entries == expected | {'CodeResources'}
                                   and (contents / 'CodeResources').is_file()), 'unexpected bundle contents'
    main = contents / 'MacOS' / (TEST_MAIN if test else MAIN)
    cli = contents / 'MacOS' / (TEST_CLI if test else CLI)
    assert {p.name for p in main.parent.iterdir()} == {main.name, cli.name}, 'unexpected app executable'
    resources = contents / 'Resources'
    assert {p.name for p in resources.iterdir()} == {'AppIcon.icns', 'THIRD_PARTY_NOTICES.md', 'third-party', 'skills'}, 'unexpected app resources'
    assert all((resources / name).is_dir() for name in ('third-party', 'skills')), 'expected resource directories'
    assert all((resources / name).is_file() for name in ('AppIcon.icns', 'THIRD_PARTY_NOTICES.md')), 'expected resource files'
    for binary in (main, cli):
        assert binary.is_file() and stat.S_IMODE(binary.stat().st_mode) == 0o755, 'expected executable mode 0755'
        assert_arm64(binary)
    return main, cli


def assert_arm64(binary):
    assert subprocess.check_output(['/usr/bin/lipo', '-archs', str(binary)], text=True,
                                   timeout=30).strip() == 'arm64', 'unexpected architecture'


def validate(app, target, *, test=False):
    app, target = pathlib.Path(app), pathlib.Path(target)
    main, cli = validate_layout(app, test=test)
    contents = app / 'Contents'
    installed_skills = contents / 'Resources/skills'
    expected_skills = ROOT / 'skills'
    assert {str(p.relative_to(installed_skills)) for p in installed_skills.rglob('*')} == {str(p.relative_to(expected_skills)) for p in expected_skills.rglob('*')}, 'unexpected Skill files'
    for source in expected_skills.rglob('*'):
        if source.is_file():
            assert (installed_skills / source.relative_to(expected_skills)).read_bytes() == source.read_bytes(), 'Skill differs from repository source'
    cli_target = target.parent / CLI
    assert stat.S_ISREG(cli_target.lstat().st_mode), 'expected current CLI release product'
    assert (contents / 'Resources/AppIcon.icns').read_bytes() == (ROOT / 'resources/AppIcon.icns').read_bytes(), 'unexpected app icon'
    assert stat.S_ISREG(target.lstat().st_mode) and target.stat().st_mode & 0o111, 'expected executable Rust macOS product'
    assert (contents / 'Info.plist').read_bytes() == expected_info(test), 'unexpected bundle Info'
    for binary in (target, cli_target):
        assert_arm64(binary)
    assert (contents / 'Resources/THIRD_PARTY_NOTICES.md').read_bytes() == (ROOT / 'THIRD_PARTY_NOTICES.md').read_bytes(), 'unexpected third-party notices'
    materials = check(contents / 'Resources/third-party')
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
    main_relative, cli_relative = main.relative_to(app), cli.relative_to(app)
    with tempfile.TemporaryDirectory(prefix='voiceport-main-proof-') as temp:
        twin = pathlib.Path(temp) / app.name
        shutil.copytree(app, twin)
        shutil.copyfile(target, twin / main_relative)
        (twin / main_relative).chmod(0o755)
        shutil.copyfile(cli_target, twin / cli_relative)
        (twin / cli_relative).chmod(0o755)
        # Certificate signatures contain timestamps/identity data. Normalize both
        # copies to ad-hoc signatures before comparing their exact signed bytes;
        # the original bundle's actual signature was verified above.
        normalized = pathlib.Path(temp) / 'normalized' / app.name
        normalized.parent.mkdir()
        shutil.copytree(app, normalized)
        empty_entitlements = pathlib.Path(temp) / 'empty-entitlements.plist'
        empty_entitlements.write_bytes(plistlib.dumps({}))
        for candidate in [twin, normalized]:
            # Clear inherited signature metadata (including linker-signed versus
            # certificate/ad-hoc allocation) in these disposable copies first.
            subprocess.run(['codesign', '--remove-signature', str(candidate / cli_relative)],
                           check=True, capture_output=True)
            subprocess.run(['codesign', '--remove-signature', str(candidate)], check=True, capture_output=True)
            # Normalize distribution runtime flags, entitlements and CLI identity
            # as well as the certificate, so only executable correspondence remains.
            options = ['codesign', '--force', '--sign', '-', '--options', '0',
                       '--timestamp=none', '--entitlements', str(empty_entitlements)]
            subprocess.run(options + ['--identifier', CLI, str(candidate / cli_relative)],
                           check=True, capture_output=True)
            subprocess.run(options + ['--identifier', APP_ID, str(candidate)],
                           check=True, capture_output=True)
        assert sha(twin / cli_relative) == sha(normalized / cli_relative), 'CLI does not match current Rust build plus signature'
        assert sha(twin / main_relative) == sha(normalized / main_relative), 'main does not match this Rust voiceport-macos build plus bundle signature'
    if test:
        # Old --skip-build inputs may copy and sign correctly but lack isolation.
        # These version/help paths exit before AppKit, credentials, or IPC.
        help_text = subprocess.check_output([str(main), '--help'], text=True, timeout=15)
        profile = json.loads(subprocess.check_output([str(cli), '--json', '--version'], timeout=15))
        data = profile.get('data', {})
        version = plistlib.loads(expected_info(test=True))['CFBundleShortVersionString']
        assert (help_text.startswith(f'{TEST_NAME} Rust macOS\n') and data.get('build_profile') == 'test'
                and data.get('version') == version), 'test package requires current, test-aware Rust products'
    return {
        'main_sha256': sha(main),
        'cargo_product_sha256': sha(target),
        'cli_sha256': sha(cli),
        'cli_signed_copy_matches': True,
        'skill_source_matches': True,
        'main_signed_copy_matches': True,
        'signed_copy_matches': True,
        'license_packages': len(materials['packages']),
    }


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', type=pathlib.Path, nargs='?')
    parser.add_argument('target', type=pathlib.Path, nargs='?', help='current Cargo release/voiceport-macos product')
    parser.add_argument('--test', action='store_true', help='check a VoicePort Test bundle')
    parser.add_argument('--write-test-info', type=pathlib.Path)
    args = parser.parse_args()
    if args.write_test_info is not None:
        if args.app or args.target or args.test:
            parser.error('--write-test-info is a standalone operation')
        with args.write_test_info.open('xb') as output:
            output.write(expected_info(test=True))
    elif args.app is None or args.target is None:
        parser.error('app and target are required')
    else:
        print(json.dumps(validate(args.app, args.target, test=args.test)))
