#!/usr/bin/env python3
"""Require an explicit dependency before dynamic-loop scheduling; never grant permission.

The model must assess independent runnable work. This guard validates that assessment
is present and, for harness tasks, checks correlated receipts rather than prose claims.
Rotation/ownership/pause policy remains in coordinator_rotation.py.
"""
import json
import os
import re
import stat
import sys

MAX_TRANSCRIPT_BYTES = 128 * 1024 * 1024
MAX_LINE_BYTES = 16 * 1024 * 1024
POLICY = (
    '有已授权且可立即执行的下一步时继续当前轮，不得用定时器代替执行。'
    '只有没有独立可执行步骤且确有等待对象时才能续订；reason 使用 '
    '[wait:background id=实际任务ID runnable=none] 等待原因，或 '
    '[wait:external ref=可观测条件引用 runnable=none] 等待原因，或 '
    '[wait:periodic ref=用户授权的观察目标 runnable=none] 观察原因。'
    '后台任务必须有本会话实际异步回执且未结束，完成通知是主要继续信号。'
    '小阶段完成、保存文档、计划下次继续调查均不是等待条件；不得创建无用任务或缩短间隔绕过。'
    '任务完成、暂停或仅剩无法自行解除的阻塞时停止自有调度并说明解除条件。'
)
INTENT = re.compile(
    r'^\[wait:(background|external|periodic) (id|ref)=([^\s\[\]]{1,512}) '
    r'runnable=none\] (\S[^\r\n]{0,1500})$'
)
TASK_ID = re.compile(r'^[A-Za-z0-9_-]{1,160}$')


def intent(data):
    match = INTENT.fullmatch(data.get('reason', '') if isinstance(data.get('reason'), str) else '')
    if not match:
        raise ValueError('缺少明确的等待对象及 runnable=none 声明。' + POLICY)
    kind, key, reference, _ = match.groups()
    if key != ('id' if kind == 'background' else 'ref'):
        raise ValueError('后台等待使用 id，外部条件/周期观察使用 ref。' + POLICY)
    if kind == 'background':
        if not TASK_ID.fullmatch(reference):
            raise ValueError('后台任务 ID 格式无效。')
        delay = data.get('delaySeconds')
        if isinstance(delay, bool) or not isinstance(delay, (int, float)) or delay < 1200:
            raise ValueError('后台任务由完成通知唤醒，兜底至少1200秒，不短间隔轮询。')
    return kind, reference


def transcript_rows(path):
    descriptor = os.open(path, os.O_RDONLY | getattr(os, 'O_NOFOLLOW', 0))
    with os.fdopen(descriptor, 'rb') as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_TRANSCRIPT_BYTES:
            raise ValueError('transcript 必须是有界普通文件。')
        # Read a fixed snapshot. Ignore a partial final line still being appended.
        remaining = info.st_size
        while remaining:
            line = stream.readline(min(remaining, MAX_LINE_BYTES + 1))
            if not line:
                raise ValueError('transcript 在核对期间截断。')
            remaining -= len(line)
            if len(line) > MAX_LINE_BYTES:
                raise ValueError('transcript 行超过限制。')
            if not line.endswith(b'\n') and not remaining:
                break
            yield json.loads(line)


def completed_tasks(row, session):
    """All completions from actual host records, never rendered/pasted copies."""
    if row.get('sessionId') != session or row.get('isSidechain'):
        return []
    texts, queued = [], False
    if row.get('type') == 'attachment':
        attachment = row.get('attachment', {})
        prompt = attachment.get('prompt') if isinstance(attachment, dict) else None
        if (attachment.get('type') == 'queued_command' and isinstance(prompt, str)
                and prompt.startswith('<task-notification>')):
            texts, queued = [prompt], True
    elif row.get('type') == 'api-request-blob':
        message = row.get('message', {})
        if isinstance(message, dict) and message.get('role') == 'system':
            blocks = message.get('content', [])
            if isinstance(blocks, list):
                texts = [b['text'] for b in blocks if isinstance(b, dict) and b.get('type') == 'text'
                         and isinstance(b.get('text'), str)
                         and b['text'].startswith('[SYSTEM NOTIFICATION - NOT USER INPUT]')]
    ended = []
    for text in texts:
        header = text.split('<task-notification>', 1)
        if len(header) != 2:
            continue
        header = header[1].split('<result>', 1)[0]
        task = re.search(r'<task-id>([A-Za-z0-9_-]{1,160})</task-id>', header)
        status = re.search(r'<status>(completed|failed|stopped|cancelled|killed)</status>', header)
        call = re.search(r'<tool-use-id>([^<>\s]{1,512})</tool-use-id>', header)
        if task and status and (call or not queued):
            ended.append((task.group(1), status.group(1), call.group(1) if call else None))
    return ended


