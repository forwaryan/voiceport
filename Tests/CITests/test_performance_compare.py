import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[2] / 'scripts'
sys.path.insert(0, str(SCRIPTS))
from performance_compare import DETACHED, MANIFEST, ROOT, STAGES, percentiles, sha256


class PerformanceCompareTests(unittest.TestCase):
    def test_cli_comparison_and_negative_controls(self):
        head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
        clean_check = dict(status='completed', passed=True, exit_code=0, timed_out=False,
                           interrupted=False, cleanup_confirmed=True, left_descendants=False)
        identity = {'sha256': '1' * 64, 'bytes': 1}

        def make(root, offset, case='good'):
            root.mkdir(exist_ok=True)
            report = dict(schema_version=1, mode='fixture', status='completed', runs=3,
                          head=head, tracked_status='', host_cpu='Apple M4 Pro',
                          machine=dict(platform='macOS-26.2-arm64-arm-64bit', machine='arm64', logical_cpus=12),
                          sample_manifest=dict(sha256=sha256(MANIFEST.read_bytes()), bytes=MANIFEST.stat().st_size),
                          probe=identity, cli=identity, checks=[{'label': 'build', **clean_check}])
            for label, scenes in [('headless', {'synthetic_dictation': STAGES}),
                                  ('detached', {scene: ('refresh_to_enabled_ms',) for scene in DETACHED})]:
                rows = []
                for scene, keys in scenes.items():
                    values = [10 + offset, 20 + offset, 30 + offset]
                    report[scene] = {key: percentiles(values) for key in keys}
                    for index, value in enumerate(values, 1):
                        row = dict(scene=scene, iteration=index, **dict.fromkeys(keys, value))
                        if label == 'headless' and index == 3 and case == 'missing-stage':
                            del row['stop_to_final_ms']
                        rows.append(row)
                if label == 'headless' and case == 'failed':
                    rows.pop()
                    report['status'] = 'failed'
                idle_scene = 'idle_panel_uncreated' if label == 'headless' else 'idle_detached_hidden'
                idle = dict(scene=idle_scene, window_seconds=1.1, panel_created=label == 'detached')
                if label == 'headless' and case == 'idle-config':
                    idle.update(window_seconds=0.1, panel_created=True)
                if label == 'headless' and case == 'copied-observations':
                    idle['window_seconds'] = 1.2  # Different bytes, identical measured vectors.
                if label != 'headless' or case != 'missing-idle':
                    rows.append(idle)
                    report[idle_scene] = idle
                if label == 'detached' and case == 'detached-idle':
                    report[idle_scene] = {**idle, 'panel_created': False}
                raw = ''.join(json.dumps(row) + '\n' for row in rows).encode()
                check = dict(clean_check, label=label, argv=['probe', label, 'cli', '3', '1'],
                             stdout_sha256=sha256(raw), stdout_bytes=len(raw))
                if label == 'headless' and case == 'failed':
                    check.update(exit_code=101, passed=False)
                (root / f'{label}.stdout').write_bytes(raw[:-3] if case == 'truncated-log' else raw)
                report['checks'].append(check)
            if case == 'wrong-source':
                report['head'] = 'a' * 40
            if case == 'wrong-fixture':
                report['sample_manifest']['sha256'] = 'b' * 64
            if case == 'different-environment':
                report['machine']['logical_cpus'] = 8
            if case == 'missing-summary':
                del report['synthetic_dictation']['stop_to_final_ms']
            path = root / 'report.json'
            data = json.dumps(report)
            path.write_text(data[:-5] if case == 'truncated-report' else data, encoding='utf-8')
            return path

        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            base = make(directory / 'base', 0)

            def compare(candidate, *args):
                return subprocess.run([sys.executable, '-I', SCRIPTS / 'performance_compare.py',
                                       base, candidate, '--base-sha', head, '--candidate-sha', head,
                                       *args], capture_output=True, text=True, timeout=15)

            for case, expected, marker in (
                ('good', 0, '+5.000 / +5.000'),
                ('missing-stage', 1, '(2 / 1 / 0)'),
                ('missing-summary', 1, 'inconsistent stage evidence'),
                ('truncated-report', 1, 'report unreadable, oversized, or truncated'),
                ('truncated-log', 1, 'stdout missing, truncated, altered, or invalid'),
                ('wrong-source', 1, 'source mismatch'),
                ('wrong-fixture', 1, 'Sample manifest mismatch'),
                ('different-environment', 1, 'Environment mismatch'),
                ('failed', 1, 'headless: failed or incomplete check (exit 101)'),
                ('missing-idle', 1, 'idle_panel_uncreated: expected exactly one idle observation'),
                ('idle-config', 1, 'idle_panel_uncreated: invalid or shorter idle window than requested'),
                ('detached-idle', 1, 'idle_detached_hidden: report idle observation differs from stdout'),
                ('copied-observations', 1, 'Both sides use identical synthetic dictation observations'),
            ):
                with self.subTest(case=case):
                    offset = 0 if case == 'copied-observations' else 5
                    result = compare(make(directory / case, offset, case), '--environment-id', 'synthetic-test')
                    self.assertEqual(result.returncode, expected, result.stderr)
                    self.assertIn(marker, result.stdout)
                    self.assertIn('first audio frame | Not measured | Not measured', result.stdout)
                    self.assertNotIn(str(directory), result.stdout)
                    self.assertTrue(all('observation only' in line and '+5.000' not in line
                                        for line in result.stdout.splitlines() if 'Offscreen refresh' in line))
                    if case not in ('good', 'missing-stage', 'missing-summary'):
                        self.assertNotRegex(result.stdout, r'\| [+-]\d+\.\d+ / [+-]\d+\.\d+ \|')
                    if case == 'idle-config':
                        self.assertIn('idle_panel_uncreated: panel creation state does not match mode', result.stdout)
                    if case == 'failed':
                        self.assertIn('(2 / 0 / 1)', result.stdout)
            result = compare(directory / 'good' / 'report.json')
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn('conditions not attested', result.stdout)
            self.assertEqual(compare(base, '--environment-id', 'synthetic-test').returncode, 1)
