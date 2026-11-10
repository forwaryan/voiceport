import json
import os
import pathlib
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
from product_inventory import inventory, FILES, FOLDERS
from license_materials import check
from check_bundle import validate


def cargo_target():
    return pathlib.Path(json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--offline', '--locked', '--no-deps', '--format-version', '1'], cwd=ROOT
    ))['target_directory']) / 'release'


def resign(app):
    subprocess.run(['codesign', '--force', '--sign', '-', str(app)], check=True, capture_output=True)
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True, capture_output=True)


class PackagingTests(unittest.TestCase):
    def test_inventory_detects_cargo_docs_untracked_permissions_and_removal(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            for name in FILES:
                (root / name).write_text('initial')
            for name in FOLDERS:
                (root / name).mkdir()
            baseline = inventory(root)
            for name in ['Cargo.lock', 'README.md', 'docs/new-untracked.md', 'crates/new.rs', 'skills/new-untracked.md']:
                path = root / name
                original = path.read_bytes() if path.exists() else None
                path.write_text('changed')
                self.assertNotEqual(inventory(root), baseline)
                if original is None:
                    path.unlink()
                else:
                    path.write_bytes(original)
            path = root / 'Cargo.toml'
            path.chmod(0o755)
            self.assertNotEqual(inventory(root), baseline)
            path.chmod(0o644)
            path.unlink()
            with self.assertRaises(ValueError):
                inventory(root)

    def test_missing_or_tampered_license_material_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = pathlib.Path(temp) / 'materials'
            shutil.copytree(ROOT / 'resources/third-party', directory)
            manifest = check(directory)
            path = directory / manifest['packages'][0]['materials'][0]['path']
            original = path.read_bytes()
            path.write_bytes(b'tampered')
            with self.assertRaises(AssertionError):
                check(directory)
            path.write_bytes(original)
            path.unlink()
            with self.assertRaises(AssertionError):
                check(directory)

    def test_extra_unlisted_resource_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = pathlib.Path(temp) / 'materials'
            shutil.copytree(ROOT / 'resources/third-party', directory)
            (directory / 'unlisted').write_text('extra')
            with self.assertRaises(AssertionError):
                check(directory)

    def test_missing_main_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            with self.assertRaises(AssertionError):
                validate(pathlib.Path(temp), 'missing')

    def test_materials_include_ring_embedded_and_mpl_source(self):
        manifest = check()
        ring = next(package for package in manifest['packages'] if package['name'] == 'ring')
        files = [material['source_relative'] for material in ring['materials']]
        for name in ['LICENSE-other-bits', 'LICENSE-BoringSSL', 'third_party/fiat/LICENSE', 'src/polyfill/once_cell/LICENSE-MIT']:
            self.assertIn(name, files)
        mpl = [package for package in manifest['packages'] if package['declared_spdx'] == 'MPL-2.0']
        self.assertTrue(mpl)
        for package in mpl:
            self.assertTrue(any(material['source_relative'].endswith('.rs') for material in package['materials']))

    def test_supplemental_licenses_require_pinned_bytes_and_real_paths(self):
        import license_materials as materials
        expected = materials.generate(False)
        block = next(package for package in expected['packages'] if package['name'] == 'block2')
        self.assertEqual(block['selection'], 'MIT')
        self.assertTrue(any(material.get('origin', {}).get('kind') == 'upstream licensing declaration'
                            for material in block['materials']))
        with tempfile.TemporaryDirectory() as temp:
            scope = pathlib.Path(temp)
            directory = scope / 'license-sources'
            shutil.copytree(materials.LICENSE_SOURCES, directory)
            declaration = directory / 'objc2' / materials.OBJC2_PINNED['block2'][1] / 'LICENSE.md'
            original = declaration.read_bytes()
            with patch.object(materials, 'LICENSE_SOURCES', directory):
                self.assertEqual(materials.generate(False), expected)
                declaration.write_bytes(b'invented licence')
                with self.assertRaisesRegex(AssertionError, 'reviewed pinned text'):
                    materials.generate(False)
                declaration.write_bytes(original)
                held = scope / 'held-declaration'
                declaration.rename(held)
                declaration.symlink_to(held)
                with self.assertRaisesRegex(AssertionError, 'regular file'):
                    materials.generate(False)
                declaration.unlink()
                with self.assertRaises(FileNotFoundError):
                    materials.generate(False)
                held.rename(declaration)
                held_root = scope / 'held-sources'
                directory.rename(held_root)
                directory.symlink_to(held_root, target_is_directory=True)
                with self.assertRaisesRegex(AssertionError, 'linked/non-directory ancestor'):
                    materials.generate(False)

    def test_unknown_arguments_and_skip_build_never_compile(self):
        with tempfile.TemporaryDirectory() as temp:
            scope = pathlib.Path(temp).resolve()
            tools = scope / 'tools'
            tools.mkdir()
            log = scope / 'cargo-calls.jsonl'
            cargo = tools / 'cargo'
            cargo.write_text(
                '#!/usr/bin/env python3\nimport json,pathlib,sys\n'
                f'with pathlib.Path({str(log)!r}).open("a") as log: log.write(json.dumps(sys.argv[1:])+"\\n")\n'
                'if sys.argv[1:] == ["metadata", "--locked", "--no-deps", "--format-version", "1"]:\n'
                f' print(json.dumps({{"target_directory": {str(scope / "empty-target")!r}}}))\n'
                'else: sys.exit(97)\n'
            )
            cargo.chmod(0o755)
            env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'],
                       VOICEPORT_APP_DIRECTORY=str(scope / 'VoicePort.app'), CARGO_NET_OFFLINE='true')
            command = ['bash', str(ROOT / 'scripts/package_app.sh')]
            for args in [['--unknown'], ['--skip-build', 'extra']]:
                result = subprocess.run(command + args, cwd=scope, env=env, capture_output=True)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn(b'Usage:', result.stderr)
                self.assertFalse(log.exists(), 'argument rejection must precede all Cargo invocations')
            result = subprocess.run(command + ['--skip-build'], cwd=scope, env=env, capture_output=True)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn(b'Rust release product is required', result.stderr)
            self.assertEqual([json.loads(line) for line in log.read_text().splitlines()],
                             [['metadata', '--locked', '--no-deps', '--format-version', '1']])
            self.assertFalse((scope / 'VoicePort.app').exists())
            result = subprocess.run(['python3', '-I', str(ROOT / 'scripts/check_bundle.py'),
                                     'missing-app', 'missing-main', 'obsolete-third-path'], capture_output=True)
            self.assertEqual(result.returncode, 2)
            self.assertIn(b'unrecognized arguments', result.stderr)

    def test_package_refuses_existing_destination_before_build(self):
        with tempfile.TemporaryDirectory() as temp:
            scope = pathlib.Path(temp).resolve()
            app = scope / 'VoicePort.app'
            app.mkdir()
            sentinel = app / 'untouched'
            sentinel.write_text('could be running')
            tools = scope / 'tools'
            tools.mkdir()
            cargo_log = scope / 'unexpected-cargo-call'
            cargo = tools / 'cargo'
            cargo.write_text('#!/bin/sh\ntouch "' + str(cargo_log) + '"\nexit 97\n')
            cargo.chmod(0o755)
            for output in [app, scope / 'linked.app']:
                if output != app:
                    output.symlink_to(app, target_is_directory=True)
                result = subprocess.run(['bash', str(ROOT / 'scripts/package_app.sh')], cwd=scope,
                                        env=dict(os.environ, VOICEPORT_APP_DIRECTORY=str(output),
                                                 PATH=str(tools) + os.pathsep + os.environ['PATH']), capture_output=True)
                self.assertEqual(result.returncode, 1)
                self.assertIn(b'App destination already exists', result.stderr)
                self.assertFalse(cargo_log.exists(), 'existing output rejection must precede builds')
                self.assertEqual(sentinel.read_text(), 'could be running')
                self.assertEqual(list(app.iterdir()), [sentinel])


class RealBundleTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.release = cargo_target()
        cls.native = cls.release / 'voiceport-macos'
        cls.cli = cls.release / 'voiceport-rs'
        if not cls.native.is_file() or not cls.cli.is_file():
            raise AssertionError('build the locked Rust workspace release before running bundle tests')
        # A caller's app is read only. Standalone runs package existing products to
        # a fresh owned path; they never build implicitly or touch dist/VoicePort.app.
        configured = os.environ.get('VOICEPORT_APP_DIRECTORY')
        if configured:
            cls.app = pathlib.Path(configured)
            if not cls.app.is_dir():
                raise AssertionError('VOICEPORT_APP_DIRECTORY must name an already packaged app for bundle tests')
        else:
            cls.scope = tempfile.TemporaryDirectory(prefix='voiceport-packaging-tests-')
            cls.addClassCleanup(cls.scope.cleanup)
            cls.app = pathlib.Path(cls.scope.name).resolve() / 'VoicePort.app'
            subprocess.run(['bash', str(ROOT / 'scripts/package_app.sh'), '--skip-build'], cwd=cls.scope.name,
                           env=dict(os.environ, VOICEPORT_APP_DIRECTORY=str(cls.app), CARGO_NET_OFFLINE='true'),
                           check=True, capture_output=True)

    def test_bundle_contains_rust_main_without_helper(self):
        self.assertTrue(validate(self.app, self.native)['main_signed_copy_matches'])
        self.assertFalse((self.app / 'Contents/Helpers').exists())

    def test_cli_and_skill_are_bound_to_current_source_and_build(self):
        result = validate(self.app, self.native)
        self.assertTrue(result['cli_signed_copy_matches'])
        self.assertTrue(result['skill_source_matches'])
        for kind in ['cli', 'skill', 'extra-skill']:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp:
                app = pathlib.Path(temp) / 'VoicePort.app'
                shutil.copytree(self.app, app)
                if kind == 'cli':
                    shutil.copyfile(self.native, app / 'Contents/MacOS/voiceport')
                    subprocess.run(['codesign', '--force', '--sign', '-', str(app / 'Contents/MacOS/voiceport')], check=True, capture_output=True)
                    expected = 'CLI does not match'
                elif kind == 'skill':
                    (app / 'Contents/Resources/skills/voiceport/SKILL.md').write_text('tampered')
                    expected = 'Skill differs'
                else:
                    (app / 'Contents/Resources/skills/extra.md').write_text('unreviewed')
                    expected = 'unexpected Skill files'
                resign(app)
                with self.assertRaisesRegex(AssertionError, expected):
                    validate(app, self.native)

    def test_app_icon_and_menu_bar_only_policy_survive_packaging(self):
        info = plistlib.loads((self.app / 'Contents/Info.plist').read_bytes())
        self.assertEqual(info['CFBundleIconFile'], 'AppIcon')
        self.assertIs(info['LSUIElement'], True)
        icon = self.app / 'Contents/Resources/AppIcon.icns'
        data = icon.read_bytes()
        self.assertEqual(data[:4], b'icns')
        self.assertEqual(int.from_bytes(data[4:8], 'big'), len(data))
        # Re-sign the mutated copy so a signature failure cannot mask whether
        # the bundle checker actually binds the installed icon to the source.
        with tempfile.TemporaryDirectory() as temp:
            app = pathlib.Path(temp) / 'VoicePort.app'
            shutil.copytree(self.app, app)
            (app / 'Contents/Resources/AppIcon.icns').write_bytes(b'wrong icon')
            resign(app)
            with self.assertRaisesRegex(AssertionError, 'unexpected app icon'):
                validate(app, self.native)

    def test_real_bundle_rejects_main_mode_and_missing_material(self):
        with tempfile.TemporaryDirectory() as temp:
            app = pathlib.Path(temp) / 'VoicePort.app'
            shutil.copytree(self.app, app)
            main = app / 'Contents/MacOS/voiceport-app'
            main.chmod(0o644)
            with self.assertRaisesRegex(AssertionError, 'main must be executable'):
                validate(app, self.native)
            main.chmod(0o755)
            (app / 'Contents/Resources/third-party/manifest.json').unlink()
            resign(app)
            with self.assertRaises(FileNotFoundError):
                validate(app, self.native)

    def test_skip_build_honors_metadata_directory_from_external_cwd(self):
        with tempfile.TemporaryDirectory() as temp:
            scope = pathlib.Path(temp).resolve()
            directory = scope / 'alternate-target'
            (directory / 'release').mkdir(parents=True)
            copied = directory / 'release/voiceport-macos'
            shutil.copy2(self.native, copied)
            copied_cli = directory / 'release/voiceport'
            shutil.copy2(self.release / 'voiceport', copied_cli)
            app = scope / 'alternate-output/VoicePort.app'
            environment = dict(os.environ, CARGO_TARGET_DIR=str(directory), CARGO_NET_OFFLINE='true',
                               VOICEPORT_APP_DIRECTORY=str(app) + os.sep)
            result = subprocess.run(['bash', str(ROOT / 'scripts/package_app.sh'), '--skip-build'],
                                    cwd=scope, env=environment, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stdout.decode() + result.stderr.decode())
            self.assertTrue(validate(app, copied)['main_signed_copy_matches'])
            self.assertEqual(set((directory / 'release').iterdir()), {copied, copied_cli})

    def test_signed_wrong_rust_main_and_semantic_license_corruption(self):
        for kind in ['main', 'unknown-both', 'unknown-declaration', 'unknown-selection', 'omitted-package', 'tampered-material']:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp:
                app = pathlib.Path(temp) / 'VoicePort.app'
                shutil.copytree(self.app, app)
                path = app / 'Contents/Resources/third-party/manifest.json'
                manifest = json.loads(path.read_text())
                if kind == 'main':
                    shutil.copyfile(self.cli, app / 'Contents/MacOS/voiceport-app')
                elif kind == 'tampered-material':
                    (path.parent / manifest['packages'][0]['materials'][0]['path']).write_bytes(b'tampered')
                else:
                    if kind in ['unknown-both', 'unknown-declaration']:
                        manifest['packages'][0]['declared_spdx'] = 'UNKNOWN'
                    if kind in ['unknown-both', 'unknown-selection']:
                        manifest['packages'][0]['selection'] = 'UNKNOWN'
                    if kind == 'omitted-package':
                        for material in manifest['packages'].pop(0)['materials']:
                            (path.parent / material['path']).unlink()
                    path.write_text(json.dumps(manifest))
                resign(app)
                if kind == 'main':
                    with self.assertRaisesRegex(AssertionError, 'main does not match'):
                        validate(app, self.native)
                elif kind == 'tampered-material':
                    with self.assertRaises(AssertionError):
                        validate(app, self.native)
                else:
                    with self.assertRaisesRegex(AssertionError, 'installed license manifest'):
                        validate(app, self.native)

    def test_bundle_rejects_linked_roots_materials_and_extra_helpers(self):
        with tempfile.TemporaryDirectory() as temp:
            scope = pathlib.Path(temp)
            app = scope / 'VoicePort.app'
            shutil.copytree(self.app, app)
            linked = scope / 'Linked.app'
            linked.symlink_to(app, target_is_directory=True)
            with self.assertRaisesRegex(AssertionError, 'expected real bundle directory'):
                validate(linked, self.native)
            materials = app / 'Contents/Resources/third-party'
            held = scope / 'held-materials'
            materials.rename(held)
            materials.symlink_to(held, target_is_directory=True)
            with self.assertRaisesRegex(AssertionError, 'unexpected bundle file type'):
                validate(app, self.native)
            materials.unlink()
            held.rename(materials)
            (app / 'Contents/Helpers').mkdir()
            shutil.copy2(self.cli, app / 'Contents/Helpers/voiceport-rs')
            resign(app)
            with self.assertRaisesRegex(AssertionError, 'no helpers are shipped'):
                validate(app, self.native)


