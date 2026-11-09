#!/usr/bin/python3
"""Create a NEW directory with a reviewable example adapter and import manifest.

Never runs the adapter, imports into VoicePort, installs dependencies or connects.
"""
import argparse
import json
import pathlib
import shutil
import sys


def prepare(destination):
    destination = destination.expanduser().absolute()
    template = pathlib.Path(__file__).resolve().parents[1] / 'templates/json_http_adapter.py'
    # Check inputs before creating anything; never reuse/overwrite an old directory.
    adapter = template.read_bytes()
    executable = pathlib.Path(sys.executable).resolve(strict=True)
    if not executable.is_file() or not shutil.which(str(executable)):
        raise ValueError('Python executable is unavailable')
    destination.mkdir(mode=0o700, parents=False, exist_ok=False)
    destination = destination.resolve(strict=True)
    script = destination / 'adapter.py'
    with script.open('xb') as output:
        output.write(adapter)
    manifest = {
        'schema_version': 1,
        'protocol_version': 1,
        'id': 'example.json-http',
        'version': '1.0.0',
        'name': '示例 JSON 音频接口（需对应服务）',
        'origin': 'VoicePort Skill example; review adapter.py and actual service contract',
        'adapter': 'jsonl',
        'capabilities': {'batch': True, 'streaming': False, 'system_preview': False},
        'fields': [
            {'id': 'endpoint', 'label': '完整 JSON 转写地址', 'kind': 'url', 'required': True},
            {'id': 'model', 'label': '模型 ID', 'kind': 'string', 'required': True},
            {'id': 'api_key', 'label': '独立 Bearer API Key', 'kind': 'secret', 'required': False},
        ],
        'program': {'executable': str(executable), 'args': ['-I', str(script)]},
    }
    path = destination / 'plugin.json'
    with path.open('x', encoding='utf-8') as output:
        json.dump(manifest, output, ensure_ascii=False, indent=2)
        output.write('\n')
    return {'adapter': str(script), 'manifest': str(path), 'executed': False}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', required=True, type=pathlib.Path,
                        help='new output directory; its parent must already exist')
    args = parser.parse_args()
    try:
        print(json.dumps(prepare(args.directory), ensure_ascii=False))
    except (OSError, ValueError):
        parser.exit(1, 'Cannot prepare example: use a new directory under an existing writable parent; inspect any partial output before choosing another directory.\n')
