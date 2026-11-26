#!/usr/bin/env python3
"""Run the owned macOS performance fixture and score fixed synthetic speech."""
import argparse
import hashlib
import json
import math
import os
import platform
from pathlib import Path
import subprocess
import sys
import unicodedata
import wave

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / 'fixtures/performance/samples.json'
BASELINE = '0b6bd0cbaed04d14e66a0c3d8567c2185298f260'
sys.path.insert(0, str(ROOT / 'scripts'))
from verification_run import full_suite_lock, run_check, termination_signals, write_report


class MeasurementError(ValueError):
    pass


def file_identity(path):
    with path.open('rb') as source:
        return {'sha256': hashlib.file_digest(source, 'sha256').hexdigest(),
                'bytes': os.fstat(source.fileno()).st_size}


def normalized(text):
    text = unicodedata.normalize('NFKC', text).casefold()
    return ''.join(ch for ch in text if not ch.isspace()
                   and unicodedata.category(ch)[0] not in 'PZ')


def edits(reference, hypothesis):
    previous = list(range(len(hypothesis) + 1))
    for index, want in enumerate(reference, 1):
        row = [index]
        for column, actual in enumerate(hypothesis, 1):
            row.append(min(row[-1] + 1, previous[column] + 1,
                           previous[column - 1] + (want != actual)))
        previous = row
    return previous[-1]


def percentiles(values):
    ordered = sorted(values)
    return {'n': len(ordered), **{
        f'p{p}': ordered[math.ceil(p * len(ordered) / 100) - 1] for p in (50, 95)
    }}


class Run:
    def __init__(self, out):
        out.mkdir(mode=0o700)
        self.out = out
        self.report = {'schema_version': 1, 'status': 'running', 'checks': [],
                       'machine': {'platform': platform.platform(),
                                   'machine': platform.machine(),
                                   'logical_cpus': os.cpu_count()},
                       'sample_manifest': file_identity(MANIFEST),
                       'unicode_version': unicodedata.unidata_version}
        self.save()

    def save(self):
        write_report(self.out / 'report.json', self.report)

    def check(self, label, argv, timeout, env=None, output=False):
        stdout = self.out / f'{label}.stdout'
        stderr = self.out / f'{label}.stderr'
        self.report['active_check'] = {'label': label, 'argv': list(map(str, argv)),
                                       'stdout': str(stdout), 'stderr': str(stderr),
                                       'timeout_seconds': timeout}
        self.save()
        result, content = run_check(argv, stdout, stderr, cwd=ROOT, env=env,
                                    timeout=timeout, read_stdout=output)
        self.report['checks'].append({'label': label, **result})
        del self.report['active_check']
        self.save()
        if not result['passed']:
            raise MeasurementError(f'{label} failed, see {stderr}')
        return content


def measurement_rows(content, scene, count):
    rows = [json.loads(line) for line in content.splitlines()]
    selected = [row for row in rows if row['scene'] == scene]
    if len(selected) != count or (count > 1 and
                                  [r['iteration'] for r in selected] != list(range(1, count + 1))):
        raise MeasurementError(f'{scene}: expected {count} ordered observations')
    return selected


def summarize(rows):
    metrics = {}
    for key in rows[0]:
        if key.endswith('_ms'):
            values = [row[key] for row in rows if row[key] is not None]
            if any(not math.isfinite(value) or value < 0 for value in values):
                raise MeasurementError(f'{key}: invalid duration')
            metrics[key] = percentiles(values) if values else {'n': 0}
    memory = [row['peak_phys_footprint_bytes'] for row in rows
              if 'peak_phys_footprint_bytes' in row]
    if memory:
        metrics['peak_phys_footprint_bytes'] = max(memory)
    return metrics


def fixture(run, args, binary, cli):
    modes = ['headless', 'detached'] if args.detached else ['headless']
    for mode in modes:
        stdout = run.check(mode, [binary, mode, cli, args.runs, args.idle_seconds],
                           timeout=40 * args.runs + args.idle_seconds + 15, output=True)
        scenes = (['synthetic_dictation'] if mode == 'headless' else
                  ['detached_panel_preparation', 'detached_panel_preparation_blocked'])
        for scene in scenes:
            rows = measurement_rows(stdout, scene, args.runs)
            run.report[scene] = summarize(rows)
        idle_scene = 'idle_panel_uncreated' if mode == 'headless' else 'idle_detached_hidden'
        idle, = measurement_rows(stdout, idle_scene, 1)
        if idle['window_seconds'] < args.idle_seconds or idle['panel_created'] != (mode == 'detached'):
            raise MeasurementError(f'{idle_scene}: incorrect idle window or panel state')
        run.report[idle_scene] = idle
        run.save()


