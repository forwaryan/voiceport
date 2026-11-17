import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('ci_scope', ROOT / 'scripts/ci_scope.py')
ci_scope = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ci_scope)


class ScopeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='voiceport-ci-scope-')
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.git('init', '-q')
        self.git('config', 'user.email', 'ci-test@example.invalid')
        self.git('config', 'user.name', 'CI test')
        for name in ['README.md', 'docs/images/README.md', 'docs/TRANSCRIPTION_PLUGINS.md', 'crates/core.rs']:
            self.write(name)
        self.base = self.commit()
        self.event = {'pull_request': {'base': {'sha': self.base}}}

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.repo, stderr=subprocess.PIPE).decode().strip()

    def write(self, name, text='initial\n'):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def commit(self):
        self.git('add', '-A')
        self.git('commit', '-qm', 'fixture')
        return self.git('rev-parse', 'HEAD')

    def full(self, event_name='pull_request', event=None):
        return ci_scope.scope(event_name, self.event if event is None else event, self.repo)[0]

    def test_readme_and_documentation_images_take_fast_path(self):
        self.write('README.md', 'updated\n')
        self.write('docs/images/new image.webp')
        self.write('docs/images/a\nmultiline.md')
        self.commit()
        self.assertFalse(self.full())

    def test_source_configuration_tests_and_unknown_files_require_full_checks(self):
        for path in ['crates/core.rs', 'Cargo.lock', '.github/workflows/ci.yml',
                     '.claude/hooks/hook.py', 'Tests/test.py', 'scripts/build.sh',
                     'resources/icon.png', 'docs/tool.py', 'docs/guide.md',
                     'docs/images/tool.py', 'unknown']:
            with self.subTest(path=path):
                self.git('reset', '--hard', self.base)
                self.write(path, 'changed\n')
                self.commit()
                self.assertTrue(self.full())

    def test_mixed_docs_and_source_requires_full_checks(self):
        self.write('README.md', 'updated\n')
        self.write('crates/core.rs', 'changed\n')
        self.commit()
        self.assertTrue(self.full())

    def test_rename_from_source_to_docs_cannot_hide_source_deletion(self):
        self.git('mv', 'crates/core.rs', 'docs/renamed.md')
        self.commit()
        self.assertTrue(self.full())

    def test_source_deletion_requires_full_checks(self):
        (self.repo / 'crates/core.rs').unlink()
        self.commit()
        self.assertTrue(self.full())

    def test_gallery_docs_deletion_takes_fast_path(self):
        (self.repo / 'docs/images/README.md').unlink()
        self.commit()
        self.assertFalse(self.full())

    def test_embedded_document_change_requires_full_checks(self):
        self.write('docs/TRANSCRIPTION_PLUGINS.md', 'changed embedded CLI content\n')
        self.commit()
        self.assertTrue(self.full())

    def test_embedded_document_deletion_requires_full_checks(self):
        (self.repo / 'docs/TRANSCRIPTION_PLUGINS.md').unlink()
        self.commit()
        self.assertTrue(self.full())

    def test_shallow_checkout_fetches_exact_missing_base(self):
        self.write('README.md', 'PR update\n')
        self.commit()
        checkout = self.repo / 'shallow-checkout'
        self.git('clone', '-q', '--depth=1', self.repo.as_uri(), str(checkout))
        missing = subprocess.run(['git', 'cat-file', '-e', self.base + '^{commit}'],
                                 cwd=checkout, capture_output=True)
        self.assertNotEqual(missing.returncode, 0)
        full, base, _ = ci_scope.scope('pull_request', self.event, checkout)
        self.assertFalse(full)
        self.assertEqual(base, self.base)
        subprocess.run(['git', 'cat-file', '-e', self.base + '^{commit}'],
                       cwd=checkout, check=True, capture_output=True)

    def test_push_and_unknown_events_always_run_full_checks(self):
        self.write('README.md', 'changed\n')
        self.commit()
        for event_name in ['push', 'workflow_dispatch', '', 'pull_request_target']:
            self.assertTrue(self.full(event_name))

    def test_empty_diff_runs_full_checks(self):
        self.assertTrue(self.full())

    def test_missing_or_malformed_base_runs_full_checks(self):
        for event in [{}, None, [], {'pull_request': None},
                      {'pull_request': {'base': {'sha': '--help'}}},
                      {'pull_request': {'base': {'sha': 42}}}]:
            with self.subTest(event=event):
                self.assertTrue(ci_scope.scope('pull_request', event, self.repo)[0])

    def test_unavailable_base_runs_full_checks(self):
        # This local repository has no origin, so a missing base cannot be
        # fetched. A truncated checkout must not become a docs-only pass.
        event = {'pull_request': {'base': {'sha': 'f' * 40}}}
        self.assertTrue(self.full(event=event))

    def test_actual_merge_checkout_includes_base_changes(self):
        self.git('checkout', '-qb', 'pr')
        self.write('README.md', 'PR update\n')
        self.commit()
        self.git('checkout', '-qb', 'base', self.base)
        self.write('crates/core.rs', 'new base code\n')
        self.commit()
        self.git('merge', '--no-ff', '-qm', 'synthetic merge', 'pr')
        self.assertTrue(self.full(), 'classify the actual tested tree, not only PR head')

    def fast_path_entrypoint(self, text):
        self.write('README.md', text)
        self.commit()
        self.write('scripts/ci_scope.py', (ROOT / 'scripts/ci_scope.py').read_text())
        event_path = self.repo / 'event.json'
        event_path.write_text(json.dumps(self.event))
        result = subprocess.run(
            [sys.executable, '-I', str(self.repo / 'scripts/ci_scope.py')],
            env=dict(os.environ, GITHUB_EVENT_NAME='pull_request', GITHUB_EVENT_PATH=str(event_path)),
            capture_output=True, text=True, check=True,
        )
        outputs = dict(line.split('=', 1) for line in result.stdout.splitlines())
        self.assertEqual(outputs['full'], 'false')
        self.assertEqual(outputs['base'], self.base)
        return subprocess.run(['git', 'diff', '--check', outputs['base'], 'HEAD', '--'],
                              cwd=self.repo, capture_output=True)

    def test_fast_path_entrypoint_and_documentation_check(self):
        self.assertEqual(self.fast_path_entrypoint('updated README\n').returncode, 0)

    def test_documentation_whitespace_error_fails_fast_path_check(self):
        self.assertNotEqual(self.fast_path_entrypoint('trailing whitespace \n').returncode, 0)

    def test_missing_event_file_emits_full_scope(self):
        result = subprocess.run(
            [sys.executable, '-I', str(ROOT / 'scripts/ci_scope.py')],
            env=dict(os.environ, GITHUB_EVENT_NAME='pull_request',
                     GITHUB_EVENT_PATH=str(self.repo / 'missing-event.json')),
            capture_output=True, text=True, check=True,
        )
        self.assertIn('full=true\n', result.stdout)

    def test_malformed_event_file_emits_full_scope(self):
        event_path = self.repo / 'event.json'
        event_path.write_text('{broken')
        result = subprocess.run(
            [sys.executable, '-I', str(ROOT / 'scripts/ci_scope.py')],
            env=dict(os.environ, GITHUB_EVENT_NAME='pull_request', GITHUB_EVENT_PATH=str(event_path)),
            capture_output=True, text=True, check=True,
        )
        self.assertIn('full=true\n', result.stdout)


if __name__ == '__main__':
    unittest.main()
