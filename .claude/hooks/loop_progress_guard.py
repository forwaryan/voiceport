#!/usr/bin/env python3
"""Redirect repeated tool exchanges without ending the task or granting authority.

Read only a bounded transcript tail. Seven identical paired results block only
another identical call, before the observed gateway stops after eight results;
a different diagnostic action remains available. Unknown
history cannot prove stagnation. Rotation, permission and wait guards stay in force.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import sys

REPEATS = 7
TAIL_BYTES = 2 * 1024 * 1024
MAX_ROWS = 512
NOISE = (
    (re.compile(r'(?m)^(Wall time:? )[0-9.]+( seconds?)'), r'\1#\2'),
    (re.compile(r'(?m)^(Chunk ID: )\S+'), r'\1#'),
)
POLICY = (
    '同一工具、相同参数已连续7次得到相同结果；本次在网关8次截断前只阻止重复操作，不结束原任务。'
    '核对已有证据与实际副作用，立即选择有新依据的诊断或替代路径继续。'
    '不要仅改耗时、调用ID、包装命令或解析上限来绕过。真实后台等待交给完成通知；'
    '未知 launch/close/写入结果先查已有 journal/精确回执，不重新发起副作用。'
    '暂停、权限和信任拒绝继续生效；有独立可执行工作时不得挂空 timer 或让用户接手。'
)


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(',', ':'))


def steady(value):
    if isinstance(value, str):
        for pattern, replacement in NOISE:
            value = pattern.sub(replacement, value)
        return value
    if isinstance(value, list):
        return [steady(item) for item in value]
    if isinstance(value, dict):
        return {key: steady(item) for key, item in value.items()}
    return value


def call_key(name, arguments):
    if not isinstance(name, str) or not isinstance(arguments, dict):
        raise ValueError('invalid tool call')
    return (name.split('.')[-1], canonical(arguments))


def recent_rows(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode):
            raise ValueError('transcript must be a regular file')
        start = max(0, before.st_size - TAIL_BYTES)
        stream.seek(start)
        raw = stream.read(before.st_size - start)
        if len(raw) != before.st_size - start:
            raise ValueError('transcript truncated')
        # Appends are fine; rewriting the inspected snapshot is not.
        stream.seek(start)
        if stream.read(len(raw)) != raw or os.fstat(stream.fileno()).st_ino != before.st_ino:
            raise ValueError('transcript snapshot changed')
    current = os.stat(path, follow_symlinks=False)
    if (current.st_dev, current.st_ino) != (before.st_dev, before.st_ino):
        raise ValueError('transcript replaced')
    lines = raw.splitlines(keepends=True)
    if start and lines:
        lines = lines[1:]  # May start in the middle of a large row.
    rows = []
    for line in lines[-MAX_ROWS:]:
        if not line.endswith(b'\n'):
            continue
        try:
            value = json.loads(line)
        except (ValueError, UnicodeError):
            # An unreadable segment is a barrier, never evidence of equality.
            rows.clear()
            continue
        if isinstance(value, dict):
            rows.append(value)
    return rows


def exchanges(rows, session):
    calls, completed = {}, []
    for row in rows:
        if row.get('sessionId') != session or row.get('isSidechain'):
            continue
        message = row.get('message', {})
        if not isinstance(message, dict):
            continue
        content = message.get('content', [])
        if row.get('type') == 'user' and not row.get('isMeta'):
            # Actual user steering begins a new attempt; tool output never does.
            if isinstance(content, str) or (isinstance(content, list) and any(
                    isinstance(b, dict) and b.get('type') == 'text' for b in content)):
                calls.clear()
                completed.clear()
        if (row.get('type') == 'api-request-blob' and message.get('role') == 'system'
                and isinstance(content, list) and any(isinstance(b, dict)
                and b.get('type') == 'text' and b.get('text', '').startswith(
                    '[SYSTEM NOTIFICATION - NOT USER INPUT]') for b in content)):
            calls.clear()
            completed.clear()
        attachment = row.get('attachment', {})
        if (row.get('type') == 'attachment' and isinstance(attachment, dict)
                and attachment.get('type') == 'queued_command'
                and isinstance(attachment.get('prompt'), str)
                and attachment['prompt'].startswith('<task-notification>')):
            calls.clear()
            completed.clear()
        if not isinstance(content, list):
            continue
        for block in content:
            if not isinstance(block, dict):
                continue
            if row.get('type') == 'assistant' and block.get('type') == 'tool_use':
                if isinstance(block.get('id'), str):
                    try:
                        calls[block['id']] = call_key(block.get('name'), block.get('input'))
                    except ValueError:
                        completed.clear()
            elif row.get('type') == 'user' and block.get('type') == 'tool_result':
                call = calls.pop(block.get('tool_use_id'), None)
                if call is None:
                    completed.clear()
                    continue
                result = {'content': steady(block.get('content')), 'is_error': block.get('is_error') is True,
                          'structured': steady(row.get('toolUseResult'))}
                digest = hashlib.sha256(canonical(result).encode()).hexdigest()
                completed.append((call, digest))
                completed = completed[-REPEATS:]
    return completed


def live_wait(transcript, session, key):
    # A name containing wait/poll or a duration alone proves no dependency.
    if key[0] != 'TaskOutput':
        return False
    arguments = json.loads(key[1])
    task = arguments.get('task_id')
    if not isinstance(task, str):
        return False
    spec = importlib.util.spec_from_file_location('loop_wait_receipts', Path(__file__).with_name('loop_wait_guard.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    # The launch can predate the fingerprint tail. Inspect bounded full receipts;
    # missing/unreadable history stays unknown and cannot accuse a live wait.
    return task in module.pending_tasks(module.transcript_rows(transcript), session)


def hook(payload):
    if payload.get('hook_event_name') != 'PreToolUse':
        return {}
    tool, data = payload.get('tool_name', ''), payload.get('tool_input', {})
    if not isinstance(tool, str) or not isinstance(data, dict):
        return {}
    name = tool.split('.')[-1]
    # Shutdown/status/rescue must remain reachable even after repeated failures.
    if name in ('CronList', 'CronDelete', 'TaskStop') or (name == 'ScheduleWakeup' and data.get('stop') is True):
        return {}
    try:
        key = call_key(name, data)
        if name in ('Bash', 'exec_command'):
            import shlex
            argv = shlex.split(data.get('command' if name == 'Bash' else 'cmd', ''))
            if (len(argv) == 6 and argv[:3] == ['python3', '-I', '.claude/hooks/coordinator_rotation.py']
                    and argv[3] in ('session-status', 'session-recover') and argv[4] == '--session'
                    and argv[5] == payload['session_id']):
                return {}  # This exception grants nothing beyond the rotation gate.
        rows = recent_rows(payload['transcript_path'])
        history = exchanges(rows, payload['session_id'])
        if len(history) < REPEATS or any(item != history[-1] for item in history):
            return {}
        if key != history[-1][0] or live_wait(payload['transcript_path'], payload['session_id'], key):
            return {}
    except (ValueError, OSError, KeyError, TypeError, AttributeError):
        return {}  # No reliable fingerprint, no accusation; never emits allow.
    return {'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'permissionDecision': 'deny',
                                  'permissionDecisionReason': 'Loop progress guard: ' + POLICY}}


def main():
    payload = json.load(sys.stdin)
    if not isinstance(payload, dict):
        raise ValueError('hook stdin must be an object')
    print(json.dumps(hook(payload), ensure_ascii=False))


if __name__ == '__main__':
    main()