class RevisionBoundaries(unittest.TestCase):
    def make_tree(self, root):
        for name in FILES:
            (root / name).write_text('owned synthetic input')
        for name in FOLDERS:
            (root / name).mkdir()

    def test_root_links_types_missing_and_directory_modes(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            self.make_tree(root)
            baseline = inventory(root)
            (root / 'docs').chmod(0o700)
            self.assertNotEqual(baseline, inventory(root))
            (root / 'docs').chmod(0o755)
            (root / 'docs/empty').mkdir()
            self.assertNotEqual(baseline, inventory(root))
            (root / 'docs/empty').rmdir()
            held = root / 'held'
            (root / 'docs').rename(held)
            (root / 'docs').symlink_to(held, target_is_directory=True)
            with self.assertRaises(ValueError):
                inventory(root)
            (root / 'docs').unlink()
            held.rename(root / 'docs')
            (root / 'Cargo.toml').unlink()
            (root / 'Cargo.toml').symlink_to(root / 'Cargo.lock')
            with self.assertRaises(ValueError):
                inventory(root)
            (root / 'Cargo.toml').unlink()
            os.mkfifo(root / 'Cargo.toml')
            with self.assertRaises(ValueError):
                inventory(root)
            (root / 'Cargo.toml').unlink()
            (root / 'Cargo.toml').write_text('owned')
            shutil.rmtree(root / 'docs')
            with self.assertRaises(ValueError):
                inventory(root)

    def test_source_manifest_is_not_self_authoritative(self):
        import license_materials as materials
        mutations = [
            lambda manifest: manifest['packages'][0].update(declared_spdx='UNKNOWN'),
            lambda manifest: manifest['packages'][0].update(selection='UNKNOWN'),
            lambda manifest: manifest['packages'].pop(0),
            lambda manifest: manifest['packages'].append(manifest['packages'][0]),
            lambda manifest: manifest.update(build_inputs={}),
            lambda manifest: manifest['packages'][0].update(enabled_features=['invented']),
            lambda manifest: manifest['packages'][0].update(scope=['invented']),
        ]
        for mutation in mutations:
            with tempfile.TemporaryDirectory() as temp:
                directory = pathlib.Path(temp) / 'materials'
                shutil.copytree(ROOT / 'resources/third-party', directory)
                manifest = json.loads((directory / 'manifest.json').read_text())
                mutation(manifest)
                (directory / 'manifest.json').write_text(json.dumps(manifest))
                with patch.object(materials, 'DEST', directory):
                    with self.assertRaisesRegex(AssertionError, 'source license manifest'):
                        materials.check(directory)

    def test_actual_entry_rejects_initial_link_and_observes_runtime_drift(self):
        inputs = inventory(ROOT)
        for case in ['initial-link', 'runtime-drift']:
            with self.subTest(case=case), tempfile.TemporaryDirectory() as temp:
                scope = pathlib.Path(temp).resolve()
                tree = scope / 'product'
                tree.mkdir()
                for directory in inputs['directories']:
                    (tree / directory).mkdir(parents=True, exist_ok=True)
                    (tree / directory).chmod(inputs['directory_modes'][directory])
                for file in inputs['files']:
                    path = tree / file['path']
                    path.write_bytes((ROOT / file['path']).read_bytes())
                    path.chmod(file['mode'])
                env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', CARGO_NET_OFFLINE='true',
                           CODEX_HOME=str(scope / 'absent-codex'), TMPDIR=str(scope))
                if case == 'initial-link':
                    (tree / 'docs').rename(scope / 'held-docs')
                    (tree / 'docs').symlink_to(scope / 'held-docs', target_is_directory=True)
                else:
                    tools = scope / 'tools'
                    tools.mkdir()
                    cargo = tools / 'cargo'
                    cargo.write_text('#!/bin/sh\nif [ "$1" = test ]; then echo changed >> "' + str(tree / 'README.md') + '"; echo "0 failed"; exit 0; fi\nexit 7\n')
                    cargo.chmod(0o755)
                    env['PATH'] = str(tools) + os.pathsep + env['PATH']
                result = subprocess.run(['python3', '-I', str(tree / 'scripts/verify.py'), '--app-smoke'],
                                        cwd=scope, env=env, capture_output=True, timeout=30)
                report = json.loads(next((tree / '.voiceport/evidence/vp012-revision-01').glob('verify-*/report.json')).read_text())
                self.assertEqual(result.returncode, 1, result.stdout.decode() + result.stderr.decode())
                self.assertFalse(report['passed'])
                if case == 'initial-link':
                    self.assertEqual(report['checks'], [])
                else:
                    self.assertFalse(report['inputs_unchanged'])
                    self.assertEqual([row['exit_code'] for row in report['checks']], [0, 0, 0, 7])
                    self.assertEqual([row['argv'][0] for row in report['checks']],
                                     ['node', 'python3', 'cargo', 'cargo'])
                    self.assertEqual(report['checks'][0]['argv'][1], '--test')
                    self.assertIn('Tests/QQBridgeTests', report['checks'][1]['argv'])
                    self.assertEqual([row['argv'][1] for row in report['checks'][2:]],
                                     ['test', 'fmt'])


if __name__ == '__main__':
    unittest.main()
