#!/usr/bin/env python3
"""Build and replay VoicePort's offscreen, synthetic macOS UI scenes."""
import argparse
import json
import os
from pathlib import Path
import platform
import stat
import struct
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from product_inventory import inventory
from verification_run import (
    full_suite_lock, log_facts, run_check, termination_signals, timestamp, write_report,
)

ROOT = Path(__file__).resolve().parent.parent
TARGET = ROOT / '.voiceport' / 'target-native-ui'
SCENES = {
    'light-short': '快捷面板就绪，单一来源与持久状态',
    'dark-long': '快捷面板展开，长标题、禁用来源与键盘遍历',
    'dark-small-recovery': '短视口内展示待找回的听写结果',
    'dark-small-history': '短视口内选中旧听写并复制到内存',
    'light-empty': '未配置来源时仍可添加',
    'dark-busy': '尝试转写时保留状态与快捷键，锁定配置动作',
    'dark-small-revealed': '短视口内显示合成正文和复制操作',
    'light-result': '原生听写页显示最终正文与投递状态',
    'light-result-edit': '原生听写页编辑临时副本',
}
LEGACY_SCENES = tuple(SCENES)[:5]
MARKERS = (
    'stable IDs, disabled reason, key-view links and scroll-to-last focus',
    'waiting recovery and once-per-generation copy verified without a clipboard',
    'two real Controller finals, one old-entry memory copy, one target write each',
    'never showed or made the fixture window key',
)
EXTENDED_MARKER = 'native-ui: owned, non-presented AppKit captures completed without activation'
TIMINGS = {'first_offscreen_frame_us', 'busy_offscreen_refresh_us'}


def git(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT, text=True, timeout=10).strip()


def inputs():
    return {'head': git('rev-parse', 'HEAD'), 'head_tree': git('rev-parse', 'HEAD^{tree}'),
            'status': git('status', '--porcelain=v1', '--untracked-files=all').splitlines(),
            'product': inventory(ROOT)}


def binary_from_build(path):
    executables = set()
    with Path(path).open() as log:
        for line in log:
            event = json.loads(line)
            if event['reason'] == 'compiler-artifact' and event.get('executable'):
                target = event['target']
                if target['kind'] == ['bin'] and target['name'] == 'voiceport-macos':
                    executables.add(Path(event['executable']).resolve(strict=True))
    if len(executables) != 1:
        raise ValueError('Cargo must identify one voiceport-macos binary')
    binary = executables.pop()
    if not binary.is_relative_to(TARGET.resolve()) or not binary.is_file():
        raise ValueError('Cargo binary is outside the isolated target')
    return binary


