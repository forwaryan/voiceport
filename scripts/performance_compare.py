#!/usr/bin/env python3
"""Render two isolated VoicePort fixture reports as local PR Markdown."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from performance_baseline import MANIFEST, ROOT, percentiles

SHA = re.compile(r'[0-9a-f]{40}')
DIGEST = re.compile(r'[0-9a-f]{64}')
LABEL = re.compile(r'[A-Za-z0-9][A-Za-z0-9 ._+(),:-]{0,127}')
LIMIT = 1024 * 1024  # Same parsed-stdout bound as the fixture runner.
PROBE_SOURCES = (
    'crates/voiceport-macos/examples/performance_baseline.rs',
    'crates/voiceport-macos/src/audio_device_fixture.rs',
    'crates/voiceport-macos/src/app/control/fixture/audio_start.rs',
    'crates/voiceport-macos/src/app/control/fixture/audio_start/performance.rs',
    'crates/voiceport-macos/src/controller/recent_dictations/fixture.rs',
)
STAGES = {
    'blocked_cli_status_ms': 'Blocked device open: CLI `status` launch → reply',
    'start_to_accepted_ms': 'CLI Start → accepted reply',
    'start_to_recording_ms': 'CLI Start → first observed Controller `Recording`',
    'stop_to_audio_validated_ms': 'CLI Stop → packaged synthetic AAC validated',
    'stop_to_final_ms': 'CLI Stop → Controller `Ready` with synthetic final text',
    'stop_to_cli_result_ms': 'CLI Stop → CLI `dictation result` reply (`delivery=none`)',
    'synthetic_completion_to_confirmed_ms': 'Separate GUI fixture: inject final text → confirmed synthetic target',
}
DETACHED = {
    'detached_panel_preparation': 'Offscreen refresh → enabled (warm)',
    'detached_panel_preparation_blocked': 'Offscreen refresh → enabled (device and update blocked)',
}


def token(value, pattern):
    return value if isinstance(value, str) and pattern.fullmatch(value) else None


def object_or_empty(value):
    return value if isinstance(value, dict) else {}


def bounded_bytes(path):
    with path.open('rb') as source:
        data = source.read(LIMIT + 1)
    if len(data) > LIMIT:
        raise ValueError('input exceeds 1 MiB')
    return data


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def git_file(head, path):
    return subprocess.check_output(['git', 'show', f'{head}:{path}'], cwd=ROOT,
                                   stderr=subprocess.DEVNULL, timeout=10)


def fixture_identity(head):
    kind = subprocess.check_output(['git', 'cat-file', '-t', head], cwd=ROOT,
                                   stderr=subprocess.DEVNULL, timeout=10)
    if kind != b'commit\n':
        raise ValueError('pinned object is not a commit')
    # Hash the exact probe and generated input definitions, not product binaries.
    return sha256(''.join(sha256(git_file(head, path)) for path in PROBE_SOURCES).encode())


def sealed_rows(path, check, scenes, runs):
    data = bounded_bytes(path)
    if (type(check.get('stdout_bytes')) is not int or len(data) != check['stdout_bytes']
            or sha256(data) != check.get('stdout_sha256') or (data and not data.endswith(b'\n'))):
        raise ValueError('stdout digest, size, or line ending mismatch')
    rows = {scene: [] for scene in scenes}
    for line in data.splitlines():
        row = json.loads(line)
        if not isinstance(row, dict):
            raise ValueError('invalid observation')
        scene = row.get('scene')
        if isinstance(scene, str) and scene in rows:
            index = row.get('iteration')
            if type(index) is not int or index != len(rows[scene]) + 1 or index > runs:
                raise ValueError('missing, repeated, or unordered iteration')
            rows[scene].append(row)
    return rows


def check_passed(check):
    return (check.get('status') == 'completed' and check.get('passed') is True
            and type(check.get('exit_code')) is int and check['exit_code'] == 0
            and check.get('timed_out') is False and check.get('interrupted') is False
            and check.get('cleanup_confirmed') is True and check.get('left_descendants') is False
            and 'error' not in check)


def milliseconds(value):
    return (type(value) in (float, int) and 0 <= value <= sys.float_info.max
            and math.isfinite(value))


class Evidence:
    def __init__(self, path, expected):
        self.expected = expected
        self.issues = []
        self.rows = {}
        self.report = {}
        self.report_digest = self.fixture = self.idle_seconds = None
        try:
            data = bounded_bytes(path)
            self.report_digest = sha256(data)
            self.report = object_or_empty(json.loads(data))
        except (OSError, ValueError, RecursionError):
            self.issues.append('report unreadable, oversized, or truncated')
        report = self.report
        self.head = token(report.get('head'), SHA)
        self.manifest = self.digest('sample_manifest')
        machine = object_or_empty(report.get('machine'))
        cpus = machine.get('logical_cpus')
        self.environment = (token(machine.get('platform'), LABEL), token(machine.get('machine'), LABEL),
                            token(report.get('host_cpu'), LABEL), cpus if type(cpus) is int and cpus > 0 else None)
        runs = report.get('runs')
        self.runs = runs if type(runs) is int and 1 <= runs <= 30 else None
        checks = report.get('checks')
        checks = checks if isinstance(checks, list) else []
        self.detached = (any(object_or_empty(check).get('label') == 'detached' for check in checks)
                         or any(scene in report for scene in DETACHED))
        if not report:
            if not self.issues:
                self.issues.append('expected a fixture report object')
            return
        if type(report.get('schema_version')) is not int or report['schema_version'] != 1 or report.get('mode') != 'fixture':
            self.issues.append('unsupported source: expected fixture report schema 1')
            return
        if report.get('status') != 'completed' or 'active_check' in report or 'error' in report:
            self.issues.append('fixture report failed or did not finish')
        if self.head != expected or report.get('tracked_status') != '':
            self.issues.append('source mismatch: expected clean checkout at pinned revision')
        if None in self.environment:
            self.issues.append('environment identity incomplete')
        if self.runs is None:
            self.issues.append('missing or invalid requested sample count')
        if not self.digest('cli') or not self.digest('probe'):
            self.issues.append('missing built CLI or probe identity')
        if self.head == expected and report.get('tracked_status') == '':
            try:
                self.fixture = fixture_identity(expected)
                manifest = git_file(expected, MANIFEST.relative_to(ROOT))
                if self.manifest != sha256(manifest) or report['sample_manifest']['bytes'] != len(manifest):
                    self.issues.append('sample manifest does not match pinned revision')
            except (OSError, ValueError, subprocess.SubprocessError):
                self.issues.append('fixture sources not available at pinned revision')
        for label in ('build', 'headless', *(['detached'] if self.detached else [])):
            matches = [c for c in map(object_or_empty, checks) if c.get('label') == label]
            if len(matches) != 1:
                self.issues.append(f'{label}: missing or repeated check')
                continue
            check, = matches
            if not check_passed(check):
                code = check.get('exit_code')
                self.issues.append(f'{label}: failed or incomplete check (exit {code if type(code) is int else "unknown"})')
            if label == 'build' or self.runs is None:
                continue
            argv = check.get('argv')
            if (not isinstance(argv, list) or len(argv) != 5 or argv[1] != label
                    or argv[3] != str(self.runs) or argv[4] not in tuple(map(str, range(1, 61)))):
                self.issues.append(f'{label}: fixture invocation does not match report')
                continue
            if label == 'headless':
                self.idle_seconds = int(argv[4])
            elif self.idle_seconds != int(argv[4]):
                self.issues.append('detached: idle window does not match headless')
            try:
                scenes = ('synthetic_dictation',) if label == 'headless' else DETACHED
                self.rows.update(sealed_rows(path.with_name(f'{label}.stdout'), check, scenes, self.runs))
            except (OSError, ValueError, RecursionError):
                self.issues.append(f'{label}: stdout missing, truncated, altered, or invalid')

    def digest(self, key):
        identity = object_or_empty(self.report.get(key))
        if type(identity.get('bytes')) is int and identity['bytes'] > 0:
            return token(identity.get('sha256'), DIGEST)
        return None

    def stage(self, scene, key):
        rows = self.rows.get(scene)
        if rows is None:
            return '— (counts unknown)', None
        values = [row[key] for row in rows if milliseconds(row.get(key))]
        stats = percentiles(values) if values else {'n': 0}
        display = f"{stats['p50']:.3f} / {stats['p95']:.3f}" if values else '—'
        display += f' ({len(values)} / {len(rows) - len(values)} / {self.runs - len(rows)})'
        recorded = object_or_empty(object_or_empty(self.report.get(scene)).get(key))
        complete = (len(values) == self.runs and type(recorded.get('n')) is int
                    and all(milliseconds(recorded.get(p)) for p in ('p50', 'p95')) and recorded == stats)
        return display, stats if complete else None


def render(base, candidate, environment_id):
    sides = (base, candidate)
    gates = [f'{name}: {issue}' for name, side in zip(('Base', 'Candidate'), sides) for issue in side.issues]
    if not environment_id:
        gates.append('Same host/build/runtime conditions not attested (`--environment-id`).')
    if base.report_digest and base.report_digest == candidate.report_digest:
        gates.append('Both sides use the same report, not independent runs.')
    for field, label in (('environment', 'Environment'), ('fixture', 'Fixture source'),
                         ('manifest', 'Sample manifest'), ('runs', 'Requested runs'), ('idle_seconds', 'Idle window')):
        if getattr(base, field) != getattr(candidate, field):
            gates.append(f'{label} mismatch.')
    lines = ['### Dictation performance comparison', '',
             '| Evidence | Base | Candidate |', '| --- | --- | --- |']
    for label, values in (
        ('Expected commit', [s.expected for s in sides]), ('Reported commit', [s.head for s in sides]),
        ('Report SHA-256', [s.report_digest for s in sides]), ('Fixture source SHA-256', [s.fixture for s in sides]),
        ('Sample manifest SHA-256', [s.manifest for s in sides]),
        ('Probe SHA-256', [s.digest('probe') for s in sides]), ('CLI SHA-256', [s.digest('cli') for s in sides]),
        ('OS / architecture / CPU / logical CPUs', [' / '.join(str(v or '—') for v in s.environment) for s in sides]),
        ('Requested runs', [s.runs for s in sides]), ('Requested idle seconds', [s.idle_seconds for s in sides]),
    ):
        lines.append(f'| {label} | {values[0] or "—"} | {values[1] or "—"} |')
    lines += ['', f'Environment attestation: `{environment_id}`.' if environment_id else 'Environment attestation: missing.',
              'Nearest-rank p50 / p95 in ms; counts are **valid / invalid / unobserved**. Missing counts are unknown.',
              'Delta is candidate − base in ms. This is not a performance budget or a pass/fail judgment.', '',
              '| Timing boundary | Base p50 / p95 (counts) | Candidate p50 / p95 (counts) | Δ p50 / p95 |',
              '| --- | ---: | ---: | ---: |']
    complete = not gates
    lines.append('| CLI Start → first audio frame | Not measured | Not measured | — |')
    for key, label in STAGES.items():
        b, c = (side.stage('synthetic_dictation', key) for side in sides)
        if b[1] is None or c[1] is None:
            complete = False
            delta = '— (missing, incomplete, or inconsistent stage evidence)'
        elif gates:
            delta = '— (pair not comparable)'
        else:
            delta = ' / '.join(f'{c[1][p] - b[1][p]:+.3f}' for p in ('p50', 'p95'))
        lines.append(f'| {label} | {b[0]} | {c[0]} | {delta} |')
    if base.detached or candidate.detached:
        for scene, label in DETACHED.items():
            b, c = (side.stage(scene, 'refresh_to_enabled_ms') for side in sides)
            note = 'observation only' if b[1] and c[1] else 'unpaired or incomplete; observation only'
            lines.append(f'| {label} | {b[0]} | {c[0]} | — ({note}) |')
            complete = complete and b[1] is not None and c[1] is not None
    if gates:
        lines += ['', *[f'- {gate}' for gate in gates]]
    scope = 'Comparable synthetic stages' if complete else 'Incomplete or incomparable evidence'
    lines += ['', f'**Pair status: {scope}.**',
              'Synthetic-device/ASR fixture only; no true microphone, model, real input target, or status-bar click measurement.']
    return '\n'.join(lines), complete


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for side in ('base', 'candidate'):
        parser.add_argument(side, type=Path, help=f'{side} fixture report.json (keep its sibling logs)')
        parser.add_argument(f'--{side}-sha', required=True, help=f'full expected {side} commit SHA')
    parser.add_argument('--environment-id', help='public-safe ID attesting both runs used the same environment')
    args = parser.parse_args()
    if not SHA.fullmatch(args.base_sha) or not SHA.fullmatch(args.candidate_sha):
        parser.error('expected full lowercase 40-character Git commit SHAs')
    if args.environment_id is not None and not token(args.environment_id, LABEL):
        parser.error('use a public-safe environment ID (letters, digits, or simple punctuation)')
    markdown, complete = render(Evidence(args.base, args.base_sha), Evidence(args.candidate, args.candidate_sha),
                                args.environment_id)
    print(markdown)
    return 0 if complete else 1


if __name__ == '__main__':
    sys.exit(main())
