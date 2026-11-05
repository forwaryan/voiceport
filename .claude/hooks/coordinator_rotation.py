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
    offset = 0
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
    state['transcript_session_verified'] = True
    state['transcript_cursor'] = {'path': str(path), 'offset': offset, 'inode': before.st_ino}
    request_rotation(state)


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
                  f"Inherit the packet's rotation_threshold ({state['threshold']}). {FINAL_ROUND_POLICY} "
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


def configure(root, session, threshold, goal=None):
    validate_threshold(threshold)
    with transaction(root) as state:
        require_owner(state, session)
        if state['phase'] != 'active':
            raise ValueError('cannot change threshold during a transfer')
        state['threshold'] = threshold
        if goal is not None:
            state['goal'] = goal
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
    return (f"Coordinator entered context round {len(state['boundaries']) + 1}/{state['threshold']} "
            f"({len(state['boundaries'])} completed compactions); no further compaction is allowed. "
            f"Finish the current tool boundary, call ScheduleWakeup(stop=true), then Bash: "
            f"python3 -I {SCRIPT} retire --session {state['owner']} --transcript YOUR_TRANSCRIPT_PATH; "
            f"then python3 -I {SCRIPT} launch --session {state['owner']}. "
            f"End this coordinator turn after launch receipt. Preserve workers/freeze; do not start new work. {FINAL_ROUND_POLICY}")


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


def writable_notes(root, session):
    """Validate the permitted tool path without opening its file or creating dirs."""
    try:
        with handoff_directory(root) as directory:
            try:
                notes_stat(directory, notes_path(root, session).name)
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
        before = path.read_bytes() if path.exists() else None
        yield state
        if before is None or load(path) != state:
            atomic(path, state)


def worker_hint(root, state):
    notes = notes_path(root, state['owner']).relative_to(root).as_posix()
    return (f"Session {state['owner']} entered round {len(state['boundaries']) + 1}/5. "
            f"Stop new work. Write a bounded checkpoint to {notes}: actual task/dispatch, attempt/freeze, "
            "completed logs/exits, running tool handles, provisional findings and remaining work. "
            f"Then run python3 -I {SCRIPT} checkpoint --session {state['owner']}. "
            "End the turn; coordinator must stop the exact old worker and continue its role in a genuinely "
            "new session. This is administrative rotation, not a product verdict. " + FINAL_ROUND_POLICY)


def allowed_worker_handoff(root, payload, state):
    name = payload.get('tool_name', '').split('.')[-1]
    data = payload.get('tool_input', {})
    if name == 'exec_command':
        name, data = 'Bash', {'command': data.get('cmd', '')}
    if name in ('Read', 'Skill'):
        return True
    if name in ('Write', 'Edit'):
        return (state['phase'] == 'requested'
                and data.get('file_path') == str(notes_path(root, state['owner']))
                and writable_notes(root, state['owner']))
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


def terminal_drift_hint(state):
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
    """Read/status or one exact actual-caller escalation, never arbitrary shell/notes/skills."""
    name = payload.get('tool_name', '').split('.')[-1]
    data = payload.get('tool_input', {})
    if name == 'Read':
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
        state['harness'] = harness
        state['hook_events'] = sorted(set(state.get('hook_events', [])) | {event})
        state['hook_implementation_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        transcript = payload.get('transcript_path')
        if not transcript and event in ('PreToolUse', 'PreCompact'):
            raise ValueError('transcript_path unavailable; cannot enforce per-session context limit')
        if transcript:
            scan(state, transcript)
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
            return {}  # Ending the turn is recovery, including recursive/native Stop.
        if state['phase'] == 'active':
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
        raise ValueError('checkpoint saved; delivery unknown; coordinator must inspect before retry') from error
    with session_transaction(root, session) as state:
        state.update(phase='handoff_ready', notify_receipt=receipt)
    return value


def hook(root, payload):
    event = payload.get('hook_event_name')
    session = payload.get('session_id')
    with transaction(root) as state:
        if not state:
            return {}
        if session in state['retired_sessions']:
            if event == 'PreCompact':
                return {'decision': 'block', 'reason': 'This coordinator retired; resume its successor.'}
            if event == 'PreToolUse' and not allowed_during_rotation(payload, state):
                return {'hookSpecificOutput': {'hookEventName': event, 'permissionDecision': 'deny',
                        'permissionDecisionReason': 'This coordinator retired. Do not write, dispatch, consume mail or restart its loop.'}}
            return {}
        if session != state['owner']:
            return worker_hook(root, payload)  # Per-session counters never change the coordinator lease.
        transcript = payload.get('transcript_path')
        if not transcript and event in ('PreToolUse', 'PreCompact'):
            raise ValueError('transcript_path unavailable; cannot enforce coordinator context limit')
        if transcript:
            scan(state, transcript)
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
    sub.add_parser('status')
    for action in ('adopt', 'retire', 'launch', 'claim', 'configure', 'checkpoint', 'session-status'):
        cmd = sub.add_parser(action)
        cmd.add_argument('--session', required=True)
        if action in ('adopt', 'retire'):
            cmd.add_argument('--transcript', required=True)
        if action == 'adopt':
            cmd.add_argument('--run', required=True)
            cmd.add_argument('--threshold', type=int, default=DEFAULT_THRESHOLD)
            cmd.add_argument('--goal', required=True)
            cmd.add_argument('--loop-prompt', required=True)
            cmd.add_argument('--retired-session', action='append', default=[])
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
            payload['rotation_harness'] = args.harness
            value = hook(args.root, payload)
            if args.harness == 'codex':
                value = codex_output(payload, value)
        elif args.action == 'checkpoint':
            value = checkpoint(args.root, args.session, args.notes_text)
        elif args.action == 'session-status':
            value = load(session_path(args.root, args.session))
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
            value = configure(args.root, args.session, args.threshold, args.goal)
        else:
            value = claim(args.root, args.session, args.token)
        print(json.dumps(value, ensure_ascii=False))
    except (ValueError, OSError, KeyError, subprocess.TimeoutExpired) as error:
        if args.action == 'hook':
            print(f'Coordinator rotation hook failed: {error}', file=sys.stderr)
            # Block tools on broken state instead of silently bypassing fencing.
            event = locals().get('payload', {}).get('hook_event_name')
            if event == 'PreCompact' and args.harness == 'codex':
                print(json.dumps({'continue': False, 'stopReason': 'Rotation state unreadable; compaction refused.'}))
                sys.exit(0)
            sys.exit(2 if event in ('PreToolUse', 'PreCompact') else 1)
        parser.exit(1, f'coordinator-rotation: {error}\n')


if __name__ == '__main__':
    main()
