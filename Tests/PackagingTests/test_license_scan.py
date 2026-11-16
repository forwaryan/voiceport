"""Source-scan regressions: each call must observe current bytes and file types."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('license_scan_under_test', ROOT / 'scripts/license_materials.py')
materials = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(materials)


class SourceScanTests(unittest.TestCase):
    def setUp(self):
        self.scope = tempfile.TemporaryDirectory(prefix='voiceport-license-scan-')
        self.addCleanup(self.scope.cleanup)
        self.root = Path(self.scope.name)
        self.source = self.root / 'registry/sample'
        self.source.mkdir(parents=True)
        (self.root / 'Cargo.toml').write_text('[workspace]\n')
        (self.root / 'Cargo.lock').write_text('version = 4\npackage = []\n')
        (self.root / 'crates').mkdir()
        (self.source / 'Cargo.toml').write_text('package metadata')
        (self.source / 'LICENSE').write_bytes(b'reviewed license text\n')
        self.header = self.source / 'header.rs'
        self.header.write_bytes(b'// CoPyRiGhT fixture\npub fn alpha() {}\n')
        self.package = {
            'id': 'sample@1.0.0', 'name': 'sample', 'version': '1.0.0',
            'source': 'registry+https://example.invalid',
            'manifest_path': str(self.source / 'Cargo.toml'),
            'repository': 'https://example.invalid/sample', 'license': 'MIT',
            'targets': [{'kind': ['lib']}],
        }
        graph = {
            'packages': [self.package], 'workspace_members': ['sample@1.0.0'],
            'resolve': {'nodes': [{'id': 'sample@1.0.0', 'deps': [], 'features': []}]},
        }
        for mock in [patch.object(materials, 'ROOT', self.root),
                     patch.object(materials, 'DEST', self.root / 'materials'),
                     patch.object(materials, 'metadata', return_value=('aarch64-apple-darwin', graph))]:
            mock.start()
            self.addCleanup(mock.stop)

    def entries(self, write=False):
        result = materials.generate(write)
        return {row['source_relative']: row for row in result['packages'][0]['materials']}

    def test_changed_and_new_attribution_files_are_observed_on_every_call(self):
        before = self.entries()
        self.assertIn('LICENSE', before)
        self.assertIn('header.rs', before)
        self.assertNotIn('Cargo.toml', before)
        changed = b'// SPDX-License-Identifier: MIT\npub fn beta() {}\n'
        self.header.write_bytes(changed)
        (self.source / 'additional.txt').write_bytes(b'REDISTRIBUTION AND USE\xff')
        after = self.entries()
        self.assertEqual(after['header.rs']['sha256'], hashlib.sha256(changed).hexdigest())
        self.assertEqual(after['header.rs']['bytes'], len(changed))
        self.assertNotEqual(before['header.rs']['sha256'], after['header.rs']['sha256'])
        self.assertIn('additional.txt', after)

    def test_replacing_source_with_link_does_not_reuse_previous_material(self):
        self.assertIn('header.rs', self.entries())
        outside = self.root / 'outside.rs'
        self.header.rename(outside)
        self.header.symlink_to(outside)
        self.assertNotIn('header.rs', self.entries())

    def test_mpl_keeps_binary_files_and_written_bytes_match_manifest(self):
        self.package['license'] = 'MPL-2.0'
        binary = b'\x00\xffpayload\x00'
        (self.source / 'plain.bin').write_bytes(binary)
        row = self.entries(write=True)['plain.bin']
        installed = materials.DEST / row['path']
        self.assertEqual(installed.read_bytes(), binary)
        self.assertEqual(row['sha256'], hashlib.sha256(binary).hexdigest())
        self.assertEqual(row['bytes'], len(binary))
        manifest = json.loads((materials.DEST / 'manifest.json').read_text())
        self.assertEqual(manifest, materials.generate(False))


if __name__ == '__main__':
    unittest.main()
