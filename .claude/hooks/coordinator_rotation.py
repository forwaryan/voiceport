#!/usr/bin/env python3
"""VoicePort coordinator lifecycle. Private state; no product ledger mutations."""
import argparse
from contextlib import contextmanager, ExitStack
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import stat
import subprocess
import sys
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[2]
STATE_NAME = 'coordinator-rotation.json'
SCRIPT = '.claude/hooks/coordinator_rotation.py'
DEFAULT_THRESHOLD = 5
HOOK_EVENTS = ('SessionStart', 'PreToolUse', 'PreCompact', 'PostCompact', 'Stop')
FINAL_ROUND_POLICY = ('All roles have at most five context rounds: the initial context plus at most four '
                      'completed compactions. On entering the final round, save and hand off immediately '
                      'after the current tool returns; veto further compaction. Never force compaction or '
                      'add stress tests. Preserve the Run, product attempt, role lineage and evidence; '
                      'verify a genuinely new session, not merely a new terminal handle.')


def validate_threshold(threshold):
    if not 1 <= threshold <= DEFAULT_THRESHOLD:
        raise ValueError('context-round limit must be between 1 and 5')


def request_rotation(state):
    if len(state['boundaries']) >= state['threshold'] - 1 and state['phase'] == 'active':
        state['phase'] = 'requested'
        boundary = state['boundaries'][state['threshold'] - 2] if state['threshold'] > 1 else 'initial'
        state['request'] = {
            'id': f"rotation-{state['epoch']}-{boundary}",
            'requested_at': now(), 'from_session': state['owner'],
        }


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


def coordinator_path(root, run_id=None):
    if run_id is None:
        return root / '.voiceport' / STATE_NAME
    if not isinstance(run_id, str) or not re.fullmatch(r'run_[A-Za-z0-9_-]{1,128}', run_id):
        raise ValueError('invalid Run identity')
    return root / '.voiceport/run-rotations' / (run_id + '.json')


