import contextlib
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import signing_identity

FIRST = '1234567890ABCDEF1234567890ABCDEF12345678'
SECOND = 'ABCDEF1234567890ABCDEF1234567890ABCDEF12'
SECURITY_OUTPUT = (f'  1) {FIRST} "Developer ID Application: Example (TEAM)"\n'
                   f'  2) {SECOND} "Apple Development: Example (TEAM)"\n'
                   '     2 valid identities found\n')


class SigningIdentityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name).resolve()
        self.config = self.root / '.config/voiceport/signing.json'
        # Every identity query is mocked, including unexpected queries.
        self.security = patch.object(signing_identity.subprocess, 'run',
                                     return_value=subprocess.CompletedProcess([], 0, SECURITY_OUTPUT, ''))
        self.run = self.security.start()
        self.addCleanup(self.security.stop)

    def save(self, identity=FIRST, **extra):
        self.config.parent.mkdir(parents=True, exist_ok=True)
        self.config.write_text(json.dumps({'schema_version': 1, 'identity': identity, **extra}), encoding='utf-8')

    def test_no_config_keeps_ad_hoc_with_warning_without_querying_security(self):
        for explicit in (None, '-'):
            with self.subTest(explicit=explicit), contextlib.redirect_stderr(io.StringIO()) as error:
                self.assertEqual(signing_identity.resolve_identity(explicit, self.config), '-')
                self.assertIn('permissions may require approval', error.getvalue())
        self.run.assert_not_called()

    def test_saved_fingerprint_is_normalized_and_checked(self):
        self.save(FIRST.lower())
        self.assertEqual(signing_identity.resolve_identity(None, self.config), FIRST)
        self.run.assert_called_once_with(['/usr/bin/security', 'find-identity', '-v', '-p', 'codesigning'],
                                         check=True, capture_output=True, text=True, timeout=15)

    def test_explicit_fingerprint_takes_priority(self):
        self.save(FIRST)
        self.assertEqual(signing_identity.resolve_identity(SECOND.lower(), self.config), SECOND)

    def test_explicit_identity_can_replace_unavailable_saved_identity(self):
        self.save('0' * 40)
        self.assertEqual(signing_identity.resolve_identity(SECOND, self.config), SECOND)

    def test_explicit_identity_works_without_saved_config(self):
        self.assertEqual(signing_identity.resolve_identity(FIRST, self.config), FIRST)

    def test_explicit_exact_name_is_resolved_to_fingerprint(self):
        self.assertEqual(signing_identity.resolve_identity('Developer ID Application: Example (TEAM)', self.config), FIRST)
        with self.assertRaisesRegex(ValueError, 'unavailable or ambiguous'):
            signing_identity.resolve_identity('Example', self.config)

    def test_ambiguous_name_fails(self):
        self.run.return_value.stdout = f'  1) {FIRST} "Same name"\n  2) {SECOND} "Same name"\n'
        with self.assertRaisesRegex(ValueError, 'unavailable or ambiguous'):
            signing_identity.resolve_identity('Same name', self.config)

    def test_explicit_ad_hoc_cannot_override_persistent_identity(self):
        self.save()
        with self.assertRaisesRegex(ValueError, 'forbidden while a persistent'):
            signing_identity.resolve_identity('-', self.config)
        self.run.assert_not_called()

    def test_empty_explicit_identity_fails_without_falling_back(self):
        self.save()
        for explicit in ('', '   '):
            with self.subTest(explicit=explicit), self.assertRaisesRegex(ValueError, 'must not be empty'):
                signing_identity.resolve_identity(explicit, self.config)
        self.run.assert_not_called()

    def test_malformed_config_fails_even_with_an_override(self):
        self.config.parent.mkdir(parents=True)
        invalid = ('', '{', 'null', '[]', '{}',
                   json.dumps({'schema_version': True, 'identity': FIRST}),
                   json.dumps({'schema_version': 2, 'identity': FIRST}),
                   json.dumps({'schema_version': 1.0, 'identity': FIRST}),
                   json.dumps({'schema_version': 1, 'identity': None}),
                   json.dumps({'schema_version': 1, 'identity': '-'}),
                   json.dumps({'schema_version': 1, 'identity': FIRST[:-1]}),
                   json.dumps({'schema_version': 1, 'identity': FIRST + '0'}),
                   json.dumps({'schema_version': 1, 'identity': 'G' * 40}),
                   json.dumps({'schema_version': 1, 'identity': FIRST + '\n'}),
                   json.dumps({'schema_version': 1, 'identity': FIRST, 'command': 'false'}),
                   '{"schema_version":1,"identity":"' + FIRST + '","identity":"' + SECOND + '"}')
        for content in invalid:
            self.config.write_text(content, encoding='utf-8')
            for explicit in (None, SECOND, '-'):
                with self.subTest(content=content, explicit=explicit), self.assertRaisesRegex(ValueError, 'signing config'):
                    signing_identity.resolve_identity(explicit, self.config)
        self.run.assert_not_called()

    def test_non_utf8_and_oversized_config_fail(self):
        self.config.parent.mkdir(parents=True)
        for content in (b'\xff', b' ' * 16385):
            self.config.write_bytes(content)
            with self.subTest(content_length=len(content)), self.assertRaisesRegex(ValueError, 'signing config'):
                signing_identity.resolve_identity(None, self.config)
        self.run.assert_not_called()

    def test_symlink_and_dangling_symlink_config_fail(self):
        self.config.parent.mkdir(parents=True)
        target = self.root / 'real.json'
        target.write_text(json.dumps({'schema_version': 1, 'identity': FIRST}), encoding='utf-8')
        self.config.symlink_to(target)
        for exists in (True, False):
            if not exists:
                target.unlink()
            with self.subTest(target_exists=exists), self.assertRaisesRegex(ValueError, 'symlink'):
                signing_identity.resolve_identity(None, self.config)
        self.run.assert_not_called()

    def test_symlink_parent_fails_even_when_config_is_absent(self):
        target = self.root / 'real'
        target.mkdir()
        (self.root / '.config').symlink_to(target)
        with self.assertRaisesRegex(ValueError, 'symlink'):
            signing_identity.resolve_identity(None, self.config)
        self.run.assert_not_called()

    def test_unreadable_config_fails(self):
        self.save()
        with patch.object(signing_identity.os, 'open', side_effect=PermissionError('fixture read denied')):
            with self.assertRaisesRegex(ValueError, 'cannot use signing config.*read denied'):
                signing_identity.resolve_identity(None, self.config)
        self.run.assert_not_called()

    def test_non_regular_config_fails(self):
        self.config.mkdir(parents=True)
        with self.assertRaisesRegex(ValueError, 'regular file'):
            signing_identity.resolve_identity(None, self.config)
        self.run.assert_not_called()

    def test_missing_identity_never_falls_back_to_ad_hoc_or_saved_identity(self):
        self.save()
        for explicit in (None, SECOND):
            self.run.return_value.stdout = '     0 valid identities found\n'
            with self.subTest(explicit=explicit), self.assertRaisesRegex(ValueError, 'refusing ad-hoc fallback'):
                signing_identity.resolve_identity(explicit, self.config)
        self.run.return_value.stdout = SECURITY_OUTPUT
        with self.assertRaisesRegex(ValueError, 'refusing ad-hoc fallback'):
            signing_identity.resolve_identity('0' * 40, self.config)

    def test_security_parser_requires_exact_valid_identity_line(self):
        self.save()
        for output in (f'1) {FIRST}0 "Name"\n', f'1) 0{FIRST} "Name"\n',
                       f'1) {FIRST} "Name" (CSSMERR_TP_CERT_EXPIRED)\n',
                       f'noise {FIRST}\n', f'1) {FIRST} "Name" trailing\n'):
            self.run.return_value.stdout = output
            with self.subTest(output=output), self.assertRaisesRegex(ValueError, 'refusing ad-hoc fallback'):
                signing_identity.resolve_identity(None, self.config)

    def test_security_failure_never_falls_back(self):
        self.save()
        for error in (OSError('fixture missing command'), subprocess.CalledProcessError(1, 'security'),
                      subprocess.TimeoutExpired('security', 15)):
            self.run.side_effect = error
            with self.subTest(error=error), self.assertRaisesRegex(ValueError, 'could not list valid signing identities'):
                signing_identity.resolve_identity(None, self.config)

    def test_main_uses_fixed_home_config_and_emits_only_resolved_identity(self):
        with patch.object(signing_identity.pathlib.Path, 'home', return_value=self.root), \
             patch.dict(signing_identity.os.environ, {'VOICEPORT_CODESIGN_IDENTITY': SECOND}, clear=True), \
             patch.object(signing_identity.sys, 'argv', ['signing_identity.py']), \
             contextlib.redirect_stdout(io.StringIO()) as output:
            self.save()
            self.assertEqual(signing_identity.main(), 0)
        self.assertEqual(output.getvalue(), SECOND + '\n')

    def test_main_failure_has_no_stdout_identity(self):
        self.save()
        self.run.return_value.stdout = ''
        with patch.object(signing_identity.pathlib.Path, 'home', return_value=self.root), \
             patch.dict(signing_identity.os.environ, {}, clear=True), \
             patch.object(signing_identity.sys, 'argv', ['signing_identity.py']), \
             contextlib.redirect_stdout(io.StringIO()) as output, \
             contextlib.redirect_stderr(io.StringIO()) as error:
            self.assertEqual(signing_identity.main(), 1)
        self.assertEqual(output.getvalue(), '')
        self.assertIn('Signing identity error:', error.getvalue())


if __name__ == '__main__':
    unittest.main()
