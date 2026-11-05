#!/usr/bin/env python3
"""VoicePort coordinator lifecycle. Private state; no product ledger mutations."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[2]
STATE_NAME = 'coordinator-rotation.json'
SCRIPT = '.claude/hooks/coordinator_rotation.py'


def now():
    return datetime.now(timezone.utc).isoformat()


def load(path):
    return json.loads(Path(path).read_text())


def atomic(path, value):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as out:
        tmp = Path(out.name)
        try:
            json.dump(value, out, ensure_ascii=False, indent=2)
            out.write('\n')
            out.flush()
            os.fsync(out.fileno())
        except BaseException:
            tmp.unlink(missing_ok=True)
            raise
    os.replace(tmp, path)


@contextmanager
def transaction(root):
    directory = root / '.voiceport'
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (directory / 'coordinator-rotation.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        path = directory / STATE_NAME
        state = load(path) if path.exists() else None
        before = json.dumps(state, sort_keys=True)
        yield state
        if state is not None and json.dumps(state, sort_keys=True) != before:
            atomic(path, state)


def invoke(root, argv):
    result = subprocess.run(['orca', *argv, '--json'], cwd=root, text=True,
                            capture_output=True, timeout=45)
    try:
        receipt = json.loads(result.stdout)
    except ValueError as error:
        raise ValueError('Orca returned no valid receipt; inspect runtime before retrying') from error
    if result.returncode or not receipt.get('ok'):
        raise ValueError('Orca refused: ' + json.dumps(receipt, ensure_ascii=False)[:2500])
    return receipt


def bound_run(root, expected):
    receipt = invoke(root, ['orchestration', 'run-current', '--full'])
    run = receipt.get('result', {}).get('run') or {}
    if (run.get('id') or run.get('runId')) != expected:
        raise ValueError('caller is not bound to the expected Run')
    return receipt


def scan(state, transcript):
    """Consume only complete JSONL records; a boundary UUID is the completion identity."""
    path = Path(transcript)
    cursor = state.setdefault('transcript_cursor', {'path': str(path), 'offset': 0, 'inode': None})
    stat = path.stat()
    if cursor['path'] != str(path) or cursor['inode'] != stat.st_ino or stat.st_size < cursor['offset']:
        cursor.update(path=str(path), offset=0, inode=stat.st_ino)
    seen = set(state['boundaries'])
    with path.open('rb') as stream:
        stream.seek(cursor['offset'])
        while True:
            line = stream.readline()
            if not line or not line.endswith(b'\n'):
                break
            record = json.loads(line)
            if (record.get('subtype') == 'compact_boundary'
                    and record.get('sessionId') == state['owner']):
                identity = record.get('uuid')
                if not identity:
                    raise ValueError('compaction boundary has no UUID; cannot count reliably')
                if identity not in seen:
                    seen.add(identity)
                    state['boundaries'].append(identity)
            cursor['offset'] = stream.tell()
    if len(seen) >= state['threshold'] and state['phase'] == 'active':
        state['phase'] = 'requested'
        state['request'] = {
            'id': f"rotation-{state['epoch']}-{state['boundaries'][state['threshold'] - 1]}",
            'requested_at': now(), 'from_session': state['owner'],
        }


def require_owner(state, session):
    if not state or state['owner'] != session:
        raise ValueError('session does not own coordinator lease')


def paused(root):
    path = root / '.voiceport/loop-state.json'
    return path.exists() and load(path).get('paused', False)


def stop_receipt(transcript, session):
    """Require a successful loop-stop after the last scheduling call in this session."""
    calls = {}
    last = None
    success = None
    with Path(transcript).open() as source:
        for line in source:
            record = json.loads(line)
            if record.get('sessionId') != session:
                continue
            content = record.get('message', {}).get('content', [])
            if not isinstance(content, list):
                continue
            for item in content:
                if item.get('type') == 'tool_use' and item.get('name', '').split('.')[-1] == 'ScheduleWakeup':
                    last = item['id']
                    calls[last] = item.get('input', {}).get('stop') is True
                    success = None
                if (item.get('type') == 'tool_result' and item.get('tool_use_id') == last
                        and calls.get(last) and not item.get('is_error')):
                    success = {'tool_use_id': last, 'result': item.get('content')}
    if not success:
        raise ValueError('call ScheduleWakeup(stop=true) successfully before retirement')
    return success


def handoff_snapshot(root, state):
    registry = load(root / '.voiceport/coordination.json')
    ledger = load(root / '.voiceport/loop-state.json')
    # Existing registry is the current role/freeze source of truth; reject an oversized packet.
    packet = {
        'schema_version': 1, 'request_id': state['request']['id'], 'created_at': now(),
        'run_id': state['run_id'], 'epoch': state['epoch'], 'from_session': state['owner'],
        'goal': state['goal'], 'loop_prompt': state['loop_prompt'],
        'paused': ledger['paused'], 'coordination': registry,
        'tasks': {key: {'status': value['status'],
                        'attempt': value['attempts'][-1]['id'] if value['attempts'] else None}
                  for key, value in ledger['tasks'].items()},
        'references': ['docs/LOOP_METHODS.md', 'automation/loops/coordinator.md',
                       'docs/SOLUTION.md', 'task_plan.md', 'progress.md', 'findings.md',
                       '.voiceport/roles.md', '.voiceport/coordination.json'],
        'git_delivery_policy': 'User authorized per-stage logical commits and push, PR creation/update and merge subject to repository checks, then fetch and pull --ff-only latest main at a safe writer/freeze boundary. Preserve known failures and distinguish WIP integration from product acceptance. Carry unfinished Git steps across handoff; never accumulate multiple rounds of uncommitted work. See docs/LOOP_METHODS.md section 6.',
        'constraints': 'Preserve workers, attempt and freeze. No real recording, credentials, upload or target input without explicit user authorization. Developer checks are not independent acceptance. Read bounded current sections, not the entire historical transcript.',
    }
    raw = json.dumps(packet, ensure_ascii=False, indent=2).encode()
    if len(raw) > 48000:
        raise ValueError('handoff exceeds 48KB; shorten private current coordination registry first')
    return packet


def adopt(root, session, run_id, transcript, threshold, goal, loop_prompt, retired):
    if threshold < 1:
        raise ValueError('threshold must be positive')
    binding = bound_run(root, run_id)
    with transaction(root) as state:
        if state is not None:
            raise ValueError('already initialized; use the existing transfer')
        value = {'schema_version': 1, 'owner': session, 'epoch': 1, 'run_id': run_id,
                 'phase': 'active', 'threshold': threshold, 'boundaries': [],
                 'retired_sessions': retired, 'request': None, 'history': [],
                 'goal': goal, 'loop_prompt': loop_prompt, 'binding_receipt': binding}
        scan(value, transcript)
        atomic(root / '.voiceport' / STATE_NAME, value)
    return value


def retire(root, session, transcript):
    proof = stop_receipt(transcript, session)
    with transaction(root) as state:
        require_owner(state, session)
        if state['phase'] not in ('requested', 'retired'):
            raise ValueError('rotation has not been requested')
        if state['phase'] == 'retired':
            return state['request']
        packet = handoff_snapshot(root, state)
        path = root / '.voiceport/handoffs' / (state['request']['id'] + '.json')
        atomic(path, packet)
        state['request'].update(packet=str(path.relative_to(root)),
                                packet_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                                loop_stop=proof, retired_at=now())
        state['phase'] = 'retired'
        state['retired_sessions'].append(session)
        return state['request']


def launch(root, session):
    with transaction(root) as state:
        require_owner(state, session)
        if state['phase'] != 'retired' or state['request'].get('launch_started_at'):
            raise ValueError('launch already attempted or source not retired; inspect receipt, never launch twice')
        request = state['request'].copy()
        token = uuid.uuid4().hex
        state['request'].update(token=token, launch_started_at=now())
        state['phase'] = 'launching'
        prompt = (f"You are the successor VoicePort coordinator. Read .claude/COORDINATOR_ROTATION.md and "
                  f"{request['packet']}. Transfer token: {token}. Source is retired and loop stopped. "
                  f"Before product writes, use the orchestration skill, bind existing Run {state['run_id']} "
                  f"with run-use --id {state['run_id']} from YOUR terminal (use --takeover-legacy only for an automatically adopted legacy Run), then run python3 -I {SCRIPT} claim "
                  f"--session YOUR_ACTUAL_SESSION_ID --token {token}. "
                  "Do not create a Run or replace workers. Confirm binding and claim before consuming mail. "
                  "Honor persistent pause. If allowed work remains, explicitly start /loop with the packet's "
                  "original loop_prompt; scheduled loops do not transfer automatically. Then handle current work. "
                  "If binding/claim fails, end without writing product state or scheduling.")
        launch_argv = ['agent', 'launch', '--agent', 'claude',
                       '--worktree', 'path:' + str(root), '--prompt', prompt]
        state['request']['launch_argv'] = launch_argv
    try:
        receipt = invoke(root, launch_argv)
    except (ValueError, OSError, subprocess.TimeoutExpired) as error:
        with transaction(root) as state:
            state['request']['launch_error'] = str(error)
        raise ValueError('launch outcome may be unknown; inspect Orca and use its replay ID, no fresh launch') from error
    delivered = receipt.get('result', {}).get('launch', {}).get('prompt', {}).get('outcome') in (
        'journaled', 'handed-to-terminal')
    with transaction(root) as state:
        record = state['request'] if state['request']['id'] == request['id'] else next(
            item for item in state['history'] if item['id'] == request['id'])
        record.update(launch_receipt=receipt, delivery_confirmed=delivered)
    if not delivered:
        raise ValueError('launch receipt does not prove prompt delivery; inspect existing successor, do not launch again')
    return receipt


def configure(root, session, threshold):
    if threshold < 1:
        raise ValueError('threshold must be positive')
    with transaction(root) as state:
        require_owner(state, session)
        if state['phase'] != 'active':
            raise ValueError('cannot change threshold during a transfer')
        state['threshold'] = threshold
        return {'threshold': threshold, 'effective': 'next hook boundary'}


def claim(root, session, token):
    with transaction(root) as state:
        if (not state or state['phase'] != 'launching'
                or state['request'].get('token') != token
                or session in state['retired_sessions']):
            raise ValueError('invalid/stale transfer token or session')
        run_id = state['run_id']
    binding = bound_run(root, run_id)
    with transaction(root) as state:
        if state['phase'] != 'launching' or state['request'].get('token') != token:
            raise ValueError('transfer already claimed')
        packet = root / state['request']['packet']
        if hashlib.sha256(packet.read_bytes()).hexdigest() != state['request']['packet_sha256']:
            raise ValueError('handoff packet changed')
        state['history'].append({**state['request'], 'to_session': session, 'claimed_at': now()})
        state.update(owner=session, epoch=state['epoch'] + 1, phase='active',
                     boundaries=[], binding_receipt=binding)
        state.pop('transcript_cursor', None)
        # Keep launch request until its receipt is recorded; it is also in history.
        return {'owner': session, 'epoch': state['epoch'], 'paused': paused(root)}


def rotation_hint(state):
    return (f"Coordinator reached {len(state['boundaries'])}/{state['threshold']} completed compactions. "
            f"Finish the current tool boundary, call ScheduleWakeup(stop=true), then Bash: "
            f"python3 -I {SCRIPT} retire --session {state['owner']} --transcript YOUR_TRANSCRIPT_PATH; "
            f"then python3 -I {SCRIPT} launch --session {state['owner']}. "
            "End this coordinator turn after launch receipt. Preserve workers/freeze; do not start new work.")


def allowed_during_rotation(payload, state):
    name = payload.get('tool_name', '').split('.')[-1]
    data = payload.get('tool_input', {})
    if name in ('Read', 'Skill'):
        return True
    if name == 'ScheduleWakeup':
        return data.get('stop') is True
    if name == 'Bash':
        try:
            argv = shlex.split(data.get('command', ''))
        except ValueError:
            return False
        # Only a single helper command, with no shell operators/substitution.
        command = data.get('command', '')
        return (len(argv) >= 4 and argv[:3] == ['python3', '-I', SCRIPT]
                and argv[3] in ('retire', 'launch', 'status')
                and not any(c in command for c in (';', '\n', '`', '$', '|', '&', '>', '<')))
    return False


def hook(root, payload):
    event = payload.get('hook_event_name')
    session = payload.get('session_id')
    with transaction(root) as state:
        if not state:
            return {}
        if session in state['retired_sessions']:
            if event == 'PreToolUse' and not allowed_during_rotation(payload, state):
                return {'hookSpecificOutput': {'hookEventName': event, 'permissionDecision': 'deny',
                        'permissionDecisionReason': 'This coordinator retired. Do not write, dispatch, consume mail or restart its loop.'}}
            return {}
        if session != state['owner']:
            return {}  # Developer/verifier sessions retain their own contracts.
        transcript = payload.get('transcript_path')
        if transcript:
            scan(state, transcript)
        if state['phase'] == 'active':
            return {}
        hint = rotation_hint(state)
        if event == 'PreToolUse' and not allowed_during_rotation(payload, state):
            return {'hookSpecificOutput': {'hookEventName': event, 'permissionDecision': 'deny',
                    'permissionDecisionReason': hint}}
        if event == 'SessionStart':
            return {'hookSpecificOutput': {'hookEventName': event, 'additionalContext': hint}}
        if event == 'Stop' and state['phase'] == 'requested' and not payload.get('stop_hook_active'):
            return {'decision': 'block', 'reason': hint}
        return {}  # PostCompact has no decision control. Next tool reconciles delayed JSONL writes.


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    sub = parser.add_subparsers(dest='action', required=True)
    sub.add_parser('hook')
    sub.add_parser('status')
    for action in ('adopt', 'retire', 'launch', 'claim', 'configure'):
        cmd = sub.add_parser(action)
        cmd.add_argument('--session', required=True)
        if action in ('adopt', 'retire'):
            cmd.add_argument('--transcript', required=True)
        if action == 'adopt':
            cmd.add_argument('--run', required=True)
            cmd.add_argument('--threshold', type=int, default=2)
            cmd.add_argument('--goal', required=True)
            cmd.add_argument('--loop-prompt', required=True)
            cmd.add_argument('--retired-session', action='append', default=[])
        if action == 'configure':
            cmd.add_argument('--threshold', type=int, required=True)
        if action == 'claim':
            cmd.add_argument('--token', required=True)
    args = parser.parse_args()
    try:
        if args.action == 'hook':
            payload = json.load(sys.stdin)
            value = hook(args.root, payload)
        elif args.action == 'status':
            state = load(args.root / '.voiceport' / STATE_NAME)
            value = {key: state[key] for key in ('owner', 'epoch', 'phase', 'threshold', 'boundaries')}
        elif args.action == 'adopt':
            value = adopt(args.root, args.session, args.run, args.transcript, args.threshold,
                          args.goal, args.loop_prompt, args.retired_session)
            value = {key: value[key] for key in ('owner', 'epoch', 'phase', 'threshold')}
        elif args.action == 'retire':
            value = retire(args.root, args.session, args.transcript)
        elif args.action == 'launch':
            value = launch(args.root, args.session)
        elif args.action == 'configure':
            value = configure(args.root, args.session, args.threshold)
        else:
            value = claim(args.root, args.session, args.token)
        print(json.dumps(value, ensure_ascii=False))
    except (ValueError, OSError, KeyError, subprocess.TimeoutExpired) as error:
        if args.action == 'hook':
            print(f'Coordinator rotation hook failed: {error}', file=sys.stderr)
            # Block tools on broken state instead of silently bypassing fencing.
            sys.exit(2 if locals().get('payload', {}).get('hook_event_name') == 'PreToolUse' else 1)
        parser.exit(1, f'coordinator-rotation: {error}\n')


if __name__ == '__main__':
    main()