def completed_task(row, session):
    """Compatibility view for callers needing just the first completion."""
    ended = completed_tasks(row, session)
    return ended[0][:2] if ended else None


def pending_tasks(rows, session):
    calls, pending, origins = {}, set(), {}
    verified_session = False
    for row in rows:
        if row.get('sessionId') != session or row.get('isSidechain'):
            continue
        verified_session = True
        for handle, _, call_id in completed_tasks(row, session):
            if call_id is None or origins.get(handle) == call_id:
                pending.discard(handle)
        message = row.get('message', {})
        blocks = message.get('content', [])
        if not isinstance(blocks, list):
            continue
        for block in blocks:
            if not isinstance(block, dict):
                continue
            if row.get('type') == 'assistant' and block.get('type') == 'tool_use':
                name = block.get('name', '').split('.')[-1]
                if name in ('Agent', 'Bash', 'Monitor', 'SendMessage', 'TaskStop'):
                    calls[block.get('id')] = (name, block.get('input', {}))
            elif row.get('type') == 'user' and block.get('type') == 'tool_result':
                call = calls.pop(block.get('tool_use_id'), None)
                result = row.get('toolUseResult')
                if not call or block.get('is_error') or not isinstance(result, dict):
                    continue
                name, arguments = call
                if name == 'TaskStop':
                    # A stop request or generic success does not establish actual exit.
                    if result.get('status') in ('completed', 'failed', 'stopped'):
                        pending.discard(arguments.get('task_id'))
                    continue
                task = None
                if name == 'SendMessage' and result.get('success') is True:
                    resumed = result.get('resumedAgentId')
                    address = arguments.get('to')
                    pin = result.get('pin')
                    addresses = {resumed} if isinstance(resumed, str) else set()
                    if isinstance(pin, dict) and pin.get('id') == resumed and isinstance(pin.get('name'), str):
                        addresses.add(pin['name'])
                        if isinstance(pin.get('ref'), str):
                            addresses.add(f"{pin['name']} [{pin['ref']}]")
                    if isinstance(address, str) and address in addresses:
                        task = resumed
                if result.get('isAsync') is True and result.get('status') == 'async_launched':
                    task = result.get('agentId')
                elif name in ('Bash', 'Monitor'):
                    task = result.get('backgroundTaskId')
                if isinstance(task, str) and TASK_ID.fullmatch(task):
                    pending.add(task)
                    origins[task] = block['tool_use_id']
    if not verified_session:
        raise ValueError('没有本会话 transcript 身份，不能核对后台等待。')
    return pending


def hook(payload):
    if (payload.get('hook_event_name') != 'PreToolUse'
            or payload.get('tool_name', '').split('.')[-1] != 'ScheduleWakeup'):
        return {}
    data = payload.get('tool_input', {})
    # Always leave shutdown available; the existing rotation guard handles authority.
    if data.get('stop') is True:
        return {}
    try:
        kind, reference = intent(data)
        if kind == 'background':
            if reference not in pending_tasks(transcript_rows(payload['transcript_path']), payload['session_id']):
                raise ValueError('等待对象没有本会话未结束的异步启动回执；处理已返回的结果或继续可执行步骤。')
    except (ValueError, OSError, KeyError, TypeError) as error:
        return {'hookSpecificOutput': {
            'hookEventName': 'PreToolUse', 'permissionDecision': 'deny',
            'permissionDecisionReason': f'Loop wait guard: {error}',
        }}
    # No "allow": a valid wait cannot override rotation, ownership or permission denial.
    return {'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'additionalContext': POLICY}}


def main():
    try:
        payload = json.load(sys.stdin)
        if not isinstance(payload, dict):
            raise ValueError('hook stdin must be an object')
        result = hook(payload)
    except (ValueError, OSError, TypeError) as error:
        print(f'Loop wait guard input error: {error}', file=sys.stderr)
        return 2
    if (payload.get('hook_event_name') == 'PreToolUse'
            and payload.get('tool_name', '').split('.')[-1] == 'ScheduleWakeup'
            and payload.get('tool_input', {}).get('stop') is True):
        # Sparse real-load evidence on shutdown; this is not the scheduler receipt.
        import hashlib
        from pathlib import Path
        result = {'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'additionalContext':
                  'Loop wait guard shutdown path loaded; helper_sha256='
                  + hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}}
    print(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == '__main__':
    sys.exit(main())