def png_facts(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or not 33 <= info.st_size <= 16 * 1024 * 1024:
        raise ValueError(f'invalid PNG file: {path.name}')
    with path.open('rb') as image:
        header = image.read(24)
        image.seek(-12, os.SEEK_END)
        trailer = image.read(12)
    if (header[:16] != b'\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR'
            or trailer != b'\x00\x00\x00\x00IEND\xaeB`\x82'):
        raise ValueError(f'incomplete PNG container: {path.name}')
    width, height = struct.unpack('>II', header[16:24])
    if not (1 <= width <= 8192 and 1 <= height <= 8192):
        raise ValueError(f'invalid PNG dimensions: {path.name}')
    facts, _ = log_facts(path, None)
    return {**facts, 'width': width, 'height': height}


def collect_scenes(directory):
    actual = {item.name for item in directory.iterdir()}
    for suite, names in [('legacy', LEGACY_SCENES), ('extended', SCENES)]:
        if actual == {f'{name}.png' for name in names}:
            return suite, {
                name: {'focus': SCENES[name], 'file': f'screens/{name}.png',
                       **png_facts(directory / f'{name}.png')}
                for name in names
            }
    raise ValueError(f'unrecognized or incomplete scene set: {sorted(actual)}')


def compare_baseline(path, report):
    baseline = json.loads(path.read_text())
    if (baseline.get('schema_version') != 1 or not baseline.get('passed')
            or baseline.get('mode') != report['mode']):
        raise ValueError('expected a passing native UI baseline report in the same mode')
    old_scenes = baseline['scenes']
    if set(old_scenes) not in (set(LEGACY_SCENES), set(SCENES)):
        raise ValueError('baseline has an unrecognized or incomplete scene set')
    new_scenes = report['scenes']
    differences = {}
    for name in sorted(old_scenes.keys() | new_scenes.keys()):
        old = old_scenes.get(name)
        new = new_scenes.get(name)
        if old is not None:
            image = f'screens/{name}.png'
            actual = png_facts(path.parent / image)
            if old.get('file') != image or any(old.get(key) != value for key, value in actual.items()):
                raise ValueError(f'baseline image digest or dimensions changed: {name}')
        if old is None:
            status = 'added'
        elif new is None:
            status = 'removed'
        elif old['sha256'] == new['sha256']:
            status = 'same_png'
        elif (old['width'], old['height']) != (new['width'], new['height']):
            status = 'size_changed'
        else:
            status = 'changed_png'
        differences[name] = {'status': status, 'before': old, 'after': new}
    facts, _ = log_facts(path, None)
    return {'report': str(path), 'report_sha256': facts['sha256'],
            'head': baseline['inputs_before']['head'],
            'environment_matches': baseline['render_environment'] == report['render_environment'],
            'scenes': differences, 'visual_review': 'required'}


def read_timings(output):
    samples = {}
    for line in output.splitlines():
        if line.startswith('native-ui-timing: '):
            _, name, value = line.split()
            if name not in TIMINGS or name in samples or not value.isascii() or not value.isdecimal():
                raise ValueError(f'invalid or duplicate offscreen timing: {line}')
            samples[name] = int(value)
    if set(samples) != TIMINGS:
        raise ValueError('missing offscreen timing observations')
    return samples


def write_deck(path, report):
    before = report.get('comparison')
    lines = ['# VoicePort 原生场景', '', f"源码 HEAD `{report['inputs_before']['head']}`。",
             '', '截图来自合成数据和未展示的 AppKit 窗口。真实菜单点击、跨应用焦点和闪烁由隔离 Aqua 验收覆盖。',
             '', '逐项命令、日志、像素尺寸与摘要见 [report.json](report.json)。']
    for name, scene in report['scenes'].items():
        lines.extend(['', f"## {scene['focus']}", '', f"![{name}]({scene['file']})"])
        if before:
            difference = before['scenes'][name]
            lines.extend(['', f"基线对照 `{difference['status']}`。"])
            if difference['before']:
                baseline_image = Path(before['report']).parent / difference['before']['file']
                relative = os.path.relpath(baseline_image, path.parent)
                lines.extend(['', f'[查看基线帧](<{relative}>)'])
    if report.get('timing_observations'):
        lines.extend(['', '## 离屏渲染耗时', '', '两条单样本记录均在 PNG 编码和写盘前结束。'])
        for name, value in report['timing_observations'].items():
            lines.extend(['', f'- `{name}` {value} μs'])
    with path.open('x') as deck:
        deck.write('\n'.join(lines) + '\n')


def verify(args, out):
    report_path = out / 'report.json'
    report = {'schema_version': 1, 'mode': 'appkit_offscreen', 'suite': args.suite,
              'started_at': timestamp(), 'status': 'running', 'passed': False, 'checks': [],
              'render_environment': {'machine': platform.machine(), 'macos': platform.mac_ver()[0],
                                     'darwin': platform.release()},
              'unverified': ['menu_bar_click_and_popover_compositing', 'positive_aqua_key_and_ax_focus',
                             'cold_start_and_reopen_latency', 'hidden_render_cost', 'real_devices_and_asr']}
    write_report(report_path, report)

    def check(name, command, cwd, env, timeout):
        report['active_check'] = {'name': name, 'argv': list(map(str, command)),
                                  'started_at': timestamp(), 'timeout_seconds': timeout}
        write_report(report_path, report)
        result, _ = run_check(command, out / f'{name}.stdout.log', out / f'{name}.stderr.log',
                              cwd=cwd, env=env, timeout=timeout)
        report['checks'].append({'name': name, **result})
        report.pop('active_check')
        write_report(report_path, report)
        if not result['passed']:
            raise ValueError(f"{name} failed: {result['status']} (exit {result['exit_code']})")
        return result

    try:
        before = inputs()
        report['inputs_before'] = {key: value for key, value in before.items() if key != 'product'}
        report['inputs_before']['product_sha256'] = before['product']['sha256']
        write_report(out / 'inputs-before.json', before)
        if before['head'] != args.expect_head:
            raise ValueError('checked-out HEAD differs from --expect-head')
        if args.expect_inputs and before['product']['sha256'] != args.expect_inputs:
            raise ValueError('product inputs differ from --expect-inputs')
        if args.suite == 'legacy' and git('status', '--porcelain=v1', '--untracked-files=all',
                                         '--', 'Cargo.toml', 'Cargo.lock', 'crates', 'resources'):
            raise ValueError('legacy source and build inputs must match the pinned HEAD')
        tools = check('tools', ['rustc', '-Vv'], ROOT, None, 10)
        report['rustc'] = Path(tools['stdout']).read_text().strip()
        build_env = dict(os.environ, CARGO_TARGET_DIR=str(TARGET), CARGO_NET_OFFLINE='true',
                         CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0', CARGO_BUILD_JOBS='2')
        build = check('build', ['cargo', 'build', '--offline', '--locked', '-p', 'voiceport-macos',
                                '--bin', 'voiceport-macos', '--message-format=json'], ROOT, build_env, 360)
        binary = binary_from_build(build['stdout'])
        binary_facts, _ = log_facts(binary, None)
        report['binary'] = {'path': str(binary), **binary_facts}
        screens = out / 'screens'
        screens.mkdir()
        fixture_env = {'PATH': '/usr/bin:/bin', 'TMPDIR': str(out), 'TZ': 'UTC'}
        replay = check('replay', [binary, '--quick-panel-fixture', screens], out, fixture_env, 30)
        suite, report['scenes'] = collect_scenes(screens)
        if suite != args.suite:
            raise ValueError(f'expected {args.suite} scene set, captured {suite}')
        replay_log = Path(replay['stdout']).read_text()
        required = MARKERS + ((EXTENDED_MARKER,) if suite == 'extended' else ())
        if any(marker not in replay_log for marker in required):
            raise ValueError('replay did not report every expected interaction checkpoint')
        if log_facts(binary, None)[0] != binary_facts:
            raise ValueError('fixture binary changed during replay')
        report['interaction_checkpoints'] = list(required)
        if suite == 'extended':
            report['timing_observations'] = read_timings(replay_log)
        if args.baseline:
            report['comparison'] = compare_baseline(args.baseline, report)
        after = inputs()
        write_report(out / 'inputs-after.json', after)
        if before != after:
            raise ValueError('source inputs changed during verification')
        write_deck(out / 'deck.md', report)
        report.update(status='completed', passed=True)
    except (ValueError, KeyError, OSError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        report.update(status='interrupted' if isinstance(error, KeyboardInterrupt) else 'failed',
                      failure=str(error) or type(error).__name__)
        if 'active_check' in report:
            report['incomplete_check'] = report.pop('active_check')
    report['finished_at'] = timestamp()
    write_report(report_path, report)
    return 0 if report['passed'] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--expect-head', required=True, help='full expected checkout commit')
    parser.add_argument('--expect-inputs', help='optional frozen product inventory SHA-256')
    parser.add_argument('--suite', choices=('legacy', 'extended'), default='extended')
    parser.add_argument('--baseline', type=lambda arg: Path(arg).resolve(), help='earlier report.json')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        parser.error('the offscreen AppKit fixture requires macOS')
    try:
        with termination_signals(), full_suite_lock(ROOT):
            root = ROOT / '.voiceport' / 'evidence'
            root.mkdir(exist_ok=True, mode=0o700)
            out = Path(tempfile.mkdtemp(prefix='native-ui.', dir=root))
            print(out, flush=True)
            return verify(args, out)
    except (OSError, ValueError, KeyboardInterrupt) as error:
        print(f'native-ui: {error or "interrupted"}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