@contextmanager
def transaction(root, run_id=None):
    path = coordinator_path(root, run_id)
    directory = path.parent
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    with path.with_suffix('.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        state = load(path) if path.exists() else None
        before = json.dumps(state, sort_keys=True)
        yield state
        if state is not None and json.dumps(state, sort_keys=True) != before:
            atomic(path, state)


class InvocationError(ValueError):
    def __init__(self, message, observation):
        super().__init__(message)
        self.observation = observation


def failure_observation(error):
    if isinstance(error, InvocationError):
        return error.observation
    return {'type': type(error).__name__, 'error': str(error),
            'stdout': str(getattr(error, 'stdout', None)), 'stderr': str(getattr(error, 'stderr', None))}


def invoke(root, argv):
    result = subprocess.run(['orca', *argv, '--json'], cwd=root, text=True,
                            capture_output=True, timeout=45)
    observation = {'argv': argv, 'exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}
    try:
        receipt = json.loads(result.stdout)
    except ValueError as error:
        raise InvocationError('Orca returned no valid receipt; inspect runtime before retrying', observation) from error
    if (not isinstance(receipt, dict) or result.returncode or receipt.get('ok') is not True
            or not isinstance(receipt.get('result'), dict)):
        raise InvocationError('Orca refused: ' + json.dumps(receipt, ensure_ascii=False)[:2500], observation)
    return receipt


def receipt_field(receipt, *keys):
    value = receipt
    for key in keys:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def launch_delivery_status(receipt):
    """Read the installed agent-launch envelope; delivery alone proves no PTY."""
    if not isinstance(receipt, dict):
        return 'malformed'
    if receipt.get('ok') is False or receipt.get('error') is not None:
        return 'error'
    if receipt.get('ok') is not True or not isinstance(receipt.get('result'), dict):
        return 'malformed'
    result = receipt['result']
    if result.get('error') is not None:
        return 'error'
    if 'prompt' not in result:
        return 'missing'
    prompt = result['prompt']
    if not isinstance(prompt, dict):
        return 'malformed'
    if prompt.get('error') is not None:
        return 'error'
    if 'outcome' not in prompt:
        return 'missing'
    outcome = prompt['outcome']
    if not isinstance(outcome, str):
        return 'malformed'
    if 'delivery' not in prompt:
        return 'missing'
    if not isinstance(prompt['delivery'], str):
        return 'malformed'
    # The installed CLI sends --prompt as delivery=submit. Draft/unknown
    # delivery cannot authorize the submitted successor handoff.
    if prompt['delivery'] != 'submit':
        return 'unknown'
    if outcome == 'journaled':
        if 'messageId' not in prompt:
            return 'missing'
        if not isinstance(prompt['messageId'], str):
            return 'malformed'
    return outcome if outcome in ('journaled', 'handed-to-terminal') else 'unknown'


def terminal_launch_delivery_status(receipt):
    status = launch_delivery_status(receipt)
    if status in ('journaled', 'handed-to-terminal'):
        outcome = receipt_field(receipt, 'result', 'outcome')
        if (not isinstance(outcome, dict) or outcome.get('kind') != 'terminal'
                or not isinstance(outcome.get('handle'), str) or not outcome['handle']):
            return 'non-terminal'
    return status


def update_transfer(state, request_id, **updates):
    """Keep late launch/close receipts when the live request moves to the next epoch."""
    records = [state['request'], *state['history']]
    for record in records:
        if record.get('id') == request_id:
            record.update(updates)


def bound_run(root, expected):
    receipt = invoke(root, ['orchestration', 'run-current', '--full'])
    run = receipt.get('result', {}).get('run') or {}
    if (run.get('id') or run.get('runId')) != expected:
        raise ValueError('caller is not bound to the expected Run')
    return receipt


def coordinator_identity(root, run_id, terminal):
    """Explicit administrative verification, never a Run takeover from a hook."""
    if not terminal or os.environ.get('ORCA_TERMINAL_HANDLE') != terminal:
        raise ValueError('coordinator caller terminal mismatch')
    binding = bound_run(root, run_id)
    receipt = invoke(root, ['orchestration', 'run-show', '--id', run_id, '--full'])
    run = receipt.get('result', {}).get('run') or {}
    if (run.get('id') != run_id or run.get('coordinator_handle') != terminal
            or type(run.get('consumer_generation')) is not int):
        raise ValueError('caller is not the current Run coordinator')
    terminal_receipt = invoke(root, ['terminal', 'show', '--terminal', terminal])
    info = terminal_receipt.get('result', {}).get('terminal') or {}
    if (info.get('handle') != terminal or info.get('worktreePath') != str(root.resolve())
            or info.get('connected') is not True or info.get('orphaned') is not False
            or not info.get('incarnationId')):
        raise ValueError('coordinator terminal receipt does not match this worktree')
    return {'binding': binding, 'run': receipt, 'terminal': terminal_receipt}


def authentic_session(root, session, fresh_after=None):
    """Read actual hook-produced authority; registration never fabricates startup."""
    proof = load(session_path(root, session))
    validate_session_state(proof, session)
    require_terminal(proof)
    if (proof.get('transcript_session_verified') is not True
            or 'PreToolUse' not in proof.get('hook_events', [])
            or proof.get('hook_implementation_sha256') != hashlib.sha256(Path(__file__).read_bytes()).hexdigest()):
        raise ValueError('authentic current PreToolUse proof required')
    scan(proof, proof['transcript_cursor']['path'])
    if fresh_after is not None:
        if (proof['boundaries'] or proof['phase'] != 'active' or not proof.get('transcript_started_at')
                or datetime.fromisoformat(proof['transcript_started_at']) <= datetime.fromisoformat(fresh_after)
                or datetime.fromisoformat(proof['created_at']) <= datetime.fromisoformat(fresh_after)):
            raise ValueError('successor must be a fresh initial0 session created after launch')
    return proof


def navigation_snapshot(root, relative, run_id):
    path = root / relative
    if (path.parent != root / '.voiceport/handoffs' or path.suffix != '.json'
            or path.is_symlink() or path.resolve().parent != (root / '.voiceport/handoffs').resolve()):
        raise ValueError('navigation must be a regular private handoff JSON file')
    with handoff_directory(root) as directory:
        fd = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        with os.fdopen(fd, 'rb') as source:
            before = os.fstat(source.fileno())
            if not stat.S_ISREG(before.st_mode):
                raise ValueError('navigation must be a regular file')
            raw = source.read(48001)
            after = os.fstat(source.fileno())
        current = os.stat(path.name, dir_fd=directory, follow_symlinks=False)
        if file_signature(before) != file_signature(after) or file_signature(before) != file_signature(current):
            raise ValueError('navigation changed during snapshot')
    if len(raw) > 48000:
        raise ValueError('navigation exceeds 48KB')
    value = json.loads(raw)
    if (not isinstance(value, dict) or value.get('run_id') != run_id
            or not isinstance(value.get('product_task_id'), str)
            or not isinstance(value.get('product_attempt_id'), str)):
        raise ValueError('navigation Run or product lineage mismatch')
    return value, hashlib.sha256(raw).hexdigest()


def register(root, session, run_id, transcript, goal, loop_prompt, navigation):
    """Register only an already-bound real coordinator; leave other leases untouched."""
    coordinator_path(root, run_id)
    navigation_snapshot(root, navigation, run_id)
    # All combined locks use Run then session order, including claim and hooks.
    with transaction(root, run_id) as existing:
        if existing is not None:
            raise ValueError('Run rotation already registered; use its existing lifecycle')
        with session_transaction(root, session) as proof:
            verified = authentic_session(root, session)
            if (str(Path(transcript).resolve()) != str(Path(verified['transcript_cursor']['path']).resolve())
                    or proof.get('coordinator_run') not in (None, run_id)):
                raise ValueError('session transcript or existing Run registration mismatch')
            identity = coordinator_identity(root, run_id, proof['terminal'])
            legacy = coordinator_path(root)
            if legacy.exists() and load(legacy).get('run_id') == run_id:
                raise ValueError('Run already has a legacy lease; use that lifecycle')
            value = {'schema_version': 1, 'owner': session, 'terminal': proof['terminal'],
                     'epoch': 1, 'run_id': run_id, 'phase': 'active', 'threshold': proof['threshold'],
                     'boundaries': list(verified['boundaries']), 'retired_sessions': [], 'history': [],
                     'request': None, 'goal': goal, 'loop_prompt': loop_prompt, 'navigation': navigation,
                     'identity_receipt': identity, 'harness': proof['harness']}
            request_rotation(value)
            # Publish marker last; a crash cannot route a worker to a partial lease.
            atomic(coordinator_path(root, run_id), value)
            proof['coordinator_run'] = run_id
    return value


def upgrade_legacy(root, session, run_id):
    """Carry an idle legacy lease into the verified per-Run lifecycle without reset."""
    coordinator_path(root, run_id)
    with transaction(root) as legacy:
        require_owner(legacy, session)
        validate_hook_accounting(legacy)
        if legacy.get('run_id') != run_id or legacy['phase'] not in ('active', 'requested') or legacy.get('migrated_run'):
            raise ValueError('legacy upgrade requires its current owner/Run and no attempted transfer')
        with transaction(root, run_id) as existing:
            if existing is not None:
                raise ValueError('Run already has a lease; reconcile before legacy upgrade')
            with session_transaction(root, session) as proof:
                verified = authentic_session(root, session)
                if proof.get('standalone') or proof.get('coordinator_run'):
                    raise ValueError('session already has another lifecycle')
                identity = coordinator_identity(root, run_id, proof['terminal'])
                registry = load(root / '.voiceport/coordination.json')
                ledger = load(root / '.voiceport/loop-state.json')
                if registry.get('run_id') != run_id:
                    raise ValueError('legacy navigation belongs to another Run')
                running = [(key, task) for key, task in ledger['tasks'].items() if task['status'] == 'running']
                if len(running) > 1:
                    raise ValueError('legacy product lineage is ambiguous')
                task, attempt = 'unbound', 'unbound'
                if running:
                    task, row = running[0]
                    if not row.get('attempts') or not isinstance(row['attempts'][-1].get('id'), str):
                        raise ValueError('legacy running product has no current attempt')
                    attempt = row['attempts'][-1]['id']
                navigation = f'.voiceport/handoffs/legacy-navigation-{session}.json'
                if (root / navigation).exists() or (root / navigation).is_symlink():
                    raise ValueError('legacy navigation already exists; inspect partial upgrade before retry')
                atomic(root / navigation, {**registry, 'product_task_id': task, 'product_attempt_id': attempt})
                value = {**legacy, 'terminal': proof['terminal'], 'identity_receipt': identity,
                         'navigation': navigation, 'harness': proof['harness']}
                value['boundaries'] = list(dict.fromkeys([*legacy['boundaries'], *verified['boundaries']]))
                request_rotation(value)
                # Save the new lease and legacy fence before publishing the marker.
                atomic(coordinator_path(root, run_id), value)
                legacy['migrated_run'] = run_id
                atomic(coordinator_path(root), legacy)
                proof['coordinator_run'] = run_id
    return {'run': run_id, 'owner': session, 'phase': value['phase'], 'boundaries': value['boundaries']}


def validate_run_state(root, state, run_id):
    validate_hook_accounting(state)
    try:
        run = state['identity_receipt']['run']['result']['run']
        terminal = state['identity_receipt']['terminal']['result']['terminal']
        valid = (state['run_id'] == run_id and run['id'] == run_id
                 and run['coordinator_handle'] == state['terminal']
                 and type(run['consumer_generation']) is int
                 and terminal['handle'] == state['terminal']
                 and terminal['worktreePath'] == str(root.resolve())
                 and isinstance(terminal['incarnationId'], str) and bool(terminal['incarnationId'])
                 and isinstance(state['navigation'], str)
                 and isinstance(state['goal'], str) and isinstance(state['loop_prompt'], str)
                 and state['phase'] in ('active', 'requested', 'retired', 'launching')
                 and isinstance(state['retired_sessions'], list)
                 and all(isinstance(s, str) and bool(s) for s in state['retired_sessions'])
                 and isinstance(state['history'], list)
                 and all(isinstance(item, dict) for item in state['history'])
                 and (state['request'] is None or isinstance(state['request'], dict))
                 and (state['phase'] == 'active' or isinstance(state['request'], dict)))
    except (KeyError, TypeError, AttributeError) as error:
        raise ValueError('invalid cached Run coordinator identity') from error
    if not valid:
        raise ValueError('invalid cached Run coordinator identity')


def require_coordinator(root, state, session, run_id):
    require_owner(state, session)
    if state.get('migrated_run'):
        raise ValueError('legacy lease was upgraded; use its per-Run lifecycle with --run')
    if run_id is not None:
        validate_run_state(root, state, run_id)
        proof = authentic_session(root, session)
        if proof.get('coordinator_run') != run_id or proof['terminal'] != state.get('terminal'):
            raise ValueError('caller has no registered Run coordinator identity')
        identity = coordinator_identity(root, run_id, proof['terminal'])
        cached = state['identity_receipt']
        if (identity['run']['result']['run']['consumer_generation'] != cached['run']['result']['run']['consumer_generation']
                or identity['terminal']['result']['terminal']['incarnationId'] != cached['terminal']['result']['terminal']['incarnationId']):
            raise ValueError('registered coordinator generation or incarnation changed')
        return proof


def verify_transcript_prefix(stream, path, size, expected_digest):
    """Verify the scanned prefix with fresh reads; append growth needs no retry.

    pread bypasses BufferedReader's possibly stale read-ahead cache. Every byte
    of the initial snapshot, including a partial last record, must still match.
    Later bytes belong to the next hook's full scan; we never chase a growing EOF.
    """
    actual = hashlib.sha256()
    offset = 0
    while offset < size:
        chunk = os.pread(stream.fileno(), min(1024 * 1024, size - offset), offset)
        if not chunk:
            raise ValueError('transcript shortened during scan; retry from its current contents')
        actual.update(chunk)
        offset += len(chunk)
    opened = os.fstat(stream.fileno())
    current = path.stat()
    if (actual.digest() != expected_digest or opened.st_size < size or current.st_size < size
            or (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino)):
        raise ValueError('transcript changed during scan; retry from its current contents')


def copy_scheduler(state):
    value = state.get('scheduler', {'owned': {}, 'deleted': {}, 'pending': {}, 'dynamic': {}})
    if (not isinstance(value, dict) or set(value) not in ({'owned', 'deleted', 'pending'}, {'owned', 'deleted', 'pending', 'dynamic'})
            or any(not isinstance(value[k], dict) for k in value)
            or any(not isinstance(key, str) or not isinstance(row, dict)
                   for rows in value.values() for key, row in rows.items())):
        raise ValueError('invalid saved scheduler provenance')
    if any(row.get('name') not in ('CronCreate', 'CronDelete') or not isinstance(row.get('input'), dict)
           for row in value['pending'].values()):
        raise ValueError('invalid saved scheduler mutation')
    if state.get('owner') and any(row.get('owner') != state['owner'] for row in value['owned'].values()):
        raise ValueError('saved scheduler belongs to another owner')
    result = json.loads(json.dumps(value))
    result.setdefault('dynamic', {})
    if any(type(row.get('order')) is not int or row['order'] < 1 or type(row.get('stop')) is not bool
           or (row.get('receipt') is not None and not isinstance(row['receipt'], dict))
           or not isinstance(row.get('after', []), list)
           or any(not isinstance(key, str) for key in row.get('after', []))
           for row in result['dynamic'].values()):
        raise ValueError('invalid saved dynamic scheduler provenance')
    for row in result['dynamic'].values():
        row.setdefault('after', [])
        if row.get('timestamp') is not None:
            validate_saved_timestamp(row['timestamp'])
    return result


def track_scheduler(scheduler, calls, row, session):
    """Persist confirmed job ownership and unresolved mutations across truncation."""
    if row.get('sessionId') != session:
        return
    content = row.get('message', {}).get('content', [])
    if not isinstance(content, list):
        return
    for item in content:
        if not isinstance(item, dict):
            continue
        name = item.get('name', '').split('.')[-1]
        if item.get('type') == 'tool_use' and name == 'ScheduleWakeup':
            dynamic = scheduler['dynamic']
            seen = calls.setdefault('_dynamic_seen', [])
            timestamp = row.get('timestamp')
            if timestamp is not None:
                validate_saved_timestamp(timestamp)
            if item['id'] not in dynamic:
                dynamic[item['id']] = {'order': max((v['order'] for v in dynamic.values()), default=0) + 1,
                                       'stop': item.get('input', {}).get('stop') is True, 'receipt': None,
                                       'timestamp': timestamp, 'after': []}
            entry = dynamic[item['id']]
            earlier = [key for key, value in dynamic.items() if timestamp and value.get('timestamp')
                       and datetime.fromisoformat(value['timestamp']) < datetime.fromisoformat(timestamp)]
            entry['after'] = sorted(set(entry['after']) | set(seen) | set(earlier))
            seen.append(item['id'])
        if item.get('type') == 'tool_result' and item.get('tool_use_id') in scheduler['dynamic']:
            result = row.get('toolUseResult')
            scheduler['dynamic'][item['tool_use_id']]['receipt'] = (result if not item.get('is_error') and isinstance(result, dict) else None)
        if item.get('type') == 'tool_use' and name in ('CronCreate', 'CronDelete'):
            value = {'name': name, 'input': item.get('input', {})}
            calls[item['id']] = value
            scheduler['pending'][item['id']] = value
        call_id = item.get('tool_use_id')
        call = calls.get(call_id) or scheduler['pending'].get(call_id)
        if item.get('type') != 'tool_result' or not call:
            continue
        if item.get('is_error'):
            scheduler['pending'].pop(call_id, None)
            continue
        result = row.get('toolUseResult')
        if not isinstance(result, dict):
            continue
        data, name = call['input'], call['name']
        job = result.get('id')
        if name == 'CronCreate' and isinstance(job, str) and job:
            scheduler['owned'][job] = {'owner': session, 'prompt': data.get('prompt'), 'cron': data.get('cron'),
                                       'tool_use_id': call_id, 'receipt': result}
            scheduler['pending'].pop(call_id, None)
        elif name == 'CronDelete' and job == data.get('id') and job in scheduler['owned']:
            scheduler['deleted'][job] = {'tool_use_id': call_id, 'receipt': result}
            scheduler['pending'].pop(call_id, None)


def scan(state, transcript):
    """Rescan a fixed byte snapshot of complete JSONL records on every hook.

    O(transcript bytes) parsing plus a fresh prefix verification deliberately
    avoids trusting an append-only cursor or inode/size metadata. Counts remain
    a monotonic union after replacement/truncation. Append-only growth is safe:
    bytes past the initial size and a partial final record are deferred to the
    next full scan. A rewritten prefix, replacement or missing identity fails
    closed. The cursor is diagnostic only; a quiescent EOF is not required.
    """
    path = Path(transcript)
    seen = set(state['boundaries'])
    added = []
    verified = False
    started = state.get('transcript_started_at')
    offset = 0
    scheduler_calls = {}
    scheduler = copy_scheduler(state)
    with path.open('rb') as stream:
        before = os.fstat(stream.fileno())
        snapshot_digest = hashlib.sha256()
        while stream.tell() < before.st_size:
            line = stream.readline(before.st_size - stream.tell())
            snapshot_digest.update(line)
            if not line.endswith(b'\n'):
                break
            record = json.loads(line)
            if not isinstance(record, dict):
                raise ValueError('transcript record must be an object')
            identity = None
            track_scheduler(scheduler, scheduler_calls, record, state['owner'])
            if state.get('harness') == 'codex':
                native = record.get('payload')
                if record.get('type') in ('session_meta', 'compacted') and not isinstance(native, dict):
                    raise ValueError('Codex identity/compacted payload must be an object')
                if record.get('type') == 'session_meta':
                    if native.get('id') != state['owner']:
                        raise ValueError('Codex transcript belongs to another session')
                    verified = True
                if record.get('type') == 'compacted':
                    if not verified:
                        raise ValueError('Codex transcript has no verified session_meta')
                    window = native.get('window_id')
                    if not isinstance(window, str) or not window:
                        raise ValueError('Codex compacted record has no window_id; refuse to guess count')
                    identity = 'codex:' + window
            else:
                if 'sessionId' in record:
                    if record['sessionId'] != state['owner']:
                        raise ValueError('Claude transcript belongs to another session')
                    verified = True
                if record.get('subtype') == 'compact_boundary':
                    if record.get('sessionId') != state['owner']:
                        raise ValueError('Claude boundary has no matching session identity')
                    identity = record.get('uuid')
                    if not isinstance(identity, str) or not identity:
                        raise ValueError('compaction boundary has no UUID; cannot count reliably')
            timestamp = None
            if state.get('harness') == 'codex' and record.get('type') == 'session_meta':
                timestamp = record.get('timestamp') or native.get('timestamp')
            elif state.get('harness') != 'codex' and record.get('sessionId') == state['owner']:
                timestamp = record.get('timestamp')
            if timestamp is not None:
                validate_saved_timestamp(timestamp)
                if started is None or datetime.fromisoformat(timestamp) < datetime.fromisoformat(started):
                    started = timestamp
            if identity and identity not in seen:
                seen.add(identity)
                added.append(identity)
            offset = stream.tell()
        if stream.tell() != before.st_size:
            raise ValueError('transcript shortened during scan; retry from its current contents')
        verify_transcript_prefix(stream, path, before.st_size, snapshot_digest.digest())
    if not verified:
        required = 'session_meta' if state.get('harness') == 'codex' else 'sessionId'
        raise ValueError('transcript has no verified ' + required)
    state['boundaries'].extend(added)
    if any(scheduler.values()) or 'scheduler' in state:
        state['scheduler'] = scheduler
    if started is not None:
        state['transcript_started_at'] = started
    state['transcript_session_verified'] = True
    state['transcript_cursor'] = {'path': str(path), 'offset': offset, 'inode': before.st_ino}
    request_rotation(state)


def require_owner(state, session):
    if not state or state['owner'] != session:
        raise ValueError('session does not own coordinator lease')


def paused(root):
    path = root / '.voiceport/loop-state.json'
    return path.exists() and load(path).get('paused', False)


def scheduler_view(transcript, session, provenance=None):
    """Correlate installed Claude toolUseResult receipts, never a submitted call alone.

    Native Codex has no verified scheduler in the installed CLI. Do not guess its
    function names or turn arbitrary function_call_output text into stop authority.
    """
    saved = copy_scheduler({'scheduler': provenance}) if provenance is not None else copy_scheduler({})
    calls = {key: (row['name'], row['input'], -1) for key, row in saved['pending'].items()}
    owned, deleted, pending = saved['owned'], saved['deleted'], set(saved['pending'])
    dynamic, last_dynamic, last_list, mutation = None, None, None, 0
    native = False
    tracker_calls = {}
    path = Path(transcript)
    with path.open('rb') as stream:
        size = os.fstat(stream.fileno()).st_size
        raw = stream.read(size)
        for index, line in enumerate(raw.splitlines(keepends=True)):
            if not line.endswith(b'\n'):
                continue
            row = json.loads(line)
            track_scheduler(saved, tracker_calls, row, session)
            if row.get('type') == 'session_meta':
                if row.get('payload', {}).get('id') != session:
                    raise ValueError('scheduler transcript belongs to another session')
                native = True
            if row.get('sessionId') != session:
                continue
            content = row.get('message', {}).get('content', [])
            if not isinstance(content, list):
                continue
            for item in content:
                if not isinstance(item, dict):
                    continue
                name = item.get('name', '').split('.')[-1]
                if item.get('type') == 'tool_use' and name in ('ScheduleWakeup', 'CronCreate', 'CronDelete', 'CronList'):
                    call_id, data = item['id'], item.get('input', {})
                    calls[call_id] = (name, data, index)
                    if name in ('CronCreate', 'CronDelete'):
                        mutation = index
                        pending.add(call_id)
                    if name == 'ScheduleWakeup':
                        last_dynamic, dynamic = call_id, None
                    continue
                call_id = item.get('tool_use_id')
                if item.get('type') != 'tool_result' or call_id not in calls:
                    continue
                name, data, position = calls[call_id]
                result = row.get('toolUseResult')
                if item.get('is_error'):
                    # A rejected create did not install anything; other failed or
                    # uncertain mutations cannot serve as a shutdown receipt.
                    if name in ('CronCreate', 'CronDelete'):
                        pending.discard(call_id)
                    continue
                if not isinstance(result, dict):
                    continue
                if name == 'CronCreate' and isinstance(result.get('id'), str) and result['id']:
                    owned[result['id']] = {'owner': session, 'prompt': data.get('prompt'),
                                          'cron': data.get('cron'), 'tool_use_id': call_id, 'receipt': result}
                    pending.discard(call_id)
                elif name == 'CronDelete' and result.get('id') == data.get('id') and data.get('id') in owned:
                    deleted[data['id']] = {'tool_use_id': call_id, 'receipt': result}
                    pending.discard(call_id)
                elif name == 'CronList' and isinstance(result.get('jobs'), list):
                    jobs = result['jobs']
                    if all(isinstance(j, dict) and isinstance(j.get('id'), str) for j in jobs):
                        last_list = {'position': position, 'tool_use_id': call_id, 'receipt': result}
                elif name == 'ScheduleWakeup' and call_id == last_dynamic and data.get('stop') is True and result.get('stopped') is True:
                    dynamic = {'tool_use_id': call_id, 'receipt': result, 'position': position}
        verify_transcript_prefix(stream, path, size, hashlib.sha256(raw).digest())
    if saved['dynamic']:
        last_dynamic = max(saved['dynamic'], key=lambda key: saved['dynamic'][key]['order'])
        # Observation order is not transcript order after a prefix is restored.
        # A stop must be proven later than every known call, by the same verified
        # snapshot or actual UTC call timestamps, including pruned pending calls.
        candidates = [(key, value) for key, value in saved['dynamic'].items()
                      if value['stop'] and (value.get('receipt') or {}).get('stopped') is True
                      and set(saved['dynamic']) - {key} <= set(value['after'])]
        dynamic = None
        if candidates:
            key, value = max(candidates, key=lambda pair: pair[1]['order'])
            dynamic = {'tool_use_id': key, 'receipt': value['receipt'], 'timestamp': value.get('timestamp')}
    return {'owned': owned, 'deleted': deleted, 'pending': sorted(pending), 'dynamic': dynamic,
            'last_dynamic': last_dynamic, 'list': last_list, 'mutation': mutation, 'native': native}


def stop_receipt(transcript, session, provenance=None):
    view = scheduler_view(transcript, session, provenance)
    if view['native']:
        raise ValueError('native Codex scheduler shutdown adapter is unverified; retirement blocked')
    if view['last_dynamic'] and not view['dynamic']:
        raise ValueError('call ScheduleWakeup(stop=true) successfully after latest scheduling call before retirement')
    if view['pending'] or set(view['owned']) - set(view['deleted']):
        raise ValueError('owned cron jobs have no confirmed CronDelete receipt; stop all before retirement')
    if view['owned']:
        listed = view['list']
        if (not listed or listed['position'] <= view['mutation']
                or set(view['owned']) & {j['id'] for j in listed['receipt']['jobs']}):
            raise ValueError('CronList must confirm all owned jobs absent after the last cron mutation')
    elif not view['dynamic']:
        raise ValueError('call ScheduleWakeup(stop=true) successfully before retirement')
    return {'kind': 'mixed' if view['owned'] and view['dynamic'] else 'cron' if view['owned'] else 'dynamic',
            'dynamic': view['dynamic'], 'cron_jobs': view['owned'], 'cron_deleted': view['deleted'],
            'cron_list': view['list']}


def scheduler_shutdown(payload, state):
    """Shutdown only; the host scopes dynamic stop/list to the actual caller."""
    name, data = payload.get('tool_name', '').split('.')[-1], payload.get('tool_input', {})
    if name == 'ScheduleWakeup':
        return data == {'stop': True}
    if name == 'CronList':
        return data == {}
    if name == 'CronDelete' and set(data) == {'id'} and isinstance(data['id'], str):
        cursor = state.get('transcript_cursor') or {}
        if cursor.get('path') and state.get('harness', 'claude') == 'claude':
            owned = scheduler_view(cursor['path'], payload['session_id'], state.get('scheduler'))['owned']
            return owned.get(data['id'], {}).get('owner') == payload['session_id']
    return False


def handoff_snapshot(root, state):
    if state.get('navigation'):
        registry, navigation_hash = navigation_snapshot(root, state['navigation'], state['run_id'])
    else:
        registry = load(root / '.voiceport/coordination.json')
        navigation_hash = None
    ledger = load(root / '.voiceport/loop-state.json')
    # Existing registry is the current role/freeze source of truth; reject an oversized packet.
    packet = {
        'schema_version': 1, 'request_id': state['request']['id'], 'created_at': now(),
        'run_id': state['run_id'], 'epoch': state['epoch'], 'from_session': state['owner'],
        'goal': state['goal'], 'loop_prompt': state['loop_prompt'],
        'paused': ledger['paused'], 'coordination': registry,
        'rotation_threshold': state['threshold'],
        'rotation_policy': FINAL_ROUND_POLICY,
        'tasks': {key: {'status': value['status'],
                        'attempt': value['attempts'][-1]['id'] if value['attempts'] else None}
                  for key, value in ledger['tasks'].items()},
        'references': ['docs/LOOP_METHODS.md', 'automation/loops/coordinator.md',
                       'docs/SOLUTION.md', 'task_plan.md', 'progress.md', 'findings.md',
                       '.voiceport/roles.md', '.voiceport/coordination.json'],
        'git_delivery_policy': 'User authorized per-stage logical commits and push, PR creation/update and merge subject to repository checks, then fetch and pull --ff-only latest main at a safe writer/freeze boundary. Preserve known failures and distinguish WIP integration from product acceptance. Carry unfinished Git steps across handoff; never accumulate multiple rounds of uncommitted work. See docs/LOOP_METHODS.md section 6.',
        'constraints': 'Preserve workers, attempt and freeze. No real recording, credentials, upload or target input without explicit user authorization. Developer checks are not independent acceptance. Read bounded current sections, not the entire historical transcript.',
    }
    if navigation_hash is not None:
        task = registry['product_task_id']
        attempt = registry['product_attempt_id']
        if task != 'unbound':
            current = ledger['tasks'].get(task) or {}
            attempts = current.get('attempts') or []
            if not attempts or attempts[-1]['id'] != attempt:
                raise ValueError('navigation product attempt no longer matches the ledger')
        elif attempt != 'unbound':
            raise ValueError('unbound product task has a bound attempt')
        packet.update(navigation=state['navigation'], navigation_sha256=navigation_hash,
                      product_task_id=task, product_attempt_id=attempt,
                      tasks={} if task == 'unbound' else {task: packet['tasks'][task]})
        packet['references'] = [state['navigation'] if ref == '.voiceport/coordination.json' else ref
                                for ref in packet['references']]
    raw = json.dumps(packet, ensure_ascii=False, indent=2).encode()
    if len(raw) > 48000:
        raise ValueError('handoff exceeds 48KB; shorten private current coordination registry first')
    return packet


def adopt(root, session, run_id, transcript, threshold, goal, loop_prompt, retired):
    validate_threshold(threshold)
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


def retire(root, session, transcript, run_id=None):
    with transaction(root, run_id) as state:
        authority = require_coordinator(root, state, session, run_id)
        if run_id is not None:
            if Path(transcript).resolve() != Path(authority['transcript_cursor']['path']).resolve():
                raise ValueError('retirement transcript differs from authentic session proof')
        scan(state, transcript)
        proof = stop_receipt(transcript, session, state.get('scheduler'))
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


def launch(root, session, run_id=None):
    with transaction(root, run_id) as state:
        require_coordinator(root, state, session, run_id)
        if state['phase'] != 'retired' or state['request'].get('launch_started_at'):
            raise ValueError('launch already attempted or source not retired; inspect receipt, never launch twice')
        request = state['request'].copy()
        token = uuid.uuid4().hex
        state['request'].update(token=token, launch_started_at=now(),
                                retiring_terminal=state.get('terminal'),
                                retiring_identity=state.get('identity_receipt'))
        state['phase'] = 'launching'
        claim_scope = f' --run {run_id}' if run_id else ''
        prompt = (f"You are the successor VoicePort coordinator. Read .claude/COORDINATOR_ROTATION.md and "
                  f"{request['packet']}. Transfer token: {token}. Source is retired and loop stopped. "
                  f"Before product writes, use the orchestration skill, bind existing Run {state['run_id']} "
                  f"with run-use --id {state['run_id']} from YOUR terminal (use --takeover-legacy only for an automatically adopted legacy Run), then run python3 -I {SCRIPT} claim "
                  f"--session YOUR_ACTUAL_SESSION_ID{claim_scope} --token {token}. "
                  "Do not create a Run or replace workers. Confirm binding and claim before consuming mail. "
                  f"Inherit the packet's rotation_threshold ({state['threshold']}). {FINAL_ROUND_POLICY} "
                  "Honor persistent pause. If allowed work remains, explicitly start /loop with the packet's "
                  "original loop_prompt; scheduled loops do not transfer automatically. Then handle current work. "
                  "If binding/claim fails, end without writing product state or scheduling.")
        if run_id is not None:
            prompt += (f' This is a per-Run lease: use claim --run {run_id} --session YOUR_ACTUAL_SESSION_ID '
                       f'--token {token}. First obtain authentic current PreToolUse/transcript initial0 proof. '
                       'After claim and launch delivery verification, use close-retired with this Run, your session '
                       'and the exact retiring terminal in the request; never close another terminal. '
                       'Do not start product work or a loop before that verification and closure receipt.')
        launch_argv = ['agent', 'launch', '--agent', 'claude', '--surface', 'terminal',
                       '--worktree', 'path:' + str(root), '--prompt', prompt]
        state['request']['launch_argv'] = launch_argv
    try:
        receipt = invoke(root, launch_argv)
    except (ValueError, OSError, subprocess.TimeoutExpired) as error:
        with transaction(root, run_id) as state:
            update_transfer(state, request['id'], launch_error=str(error), launch_failure=failure_observation(error))
        raise ValueError('launch outcome may be unknown; inspect Orca and use its replay ID, no fresh launch') from error
    delivery_status = terminal_launch_delivery_status(receipt)
    delivered = delivery_status in ('journaled', 'handed-to-terminal')
    with transaction(root, run_id) as state:
        update_transfer(state, request['id'], launch_receipt=receipt,
                        delivery_status=delivery_status, delivery_confirmed=delivered)
    if not delivered:
        raise ValueError('launch receipt does not prove prompt delivery; inspect existing successor, do not launch again')
    return receipt


def configure(root, session, threshold, goal=None, run_id=None):
    validate_threshold(threshold)
    with transaction(root, run_id) as state:
        require_coordinator(root, state, session, run_id)
        if state['phase'] != 'active':
            raise ValueError('cannot change threshold during a transfer')
        state['threshold'] = threshold
        if goal is not None:
            state['goal'] = goal
        return {'threshold': threshold, 'effective': 'next hook boundary'}


def claim(root, session, token, run_id=None):
    scope = run_id
    with transaction(root, scope) as state:
        if (not state or state['phase'] != 'launching'
                or state['request'].get('token') != token
                or session == state['owner'] or session in state['retired_sessions']):
            raise ValueError('invalid/stale transfer token or session')
        if scope:
            validate_run_state(root, state, scope)
        expected_run = state['run_id']
        started = state['request']['launch_started_at']
    binding = bound_run(root, expected_run)
    proof = authentic_session(root, session, fresh_after=started) if scope else None
    identity = coordinator_identity(root, expected_run, proof['terminal']) if proof else None
    with transaction(root, scope) as state:
        if state['phase'] != 'launching' or state['request'].get('token') != token:
            raise ValueError('transfer already claimed')
        packet = root / state['request']['packet']
        if hashlib.sha256(packet.read_bytes()).hexdigest() != state['request']['packet_sha256']:
            raise ValueError('handoff packet changed')
        if proof:
            if state['request'].get('launch_receipt') and (not state['request'].get('delivery_confirmed')
                    or receipt_field(state['request']['launch_receipt'], 'result', 'outcome', 'handle') != proof['terminal']):
                raise ValueError('claimant is not the verified launch target')
            generation = identity['run']['result']['run']['consumer_generation']
            prior_generation = state['identity_receipt']['run']['result']['run']['consumer_generation']
            if (proof['terminal'] == state['terminal'] or proof.get('coordinator_run') not in (None, scope)
                    or generation <= prior_generation):
                raise ValueError('successor terminal/Run generation is not fresh')
            # Cache identity before publishing the new owner. A crash leaves it
            # fenced by the still-launching lease; the same claim can be inspected.
            with session_transaction(root, session) as saved:
                require_terminal(saved)
                saved['coordinator_run'] = scope
            state.update(terminal=proof['terminal'], identity_receipt=identity, harness=proof['harness'],
                         successor_verified={'session': session, 'terminal': proof['terminal'],
                                             'proof': proof, 'identity': identity, 'at': now()})
        state['history'].append({**state['request'], 'to_session': session, 'claimed_at': now()})
        state.update(owner=session, epoch=state['epoch'] + 1, phase='active',
                     boundaries=[], binding_receipt=binding)
        state.pop('transcript_cursor', None)
        state.pop('scheduler', None)  # Scheduler ownership belongs to the retired session.
        # Keep launch request until its receipt is recorded; it is also in history.
        return {'owner': session, 'epoch': state['epoch'], 'paused': paused(root)}


def close_retired(root, session, run_id, terminal):
    """Exactly one close attempt after same-Run claim and verified fresh startup."""
    with transaction(root, run_id) as state:
        require_coordinator(root, state, session, run_id)
        request = state['request']
        if (state['phase'] != 'active' or not state.get('successor_verified')
                or state['successor_verified']['session'] != session
                or not request.get('delivery_confirmed')
                or not terminal or terminal != request.get('retiring_terminal')
                or terminal == state['terminal'] or request.get('close_started_at')
                or receipt_field(request.get('launch_receipt'), 'result', 'outcome', 'handle') != state['terminal']):
            raise ValueError('close requires verified successor and exact retiring target; never retry unknown close')
        receipt = invoke(root, ['terminal', 'show', '--terminal', terminal])
        observed = receipt.get('result', {}).get('terminal') or {}
        expected = request['retiring_identity']['terminal']['result']['terminal']
        if any(observed.get(key) != expected.get(key) for key in ('handle', 'incarnationId', 'worktreePath')):
            raise ValueError('retiring terminal identity changed; refuse close')
        update_transfer(state, request['id'], close_started_at=now(), close_target_receipt=receipt)
        request_id = request['id']
    try:
        receipt = invoke(root, ['terminal', 'close', '--terminal', terminal])
    except (ValueError, OSError, subprocess.TimeoutExpired) as error:
        with transaction(root, run_id) as state:
            update_transfer(state, request_id, close_error=str(error), close_failure=failure_observation(error))
        raise ValueError('close outcome unknown; inspect exact terminal and receipt, never blind retry') from error
    confirmed = (receipt_field(receipt, 'result', 'close', 'handle') == terminal
                 and receipt_field(receipt, 'result', 'close', 'ptyKilled') is True)
    with transaction(root, run_id) as state:
        update_transfer(state, request_id, close_receipt=receipt, close_confirmed=confirmed)
        if not confirmed:
            update_transfer(state, request_id, close_error='receipt does not prove exact terminal PTY stop')
    if not confirmed:
        raise ValueError('close outcome unknown; inspect stored receipt and exact terminal, never blind retry')
    return receipt


def rotation_hint(state):
    if not state.get('navigation'):
        return (f"Legacy coordinator {state['owner']} must first upgrade its existing Run without resetting counts: "
                f"python3 -I {SCRIPT} upgrade-legacy --session {state['owner']} --run {state['run_id']}. "
                "Then stop all owned schedulers and use the per-Run retire/launch/claim/close-retired flow. "
                + FINAL_ROUND_POLICY)
    scope = f" --run {state['run_id']}" if state.get('navigation') else ''
    return (f"Coordinator entered context round {len(state['boundaries']) + 1}/{state['threshold']} "
            f"({len(state['boundaries'])} completed compactions); no further compaction is allowed. "
            f"Finish the current tool boundary, call ScheduleWakeup(stop=true), then Bash: "
            f"python3 -I {SCRIPT} retire --session {state['owner']}{scope} --transcript YOUR_TRANSCRIPT_PATH; "
            f"then python3 -I {SCRIPT} launch --session {state['owner']}{scope}. "
            f"End this coordinator turn after launch receipt. Preserve workers/freeze; do not start new work. {FINAL_ROUND_POLICY}")


def allowed_during_rotation(payload, state):
    name = payload.get('tool_name', '').split('.')[-1]
    data = payload.get('tool_input', {})
    if name in ('Read', 'Skill'):
        return True
    if scheduler_shutdown(payload, state):
        return True
    if name == 'Bash':
        try:
            argv = shlex.split(data.get('command', ''))
        except ValueError:
            return False
        # Only a single helper command, with no shell operators/substitution.
        command = data.get('command', '')
        if argv == ['python3', '-I', SCRIPT, 'upgrade-legacy', '--session', state['owner'], '--run', state['run_id']]:
            return state['phase'] in ('active', 'requested')
        return (len(argv) >= 4 and argv[:3] == ['python3', '-I', SCRIPT]
                and argv[3] in ('retire', 'launch', 'status')
                and not any(c in command for c in (';', '\n', '`', '$', '|', '&', '>', '<')))
    return False


def scoped_admin(payload, state):
    name = payload.get('tool_name', '').split('.')[-1]
    data = payload.get('tool_input', {})
    if name == 'Read':
        return True
    if scheduler_shutdown(payload, state):
        return True
    if name == 'exec_command':
        name, data = 'Bash', {'command': data.get('cmd', '')}
    if name != 'Bash':
        return False
    command = data.get('command', '')
    if len(command.encode()) > 48000 or any(c in command for c in (';', '\n', '\r', '`', '$', '|', '&', '>', '<')):
        return False
    try:
        argv = shlex.split(command)
    except ValueError:
        return False
    # On uncertain side effects allow bounded inspection, never another launch,
    # send, Run takeover or close outside the guarded helper.
    if argv in (['orca', 'orchestration', 'run-show', '--id', state['run_id'], '--full', '--json'],
                ['orca', 'orchestration', 'run-current', '--full', '--json']):
        return True
    if (argv[:4] == ['orca', 'terminal', 'show', '--terminal'] and len(argv) == 6
            and argv[5] == '--json' and re.fullmatch(r'term_[A-Za-z0-9_-]+', argv[4])):
        return True
    worktree = state['identity_receipt']['terminal']['result']['terminal']['worktreePath']
    if argv == ['orca', 'terminal', 'list', '--worktree', 'path:' + worktree, '--limit', '20', '--json']:
        return True
    if argv[:3] != ['python3', '-I', SCRIPT] or len(argv) < 4:
        return False
    action = argv[3]
    flags = argv[4::2]
    if len(argv[4:]) % 2 or len(flags) != len(set(flags)):
        return False
    values = dict(zip(flags, argv[5::2]))
    if action == 'status':
        return values == {'--run': state['run_id']}
    expected = {'--session': state['owner'], '--run': state['run_id']}
    if action == 'retire' and state['phase'] == 'requested':
        return set(values) == {*expected, '--transcript'} and all(values[k] == v for k, v in expected.items())
    if action == 'launch' and state['phase'] == 'retired':
        return values == expected
    if (action == 'close-retired' and state['phase'] == 'active'
            and state.get('successor_verified') and state['request'].get('retiring_terminal')):
        return values == {**expected, '--terminal': state['request']['retiring_terminal']}
    return False


def registration_command(payload, session):
    """Final-round registration is administrative; register itself proves authority."""
    name = payload.get('tool_name', '').split('.')[-1]
    data = payload.get('tool_input', {})
    command = data.get('cmd' if name == 'exec_command' else 'command', '')
    if name not in ('Bash', 'exec_command') or not isinstance(command, str):
        return False
    if len(command.encode()) > 48000 or any(c in command for c in (';', '\n', '\r', '`', '$', '|', '&', '>', '<')):
        return False
    try:
        argv = shlex.split(command)
    except ValueError:
        return False
    if argv[:4] != ['python3', '-I', SCRIPT, 'register'] or len(argv) != 16:
        return False
    values = dict(zip(argv[4::2], argv[5::2]))
    flags = set(values)
    base = {'--session', '--run', '--transcript', '--goal', '--navigation'}
    return (flags in (base | {'--loop-prompt'}, base | {'--loop-prompt-file'})
            and values['--session'] == session
            and ('--loop-prompt-file' not in values or values['--loop-prompt-file'] == '.claude/loop.md'))


def merge_session_proof(state, proof):
    state['boundaries'] = list(dict.fromkeys([*state['boundaries'], *proof['boundaries']]))
    for key in ('transcript_cursor', 'transcript_started_at', 'transcript_session_verified', 'scheduler'):
        if key in proof:
            state[key] = proof[key]
    request_rotation(state)


def run_hook(root, payload, run_id):
    # Keep independent authentic accounting and its immutable drift latch. The
    # Run lease changes the administrative gate, never the session identity/count.
    result = worker_hook(root, payload)
    proof = load(session_path(root, payload['session_id']))
    if proof.get('terminal_drift'):
        return result
    with transaction(root, run_id) as state:
        if not state or state.get('run_id') != run_id:
            raise ValueError('registered Run coordinator state is missing or mismatched')
        validate_run_state(root, state, run_id)
        session = payload['session_id']
        retired = state.get('retired_sessions')
        if not isinstance(retired, list) or state['phase'] not in ('active', 'requested', 'retired', 'launching'):
            raise ValueError('invalid scoped coordinator routing')
        if session != state['owner'] and session not in retired:
            raise ValueError('session is not a registered owner or retired predecessor')
        if session == state['owner']:
            require_terminal(state)
            merge_session_proof(state, proof)
        pending_close = (bool(state.get('successor_verified')) and bool(state['request'].get('launch_started_at'))
                         and not state['request'].get('close_confirmed'))
        if session not in retired and state['phase'] == 'active' and not pending_close:
            return {}
        event = payload['hook_event_name']
        hint = rotation_hint(state) if session not in retired else 'Retired coordinator is fenced; use its successor.'
        if pending_close and session == state['owner']:
            hint = 'Verify delivery and close the exact retiring terminal with close-retired before product work or loop.'
        if event == 'PreCompact':
            return {'decision': 'block', 'reason': hint}
        allowed = scoped_admin(payload, state)
        if session != state['owner']:
            allowed = payload.get('tool_name', '').split('.')[-1] == 'Read' or scheduler_shutdown(payload, proof)
        if event == 'PreToolUse' and not allowed:
            return {'hookSpecificOutput': {'hookEventName': event, 'permissionDecision': 'deny',
                                          'permissionDecisionReason': hint}}
        if event == 'SessionStart':
            return {'hookSpecificOutput': {'hookEventName': event, 'additionalContext': hint}}
        if event == 'Stop' and state['phase'] == 'requested' and not payload.get('stop_hook_active'):
            return {'decision': 'block', 'reason': hint}
        return {}


def session_path(root, session):
    if not isinstance(session, str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', session):
        raise ValueError('invalid session identity')
    return root / '.voiceport/session-rotations' / (session + '.json')


def notes_path(root, session):
    session_path(root, session)
    return root / '.voiceport/handoffs' / ('session-rotation-' + session + '.md')


@contextmanager
def handoff_directory(root, create=False):
    """Anchor traversal to directory descriptors; reject symlinks and substitution."""
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_NONBLOCK
    with ExitStack() as stack:
        root_fd = os.open(root, flags)
        stack.callback(os.close, root_fd)
        parent = root_fd
        links = []
        for name in ('.voiceport', 'handoffs'):
            if create:
                try:
                    os.mkdir(name, mode=0o700, dir_fd=parent)
                except FileExistsError:
                    pass
            child = os.open(name, flags, dir_fd=parent)
            stack.callback(os.close, child)
            links.append((parent, name, child))
            parent = child
        yield parent
        # The descriptor anchors reads even if a parent is renamed; additionally
        # reject a checkpoint whose published spelling no longer names that file.
        current = os.stat(root, follow_symlinks=False)
        anchored = os.fstat(root_fd)
        if (current.st_dev, current.st_ino) != (anchored.st_dev, anchored.st_ino):
            raise ValueError('checkpoint root changed during access')
        for parent, name, child in links:
            current = os.stat(name, dir_fd=parent, follow_symlinks=False)
            anchored = os.fstat(child)
            if (current.st_dev, current.st_ino) != (anchored.st_dev, anchored.st_ino):
                raise ValueError('checkpoint handoff directory changed during access')


def notes_stat(directory, name):
    value = os.stat(name, dir_fd=directory, follow_symlinks=False)
    if not stat.S_ISREG(value.st_mode) or not 1 <= value.st_size <= 48000:
        raise ValueError('checkpoint notes must be a nonempty regular file of at most 48KB, without symlinks')
    return value


def file_signature(value):
    return (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns)


def checkpoint_notes(root, session, text=None):
    """Bound reads to 48KB and never open a known symlink, FIFO or device.

    O_NONBLOCK and post-open fstat cover a special-file substitution between lstat
    and open: no bytes are read from it. Directory-relative opens never follow a
    substituted parent. External writers still require coordinator reconciliation.
    """
    name = notes_path(root, session).name
    if text is not None:
        raw = text.encode()
        if not 1 <= len(raw) <= 48000:
            raise ValueError('checkpoint notes exceed 48KB or are empty')
    with handoff_directory(root, create=text is not None) as directory:
        if text is not None:
            fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_NONBLOCK,
                         0o600, dir_fd=directory)
            with os.fdopen(fd, 'wb') as out:
                out.write(raw)
                out.flush()
                os.fsync(out.fileno())
        before = notes_stat(directory, name)  # Reject known special files before open.
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        with os.fdopen(fd, 'rb') as source:
            opened = os.fstat(source.fileno())
            if not stat.S_ISREG(opened.st_mode) or file_signature(before) != file_signature(opened):
                raise ValueError('checkpoint notes changed before open')
            raw = source.read(48001)
            after = os.fstat(source.fileno())
        current = notes_stat(directory, name)
        if (not 1 <= len(raw) <= 48000 or len(raw) != before.st_size
                or file_signature(before) != file_signature(after)
                or file_signature(before) != file_signature(current)):
            raise ValueError('checkpoint notes changed during read')
    return hashlib.sha256(raw).hexdigest()


def writable_notes(root, session, name=None):
    """Validate the permitted tool path without opening its file or creating dirs."""
    try:
        with handoff_directory(root) as directory:
            try:
                notes_stat(directory, name or notes_path(root, session).name)
            except FileNotFoundError:
                pass
        return True
    except FileNotFoundError:
        # A missing handoff directory is normal before the first notes write.
        # Validate the existing parent separately so a symlink is never allowed.
        try:
            flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_NONBLOCK
            with ExitStack() as stack:
                fd = os.open(root, flags)
                stack.callback(os.close, fd)
                voice = os.open('.voiceport', flags, dir_fd=fd)
                stack.callback(os.close, voice)
                try:
                    os.stat('handoffs', dir_fd=voice, follow_symlinks=False)
                except FileNotFoundError:
                    return True
        except OSError:
            pass
    except (ValueError, OSError):
        pass
    return False


def require_terminal(state):
    terminal = os.environ.get('ORCA_TERMINAL_HANDLE')
    if not terminal or terminal != state.get('terminal'):
        raise ValueError('actual ORCA_TERMINAL_HANDLE must match the recorded session terminal')
    if state.get('terminal_drift'):
        raise ValueError('session terminal drift is fenced; coordinator must stop/release and start a fresh session')


def validate_hook_accounting(state):
    """Validate JSON shapes consumed by hook routing and transcript accounting.

    Coordinator records predate schema_version/harness and may retain their last
    request after claim. Do not impose worker lifecycle fields on that role.
    """
    if (not isinstance(state, dict) or not isinstance(state.get('owner'), str)
            or not state['owner'] or type(state.get('epoch')) is not int or state['epoch'] < 1
            or type(state.get('threshold')) is not int
            or not 1 <= state['threshold'] <= DEFAULT_THRESHOLD
            or not isinstance(state.get('phase'), str)):
        raise ValueError('invalid saved hook accounting')
    boundaries = state.get('boundaries')
    if (not isinstance(boundaries, list)
            or any(not isinstance(b, str) or not b for b in boundaries)
            or len(set(boundaries)) != len(boundaries)):
        raise ValueError('invalid saved session boundaries; refuse to reset counts')
    if 'scheduler' in state:
        copy_scheduler(state)


def validate_saved_timestamp(value):
    if not isinstance(value, str):
        raise ValueError('invalid saved session timestamp')
    timestamp = datetime.fromisoformat(value)
    if timestamp.tzinfo is None:
        raise ValueError('saved session timestamp has no timezone')


def validate_session_state(state, session):
    """Private saved authority must be coherent before it can fence or recover."""
    if not isinstance(state, dict) or state.get('owner') != session:
        raise ValueError('saved session identity does not match the actual session')
    validate_hook_accounting(state)
    if 'coordinator_run' in state:
        coordinator_path(Path('.'), state['coordinator_run'])
    if (not {'schema_version', 'epoch', 'threshold', 'phase', 'boundaries',
             'terminal', 'created_at', 'request'}.issubset(state)
            or type(state.get('schema_version')) is not int or state['schema_version'] != 1
            or state.get('phase') not in ('active', 'requested', 'notifying', 'handoff_ready', 'retired', 'launching', 'claimed')
            or not isinstance(state.get('created_at'), str) or not state['created_at']
            or (state.get('terminal') is not None and not isinstance(state['terminal'], str))):
        raise ValueError('invalid saved session authority')
    # Optional history on older records is absent, never an arbitrary iterable.
    if 'hook_events' in state:
        events = state['hook_events']
        if not isinstance(events, list) or any(not isinstance(e, str) or e not in HOOK_EVENTS for e in events):
            raise ValueError('invalid saved hook event history')
    if state['request'] is not None and not isinstance(state['request'], dict):
        raise ValueError('invalid saved session request')
    if 'checkpoint' in state and not isinstance(state['checkpoint'], dict):
        raise ValueError('invalid saved checkpoint')
    if state['phase'] in ('retired', 'launching', 'claimed') and not state.get('standalone'):
        raise ValueError('standalone phase without registered authority')
    if 'standalone' in state:
        standalone = state['standalone']
        if (not isinstance(standalone, dict) or not isinstance(standalone.get('contract'), dict)
                or not isinstance(standalone.get('identity'), dict)
                or not isinstance(standalone.get('contract_sha256'), str)
                or re.fullmatch(r'[0-9a-f]{64}', standalone['contract_sha256']) is None):
            raise ValueError('invalid standalone authority')
    if 'predecessor' in state:
        session_path(Path('.'), state['predecessor'])
        if state['predecessor'] == session or not state.get('standalone'):
            raise ValueError('invalid standalone predecessor')
    validate_saved_timestamp(state['created_at'])
    if 'transcript_started_at' in state:
        validate_saved_timestamp(state['transcript_started_at'])
    if 'harness' in state and state['harness'] not in ('claude', 'codex'):
        raise ValueError('invalid saved session harness')
    if 'transcript_session_verified' in state and type(state['transcript_session_verified']) is not bool:
        raise ValueError('invalid saved transcript verification')
    if state.get('transcript_session_verified') or 'transcript_cursor' in state:
        cursor = state.get('transcript_cursor')
        if (state.get('harness') not in ('claude', 'codex') or not isinstance(cursor, dict)
                or not isinstance(cursor.get('path'), str) or not cursor['path']
                or type(cursor.get('offset')) is not int or cursor['offset'] < 0
                or type(cursor.get('inode')) is not int or cursor['inode'] <= 0):
            raise ValueError('invalid saved transcript verification evidence')
    if 'terminal_drift' in state:
        drift = state['terminal_drift']
        if (not isinstance(drift, dict)
                or not {'stored_terminal', 'actual_terminal', 'detected_at', 'event'}.issubset(drift)
                or drift['stored_terminal'] != state['terminal']
                or not isinstance(drift['detected_at'], str) or not drift['detected_at']
                or not isinstance(drift['event'], str) or drift['event'] not in HOOK_EVENTS
                or (drift['actual_terminal'] is not None and not isinstance(drift['actual_terminal'], str))
                or (drift['actual_terminal'] and drift['actual_terminal'] == state['terminal'])):
            raise ValueError('invalid saved terminal drift evidence')
        validate_saved_timestamp(drift['detected_at'])


@contextmanager
def session_transaction(root, session):
    path = session_path(root, session)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with path.with_suffix('.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        state = load(path) if path.exists() else {
            'schema_version': 1, 'owner': session, 'epoch': 1, 'threshold': DEFAULT_THRESHOLD,
            'phase': 'active', 'boundaries': [], 'request': None,
            'terminal': os.environ.get('ORCA_TERMINAL_HANDLE'), 'created_at': now(),
        }
        validate_session_state(state, session)
        before = path.read_bytes() if path.exists() else None
        yield state
        if before is None or load(path) != state:
            atomic(path, state)


def contract_path(root, session):
    session_path(root, session)
    return root / '.voiceport/handoffs' / ('session-contract-' + session + '.json')


def read_handoff(root, relative):
    path = root / relative
    if path.parent != root / '.voiceport/handoffs' or path.is_symlink():
        raise ValueError('handoff must be a direct regular private file')
    with handoff_directory(root) as directory:
        before = notes_stat(directory, path.name)
        fd = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        with os.fdopen(fd, 'rb') as source:
            opened = os.fstat(source.fileno())
            if not stat.S_ISREG(opened.st_mode) or file_signature(opened) != file_signature(before):
                raise ValueError('handoff changed before read')
            raw = source.read(48001)
            after = os.fstat(source.fileno())
        current = notes_stat(directory, path.name)
        if (len(raw) != before.st_size or file_signature(before) != file_signature(after)
                or file_signature(before) != file_signature(current)):
            raise ValueError('handoff changed during read')
    return json.loads(raw), hashlib.sha256(raw).hexdigest()


def standalone_identity(root, proof):
    """Missing local worker navigation never grants independent lifecycle authority."""
    require_terminal(proof)
    terminal = proof['terminal']
    binding = invoke(root, ['orchestration', 'run-current', '--full'])
    if 'run' not in binding['result'] or binding['result']['run'] is not None:
        raise ValueError('bound Run requires its coordinator/worker lifecycle, not standalone rotation')
    # Include context-only Dispatches and settled-but-retained workers. Pagination
    # must exhaust the all-Run view; unknown/truncated membership fails closed.
    cursor, pages, visited = None, [], set()
    for _ in range(32):
        argv = ['orchestration', 'worker-list', '--limit', '100', '--full']
        if cursor:
            argv += ['--cursor', cursor]
        page = invoke(root, argv)
        result = page['result']
        if result.get('scope', {}).get('source') != 'all' or not isinstance(result.get('workers'), list):
            raise ValueError('complete all-Run worker membership proof required')
        rows = result['workers']
        if any(not isinstance(row, dict) or 'agentTerminalHandle' not in row
               or (row.get('resource') is not None and not isinstance(row['resource'], dict)) for row in rows):
            raise ValueError('invalid full worker membership schema')
        if any(terminal in (row.get('agentTerminalHandle'), (row.get('resource') or {}).get('terminalHandle'))
               for row in rows):
            raise ValueError('managed worker must checkpoint to its coordinator; standalone takeover refused')
        pages.append({'sha256': hashlib.sha256(json.dumps(page, sort_keys=True).encode()).hexdigest(),
                      'rows': len(rows), 'cursor': cursor})
        pagination = result.get('page')
        if (not isinstance(pagination, dict) or type(pagination.get('hasMore')) is not bool
                or 'nextCursor' not in pagination
                or pagination['hasMore'] != (pagination['nextCursor'] is not None)):
            raise ValueError('worker membership pagination proof missing or inconsistent')
        cursor = pagination['nextCursor']
        if cursor is None:
            break
        if not isinstance(cursor, str) or not cursor or cursor in visited:
            raise ValueError('invalid worker membership cursor')
        visited.add(cursor)
    else:
        raise ValueError('worker membership exceeds bounded inspection; reconcile before rotation')
    registry_path = root / '.voiceport/coordination.json'
    if registry_path.exists():
        registry = load(registry_path)
        if any(row.get('terminal') == terminal for row in registry.get('workers', {}).values()):
            raise ValueError('registered worker requires coordinator reconciliation')
    receipt = invoke(root, ['terminal', 'show', '--terminal', terminal])
    info = receipt['result'].get('terminal') or {}
    if (info.get('handle') != terminal or info.get('worktreePath') != str(root.resolve())
            or info.get('connected') is not True or info.get('orphaned') is not False
            or not info.get('incarnationId') or info.get('agentIdentity') != proof['harness']):
        raise ValueError('standalone terminal/worktree/agent identity mismatch')
    return {'binding': binding, 'membership_pages': pages, 'terminal': receipt, 'at': now()}


def task_message(transcript, session, message_uuid):
    """Read an exact human input, never a pasted system event or agent report."""
    found = []
    path = Path(transcript)
    if path.is_symlink() or not path.is_file():
        raise ValueError('task evidence must use a regular authentic transcript')
    with path.open('rb') as stream:
        size = os.fstat(stream.fileno()).st_size
        raw = stream.read(size)
        for line in raw.splitlines(keepends=True):
            if not line.endswith(b'\n'):
                continue
            row = json.loads(line)
            if row.get('uuid') != message_uuid:
                continue
            text = row.get('message', {}).get('content')
            if (row.get('sessionId') != session or row.get('isSidechain')
                    or row.get('type') != 'user' or row.get('origin', {}).get('kind') != 'human'
                    or row.get('message', {}).get('role') != 'user'
                    or not isinstance(text, str) or not text.strip()):
                raise ValueError('task/authorization UUID must identify actual human text in this session')
            validate_saved_timestamp(row.get('timestamp'))
            found.append((text, {'session': session, 'transcript': str(path), 'uuid': message_uuid,
                                'timestamp': row['timestamp'], 'sha256': hashlib.sha256(line).hexdigest()}))
        verify_transcript_prefix(stream, path, size, hashlib.sha256(raw).digest())
    if len(found) != 1:
        raise ValueError('task/authorization evidence missing or duplicated')
    return found[0]


def validate_task_authority(authority):
    contract = authority.get('contract', {})
    if (set(contract) != {'goal', 'task_prompt', 'authorization', 'paused', 'active_tools'}
            or any(not isinstance(contract.get(k), str) or not contract[k].strip()
                   for k in ('goal', 'task_prompt', 'authorization'))
            or type(contract.get('paused')) is not bool or contract.get('active_tools') != []):
        raise ValueError('invalid task-only recovery authority; no loop prompt may be invented')
    evidence = authority.get('evidence', {})
    if set(evidence) != {'task', 'authorization'}:
        raise ValueError('task and recovery authorization evidence required')
    for key, field in (('task', 'task_prompt'), ('authorization', 'authorization')):
        pin = evidence[key]
        text, observed = task_message(pin['transcript'], pin['session'], pin['uuid'])
        if observed != pin or text != contract[field]:
            raise ValueError('original task/authorization transcript evidence changed')
    digest = hashlib.sha256(json.dumps(contract, ensure_ascii=False, sort_keys=True).encode()).hexdigest()
    if digest != authority.get('contract_sha256'):
        raise ValueError('task recovery authority digest changed')
    return contract


def session_task_register(root, session, task_uuid, authorization_uuid, goal):
    """Proactive task-only rescue basis. Never available to a drift-fenced caller.

    UUIDs pin the exact original task and the user's explicit recovery authorization.
    Their meaning/scope must be read by management; registration grants no scheduler.
    """
    if not isinstance(goal, str) or not goal.strip() or len(goal.encode()) > 2000:
        raise ValueError('task goal must be nonempty and at most 2KB')
    with session_transaction(root, session) as state:
        proof = authentic_session(root, session)  # includes require_terminal/fence check
        if (state.get('coordinator_run') or state['phase'] != 'active'
                or state.get('standalone') or state.get('predecessor')
                or state.get('administrative_predecessor') or proof.get('harness') != 'claude'):
            raise ValueError('task recovery registration requires an unregistered active Claude top-level session')
        legacy = coordinator_path(root)
        if legacy.exists() and load(legacy).get('owner') == session:
            raise ValueError('legacy coordinator must use its existing lease')
        transcript = proof['transcript_cursor']['path']
        task, task_pin = task_message(transcript, session, task_uuid)
        authorization, auth_pin = task_message(transcript, session, authorization_uuid)
        contract = {'goal': goal, 'task_prompt': task, 'authorization': authorization,
                    'paused': bool(paused(root) or state.get('inherited_pause')), 'active_tools': []}
        authority = {'kind': 'task', 'contract': contract,
                     'contract_sha256': hashlib.sha256(json.dumps(contract, ensure_ascii=False, sort_keys=True).encode()).hexdigest(),
                     'evidence': {'task': task_pin, 'authorization': auth_pin}}
        if len(json.dumps(authority, ensure_ascii=False).encode()) > 24000:
            raise ValueError('task recovery evidence exceeds bounded handoff budget')
        authority['identity'] = standalone_identity(root, proof)
        state['standalone'] = authority
        state['boundaries'] = proof['boundaries']
        request_rotation(state)
        return authority


def session_register(root, session, contract):
    expected = contract_path(root, session).relative_to(root).as_posix()
    if contract != expected:
        raise ValueError('contract must use this session exact private path')
    value, digest = read_handoff(root, contract)
    if (not isinstance(value, dict) or set(value) != {'goal', 'loop_prompt', 'authorization', 'paused', 'active_tools'}
            or any(not isinstance(value.get(k), str) or not value[k].strip()
                   for k in ('goal', 'loop_prompt', 'authorization'))
            or type(value.get('paused')) is not bool or not isinstance(value.get('active_tools'), list)):
        raise ValueError('contract requires goal, full loop_prompt, authorization, paused bool and active_tools list')
    if value['active_tools']:
        raise ValueError('settle active tools before registering an immutable rotation contract')
    with session_transaction(root, session) as state:
        proof = authentic_session(root, session)
        if state.get('coordinator_run') or state['phase'] not in ('active', 'requested'):
            raise ValueError('session already belongs to another lifecycle')
        if state.get('standalone'):
            if state['standalone']['contract_sha256'] != digest:
                raise ValueError('registered rotation contract is immutable')
            return state['standalone']
        legacy = coordinator_path(root)
        if legacy.exists() and load(legacy).get('owner') == session:
            raise ValueError('legacy coordinator must use its existing lease')
        identity = standalone_identity(root, proof)
        state['standalone'] = {'contract': value, 'contract_sha256': digest, 'identity': identity}
        state['boundaries'] = proof['boundaries']
        request_rotation(state)
        return state['standalone']


def session_request(root, session, reason):
    """Voluntary early handoff uses the normal lifecycle without changing accounting."""
    if not isinstance(reason, str) or not reason.strip() or len(reason.encode()) > 2000:
        raise ValueError('early handoff requires a nonempty reason of at most 2KB')
    with session_transaction(root, session) as state:
        proof = authentic_session(root, session)
        if not state.get('standalone') or state.get('coordinator_run') or state['phase'] not in ('active', 'requested'):
            raise ValueError('early handoff requires a registered active standalone session')
        standalone_identity(root, proof)
        if state.get('predecessor'):
            previous = load(session_path(root, state['predecessor']))
            if not (previous.get('request') or {}).get('close_confirmed'):
                raise ValueError('close the prior predecessor before another rotation')
        state['boundaries'] = proof['boundaries']
        if state['phase'] == 'active':
            state.update(phase='requested', request={
                'id': 'rotation-' + uuid.uuid4().hex, 'requested_at': now(),
                'from_session': session, 'trigger': 'voluntary', 'reason': reason,
                'completed_compactions_at_request': len(proof['boundaries']),
            })
        return state['request']


def session_retire(root, session):
    with session_transaction(root, session) as state:
        proof = authentic_session(root, session)
        if not state.get('standalone') or state['phase'] not in ('requested', 'retired'):
            raise ValueError('standalone rotation must be registered and requested')
        if state['phase'] == 'retired':
            return state['request']
        if state.get('predecessor'):
            predecessor = load(session_path(root, state['predecessor']))
            if not (predecessor.get('request') or {}).get('close_confirmed'):
                raise ValueError('close the prior predecessor before another rotation')
        if state['standalone']['contract']['active_tools']:
            raise ValueError('settle active tools and record actual outcomes in notes before retirement')
        standalone_identity(root, proof)
        digest = checkpoint_notes(root, session)
        stop = stop_receipt(proof['transcript_cursor']['path'], session, proof.get('scheduler'))
        packet = {'schema_version': 1, 'from_session': session, 'terminal': state['terminal'],
                  'request_id': state['request']['id'], 'contract': state['standalone']['contract'],
                  'paused': state['standalone']['contract']['paused'] or paused(root),
                  'notes': notes_path(root, session).relative_to(root).as_posix(), 'notes_sha256': digest,
                  'threshold': state['threshold'], 'loop_stop': stop, 'created_at': now()}
        if len(json.dumps(packet, ensure_ascii=False).encode()) > 48000:
            raise ValueError('standalone packet exceeds 48KB')
        relative = f'.voiceport/handoffs/session-transfer-{session}.json'
        atomic(root / relative, packet)
        state['request'].update(packet=relative, packet_sha256=hashlib.sha256((root / relative).read_bytes()).hexdigest(),
                                loop_stop=stop, retired_at=now())
        state['phase'] = 'retired'
        return state['request']


def session_launch(root, session):
    with session_transaction(root, session) as state:
        proof = authentic_session(root, session)
        if not state.get('standalone') or state['phase'] != 'retired' or state['request'].get('launch_started_at'):
            raise ValueError('source must be retired; launch already attempted or unavailable')
        identity = standalone_identity(root, proof)
        request = state['request']
        packet, digest = read_handoff(root, request['packet'])
        if digest != request['packet_sha256'] or checkpoint_notes(root, session) != packet['notes_sha256']:
            raise ValueError('standalone packet or notes changed before launch')
        token = uuid.uuid4().hex
        prompt = (f'You inherit a directly opened top-level task, not a coordinator/worker assignment. '
                  f'Read .claude/COORDINATOR_ROTATION.md, {request["packet"]} and its notes. '
                  f'Preserve the original goal, authorization, pause, unfinished work and evidence. '
                  f'Obtain actual fresh session/terminal PreToolUse proof, then run python3 -I {SCRIPT} '
                  f'session-claim --session YOUR_ACTUAL_NEW_SESSION_ID --source {session} --token {token}. '
                  f'Then session-close --session YOUR_ACTUAL_NEW_SESSION_ID --source {session} '
                  f'--terminal {state["terminal"]}. Closure may wait for the source launch receipt. '
                  'Before successful exact predecessor PTY closure, do no product work and install no loop. '
                  'After closure recheck packet pause and current pause; only if authorized and work remains '
                  'continue the remaining task. For kind=task, task_prompt is the original task, with no scheduling authority. '
                  'Only an original authorized loop contract with a full loop_prompt permits restoring one loop. '
                  'Do not create a Run or adopt unrelated product coordination/Git authorization. '
                  + FINAL_ROUND_POLICY)
        argv = ['agent', 'launch', '--agent', proof['harness'], '--surface', 'terminal',
                '--worktree', 'path:' + str(root), '--prompt', prompt]
        request.update(token=token, launch_started_at=now(), launch_argv=argv, retiring_identity=identity)
        state['phase'] = 'launching'
    try:
        receipt = invoke(root, argv)
    except (ValueError, OSError, subprocess.TimeoutExpired) as error:
        with session_transaction(root, session) as state:
            state['request'].update(launch_error=str(error), launch_failure=failure_observation(error))
        raise ValueError('launch outcome unknown; inspect stored evidence, never fresh launch twice') from error
    delivery = terminal_launch_delivery_status(receipt)
    with session_transaction(root, session) as state:
        state['request'].update(launch_receipt=receipt, delivery_status=delivery,
                                delivery_confirmed=delivery in ('journaled', 'handed-to-terminal'))
    if delivery not in ('journaled', 'handed-to-terminal'):
        raise ValueError('launch receipt does not prove prompt delivery; do not launch again')
    return receipt


def session_claim(root, session, source, token):
    if source == session:
        raise ValueError('successor must be a different session')
    # Always predecessor then successor lock order. Hooks never hold both locks.
    with session_transaction(root, source) as old:
        request = old['request'] or {}
        if not old.get('standalone') or old['phase'] != 'launching' or request.get('token') != token:
            raise ValueError('invalid/stale standalone transfer token or source')
        proof = authentic_session(root, session, fresh_after=request['launch_started_at'])
        if proof.get('standalone') or proof.get('coordinator_run') or proof['terminal'] == old['terminal']:
            raise ValueError('successor already belongs to a lifecycle or terminal is not fresh')
        if request.get('launch_receipt') and (not request.get('delivery_confirmed')
                or receipt_field(request['launch_receipt'], 'result', 'outcome', 'handle') != proof['terminal']):
            raise ValueError('claimant is not the verified launch target')
        identity = standalone_identity(root, proof)
        packet, digest = read_handoff(root, request['packet'])
        if digest != request['packet_sha256'] or checkpoint_notes(root, source) != packet['notes_sha256']:
            raise ValueError('standalone packet or notes changed')
        with session_transaction(root, session) as successor:
            successor.update(standalone=old['standalone'], predecessor=source, threshold=old['threshold'],
                             inherited_pause=packet['paused'], startup_identity=identity)
            # Persist the source fence before publishing the successor authority.
            old['phase'] = 'claimed'
            request.update(to_session=session, successor_terminal=proof['terminal'], claimed_at=now(),
                           successor_identity=identity)
            atomic(session_path(root, source), old)
    return {'owner': session, 'source': source, 'paused': packet['paused'], 'close_required': True}


def session_close(root, session, source, terminal):
    proof = authentic_session(root, session)
    if proof.get('predecessor') != source:
        raise ValueError('caller is not this transfer successor')
    with session_transaction(root, source) as old:
        request = old['request'] or {}
        if (old['phase'] != 'claimed' or request.get('to_session') != session
                or request.get('successor_terminal') != proof['terminal'] or terminal != old['terminal']
                or not request.get('delivery_confirmed') or request.get('close_started_at')
                or receipt_field(request.get('launch_receipt'), 'result', 'outcome', 'handle') != proof['terminal']):
            raise ValueError('close requires verified launch target and exact predecessor; never retry unknown close')
        standalone_identity(root, proof)
        expected = request['retiring_identity']['terminal']['result']['terminal']
        receipt = invoke(root, ['terminal', 'show', '--terminal', terminal])
        observed = receipt['result'].get('terminal') or {}
        if any(observed.get(k) != expected.get(k) for k in ('handle', 'incarnationId', 'worktreePath')):
            raise ValueError('predecessor terminal incarnation changed')
        request.update(close_started_at=now(), close_target_receipt=receipt)
    try:
        receipt = invoke(root, ['terminal', 'close', '--terminal', terminal])
    except (ValueError, OSError, subprocess.TimeoutExpired) as error:
        with session_transaction(root, source) as old:
            old['request'].update(close_error=str(error), close_failure=failure_observation(error))
        raise ValueError('close outcome unknown; inspect exact target, never blind retry') from error
    confirmed = (receipt_field(receipt, 'result', 'close', 'handle') == terminal
                 and receipt_field(receipt, 'result', 'close', 'ptyKilled') is True)
    with session_transaction(root, source) as old:
        old['request'].update(close_receipt=receipt, close_confirmed=confirmed)
    if not confirmed:
        raise ValueError('close receipt does not prove exact predecessor PTY exit')
    return receipt


def standalone_hint(state):
    session = state['owner']
    return (f'Top-level session {session} must hand off now. Save bounded notes at '
            f'.voiceport/handoffs/session-rotation-{session}.md; stop all owned schedulers with confirmed receipts. '
            f'Run python3 -I {SCRIPT} session-retire --session {session}, then '
            f'python3 -I {SCRIPT} session-launch --session {session}; end this turn after delivery. '
            'The fresh successor must session-claim and session-close the exact predecessor before work/loop. '
            + FINAL_ROUND_POLICY)


def worker_hint(root, state):
    if state.get('standalone'):
        return standalone_hint(state)
    notes = notes_path(root, state['owner']).relative_to(root).as_posix()
    return (f"Session {state['owner']} entered round {len(state['boundaries']) + 1}/5. "
            f"Stop new work. Write a bounded checkpoint to {notes}: actual task/dispatch, attempt/freeze, "
            "completed logs/exits, running tool handles, provisional findings and remaining work. "
            f"Then run python3 -I {SCRIPT} checkpoint --session {state['owner']}. "
            "End the turn; coordinator must stop the exact old worker and continue its role in a genuinely "
            "new session. If this is a directly opened top-level session with no worker assignment, instead write "
            f".voiceport/handoffs/session-contract-{state['owner']}.json with goal, full loop_prompt, authorization, "
            "paused boolean and active_tools list; then run "
            f"python3 -I {SCRIPT} session-register --session {state['owner']} "
            f"--contract .voiceport/handoffs/session-contract-{state['owner']}.json. "
            "Registration verifies real Orca membership; never fabricate a worker or take over a Run. "
            "ScheduleWakeup(stop=true) and owned CronDelete remain available. "
            "This is administrative rotation, not a product verdict. " + FINAL_ROUND_POLICY)


def session_admin(root, payload, state):
    name, data = payload.get('tool_name', '').split('.')[-1], payload.get('tool_input', {})
    if name not in ('Bash', 'exec_command'):
        return False
    command = data.get('cmd' if name == 'exec_command' else 'command', '')
    if len(command.encode()) > 48000 or any(c in command for c in (';', '\n', '\r', '\x00', '`', '$', '|', '&', '>', '<', '(', ')', '*', '?', '[', ']', '{', '}', '~')):
        return False
    try:
        argv = shlex.split(command)
    except ValueError:
        return False
    if argv[:3] == ['orca', 'terminal', 'show']:
        return len(argv) == 6 and argv[3] == '--terminal' and argv[5] == '--json' and re.fullmatch(r'term_[A-Za-z0-9_-]+', argv[4]) is not None
    if argv == ['orca', 'terminal', 'list', '--worktree', 'path:' + str(root.resolve()), '--limit', '20', '--json']:
        return True
    if argv[:3] != ['python3', '-I', SCRIPT] or len(argv) < 4 or len(argv[4:]) % 2:
        return False
    flags = argv[4::2]
    if len(flags) != len(set(flags)):
        return False
    values = dict(zip(flags, argv[5::2]))
    expected = {'--session': state['owner']}
    action = argv[3]
    if action == 'session-status':
        return values == expected or (state.get('predecessor') and values == {'--session': state['predecessor']})
    if action == 'session-register' and state['phase'] in ('active', 'requested') and not state.get('standalone'):
        return values == {**expected, '--contract': contract_path(root, state['owner']).relative_to(root).as_posix()}
    if action == 'session-retire' and state.get('standalone') and state['phase'] == 'requested':
        return values == expected
    if action == 'session-launch' and state.get('standalone') and state['phase'] == 'retired':
        return values == expected
    if action == 'session-close' and state.get('predecessor'):
        old = load(session_path(root, state['predecessor']))
        return values == {**expected, '--source': state['predecessor'], '--terminal': old['terminal']}
    return False


def standalone_hook(root, payload):
    result = worker_hook(root, payload)
    state = load(session_path(root, payload['session_id']))
    if state.get('terminal_drift') or not state.get('predecessor'):
        return result
    old = load(session_path(root, state['predecessor']))
    request = old.get('request') or {}
    if (old.get('phase') == 'claimed' and request.get('to_session') == state['owner']
            and request.get('successor_terminal') == state['terminal'] and request.get('close_confirmed')):
        return result
    event = payload['hook_event_name']
    hint = 'Successor must verify launch delivery and session-close the exact predecessor before work or loop.'
    if event == 'PreCompact':
        return {'decision': 'block', 'reason': hint}
    if event == 'PreToolUse' and not (payload.get('tool_name', '').split('.')[-1] == 'Read'
                                      or scheduler_shutdown(payload, state) or session_admin(root, payload, state)):
        return {'hookSpecificOutput': {'hookEventName': event, 'permissionDecision': 'deny', 'permissionDecisionReason': hint}}
    return result


def allowed_worker_handoff(root, payload, state):
    name = payload.get('tool_name', '').split('.')[-1]
    data = payload.get('tool_input', {})
    if name == 'exec_command':
        name, data = 'Bash', {'command': data.get('cmd', '')}
    if scheduler_shutdown(payload, state) or session_admin(root, payload, state):
        return True
    if registration_command(payload, state['owner']):
        return True
    if name in ('Read', 'Skill'):
        return True
    if name in ('Write', 'Edit'):
        if state['phase'] != 'requested':
            return False
        if data.get('file_path') == str(notes_path(root, state['owner'])):
            return writable_notes(root, state['owner'])
        path = contract_path(root, state['owner'])
        return (not state.get('standalone') and data.get('file_path') == str(path)
                and writable_notes(root, state['owner'], path.name))
    if name == 'Bash':
        try:
            argv = shlex.split(data.get('command', ''))
        except ValueError:
            return False
        if any(c in data.get('command', '') for c in (';', '\n', '`', '$', '|', '&', '>', '<')):
            return False
        base = ["python3", "-I", SCRIPT, "checkpoint", "--session", state['owner']]
        return (argv == base
                or (state['phase'] == 'requested' and argv[:6] == base
                    and len(argv) == 8 and argv[6] == '--notes-text')
                or argv == ["python3", "-I", SCRIPT, "session-status", "--session", state['owner']])
    return False


def standalone_recovery_candidate(state):
    """Eligibility hint only; the recovery entry rechecks live Orca authority."""
    return (bool(state.get('standalone')) and not state.get('coordinator_run')
            and state.get('harness') == 'claude' and bool(state.get('terminal_drift'))
            and bool(os.environ.get('ORCA_TERMINAL_HANDLE'))
            and os.environ.get('ORCA_TERMINAL_HANDLE') == state['terminal_drift'].get('actual_terminal'))


def session_recover(root, session, rotation=None):
    """Use a separate administrative lifecycle; never unlock the drifted owner."""
    import importlib.util
    spec = importlib.util.spec_from_file_location('session_recovery', Path(__file__).with_name('session_recovery.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.start(root, session, rotation=rotation or sys.modules.get(__name__))


def terminal_drift_hint(state):
    if standalone_recovery_candidate(state):
        return (f"Session {state['owner']} is fenced for terminal drift. Do not continue product work, "
                "rebind the old session, or reset its counts. Stop all owned schedulers using exact "
                "ScheduleWakeup(stop=true), CronList and proven-owned CronDelete receipts, then run "
                f"python3 -I {SCRIPT} session-recover --session {state['owner']}. "
                "This narrowly allowed entry starts one fresh management session, which collects the "
                "administrative handoff, verifies exact conflict PTY closure, then starts and verifies "
                "a fresh product successor with the original contract. After confirmed recovery prompt "
                "delivery, end this old turn; do not ask the user to copy a prompt or open a terminal. "
                "For an unknown launch, inspect the saved recovery receipt without fresh retry. "
                + FINAL_ROUND_POLICY)
    return (f"Session {state['owner']} is fenced for terminal drift: stored terminal={state.get('terminal')!r}, "
            f"actual ORCA_TERMINAL_HANDLE={os.environ.get('ORCA_TERMINAL_HANDLE')!r}; "
            f"first mismatch={state['terminal_drift']!r}. Stop ordinary work and compaction; "
            "do not rebind state, reset counts, or checkpoint/send as the old terminal. "
            "Escalate using your actual Orca preamble terminal/Task/Dispatch, then end the turn. "
            "Coordinator must capture an administrative handoff from true state/transcript and retained prior work "
            "(no successful old-terminal checkpoint), stop/release the exact mismatched assignment with actual exit proof, "
            "then start a genuinely fresh session: same Task retry if spec unchanged, explicit successor if changed; "
            "preserve logical role/Task/Dispatch history. Confirm new session ID, exact terminal and zero count; "
            "this old session stays fenced, including if reopened at its original terminal. " + FINAL_ROUND_POLICY)


def allowed_terminal_recovery(payload, state):
    """Read, owned shutdown, exact rescue, or worker escalation; no product authority."""
    tool = payload.get('tool_name', '')
    data = payload.get('tool_input', {})
    if not isinstance(tool, str) or not isinstance(data, dict):
        return False
    name = tool.split('.')[-1]
    if name == 'Read' or scheduler_shutdown(payload, state):
        return True
    if name == 'exec_command':
        name, data = 'Bash', {'command': data.get('cmd', '')}
    if name != 'Bash':
        return False
    command = data.get('command', '')
    if not isinstance(command, str) or len(command.encode('utf-8')) > 48000:
        return False
    if any(c in command for c in (';', '\n', '\r', '\x00', '`', '$', '|', '&', '>', '<', '(', ')',
                                   '*', '?', '[', ']', '{', '}', '~')):
        return False
    try:
        argv = shlex.split(command)
    except ValueError:
        return False
    if argv == ['python3', '-I', SCRIPT, 'session-status', '--session', state['owner']]:
        return True
    if (standalone_recovery_candidate(state)
            and argv == ['python3', '-I', SCRIPT, 'session-recover', '--session', state['owner']]):
        return True
    flags = {'--from', '--type', '--subject', '--body', '--task-id', '--dispatch-id'}
    if (argv[:3] != ['orca', 'orchestration', 'send'] or len(argv) != 15
            or set(argv[3::2]) != flags):
        return False
    values = dict(zip(argv[3::2], argv[4::2]))
    actual = os.environ.get('ORCA_TERMINAL_HANDLE')
    # Orca validates Task/Dispatch membership against the actual caller. Never use
    # stale registry mapping as authority or accept a send on behalf of terminal A.
    return (bool(actual) and values['--from'] == actual and values['--type'] == 'escalation'
            and bool(values['--subject'].strip()) and bool(values['--body'].strip())
            and re.fullmatch(r'task_[A-Za-z0-9_-]+', values['--task-id']) is not None
            and re.fullmatch(r'ctx_[A-Za-z0-9_-]+', values['--dispatch-id']) is not None)


def codex_output(payload, value):
    if payload.get('hook_event_name') == 'PreCompact' and value.get('decision') == 'block':
        return {'continue': False, 'stopReason': value['reason']}
    if payload.get('hook_event_name') == 'PreToolUse':
        specific = value.get('hookSpecificOutput', {})
        if specific.get('permissionDecision') == 'deny':
            return {'decision': 'block', 'reason': specific['permissionDecisionReason']}
    return value


def worker_hook(root, payload):
    event = payload.get('hook_event_name')
    with session_transaction(root, payload.get('session_id')) as state:
        harness = payload.get('rotation_harness', 'claude')
        if state.get('harness', harness) != harness:
            raise ValueError('session harness changed; reconcile identity before continuing')
        authenticated = (session_path(root, state['owner']).exists()
                         and state.get('transcript_session_verified') is True)
        actual = os.environ.get('ORCA_TERMINAL_HANDLE')
        if authenticated and (not actual or actual != state.get('terminal')):
            if not state.get('terminal_drift'):
                state['terminal_drift'] = {'stored_terminal': state.get('terminal'),
                                         'actual_terminal': actual, 'detected_at': now(), 'event': event}
                # Commit only this observation, under the session lock, BEFORE the
                # abortable scan transaction. A transcript exception must not lose it.
                atomic(session_path(root, state['owner']), state)
        transcript = payload.get('transcript_path')
        # Legacy unverified records can acquire a harness only through a successful
        # scan. Use a candidate so failed scans never persist provisional history.
        candidate = {**state, 'harness': harness, 'boundaries': list(state['boundaries'])}
        try:
            if not transcript and event in ('PreToolUse', 'PreCompact'):
                raise ValueError('transcript_path unavailable; cannot enforce per-session context limit')
            if transcript:
                scan(candidate, transcript)
        except (ValueError, OSError) as error:
            if not authenticated or not state.get('terminal_drift'):
                raise
            # Diagnostic sidecar is not authority and never changes verification,
            # count, phase, checkpoint or uncertain-delivery state. Keep the most
            # recent failed scan even after a later successful scan; Read can inspect it.
            diagnostic = {
                'owner': state['owner'], 'harness': harness, 'event': event,
                'stored_terminal': state.get('terminal'), 'actual_terminal': actual,
                'at_utc': now(), 'error_type': type(error).__name__, 'error': str(error),
                'transcript_path': transcript, 'scan_verified': False,
                'last_verified_completed_compactions': len(state['boundaries']),
                'last_verified_cursor': state.get('transcript_cursor'),
            }
            try:
                atomic(session_path(root, state['owner']).with_suffix('.scan-error.json'), diagnostic)
            except OSError as diagnostic_error:
                # This optional write cannot revoke an already committed fence.
                # stderr is the supported fallback; never claim it was persisted.
                print(f'Rotation scan failed: {error}; scan diagnostic not persisted: {diagnostic_error}',
                      file=sys.stderr)
            if event == 'Stop':
                return {}  # Failed scans never prevent recursive/native drift Stop.
            if event == 'PreToolUse' and allowed_terminal_recovery(payload, state):
                return {}  # Prior saved authority only; this scan did not verify anything.
            raise
        state.update(candidate)
        state['hook_events'] = sorted(set(state.get('hook_events', [])) | {event})
        state['hook_implementation_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        actual = os.environ.get('ORCA_TERMINAL_HANDLE')
        if not actual or actual != state.get('terminal'):
            # Latch the first mismatch without rebinding or changing the count.
            state.setdefault('terminal_drift', {'stored_terminal': state.get('terminal'),
                             'actual_terminal': actual, 'detected_at': now(), 'event': event})
        if state.get('terminal_drift'):
            hint = terminal_drift_hint(state)
            if event == 'PreCompact':
                return {'decision': 'block', 'reason': hint}
            if event == 'PreToolUse' and not allowed_terminal_recovery(payload, state):
                return {'hookSpecificOutput': {'hookEventName': event, 'permissionDecision': 'deny',
                                              'permissionDecisionReason': hint}}
            if event == 'SessionStart':
                return {'hookSpecificOutput': {'hookEventName': event, 'additionalContext': hint}}
            if (event == 'Stop' and standalone_recovery_candidate(state)
                    and not payload.get('stop_hook_active') and not paused(root)
                    and not state.get('inherited_pause')
                    and not state['standalone']['contract'].get('paused')
                    and not (root / '.voiceport/session-recoveries' / (state['owner'] + '.json')).exists()):
                return {'decision': 'block', 'reason': hint}
            return {}  # Workers, paused tasks, attempted rescue and recursive Stop may end.
        if state['phase'] == 'active':
            name = payload.get('tool_name', '').split('.')[-1]
            scheduling = name == 'CronCreate' or (name == 'ScheduleWakeup' and payload.get('tool_input', {}).get('stop') is not True)
            if (event == 'PreToolUse' and scheduling and not state.get('coordinator_run')
                    and (not state.get('standalone') or state['standalone'].get('kind') == 'task')):
                return {'hookSpecificOutput': {'hookEventName': event, 'permissionDecision': 'deny',
                        'permissionDecisionReason': 'Before starting a top-level loop, session-register its exact original prompt and authorization contract per .claude/COORDINATOR_ROTATION.md. Managed workers must use their coordinator, never install a loop.'}}
            return {}
        hint = worker_hint(root, state)
        if event == 'PreCompact':
            return {'decision': 'block', 'reason': hint}
        if event == 'PreToolUse' and not allowed_worker_handoff(root, payload, state):
            return {'hookSpecificOutput': {'hookEventName': event, 'permissionDecision': 'deny',
                                          'permissionDecisionReason': hint}}
        if event == 'SessionStart':
            return {'hookSpecificOutput': {'hookEventName': event, 'additionalContext': hint}}
        if event == 'Stop' and state['phase'] == 'requested' and not payload.get('stop_hook_active'):
            return {'decision': 'block', 'reason': hint}
        return {}


def checkpoint(root, session, notes_text=None):
    """Worker saves its context only; coordinator alone changes actual Orca lifecycle."""
    with session_transaction(root, session) as state:
        require_terminal(state)
        if state['phase'] not in ('requested', 'handoff_ready'):
            raise ValueError('session rotation was not requested or notification is already latched')
        if state['phase'] == 'handoff_ready' and notes_text is not None:
            raise ValueError('checkpoint notes are immutable after notification')
        notes = notes_path(root, session)
        digest = checkpoint_notes(root, session, notes_text)
        registry = load(root / '.voiceport/coordination.json')
        roles = [(role, row) for role, row in registry.get('workers', {}).items()
                 if row.get('terminal') == state.get('terminal') and state.get('terminal')]
        if len(roles) != 1:
            raise ValueError('session terminal is not one exact registered worker; coordinator must reconcile identity')
        role, worker = roles[0]
        if state['phase'] == 'handoff_ready':
            saved = state['checkpoint']
            if saved['notes_sha256'] != digest:
                raise ValueError('checkpoint notes changed after notification; coordinator must reconcile')
            if (saved['role'], saved['run'], saved['task'], saved['dispatch']) != (
                    role, registry['run_id'], worker['task_id'], worker['dispatch_id']):
                raise ValueError('checkpoint registry identity changed; coordinator must reconcile')
            require_terminal(state)
            return saved
        value = {'session': session, 'terminal': state['terminal'], 'role': role,
                 'run': registry['run_id'], 'task': worker['task_id'], 'dispatch': worker['dispatch_id'],
                 'product_attempt': registry['product_attempt'], 'freeze': registry.get('freeze'),
                 'notes': str(notes.relative_to(root)), 'notes_sha256': digest,
                 'completed_compactions': len(state['boundaries']), 'policy': FINAL_ROUND_POLICY,
                 'at_utc': now(), 'product_verdict': 'not_completed_administrative_rotation'}
        # Persist before sending; uncertain delivery must not produce duplicate messages.
        state.update(phase='notifying', checkpoint=value)
    try:
        if checkpoint_notes(root, session) != value['notes_sha256']:
            raise ValueError('checkpoint notes changed before notification')
        require_terminal(state)
        receipt = invoke(root, ['orchestration', 'send', '--from', value['terminal'],
                               '--to', 'run:' + value['run'], '--type', 'escalation',
                               '--subject', 'Session rotation checkpoint ready',
                               '--body', json.dumps(value, ensure_ascii=False),
                               '--task-id', value['task'], '--dispatch-id', value['dispatch']])
    except (ValueError, OSError, subprocess.TimeoutExpired) as error:
        with session_transaction(root, session) as state:
            state['notify_error'] = str(error)
            state['notify_failure'] = failure_observation(error)
        raise ValueError('checkpoint saved; delivery unknown; coordinator must inspect before retry') from error
    with session_transaction(root, session) as state:
        state.update(phase='handoff_ready', notify_receipt=receipt)
    return value


def hook(root, payload):
    if not isinstance(payload, dict) or payload.get('hook_event_name') not in HOOK_EVENTS:
        raise ValueError('invalid hook event payload')
    event = payload['hook_event_name']
    session = payload.get('session_id')
    session_path(root, session)  # Validate before routing through any saved role.
    if event == 'PreToolUse':
        tool, data = payload.get('tool_name', ''), payload.get('tool_input', {})
        if not isinstance(tool, str) or not isinstance(data, dict):
            raise ValueError('invalid tool payload')
        command_key = 'cmd' if tool.split('.')[-1] == 'exec_command' else 'command'
        if command_key in data and not isinstance(data[command_key], str):
            raise ValueError('invalid tool command')
    if payload.get('transcript_path') is not None and not isinstance(payload['transcript_path'], str):
        raise ValueError('invalid transcript path')
    saved_path = session_path(root, session)
    transcript = payload.get('transcript_path')
    if (event == 'SessionStart' and not saved_path.exists()
            and (not transcript or not Path(transcript).exists())):
        # The host can announce startup before creating its JSONL. This is not
        # transcript authority: create no state/proof and require the first actual
        # tool hook to verify identity, accounting and the current implementation.
        return {'hookSpecificOutput': {'hookEventName': event, 'additionalContext':
                'Rotation startup transcript is not yet available. Before work or loop, '
                'the first real PreToolUse must verify actual session/terminal and context accounting.'}}
    if saved_path.exists():
        saved = load(saved_path)
        validate_session_state(saved, session)
        if saved.get('coordinator_run') is not None:
            return run_hook(root, payload, saved['coordinator_run'])
    with transaction(root) as state:
        if state is None:
            return standalone_hook(root, payload)
        validate_hook_accounting(state)
        if state.get('migrated_run') and session == state['owner']:
            return run_hook(root, payload, state['migrated_run'])
        retired = state.get('retired_sessions')
        if (not isinstance(retired, list) or any(not isinstance(s, str) or not s for s in retired)
                or state['phase'] not in ('active', 'requested', 'retired', 'launching')
                or ('harness' in state and state['harness'] not in ('claude', 'codex'))):
            raise ValueError('invalid saved coordinator hook routing')
        if session in retired:
            if event == 'PreCompact':
                return {'decision': 'block', 'reason': 'This coordinator retired; resume its successor.'}
            if event == 'PreToolUse' and not allowed_during_rotation(payload, state):
                return {'hookSpecificOutput': {'hookEventName': event, 'permissionDecision': 'deny',
                        'permissionDecisionReason': 'This coordinator retired. Do not write, dispatch, consume mail or restart its loop.'}}
            return {}
        if session != state['owner']:
            return standalone_hook(root, payload)  # Per-session counters never change the coordinator lease.
        result = worker_hook(root, payload)  # Produce authentic proof for guarded legacy upgrade.
        proof = load(session_path(root, session))
        if proof.get('terminal_drift'):
            return result
        transcript = payload.get('transcript_path')
        if not transcript and event in ('PreToolUse', 'PreCompact'):
            raise ValueError('transcript_path unavailable; cannot enforce coordinator context limit')
        if transcript:
            # Reuse this hook's verified fixed snapshot. A second scan could
            # consume an appended boundary inside the same tool permission check.
            merge_session_proof(state, proof)
        if state['phase'] == 'active':
            return {}
        hint = rotation_hint(state)
        if event == 'PreCompact':
            return {'decision': 'block', 'reason': hint}
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
    hook_cmd = sub.add_parser('hook')
    hook_cmd.add_argument('--harness', choices=('claude', 'codex'), default='claude')
    sub.add_parser('status').add_argument('--run')
    for action in ('adopt', 'register', 'retire', 'launch', 'claim', 'close-retired', 'configure', 'checkpoint', 'session-status',
                   'session-register', 'session-task-register', 'session-request', 'session-retire', 'session-launch', 'session-claim', 'session-close', 'session-recover', 'upgrade-legacy'):
        cmd = sub.add_parser(action)
        cmd.add_argument('--session', required=True)
        if action in ('retire', 'launch', 'claim', 'configure'):
            cmd.add_argument('--run')
        if action == 'upgrade-legacy':
            cmd.add_argument('--run', required=True)
        if action == 'close-retired':
            cmd.add_argument('--run', required=True)
            cmd.add_argument('--terminal', required=True)
        if action in ('adopt', 'register', 'retire'):
            cmd.add_argument('--transcript', required=True)
        if action == 'register':
            cmd.add_argument('--navigation', required=True)
            cmd.add_argument('--run', required=True)
            cmd.add_argument('--goal', required=True)
            prompt = cmd.add_mutually_exclusive_group(required=True)
            prompt.add_argument('--loop-prompt')
            prompt.add_argument('--loop-prompt-file', choices=('.claude/loop.md',))
        if action == 'adopt':
            cmd.add_argument('--run', required=True)
            cmd.add_argument('--threshold', type=int, default=DEFAULT_THRESHOLD)
            cmd.add_argument('--goal', required=True)
            cmd.add_argument('--loop-prompt', required=True)
            cmd.add_argument('--retired-session', action='append', default=[])
        if action == 'session-task-register':
            cmd.add_argument('--task-uuid', required=True)
            cmd.add_argument('--authorization-uuid', required=True)
            cmd.add_argument('--goal', required=True)
        if action == 'session-register':
            cmd.add_argument('--contract', required=True)
        if action == 'session-request':
            cmd.add_argument('--reason', required=True)
        if action in ('session-claim', 'session-close'):
            cmd.add_argument('--source', required=True)
        if action == 'session-claim':
            cmd.add_argument('--token', required=True)
        if action == 'session-close':
            cmd.add_argument('--terminal', required=True)
        if action == 'checkpoint':
            cmd.add_argument('--notes-text', help='bounded single-line notes for native Codex exec_command')
        if action == 'configure':
            cmd.add_argument('--threshold', type=int, required=True)
            cmd.add_argument('--goal', help='update the goal carried into future handoffs')
        if action == 'claim':
            cmd.add_argument('--token', required=True)
    args = parser.parse_args()
    try:
        if args.action == 'hook':
            payload = json.load(sys.stdin)
            if not isinstance(payload, dict):
                raise ValueError('hook payload must be an object')
            payload['rotation_harness'] = args.harness
            value = hook(args.root, payload)
            if args.harness == 'codex':
                value = codex_output(payload, value)
        elif args.action == 'upgrade-legacy':
            value = upgrade_legacy(args.root, args.session, args.run)
        elif args.action == 'session-task-register':
            value = session_task_register(args.root, args.session, args.task_uuid, args.authorization_uuid, args.goal)
        elif args.action == 'session-register':
            value = session_register(args.root, args.session, args.contract)
        elif args.action == 'session-request':
            value = session_request(args.root, args.session, args.reason)
        elif args.action == 'session-recover':
            value = session_recover(args.root, args.session)
        elif args.action == 'session-retire':
            value = session_retire(args.root, args.session)
        elif args.action == 'session-launch':
            value = session_launch(args.root, args.session)
        elif args.action == 'session-claim':
            value = session_claim(args.root, args.session, args.source, args.token)
        elif args.action == 'session-close':
            value = session_close(args.root, args.session, args.source, args.terminal)
        elif args.action == 'checkpoint':
            value = checkpoint(args.root, args.session, args.notes_text)
        elif args.action == 'session-status':
            value = load(session_path(args.root, args.session))
        elif args.action == 'status':
            state = load(coordinator_path(args.root, args.run))
            value = state if args.run else {key: state[key] for key in ('owner', 'epoch', 'phase', 'threshold', 'boundaries')}
        elif args.action == 'register':
            loop_prompt = args.loop_prompt
            if args.loop_prompt_file:
                path = args.root / args.loop_prompt_file
                if path.is_symlink() or not path.is_file() or path.stat().st_size > 48000:
                    raise ValueError('loop prompt must be a regular project loop file of at most 48KB')
                loop_prompt = path.read_text()
            value = register(args.root, args.session, args.run, args.transcript, args.goal, loop_prompt, args.navigation)
        elif args.action == 'close-retired':
            value = close_retired(args.root, args.session, args.run, args.terminal)
        elif args.action == 'adopt':
            value = adopt(args.root, args.session, args.run, args.transcript, args.threshold,
                          args.goal, args.loop_prompt, args.retired_session)
            value = {key: value[key] for key in ('owner', 'epoch', 'phase', 'threshold')}
        elif args.action == 'retire':
            value = retire(args.root, args.session, args.transcript, args.run)
        elif args.action == 'launch':
            value = launch(args.root, args.session, args.run)
        elif args.action == 'configure':
            value = configure(args.root, args.session, args.threshold, args.goal, args.run)
        else:
            value = claim(args.root, args.session, args.token, args.run)
        print(json.dumps(value, ensure_ascii=False))
    except (ValueError, OSError, KeyError, subprocess.TimeoutExpired) as error:
        if args.action == 'hook':
            print(f'Coordinator rotation hook failed: {error}', file=sys.stderr)
            # Block tools on broken state instead of silently bypassing fencing.
            failed_payload = locals().get('payload')
            event = failed_payload.get('hook_event_name') if isinstance(failed_payload, dict) else None
            if event == 'PreCompact' and args.harness == 'codex':
                print(json.dumps({'continue': False, 'stopReason': 'Rotation state unreadable; compaction refused.'}))
                sys.exit(0)
            sys.exit(2 if event in ('PreToolUse', 'PreCompact') else 1)
        parser.exit(1, f'coordinator-rotation: {error}\n')


if __name__ == '__main__':
    main()