def synthesize(run, args, manifest):
    if not args.espeak.is_absolute():
        raise MeasurementError('--espeak must be an explicit absolute path to a local binary')
    recipe = manifest['synth']
    version = run.check('espeak-version', [args.espeak, '--version'], 10, output=True)
    if f"{recipe['engine']} text-to-speech: {recipe['version']}" not in version.decode():
        raise MeasurementError('eSpeak NG version differs from the fixed recipe')
    identities = {'manifest': file_identity(MANIFEST), 'espeak': file_identity(args.espeak),
                  'samples': {}}
    for sample in manifest['samples']:
        rate = None
        parts = []
        for index, (voice, text) in enumerate(sample['segments']):
            name = f"{sample['id']}-{index}"
            part = run.out / f'{name}.wav'
            run.check(name, [args.espeak, '-v', voice, '-s', recipe['words_per_minute'],
                             '-w', part, text], 20)
            with wave.open(str(part), 'rb') as wav:
                if wav.getnchannels() != 1 or wav.getsampwidth() != 2 or wav.getnframes() == 0:
                    raise MeasurementError(f'{name}: expected nonempty mono PCM16')
                rate = rate or wav.getframerate()
                if rate != wav.getframerate():
                    raise MeasurementError(f'{name}: segment sample rate differs')
                parts.append(wav.readframes(wav.getnframes()))
        gap = b'\0\0' * (rate * recipe['gap_ms'] // 1000)
        destination = run.out / (sample['id'] + '.wav')
        with wave.open(str(destination), 'wb') as wav:
            wav.setparams((1, 2, rate, 0, 'NONE', 'not compressed'))
            wav.writeframes(gap.join(parts))
        identities['samples'][destination.name] = file_identity(destination)
    write_report(run.out / 'samples.identity.json', identities)
    run.report['synthesized_inputs'] = identities


def model_suite(run, args, manifest, binary):
    samples = args.samples_dir.resolve(strict=True)
    identities = json.loads((samples / 'samples.identity.json').read_text())
    if identities['manifest'] != file_identity(MANIFEST):
        raise MeasurementError('audio manifest digest does not match the fixed text manifest')
    for sample in manifest['samples']:
        name = sample['id'] + '.wav'
        if file_identity(samples / name) != identities['samples'][name]:
            raise MeasurementError(f'{name}: audio digest differs from its recorded identity')
    for origin, directory, known_suffix in (
        ('model', args.model_dir, ('.onnx', 'tokens.txt')),
        ('runtime', args.runtime_dir, ('.dylib',)),
    ):
        files = sorted(path for path in directory.iterdir() if path.name.endswith(known_suffix))
        if not files or any(path.is_symlink() or not path.is_file() for path in files):
            raise MeasurementError(f'{origin}: supply an owned directory with regular input files')
        run.report[origin] = {path.name: file_identity(path) for path in files}
    run.report.update(model_id=args.model_id, samples=identities['samples'], model_measurements={})
    for sample in manifest['samples']:
        name = sample['id']
        argv = [binary, 'model', args.model_id, args.model_dir.resolve(), args.runtime_dir.resolve(),
                samples / (name + '.wav'), sample['asr_language']]
        cold = []
        for index in range(1, args.runs + 1):
            stdout = run.check(f'{name}-cold-{index}', [*argv, 1], 150, output=True)
            cold.extend(measurement_rows(stdout, 'local_file_recognizer', 1))
        stdout = run.check(f'{name}-warm', [*argv, args.runs + 1], 150 * (args.runs + 1), output=True)
        warm = measurement_rows(stdout, 'local_file_recognizer', args.runs + 1)[1:]
        for label, rows in (('process_cold', cold), ('process_warm', warm)):
            stats = summarize(rows)
            reference = normalized(sample['reference'])
            distance = [edits(reference, normalized(row['text'])) for row in rows]
            stats['cer'] = {'edits': sum(distance), 'reference_characters': len(reference) * len(rows),
                            'ratio': sum(distance) / (len(reference) * len(rows))}
            stats['recognizer_open_share'] = percentiles([row['open_ms'] / row['final_ms'] for row in rows])
            run.report['model_measurements'][f'{name}/{label}'] = stats
        run.save()
    before_and_after = [(args.model_dir, 'model'), (args.runtime_dir, 'runtime'), (samples, 'samples')]
    for directory, key in before_and_after:
        if any(file_identity(directory / name) != identity for name, identity in run.report[key].items()):
            raise MeasurementError(f'{key} inputs changed during measurement')


def execute(args):
    root = args.evidence_dir.resolve()
    manifest = json.loads(MANIFEST.read_text(encoding='utf-8'))
    with termination_signals(), full_suite_lock(ROOT):
        run = Run(root)
        try:
            if args.mode == 'synth':
                synthesize(run, args, manifest)
            else:
                head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
                if head != args.expected_head:
                    raise MeasurementError(f'expected HEAD {args.expected_head}, found {head}')
                target = args.target_dir.resolve() if args.target_dir else root / 'target'
                run.report.update(head=head, runs=args.runs, mode=args.mode,
                                  tracked_status=subprocess.check_output(
                                      ['git', 'status', '--porcelain=v1'], cwd=ROOT, text=True),
                                  host_cpu=subprocess.check_output(
                                      ['/usr/sbin/sysctl', '-n', 'machdep.cpu.brand_string'], text=True).strip())
                run.save()
                if args.target_dir and not target.is_relative_to(ROOT / '.voiceport'):
                    raise MeasurementError('--target-dir must be an isolated target below this checkout’s .voiceport')
                cargo = ['cargo', 'build', '--offline', '--locked', '-p', 'voiceport-macos',
                         '--example', 'performance_baseline']
                if args.mode == 'fixture':
                    cargo += ['-p', 'voiceport-cli', '--bin', 'voiceport']
                run.check('build', cargo, 600, env={**os.environ, 'CARGO_TARGET_DIR': str(target),
                                                   'CARGO_BUILD_JOBS': '2'})
                binary = target / 'debug/examples/performance_baseline'
                run.report['probe'] = file_identity(binary)
                if args.mode == 'fixture':
                    cli = target / 'debug/voiceport'
                    run.report['cli'] = file_identity(cli)
                    fixture(run, args, binary, cli)
                else:
                    model_suite(run, args, manifest, binary)
            run.report['status'] = 'completed'
        except (ValueError, OSError, subprocess.SubprocessError, KeyboardInterrupt) as error:
            run.report.update(status='failed', error=str(error))
            raise
        finally:
            run.save()
    print(run.out / 'report.json')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='mode', required=True)
    fixture_parser = sub.add_parser('fixture', help='private CLI, synthetic audio and idle counters')
    model_parser = sub.add_parser('model', help='explicitly supplied offline ASR model and samples')
    synth_parser = sub.add_parser('synth', help='generate fixed speech using a supplied offline synthesizer')
    for subparser in (fixture_parser, model_parser, synth_parser):
        subparser.add_argument('--evidence-dir', type=Path, required=True, help='new private output directory')
    for subparser in (fixture_parser, model_parser):
        subparser.add_argument('--runs', type=int, default=30, choices=range(1, 31), metavar='1..30')
        subparser.add_argument('--target-dir', type=Path, help='reuse this checkout’s already built isolated target')
        subparser.add_argument('--expected-head', default=BASELINE)
    fixture_parser.add_argument('--idle-seconds', type=int, default=60, choices=range(1, 61), metavar='1..60')
    fixture_parser.add_argument('--detached', action='store_true', help='also initialize offscreen AppKit views')
    model_parser.add_argument('--model-id', required=True)
    for name in ('model-dir', 'runtime-dir', 'samples-dir'):
        model_parser.add_argument('--' + name, type=Path, required=True)
    synth_parser.add_argument('--espeak', type=Path, required=True)
    try:
        execute(parser.parse_args())
    except (ValueError, OSError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
