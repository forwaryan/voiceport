"""Lifecycle checks use temporary projects and mocked Orca; never launch real agents."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('rotation', Path(__file__).with_name('coordinator_rotation.py'))
r = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(r)


def installed_launch_receipt(outcome='journaled'):
    # Installed agentLaunchFixture/formatAgentLaunch use result.prompt directly.
    # Journaled structured delivery alone is not proof of a terminal successor.
    receipt = {'id': 'req_launch', 'ok': True, 'result': {
        'outcome': {'kind': 'structured', 'sessionId': 'codex_1', 'handle': 'agent_1'},
        'worktreeId': 'synthetic-worktree',
        'receipt': {'mode': 'structured', 'preferred': 'structured', 'reason': 'user_default',
                    'detail': 'Started a structured chat session, the default for new agent tabs in your settings.'},
        'prompt': {'delivery': 'submit', 'outcome': outcome, 'messageId': 'msg_1'}},
        '_meta': {'runtimeId': 'runtime-1'}}
    if outcome == 'handed-to-terminal':
        receipt['result']['outcome'] = {'kind': 'terminal', 'handle': 'term_fixture'}
        receipt['result']['receipt'].update(mode='terminal', preferred='terminal', detail='Synthetic terminal delivery')
        del receipt['result']['prompt']['messageId']
    return receipt


class RotationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / '.voiceport').mkdir()
        self.path = self.root / '.voiceport' / r.STATE_NAME
        self.transcripts = {}
        self.transcript = self.transcript_for('old')
        terminal = patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'worker-terminal'})
        terminal.start()
        self.addCleanup(terminal.stop)
        self.state = {'schema_version': 1, 'owner': 'old', 'epoch': 1, 'run_id': 'run_one',
                      'phase': 'active', 'threshold': 3, 'boundaries': [], 'retired_sessions': [],
                      'request': None, 'history': [], 'goal': 'deliver', 'loop_prompt': 'continue safely'}
        r.atomic(self.path, self.state)
        r.atomic(self.root / '.voiceport/coordination.json', {'run_id': 'run_one',
                 'developer': {'dispatch': 'developer'}, 'verifier': {'dispatch': 'verifier'},
                 'freeze': {'sha256': 'unchanged'}, 'product_attempt': 'same-attempt'})
        r.atomic(self.root / '.voiceport/loop-state.json', {'paused': True, 'tasks': {
            'VP-013': {'status': 'running', 'attempts': [{'id': 'same-attempt'}]}}})

    def transcript_for(self, session):
        # Real Claude transcripts have one session identity, including before
        # their first compaction. Each synthetic role now has its own transcript.
        if session not in self.transcripts:
            path = self.root / (session + '.jsonl')
            path.write_text(json.dumps({'type': 'user', 'sessionId': session}) + '\n')
            self.transcripts[session] = path
        return self.transcripts[session]

    def append(self, value, session='old'):
        with self.transcript_for(session).open('a') as out:
            out.write(json.dumps(value) + '\n')

    def boundary(self, identity, session='old'):
        self.append({'subtype': 'compact_boundary', 'uuid': identity, 'sessionId': session}, session)

    def payload(self, event='PreToolUse', session='old', name='Edit', data=None):
        return {'hook_event_name': event, 'session_id': session, 'transcript_path': str(self.transcript_for(session)),
                'tool_name': name, 'tool_input': data or {}}

    def request(self):
        self.boundary('one')
        self.boundary('two')
        r.hook(self.root, self.payload())

    def stop(self):
        self.append({'sessionId': 'old', 'message': {'content': [
            {'type': 'tool_use', 'name': 'ScheduleWakeup', 'id': 'stop', 'input': {'stop': True}}]}})
        self.append({'sessionId': 'old', 'message': {'content': [
            {'type': 'tool_result', 'tool_use_id': 'stop', 'content': 'Loop stopped'}]}})

    def test_completed_boundaries_deduplicate_across_both_hook_channels(self):
        self.boundary('one')
        for event in ('PostCompact', 'SessionStart', 'PreToolUse'):
            r.hook(self.root, self.payload(event))
        self.assertEqual(r.load(self.path)['boundaries'], ['one'])
        self.boundary('one')
        self.boundary('worker', 'verifier')
        self.assertEqual(r.hook(self.root, self.payload()), {})
        self.boundary('two')
        result = r.hook(self.root, self.payload())
        self.assertEqual(result['hookSpecificOutput']['permissionDecision'], 'deny')
        request = r.load(self.path)['request']['id']
        r.hook(self.root, self.payload('PostCompact'))
        self.assertEqual(r.load(self.path)['request']['id'], request)

    def test_partial_transcript_and_delayed_postcompact_are_reconciled(self):
        self.assertEqual(r.hook(self.root, self.payload('PostCompact')), {})
        value = json.dumps({'subtype': 'compact_boundary', 'uuid': 'late', 'sessionId': 'old'})
        with self.transcript.open('a') as out:
            out.write(value[:20])
        r.hook(self.root, self.payload())
        self.assertEqual(r.load(self.path)['boundaries'], [])
        with self.transcript.open('a') as out:
            out.write(value[20:] + '\n')
        r.hook(self.root, self.payload())
        self.assertEqual(r.load(self.path)['boundaries'], ['late'])

    def test_replaced_transcript_does_not_double_count(self):
        self.boundary('one')
        r.hook(self.root, self.payload())
        old = self.transcript.with_suffix('.old')
        self.transcript.rename(old)
        self.transcript.touch()
        self.boundary('one')
        self.boundary('two')
        r.hook(self.root, self.payload())
        self.assertEqual(r.load(self.path)['boundaries'], ['one', 'two'])

    def test_worker_counts_independently_without_writing_coordinator_lease(self):
        before = self.path.read_bytes(), self.path.stat().st_mtime_ns
        self.boundary('one', 'worker')
        self.assertEqual(r.hook(self.root, self.payload(session='worker')), {})
        self.assertEqual((self.path.read_bytes(), self.path.stat().st_mtime_ns), before)
        self.assertEqual(r.load(r.session_path(self.root, 'worker'))['boundaries'], ['one'])

    def test_threshold_fences_dispatch_and_schedule_but_allows_retirement(self):
        self.request()
        for name, data in [('Edit', {}), ('Bash', {'command': 'orca orchestration send --to x'}),
                           ('ScheduleWakeup', {'delaySeconds': 1200})]:
            result = r.hook(self.root, self.payload(name=name, data=data))
            self.assertEqual(result['hookSpecificOutput']['permissionDecision'], 'deny')
        for name, data in [('Read', {}), ('ScheduleWakeup', {'stop': True}),
                           ('Bash', {'command': f'python3 -I {r.SCRIPT} retire --session old --transcript x'})]:
            self.assertEqual(r.hook(self.root, self.payload(name=name, data=data)), {})
        hostile = {'command': f'python3 -I {r.SCRIPT} status; orca orchestration send --to x'}
        self.assertIn('hookSpecificOutput', r.hook(self.root, self.payload(name='Bash', data=hostile)))

    def test_stop_gate_is_bounded_and_never_blocks_retired_source(self):
        self.request()
        self.assertEqual(r.hook(self.root, self.payload('Stop'))['decision'], 'block')
        self.assertEqual(r.hook(self.root, {**self.payload('Stop'), 'stop_hook_active': True}), {})
        self.stop()
        r.retire(self.root, 'old', str(self.transcript))
        self.assertEqual(r.hook(self.root, self.payload('Stop')), {})

    def test_retirement_requires_real_success_after_latest_schedule(self):
        self.request()
        with self.assertRaisesRegex(ValueError, 'ScheduleWakeup'):
            r.retire(self.root, 'old', str(self.transcript))
        self.stop()
        self.append({'sessionId': 'old', 'message': {'content': [
            {'type': 'tool_use', 'name': 'ScheduleWakeup', 'id': 'restart', 'input': {'delaySeconds': 1200}}]}})
        with self.assertRaisesRegex(ValueError, 'ScheduleWakeup'):
            r.retire(self.root, 'old', str(self.transcript))

    def test_launch_failure_is_latched_and_does_not_spawn_twice(self):
        self.request()
        self.stop()
        r.retire(self.root, 'old', str(self.transcript))
        with patch.object(r, 'invoke', side_effect=subprocess.TimeoutExpired('orca', 45)) as invoke:
            with self.assertRaisesRegex(ValueError, 'unknown'):
                r.launch(self.root, 'old')
            with self.assertRaisesRegex(ValueError, 'already attempted'):
                r.launch(self.root, 'old')
            self.assertEqual(invoke.call_count, 1)
        self.assertEqual(r.load(self.path)['phase'], 'launching')

    def test_claim_preserves_workers_pause_and_attempt_then_fences_old_owner(self):
        before = (self.root / '.voiceport/coordination.json').read_bytes()
        self.request()
        self.stop()
        request = r.retire(self.root, 'old', str(self.transcript))
        packet = r.load(self.root / request['packet'])
        self.assertTrue(packet['paused'])
        self.assertEqual(packet['tasks']['VP-013']['attempt'], 'same-attempt')
        with patch.object(r, 'invoke', return_value=installed_launch_receipt()):
            r.launch(self.root, 'old')
        token = r.load(self.path)['request']['token']
        with patch.object(r, 'bound_run', return_value={'verified': True}):
            claim = r.claim(self.root, 'new', token)
            with self.assertRaisesRegex(ValueError, 'stale'):
                r.claim(self.root, 'other', token)
        self.assertEqual(claim, {'owner': 'new', 'epoch': 2, 'paused': True})
        self.assertEqual(r.load(self.path)['boundaries'], [])
        self.assertEqual((self.root / '.voiceport/coordination.json').read_bytes(), before)
        result = r.hook(self.root, self.payload(name='Write'))
        self.assertEqual(result['hookSpecificOutput']['permissionDecision'], 'deny')
        self.assertEqual(r.hook(self.root, self.payload(session='verifier')), {})

    def test_failed_binding_or_packet_tamper_does_not_grant_lease(self):
        self.request()
        self.stop()
        request = r.retire(self.root, 'old', str(self.transcript))
        with patch.object(r, 'invoke', return_value=installed_launch_receipt('handed-to-terminal')):
            r.launch(self.root, 'old')
        token = r.load(self.path)['request']['token']
        with patch.object(r, 'bound_run', side_effect=ValueError('not bound')):
            with self.assertRaisesRegex(ValueError, 'not bound'):
                r.claim(self.root, 'new', token)
        (self.root / request['packet']).write_text('{}')
        with patch.object(r, 'bound_run', return_value={}):
            with self.assertRaisesRegex(ValueError, 'changed'):
                r.claim(self.root, 'new', token)
        self.assertEqual(r.load(self.path)['owner'], 'old')

    def test_claim_before_launch_returns_is_not_overwritten_by_launch_receipt(self):
        self.request()
        self.stop()
        r.retire(self.root, 'old', str(self.transcript))
        def launched(*_):
            token = r.load(self.path)['request']['token']
            with patch.object(r, 'bound_run', return_value={}):
                r.claim(self.root, 'new', token)
            return installed_launch_receipt()
        with patch.object(r, 'invoke', side_effect=launched):
            r.launch(self.root, 'old')
        self.assertEqual(r.load(self.path)['owner'], 'new')
        self.assertEqual(r.load(self.path)['phase'], 'active')

    def test_configure_threshold_only_by_active_owner(self):
        r.configure(self.root, 'old', 1)
        self.boundary('one')
        r.hook(self.root, self.payload())
        self.assertEqual(r.load(self.path)['phase'], 'requested')
        with self.assertRaisesRegex(ValueError, 'transfer'):
            r.configure(self.root, 'old', 3)
        with self.assertRaisesRegex(ValueError, 'own'):
            r.configure(self.root, 'worker', 3)

    def test_launch_without_delivery_receipt_remains_latched(self):
        self.request()
        self.stop()
        r.retire(self.root, 'old', str(self.transcript))
        with patch.object(r, 'invoke', return_value={'ok': True, 'result': {'launch': {}}}):
            with self.assertRaisesRegex(ValueError, 'prompt delivery'):
                r.launch(self.root, 'old')
        state = r.load(self.path)
        self.assertFalse(state['request']['delivery_confirmed'])
        self.assertIn('--prompt', state['request']['launch_argv'])
        self.assertEqual(state['phase'], 'launching')

    def test_adopt_cli_defaults_to_five_context_rounds(self):
        argv = ['rotation', 'adopt', '--session', 'new', '--run', 'run_one',
                '--transcript', str(self.transcript), '--goal', 'deliver', '--loop-prompt', 'continue']
        with patch.object(sys, 'argv', argv), patch.object(r, 'adopt', return_value=self.state) as adopt, patch('builtins.print'):
            r.main()
        self.assertEqual(adopt.call_args.args[4], 5)

    def test_five_context_policy_hands_off_after_four_and_survives_claim(self):
        r.configure(self.root, 'old', 5, goal='deliver with five context rounds')
        for number in range(1, 4):
            self.boundary(str(number))
            self.assertEqual(r.hook(self.root, self.payload()), {})
        self.assertEqual(r.load(self.path)['phase'], 'active')
        self.boundary('4')
        self.assertEqual(r.hook(self.root, self.payload())['hookSpecificOutput']['permissionDecision'], 'deny')
        self.stop()
        request = r.retire(self.root, 'old', str(self.transcript))
        packet = r.load(self.root / request['packet'])
        self.assertEqual(packet['rotation_threshold'], 5)
        self.assertEqual(packet['goal'], 'deliver with five context rounds')
        self.assertEqual(packet['rotation_policy'], r.FINAL_ROUND_POLICY)
        with patch.object(r, 'invoke', return_value=installed_launch_receipt()):
            r.launch(self.root, 'old')
        state = r.load(self.path)
        self.assertIn(r.FINAL_ROUND_POLICY, state['request']['launch_argv'][-1])
        with patch.object(r, 'bound_run', return_value={}):
            r.claim(self.root, 'new', state['request']['token'])
        self.assertEqual(r.load(self.path)['threshold'], 5)
        self.assertEqual(r.load(self.path)['boundaries'], [])

    def test_last_round_vetoes_both_manual_and_automatic_compaction(self):
        self.request()
        for trigger in ('manual', 'auto'):
            result = r.hook(self.root, {**self.payload('PreCompact'), 'trigger': trigger})
            self.assertEqual(result['decision'], 'block')
        self.assertEqual(len(r.load(self.path)['boundaries']), 2)

    def test_all_worker_roles_stop_after_four_without_changing_the_lease(self):
        before = self.path.read_bytes()
        for session in ('developer', 'verifier', 'evaluator', 'protocol-support'):
            for index in range(4):
                self.boundary(f'{session}-{index}', session)
            payload = self.payload(session=session)
            self.assertEqual(r.hook(self.root, payload)['hookSpecificOutput']['permissionDecision'], 'deny')
            self.assertEqual(r.hook(self.root, {**payload, 'hook_event_name': 'PreCompact'})['decision'], 'block')
            self.assertEqual(len(r.load(r.session_path(self.root, session))['boundaries']), 4)
        self.assertEqual(self.path.read_bytes(), before)
        self.assertEqual(r.hook(self.root, self.payload(session='fresh-worker')), {})
        self.assertEqual(r.load(r.session_path(self.root, 'fresh-worker'))['boundaries'], [])

    def test_terminal_drift_fences_active_work_and_compaction_without_reset(self):
        for harness in ('claude', 'codex'):
            session = 'drift-' + harness
            if harness == 'codex':
                self.transcript_for(session).write_text(json.dumps({'type': 'session_meta', 'payload': {'id': session}}) + '\n')
                self.append({'type': 'compacted', 'payload': {'window_id': 'one'}}, session)
            else:
                self.boundary('one', session)
            payload = {**self.payload(session=session), 'rotation_harness': harness}
            self.assertEqual(r.hook(self.root, payload), {})
            with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}):
                self.assertEqual(r.hook(self.root, payload).get('hookSpecificOutput', {}).get('permissionDecision'), 'deny')
                start = r.hook(self.root, {**payload, 'hook_event_name': 'SessionStart'})
                self.assertIn('terminal', start['hookSpecificOutput']['additionalContext'])
                for name, data in [('Edit', {}), ('Bash', {'command': 'cargo test'}),
                                   ('Write', {'file_path': str(r.notes_path(self.root, session))}),
                                   ('Skill', {'skill': 'run'})]:
                    value = r.hook(self.root, {**payload, 'tool_name': name, 'tool_input': data})
                    self.assertEqual(r.codex_output(payload, value).get('decision') if harness == 'codex'
                                     else value['hookSpecificOutput']['permissionDecision'],
                                     'block' if harness == 'codex' else 'deny')
                compact = {**payload, 'hook_event_name': 'PreCompact'}
                result = r.hook(self.root, compact)
                if harness == 'codex':
                    result = r.codex_output(compact, result)
                self.assertEqual(result.get('continue') if harness == 'codex' else result['decision'],
                                 False if harness == 'codex' else 'block')
                for active in (False, True):
                    self.assertEqual(r.hook(self.root, {**payload, 'hook_event_name': 'Stop', 'stop_hook_active': active}), {})
            state = r.load(r.session_path(self.root, session))
            self.assertEqual(state['terminal'], 'worker-terminal')
            self.assertEqual(state['boundaries'], ['codex:one'] if harness == 'codex' else ['one'])
            self.assertEqual(state['terminal_drift']['actual_terminal'], 'terminal-B')
            # Returning to A, even below the cap, cannot reopen a drifted/settled role.
            self.assertEqual(r.hook(self.root, payload)['hookSpecificOutput']['permissionDecision'], 'deny')

    def test_terminal_drift_only_allows_read_status_and_actual_dispatch_escalation(self):
        import shlex
        payload = self.payload(session='worker')
        r.hook(self.root, payload)
        with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}):
            r.hook(self.root, payload)
            command = shlex.join(['orca', 'orchestration', 'send', '--from', 'terminal-B', '--type', 'escalation',
                                  '--subject', 'Blocked terminal drift', '--body', 'Stop work. Administrative recovery required.',
                                  '--task-id', 'task_actual', '--dispatch-id', 'ctx_actual'])
            for name, data in [('Read', {}), ('Bash', {'command': f'python3 -I {r.SCRIPT} session-status --session worker'}),
                               ('Bash', {'command': command}), ('exec_command', {'cmd': command})]:
                self.assertEqual(r.hook(self.root, {**payload, 'tool_name': name, 'tool_input': data}), {})
            for bad in [command.replace('terminal-B', 'worker-terminal'), command.replace('escalation', 'worker_done'),
                        command + ' --to run_other', command + '; cargo test', command + ' --body duplicate',
                        command.replace('Blocked terminal drift', '*'), command.replace('Blocked terminal drift', '{a,b}'),
                        command.replace('ctx_actual', 'unknown'), f'python3 -I {r.SCRIPT} checkpoint --session worker',
                        'python3 -c "print(1)"', 'orca orchestration send --from terminal-B']:
                result = r.hook(self.root, {**payload, 'tool_name': 'Bash', 'tool_input': {'command': bad}})
                self.assertEqual(result.get('hookSpecificOutput', {}).get('permissionDecision'), 'deny', bad)

    def test_terminal_drift_fourth_boundary_cannot_checkpoint_as_old_caller(self):
        notes = self.worker_requested()
        notes.parent.mkdir(parents=True, exist_ok=True)
        notes.write_text('Retained original task/dispatch history and work.')
        prior = notes.read_bytes()
        with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}), patch.object(r, 'invoke') as invoke:
            r.hook(self.root, self.payload(session='worker'))
            with self.assertRaisesRegex(ValueError, 'actual ORCA_TERMINAL_HANDLE'):
                r.checkpoint(self.root, 'worker', 'must not overwrite')
            invoke.assert_not_called()
        with patch.object(r, 'invoke', return_value={'ok': True}) as invoke:
            with self.assertRaisesRegex(ValueError, 'drift'):
                r.checkpoint(self.root, 'worker')
            invoke.assert_not_called()
        self.assertEqual(notes.read_bytes(), prior)
        self.assertEqual(len(r.load(r.session_path(self.root, 'worker'))['boundaries']), 4)

    def test_terminal_drift_missing_identity_and_notified_sessions_stay_fenced(self):
        for phase in ('active', 'requested', 'notifying', 'handoff_ready'):
            session = 'phase-' + phase
            payload = self.payload(session=session)
            r.hook(self.root, payload)
            path = r.session_path(self.root, session)
            state = r.load(path)
            state['phase'] = phase
            r.atomic(path, state)
            with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': ''}):
                result = r.hook(self.root, payload)
                self.assertEqual(result['hookSpecificOutput']['permissionDecision'], 'deny')
                self.assertEqual(r.hook(self.root, {**payload, 'hook_event_name': 'Stop'}), {})
            self.assertEqual(r.hook(self.root, payload)['hookSpecificOutput']['permissionDecision'], 'deny')
            with self.assertRaisesRegex(ValueError, 'drift'):
                r.checkpoint(self.root, session)
        with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': ''}):
            payload = self.payload(session='missing-initial-terminal')
            self.assertEqual(r.hook(self.root, payload)['hookSpecificOutput']['permissionDecision'], 'deny')
        self.assertEqual(r.hook(self.root, payload)['hookSpecificOutput']['permissionDecision'], 'deny')
        self.assertEqual(r.load(r.session_path(self.root, 'missing-initial-terminal'))['terminal'], '')

    def test_terminal_drift_retains_counts_after_growth_and_registry_reassignment(self):
        payload = self.payload(session='worker')
        self.boundary('one', 'worker')
        r.hook(self.root, payload)
        with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}):
            r.hook(self.root, {**payload, 'hook_event_name': 'PostCompact'})
            original = r.load(r.session_path(self.root, 'worker'))['terminal_drift']
            for index in range(2, 5):
                self.boundary(str(index), 'worker')
            registry = r.load(self.root / '.voiceport/coordination.json')
            registry['workers'] = {'verifier': {'terminal': 'terminal-B', 'task_id': 'task-original', 'dispatch_id': 'ctx-new'}}
            r.atomic(self.root / '.voiceport/coordination.json', registry)
            self.assertEqual(r.hook(self.root, payload)['hookSpecificOutput']['permissionDecision'], 'deny')
            self.transcript_for('worker').write_text(json.dumps({'sessionId': 'worker'}) + '\n')
            self.assertEqual(r.hook(self.root, payload)['hookSpecificOutput']['permissionDecision'], 'deny')
            state = r.load(r.session_path(self.root, 'worker'))
            self.assertEqual(state['boundaries'], ['one', '2', '3', '4'])
            self.assertEqual(state['terminal_drift'], original)
            self.assertEqual(state['terminal'], 'worker-terminal')
            with self.assertRaisesRegex(ValueError, 'actual ORCA_TERMINAL_HANDLE'):
                r.checkpoint(self.root, 'worker', 'No old-terminal success')

    def test_administrative_release_and_fresh_session_recovery_is_synthetic(self):
        notes = self.worker_requested()
        notes.parent.mkdir(parents=True, exist_ok=True)
        notes.write_text('Original Task/Dispatch; provisional work retained, no successful checkpoint.')
        protected = {p: p.read_bytes() for p in [self.path, self.root / '.voiceport/coordination.json',
                                                self.root / '.voiceport/loop-state.json']}
        with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}):
            r.hook(self.root, self.payload(session='worker'))
            old = r.load(r.session_path(self.root, 'worker'))
            # Coordinator-owned model only: no helper API stops/releases/starts workers.
            administrative = {'old_session': 'worker', 'old_terminal': 'worker-terminal',
                              'actual_terminal': 'terminal-B', 'task': 'task-original', 'dispatch': 'ctx-original',
                              'exit': 'synthetic-confirmed', 'released': True, 'notes': notes.read_text()}
            self.assertTrue(administrative['released'])
            fresh = self.payload(session='fresh-worker-B')
            self.assertEqual(r.hook(self.root, fresh), {})
            state = r.load(r.session_path(self.root, 'fresh-worker-B'))
            self.assertEqual((state['owner'], state['terminal'], state['boundaries']),
                             ('fresh-worker-B', 'terminal-B', []))
            self.assertEqual(r.load(r.session_path(self.root, 'worker')), old)
            self.assertEqual(notes.read_text(), administrative['notes'])
            for p, raw in protected.items():
                self.assertEqual(p.read_bytes(), raw)
            # Register the fresh synthetic retry, keeping explicit old assignment history.
            registry = r.load(self.root / '.voiceport/coordination.json')
            registry['workers']['verifier'].update(terminal='terminal-B', dispatch_id='ctx-fresh')
            registry['history'] = [administrative]
            r.atomic(self.root / '.voiceport/coordination.json', registry)
            for index in range(4):
                self.boundary(str(index), 'fresh-worker-B')
            self.assertEqual(r.hook(self.root, fresh)['hookSpecificOutput']['permissionDecision'], 'deny')
            with patch.object(r, 'invoke', return_value={'ok': True}) as invoke:
                packet = r.checkpoint(self.root, 'fresh-worker-B', 'Fresh independent handoff notes.')
                self.assertEqual(r.checkpoint(self.root, 'fresh-worker-B'), packet)
                self.assertEqual(invoke.call_count, 1)
                self.assertIn('terminal-B', invoke.call_args.args[1])
            self.assertEqual(packet['task'], 'task-original')
            self.assertEqual(packet['dispatch'], 'ctx-fresh')
            self.assertEqual(r.hook(self.root, {**fresh, 'hook_event_name': 'Stop'}), {})
            self.assertEqual(r.load(r.session_path(self.root, 'worker')), old)
            self.assertEqual(r.hook(self.root, self.payload(session='worker'))['hookSpecificOutput']['permissionDecision'], 'deny')

    def test_already_over_limit_worker_is_not_reset_by_resume(self):
        for index in range(10):
            self.boundary(str(index), 'worker')
        payload = self.payload('SessionStart', session='worker')
        self.assertIn('additionalContext', r.hook(self.root, payload)['hookSpecificOutput'])
        r.hook(self.root, payload)
        self.assertEqual(len(r.load(r.session_path(self.root, 'worker'))['boundaries']), 10)
        notes = str(r.notes_path(self.root, 'worker'))
        self.assertEqual(r.hook(self.root, self.payload(session='worker', name='Write', data={'file_path': notes})), {})
        forbidden = r.hook(self.root, self.payload(session='worker', name='Write', data={'file_path': str(self.root / 'production.rs')}))
        self.assertEqual(forbidden['hookSpecificOutput']['permissionDecision'], 'deny')

    def test_worker_checkpoint_notifies_once_and_preserves_attempt_and_freeze(self):
        for index in range(4):
            self.boundary(str(index), 'worker')
        with patch.dict('os.environ', {'ORCA_TERMINAL_HANDLE': 'worker-terminal'}):
            r.hook(self.root, self.payload(session='worker'))
        registry_path = self.root / '.voiceport/coordination.json'
        registry = r.load(registry_path)
        registry['workers'] = {'verifier': {'terminal': 'worker-terminal', 'task_id': 'task-original', 'dispatch_id': 'ctx-original'}}
        r.atomic(registry_path, registry)
        before = registry_path.read_bytes()
        notes = r.notes_path(self.root, 'worker')
        notes.parent.mkdir(parents=True)
        notes.write_text('Completed commands and remaining review; no product verdict.')
        with patch.object(r, 'invoke', return_value={'ok': True}) as invoke:
            checkpoint = r.checkpoint(self.root, 'worker')
            self.assertEqual(r.checkpoint(self.root, 'worker'), checkpoint)
            self.assertEqual(invoke.call_count, 1)
        self.assertEqual(checkpoint['product_attempt'], 'same-attempt')
        self.assertEqual(checkpoint['freeze'], {'sha256': 'unchanged'})
        self.assertEqual(checkpoint['task'], 'task-original')
        self.assertEqual(registry_path.read_bytes(), before)
        self.assertEqual(r.hook(self.root, self.payload('Stop', session='worker')), {})

    def test_codex_compaction_identity_and_harness_specific_veto(self):
        native = self.transcript_for('native-codex')
        native.write_text(json.dumps({'type': 'session_meta', 'payload': {'id': 'native-codex'}}) + '\n')
        for index in range(4):
            row = {'type': 'compacted', 'payload': {'window_id': f'window-{index}'}}
            self.append(row, 'native-codex')
            self.append(row, 'native-codex')
        payload = {**self.payload('PreCompact', session='native-codex'), 'rotation_harness': 'codex'}
        result = r.codex_output(payload, r.hook(self.root, payload))
        self.assertEqual(result['continue'], False)
        self.assertNotIn('decision', result)
        self.assertEqual(len(r.load(r.session_path(self.root, 'native-codex'))['boundaries']), 4)
        tool = {**payload, 'hook_event_name': 'PreToolUse', 'tool_name': 'exec_command', 'tool_input': {'cmd': 'cargo test'}}
        self.assertEqual(r.codex_output(tool, r.hook(self.root, tool))['decision'], 'block')
        tool['tool_input']['cmd'] = f"python3 -I {r.SCRIPT} checkpoint --session native-codex --notes-text 'Saved current scope and remaining work'"
        self.assertEqual(r.hook(self.root, tool), {})
        tool['tool_input']['cmd'] += '; cargo test'
        self.assertEqual(r.codex_output(tool, r.hook(self.root, tool))['decision'], 'block')

    def test_codex_foreign_rollout_or_unknown_schema_cannot_reset_count(self):
        native = self.transcript_for('codex')
        native.write_text(json.dumps({'type': 'session_meta', 'payload': {'id': 'another-session'}}) + '\n')
        with self.assertRaisesRegex(ValueError, 'another session'):
            r.hook(self.root, {**self.payload(session='codex'), 'rotation_harness': 'codex'})
        native.write_text(json.dumps({'type': 'session_meta', 'payload': {'id': 'codex'}}) + '\n')
        self.append({'type': 'compacted', 'payload': {}}, 'codex')
        with self.assertRaisesRegex(ValueError, 'window_id'):
            r.hook(self.root, {**self.payload(session='codex'), 'rotation_harness': 'codex'})

    def test_threshold_cannot_exceed_five_and_session_names_cannot_escape(self):
        with self.assertRaisesRegex(ValueError, 'between 1 and 5'):
            r.configure(self.root, 'old', 6)
        with self.assertRaisesRegex(ValueError, 'invalid session'):
            r.session_path(self.root, '../escape')

    def worker_requested(self):
        for index in range(4):
            self.boundary(str(index), 'worker')
        r.hook(self.root, self.payload(session='worker'))
        registry_path = self.root / '.voiceport/coordination.json'
        registry = r.load(registry_path)
        registry['workers'] = {'verifier': {'terminal': 'worker-terminal', 'task_id': 'task-original',
                                           'dispatch_id': 'ctx-original'}}
        r.atomic(registry_path, registry)
        return r.notes_path(self.root, 'worker')

    def test_same_inode_same_size_rewrite_and_truncation_never_skip_or_reset(self):
        for session in ('old', 'worker'):
            with self.subTest(session=session):
                path = self.transcript_for(session)
                path.write_text(json.dumps({'sessionId': session, 'pad': 'x' * 4096}) + '\n')
                r.hook(self.root, self.payload(session=session))
                inode, size = path.stat().st_ino, path.stat().st_size
                rows = ''.join(json.dumps({'sessionId': session, 'subtype': 'compact_boundary',
                                          'uuid': f'rewrite-{index}'}) + '\n' for index in range(4))
                pad = json.dumps({'pad': ''}) + '\n'
                rows += json.dumps({'pad': 'x' * (size - len(rows.encode()) - len(pad.encode()))}) + '\n'
                self.assertEqual(len(rows.encode()), size)
                path.write_text(rows)
                self.assertEqual(path.stat().st_ino, inode)
                self.assertEqual(r.hook(self.root, self.payload(session=session))['hookSpecificOutput']['permissionDecision'], 'deny')
                state_path = self.path if session == 'old' else r.session_path(self.root, session)
                saved = r.load(state_path)['boundaries']
                self.assertEqual(len(saved), 4)
                path.write_text(json.dumps({'sessionId': session}) + '\n')
                r.hook(self.root, self.payload(session=session))
                self.assertEqual(r.load(state_path)['boundaries'], saved)

    def test_coordinator_missing_transcript_fails_closed_in_both_cli_events(self):
        for event in ('PreToolUse', 'PreCompact'):
            with self.subTest(event=event):
                payload = self.payload(event)
                del payload['transcript_path']
                result = subprocess.run([sys.executable, '-B', '-I', str(Path(r.__file__)), '--root', str(self.root), 'hook'],
                                        input=json.dumps(payload), capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 2)
                self.assertIn('transcript_path unavailable', result.stderr)
                self.assertEqual(r.load(self.path)['boundaries'], [])

    def test_claude_identity_rechecked_and_foreign_or_missing_identity_rejected(self):
        for session in ('old', 'worker'):
            path = self.transcript_for(session)
            self.boundary('saved', session)
            r.hook(self.root, self.payload(session=session))
            state_path = self.path if session == 'old' else r.session_path(self.root, session)
            saved = r.load(state_path)
            for rows in ([], [{'type': 'metadata'}], [{'sessionId': 'foreign'}],
                         [{'sessionId': session}, {'sessionId': 'foreign'}],
                         [{'sessionId': session}, {'subtype': 'compact_boundary', 'uuid': 'unowned'}]):
                with self.subTest(session=session, rows=rows):
                    path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
                    with self.assertRaises(ValueError):
                        r.hook(self.root, self.payload(session=session))
                    self.assertEqual(r.load(state_path), saved)

    def test_codex_requires_session_meta_even_without_compaction_and_after_rewrite(self):
        path = self.transcript_for('native')
        path.write_text(json.dumps({'type': 'session_meta', 'payload': {'id': 'native'}}) + '\n')
        self.append({'type': 'compacted', 'payload': {'window_id': 'saved'}}, 'native')
        payload = {**self.payload(session='native'), 'rotation_harness': 'codex'}
        r.hook(self.root, payload)
        saved = r.load(r.session_path(self.root, 'native'))
        for row in ({'type': 'event_msg', 'payload': {'type': 'task_started'}},
                    {'type': 'session_meta', 'payload': {'id': 'foreign'}}):
            path.write_text(json.dumps(row) + '\n')
            with self.assertRaises(ValueError):
                r.hook(self.root, payload)
            self.assertEqual(r.load(r.session_path(self.root, 'native')), saved)
        path.write_text(json.dumps({'type': 'session_meta', 'payload': {'id': 'native'}}) + '\n')
        self.assertEqual(r.hook(self.root, payload), {})
        self.assertEqual(r.load(r.session_path(self.root, 'native'))['boundaries'], ['codex:saved'])

    def test_malformed_transcript_identity_schema_fails_closed_in_cli(self):
        for harness, row in (('claude', []), ('claude', 'metadata'),
                             ('codex', {'type': 'session_meta', 'payload': None}),
                             ('codex', {'type': 'compacted', 'payload': []})):
            with self.subTest(harness=harness, row=row):
                path = self.transcript_for('worker')
                path.write_text(json.dumps(row) + '\n')
                result = subprocess.run([sys.executable, '-B', '-I', str(Path(r.__file__)), '--root', str(self.root),
                                         'hook', '--harness', harness], input=json.dumps(self.payload(session='worker')),
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 2)
                self.assertIn('must be an object', result.stderr)

    def test_transcript_replacement_during_scan_cannot_authorize_work(self):
        self.boundary('saved', 'worker')
        payload = self.payload(session='worker')
        r.hook(self.root, payload)
        saved = r.load(r.session_path(self.root, 'worker'))
        original = r.json.loads
        replaced = False
        path = self.transcript_for('worker')
        def replace_transcript(raw, *args, **kwargs):
            nonlocal replaced
            row = original(raw, *args, **kwargs)
            if isinstance(raw, bytes) and not replaced:
                replaced = True
                replacement = self.root / 'replacement.jsonl'
                replacement.write_text(json.dumps({'sessionId': 'foreign'}) + '\n')
                replacement.replace(path)
            return row
        with patch.object(r.json, 'loads', side_effect=replace_transcript):
            with self.assertRaisesRegex(ValueError, 'changed during scan'):
                r.hook(self.root, payload)
        self.assertTrue(replaced)
        self.assertEqual(r.load(r.session_path(self.root, 'worker')), saved)

    def test_append_only_growth_during_scan_does_not_require_quiescent_eof(self):
        r.configure(self.root, 'old', 5)
        for session, harness in (('old', 'claude'), ('worker', 'claude'), ('native', 'codex')):
            with self.subTest(session=session):
                path = self.transcript_for(session)
                if harness == 'codex':
                    path.write_text(json.dumps({'type': 'session_meta', 'payload': {'id': session}}) + '\n')
                for index in range(3):
                    row = ({'type': 'compacted', 'payload': {'window_id': str(index)}} if harness == 'codex'
                           else {'subtype': 'compact_boundary', 'sessionId': session, 'uuid': str(index)})
                    self.append(row, session)
                payload = {**self.payload(session=session), 'rotation_harness': harness}
                r.hook(self.root, payload)
                original_loads, original_pread = r.json.loads, r.os.pread
                appended_parse = appended_verify = False
                def append_result():
                    row = ({'type': 'event_msg', 'payload': {'type': 'task_started'}} if harness == 'codex'
                           else {'type': 'user', 'sessionId': session, 'message': {'content': []}})
                    self.append(row, session)
                def parse_and_append(raw, *args, **kwargs):
                    nonlocal appended_parse
                    row = original_loads(raw, *args, **kwargs)
                    if isinstance(raw, bytes) and not appended_parse:
                        appended_parse = True
                        append_result()
                    return row
                def verify_and_append(*args):
                    nonlocal appended_verify
                    if not appended_verify:
                        appended_verify = True
                        append_result()
                    return original_pread(*args)
                with patch.object(r.json, 'loads', side_effect=parse_and_append), patch.object(r.os, 'pread', side_effect=verify_and_append):
                    self.assertEqual(r.hook(self.root, payload), {})
                self.assertTrue(appended_parse and appended_verify)
                state_path = self.path if session == 'old' else r.session_path(self.root, session)
                self.assertEqual(len(r.load(state_path)['boundaries']), 3)
                row = ({'type': 'compacted', 'payload': {'window_id': 'four'}} if harness == 'codex'
                       else {'subtype': 'compact_boundary', 'sessionId': session, 'uuid': 'four'})
                self.append(row, session)
                self.assertEqual(r.hook(self.root, payload)['hookSpecificOutput']['permissionDecision'], 'deny')
                self.assertEqual(len(r.load(state_path)['boundaries']), 4)

    def test_in_progress_boundary_completed_during_scan_uses_fixed_snapshot(self):
        r.configure(self.root, 'old', 5)
        for session, harness in (('old', 'claude'), ('worker', 'claude'), ('native', 'codex')):
            with self.subTest(session=session):
                path = self.transcript_for(session)
                if harness == 'codex':
                    path.write_text(json.dumps({'type': 'session_meta', 'payload': {'id': session}}) + '\n')
                for index in range(3):
                    row = ({'type': 'compacted', 'payload': {'window_id': str(index)}} if harness == 'codex'
                           else {'subtype': 'compact_boundary', 'sessionId': session, 'uuid': str(index)})
                    self.append(row, session)
                fourth = json.dumps({'type': 'compacted', 'payload': {'window_id': 'four'}} if harness == 'codex'
                                    else {'subtype': 'compact_boundary', 'sessionId': session, 'uuid': 'four'})
                cut = len(fourth) // 2
                with path.open('a') as out:
                    out.write(fourth[:cut])
                initial_size = path.stat().st_size
                original = r.json.loads
                completed = False
                def complete_tail(raw, *args, **kwargs):
                    nonlocal completed
                    row = original(raw, *args, **kwargs)
                    if isinstance(raw, bytes) and not completed:
                        completed = True
                        with path.open('a') as out:
                            out.write(fourth[cut:] + '\n')
                    return row
                payload = {**self.payload(session=session), 'rotation_harness': harness}
                with patch.object(r.json, 'loads', side_effect=complete_tail):
                    self.assertEqual(r.hook(self.root, payload), {})
                self.assertTrue(completed)
                state_path = self.path if session == 'old' else r.session_path(self.root, session)
                state = r.load(state_path)
                self.assertEqual(len(state['boundaries']), 3)
                self.assertLess(state['transcript_cursor']['offset'], initial_size)
                self.assertEqual(r.hook(self.root, payload)['hookSpecificOutput']['permissionDecision'], 'deny')
                self.assertEqual(len(r.load(state_path)['boundaries']), 4)

    def test_prefix_rewrite_cannot_be_disguised_as_append_only_growth(self):
        self.boundary('saved', 'worker')
        path = self.transcript_for('worker')
        payload = self.payload(session='worker')
        r.hook(self.root, payload)
        saved = r.load(r.session_path(self.root, 'worker'))
        initial = path.read_bytes()
        inode = path.stat().st_ino
        original = r.json.loads
        for before, after in ((b'worker', b'rogue!'), (b'saved', b'other')):
            with self.subTest(rewrite=after):
                path.write_bytes(initial)
                rewritten = False
                def rewrite_and_grow(raw, *args, **kwargs):
                    nonlocal rewritten
                    row = original(raw, *args, **kwargs)
                    if isinstance(raw, bytes) and not rewritten:
                        rewritten = True
                        path.write_bytes(initial.replace(before, after) + b'{"type":"metadata"}\n')
                    return row
                with patch.object(r.json, 'loads', side_effect=rewrite_and_grow):
                    with self.assertRaisesRegex(ValueError, 'changed during scan'):
                        r.hook(self.root, payload)
                self.assertTrue(rewritten)
                self.assertEqual(path.stat().st_ino, inode)
                self.assertGreater(path.stat().st_size, len(initial))
                self.assertEqual(r.load(r.session_path(self.root, 'worker')), saved)

    def test_notes_tools_fenced_in_notifying_and_handoff_ready(self):
        notes = self.worker_requested()
        self.assertEqual(r.hook(self.root, self.payload(session='worker', name='Write', data={'file_path': str(notes)})), {})
        with patch.object(r, 'invoke', side_effect=subprocess.TimeoutExpired('mock', 45)) as invoke:
            with self.assertRaisesRegex(ValueError, 'unknown'):
                r.checkpoint(self.root, 'worker', 'Original notes')
            with self.assertRaises(ValueError):
                r.checkpoint(self.root, 'worker')
            self.assertEqual(invoke.call_count, 1)
        for phase in ('notifying', 'handoff_ready'):
            with r.session_transaction(self.root, 'worker') as state:
                state['phase'] = phase
            for name in ('Write', 'Edit'):
                result = r.hook(self.root, self.payload(session='worker', name=name, data={'file_path': str(notes)}))
                self.assertEqual(result['hookSpecificOutput']['permissionDecision'], 'deny')
            command = f'python3 -I {r.SCRIPT} checkpoint --session worker --notes-text replacement'
            result = r.hook(self.root, self.payload(session='worker', name='Bash', data={'command': command}))
            self.assertEqual(result['hookSpecificOutput']['permissionDecision'], 'deny')
        self.assertEqual(notes.read_text(), 'Original notes')

    def test_cached_checkpoint_reconciles_notes_and_never_resends(self):
        notes = self.worker_requested()
        with patch.object(r, 'invoke', return_value={'ok': True}) as invoke:
            value = r.checkpoint(self.root, 'worker', 'Original notes')
            self.assertEqual(r.checkpoint(self.root, 'worker'), value)
            with self.assertRaisesRegex(ValueError, 'immutable'):
                r.checkpoint(self.root, 'worker', 'replacement')
            notes.write_text('Tampered notes')
            with self.assertRaisesRegex(ValueError, 'changed after notification'):
                r.checkpoint(self.root, 'worker')
            notes.unlink()
            with self.assertRaises(OSError):
                r.checkpoint(self.root, 'worker')
            notes.symlink_to(self.root / 'outside')
            with self.assertRaises((OSError, ValueError)):
                r.checkpoint(self.root, 'worker')
            self.assertEqual(invoke.call_count, 1)
        self.assertEqual(r.load(r.session_path(self.root, 'worker'))['phase'], 'handoff_ready')

    def test_checkpoint_terminal_must_match_before_write_cached_return_and_send(self):
        notes = self.worker_requested()
        with patch.object(r, 'invoke', return_value={'ok': True}) as invoke:
            for terminal in ('foreign-terminal', ''):
                with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': terminal}):
                    with self.assertRaisesRegex(ValueError, 'ORCA_TERMINAL_HANDLE'):
                        r.checkpoint(self.root, 'worker', 'Wrong caller')
            self.assertFalse(notes.exists())
            self.assertEqual(invoke.call_count, 0)
            value = r.checkpoint(self.root, 'worker', 'Original notes')
            for terminal in ('foreign-terminal', ''):
                with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': terminal}):
                    with self.assertRaisesRegex(ValueError, 'ORCA_TERMINAL_HANDLE'):
                        r.checkpoint(self.root, 'worker')
            self.assertEqual(r.checkpoint(self.root, 'worker'), value)
            self.assertEqual(invoke.call_count, 1)

    def test_terminal_is_rechecked_after_notes_before_send(self):
        self.worker_requested()
        read_notes = r.checkpoint_notes
        reads = 0
        def change_terminal(*args):
            nonlocal reads
            digest = read_notes(*args)
            reads += 1
            if reads == 2:
                os.environ['ORCA_TERMINAL_HANDLE'] = 'foreign-terminal'
            return digest
        with patch.object(r, 'checkpoint_notes', side_effect=change_terminal), patch.object(r, 'invoke') as invoke:
            with self.assertRaisesRegex(ValueError, 'unknown'):
                r.checkpoint(self.root, 'worker', 'Original notes')
            self.assertEqual(invoke.call_count, 0)
        self.assertEqual(r.load(r.session_path(self.root, 'worker'))['phase'], 'notifying')

    def test_cached_checkpoint_rechecks_terminal_and_exact_registry_match(self):
        self.worker_requested()
        with patch.object(r, 'invoke', return_value={'ok': True}) as invoke:
            r.checkpoint(self.root, 'worker', 'Original notes')
            read_notes = r.checkpoint_notes
            def change_terminal(*args):
                digest = read_notes(*args)
                os.environ['ORCA_TERMINAL_HANDLE'] = 'foreign-terminal'
                return digest
            with patch.object(r, 'checkpoint_notes', side_effect=change_terminal):
                with self.assertRaisesRegex(ValueError, 'ORCA_TERMINAL_HANDLE'):
                    r.checkpoint(self.root, 'worker')
            os.environ['ORCA_TERMINAL_HANDLE'] = 'worker-terminal'
            registry_path = self.root / '.voiceport/coordination.json'
            registry = r.load(registry_path)
            registry['workers']['duplicate'] = copy.deepcopy(registry['workers']['verifier'])
            r.atomic(registry_path, registry)
            with self.assertRaisesRegex(ValueError, 'one exact registered worker'):
                r.checkpoint(self.root, 'worker')
            del registry['workers']['duplicate']
            registry['workers']['verifier']['dispatch_id'] = 'new-dispatch'
            r.atomic(registry_path, registry)
            with self.assertRaisesRegex(ValueError, 'registry identity changed'):
                r.checkpoint(self.root, 'worker')
            self.assertEqual(invoke.call_count, 1)

    def test_notes_known_special_symlink_empty_and_oversized_never_opened(self):
        notes = self.worker_requested()
        notes.parent.mkdir()
        outside = self.root / 'outside.md'
        outside.write_text('Outside notes')
        real_open = os.open
        for kind in ('symlink', 'dangling', 'fifo', 'directory', 'empty', 'oversized'):
            with self.subTest(kind=kind):
                if kind in ('symlink', 'dangling'):
                    notes.symlink_to(outside if kind == 'symlink' else self.root / 'missing')
                elif kind == 'fifo':
                    os.mkfifo(notes)
                elif kind == 'directory':
                    notes.mkdir()
                else:
                    notes.write_text('' if kind == 'empty' else 'x' * 48001)
                def guarded_open(path, *args, **kwargs):
                    if path == notes.name:
                        self.fail('Invalid notes file was opened')
                    return real_open(path, *args, **kwargs)
                with patch.object(r.os, 'open', side_effect=guarded_open), patch.object(r, 'invoke') as invoke:
                    with self.assertRaises((ValueError, OSError)):
                        r.checkpoint(self.root, 'worker')
                    self.assertEqual(invoke.call_count, 0)
                result = r.hook(self.root, self.payload(session='worker', name='Write', data={'file_path': str(notes)}))
                self.assertEqual(result['hookSpecificOutput']['permissionDecision'], 'deny')
                if kind == 'directory':
                    notes.rmdir()
                else:
                    notes.unlink()
        with patch.object(r, 'invoke', return_value={'ok': True}):
            value = r.checkpoint(self.root, 'worker', 'x' * 48000)
        self.assertEqual(value['notes_sha256'], r.hashlib.sha256(b'x' * 48000).hexdigest())

    def test_handoff_parent_symlinks_and_parent_substitution_rejected(self):
        notes = self.worker_requested()
        outside = self.root / 'outside'
        outside.mkdir()
        (outside / notes.name).write_text('Outside notes')
        notes.parent.symlink_to(outside, target_is_directory=True)
        with patch.object(r, 'invoke') as invoke:
            with self.assertRaises(OSError):
                r.checkpoint(self.root, 'worker')
            self.assertEqual(invoke.call_count, 0)
        result = r.hook(self.root, self.payload(session='worker', name='Write', data={'file_path': str(notes)}))
        self.assertEqual(result['hookSpecificOutput']['permissionDecision'], 'deny')
        notes.parent.unlink()
        notes.parent.mkdir()
        notes.write_text('Inside notes')
        real_open = os.open
        moved = self.root / '.voiceport/anchored-handoffs'
        def substitute_parent(path, *args, **kwargs):
            if path == notes.name:
                notes.parent.rename(moved)
                notes.parent.symlink_to(outside, target_is_directory=True)
            return real_open(path, *args, **kwargs)
        with patch.object(r.os, 'open', side_effect=substitute_parent), patch.object(r, 'invoke') as invoke:
            with self.assertRaisesRegex(ValueError, 'directory changed'):
                r.checkpoint(self.root, 'worker')
            self.assertEqual(invoke.call_count, 0)
        self.assertEqual((outside / notes.name).read_text(), 'Outside notes')
        self.assertEqual((moved / notes.name).read_text(), 'Inside notes')

    def test_voiceport_parent_symlink_rejected_for_notes_access(self):
        notes = self.worker_requested()
        notes.parent.mkdir()
        notes.write_text('Original notes')
        voice = self.root / '.voiceport'
        actual = self.root / 'original-voiceport'
        voice.rename(actual)
        voice.symlink_to(actual, target_is_directory=True)
        with self.assertRaises(OSError):
            r.checkpoint_notes(self.root, 'worker')
        self.assertFalse(r.writable_notes(self.root, 'worker'))

    def error_session(self, session, harness):
        path = self.transcript_for(session)
        if harness == 'codex':
            path.write_text(json.dumps({'type': 'session_meta', 'payload': {'id': session}}) + '\n')
            self.append({'type': 'compacted', 'payload': {'window_id': 'saved'}}, session)
        else:
            self.boundary('saved', session)
        payload = {**self.payload(session=session), 'rotation_harness': harness}
        self.assertEqual(r.hook(self.root, payload), {})
        return path, payload, r.session_path(self.root, session)

    def test_first_drift_survives_all_scan_errors_and_return_to_original_terminal(self):
        protected = {p: p.read_bytes() for p in (self.path, self.root / '.voiceport/coordination.json',
                                                self.root / '.voiceport/loop-state.json')}
        faults = ('missing_path', 'missing_file', 'foreign', 'malformed', 'prefix_rewrite')
        for harness in ('claude', 'codex'):
            for terminal in ('terminal-B', ''):
                for index, fault in enumerate(faults):
                    with self.subTest(harness=harness, terminal=terminal, fault=fault):
                        session = f'error-{harness}-{bool(terminal)}-{fault}'
                        path, payload, sp = self.error_session(session, harness)
                        original = path.read_bytes()
                        notes = r.notes_path(self.root, session)
                        notes.parent.mkdir(exist_ok=True)
                        notes.write_text('Retained old assignment, unfinished work and logs.')
                        saved = r.load(sp)
                        saved.update(phase=('active', 'requested', 'notifying', 'handoff_ready')[index % 4],
                                     request={'id': 'old-request'}, history=[{'dispatch': 'ctx_old'}],
                                     checkpoint={'notes_sha256': 'old-digest'}, notes=str(notes),
                                     notify_error='Delivery unknown: never resend')
                        r.atomic(sp, saved)
                        bad = {**payload, 'hook_event_name': 'PreCompact' if index % 2 else 'PreToolUse'}
                        if fault == 'missing_path':
                            bad.pop('transcript_path')
                        elif fault == 'missing_file':
                            bad['transcript_path'] = str(self.root / 'absent.jsonl')
                        elif fault == 'foreign':
                            row = ({'sessionId': 'foreign'} if harness == 'claude' else
                                   {'type': 'session_meta', 'payload': {'id': 'foreign'}})
                            path.write_text(json.dumps(row) + '\n')
                        elif fault == 'malformed':
                            # Even a parsed new boundary cannot commit before prefix verification.
                            row = ({'sessionId': session, 'subtype': 'compact_boundary', 'uuid': 'pending'}
                                   if harness == 'claude' else {'type': 'compacted', 'payload': {'window_id': 'pending'}})
                            path.write_bytes(original + (json.dumps(row) + '\n{broken json}\n').encode())
                        with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': terminal}):
                            if fault == 'prefix_rewrite':
                                with patch.object(r, 'verify_transcript_prefix', side_effect=ValueError('transcript changed during scan')):
                                    with self.assertRaisesRegex(ValueError, 'changed during scan'):
                                        r.hook(self.root, bad)
                            else:
                                with self.assertRaises((ValueError, OSError)):
                                    r.hook(self.root, bad)
                        failed = r.load(sp)
                        first = failed.pop('terminal_drift')
                        self.assertEqual(failed, saved)
                        self.assertEqual(first['actual_terminal'], terminal)
                        self.assertEqual(first['event'], bad['hook_event_name'])
                        diagnostic = r.load(sp.with_suffix('.scan-error.json'))
                        self.assertFalse(diagnostic['scan_verified'])
                        self.assertEqual(diagnostic['owner'], session)
                        self.assertEqual(diagnostic['last_verified_completed_compactions'], 1)
                        self.assertEqual(diagnostic['last_verified_cursor'], saved['transcript_cursor'])
                        self.assertTrue(diagnostic['error'])
                        error_bytes = sp.with_suffix('.scan-error.json').read_bytes()
                        path.write_bytes(original)
                        self.assertEqual(r.hook(self.root, payload)['hookSpecificOutput']['permissionDecision'], 'deny')
                        compact = {**payload, 'hook_event_name': 'PreCompact'}
                        value = r.hook(self.root, compact)
                        self.assertEqual(r.codex_output(compact, value).get('continue') if harness == 'codex'
                                         else value['decision'], False if harness == 'codex' else 'block')
                        row = ({'sessionId': session, 'subtype': 'compact_boundary', 'uuid': 'authentic-next'}
                               if harness == 'claude' else {'type': 'compacted', 'payload': {'window_id': 'authentic-next'}})
                        self.append(row, session)
                        r.hook(self.root, payload)
                        current = r.load(sp)
                        self.assertEqual(len(current['boundaries']), 2)
                        self.assertEqual(current['terminal_drift'], first)
                        self.assertEqual(current['terminal'], saved['terminal'])
                        self.assertEqual(notes.read_text(), 'Retained old assignment, unfinished work and logs.')
                        self.assertEqual(sp.with_suffix('.scan-error.json').read_bytes(), error_bytes)
                        with patch.object(r, 'invoke') as invoke:
                            with self.assertRaisesRegex(ValueError, 'drift'):
                                r.checkpoint(self.root, session)
                            invoke.assert_not_called()
        for p, raw in protected.items():
            self.assertEqual(p.read_bytes(), raw)

    def test_authenticated_recovery_survives_each_scan_fault_and_keeps_failed_evidence(self):
        import shlex
        for harness in ('claude', 'codex'):
            for fault in ('missing_path', 'missing_file', 'foreign', 'malformed', 'prefix_rewrite'):
                with self.subTest(harness=harness, fault=fault):
                    session = f'recovery-{harness}-{fault}'
                    path, payload, sp = self.error_session(session, harness)
                    with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}), patch.object(r, 'invoke') as invoke:
                        r.hook(self.root, payload)
                        saved = sp.read_bytes()
                        bad = dict(payload)
                        if fault == 'missing_path':
                            bad.pop('transcript_path')
                        elif fault == 'missing_file':
                            bad['transcript_path'] = str(self.root / 'absent.jsonl')
                        elif fault == 'foreign':
                            row = ({'sessionId': 'foreign'} if harness == 'claude' else
                                   {'type': 'session_meta', 'payload': {'id': 'foreign'}})
                            path.write_text(json.dumps(row) + '\n')
                        elif fault == 'malformed':
                            path.write_text('{broken json}\n')
                        verify = r.verify_transcript_prefix
                        def prefix(*args):
                            if fault == 'prefix_rewrite':
                                raise ValueError('transcript changed during scan')
                            return verify(*args)
                        command = shlex.join(['orca', 'orchestration', 'send', '--from', 'terminal-B', '--type', 'escalation',
                                              '--subject', 'Terminal drift', '--body', 'Stop exact assignment.',
                                              '--task-id', 'task_actual', '--dispatch-id', 'ctx_actual'])
                        with patch.object(r, 'verify_transcript_prefix', side_effect=prefix):
                            for name, data in [('Read', {}), ('Bash', {'command': f'python3 -I {r.SCRIPT} session-status --session {session}'}),
                                               ('Bash', {'command': command}), ('exec_command', {'cmd': command})]:
                                self.assertEqual(r.hook(self.root, {**bad, 'tool_name': name, 'tool_input': data}), {})
                            self.assertEqual(sp.read_bytes(), saved)
                            diagnostic = r.load(sp.with_suffix('.scan-error.json'))
                            self.assertEqual(diagnostic['last_verified_completed_compactions'], 1)
                            self.assertFalse(diagnostic['scan_verified'])
                            for event in ('PreToolUse', 'PreCompact'):
                                with self.assertRaises((ValueError, OSError)):
                                    r.hook(self.root, {**bad, 'hook_event_name': event})
                            for native in (False, True):
                                for recursive in (False, True):
                                    stop = {**bad, 'hook_event_name': 'Stop',
                                            'stopHookActive' if native else 'stop_hook_active': recursive}
                                    self.assertEqual(r.hook(self.root, stop), {})
                        invoke.assert_not_called()
                        current = r.load(sp)
                        self.assertEqual(current['boundaries'], json.loads(saved)['boundaries'])
                        self.assertEqual(current['terminal_drift'], json.loads(saved)['terminal_drift'])

    def test_corrupt_or_unrecognized_session_never_uses_failed_scan_recovery(self):
        for harness in ('claude', 'codex'):
            session = 'new-error-' + harness
            payload = {**self.payload(session=session, name='Read'), 'rotation_harness': harness}
            path = self.transcript_for(session)
            path.write_text('{broken json}\n')
            with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}):
                with self.assertRaises(ValueError):
                    r.hook(self.root, payload)
            self.assertFalse(r.session_path(self.root, session).exists())
            session = 'corrupt-error-' + harness
            path, payload, sp = self.error_session(session, harness)
            with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}):
                r.hook(self.root, payload)
            saved = r.load(sp)
            path.write_text('{broken json}\n')
            mutations = [{'owner': 'someone-else'}, {'schema_version': 2}, {'threshold': 6},
                         {'boundaries': None}, {'boundaries': ['saved', 'saved']}, {'boundaries': [1]},
                         {'terminal_drift': {}}, {'transcript_cursor': {}}, {'transcript_session_verified': 'true'},
                         {'harness': 'codex' if harness == 'claude' else 'claude'},
                         {'transcript_session_verified': False}]
            with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}), patch.object(r, 'invoke') as invoke:
                for mutation in mutations:
                    with self.subTest(harness=harness, mutation=mutation):
                        r.atomic(sp, {**saved, **mutation})
                        before = sp.read_bytes()
                        with self.assertRaises(ValueError):
                            r.hook(self.root, {**payload, 'tool_name': 'Read'})
                        self.assertEqual(sp.read_bytes(), before)
                        self.assertFalse(sp.with_suffix('.scan-error.json').exists())
                sp.write_text('{broken state}')
                with self.assertRaises(ValueError):
                    r.hook(self.root, {**payload, 'tool_name': 'Read'})
                self.assertEqual(sp.read_text(), '{broken state}')
                invoke.assert_not_called()

    def test_failed_scan_recovery_parser_keeps_negatives_and_tool_shapes_closed(self):
        import shlex
        for harness in ('claude', 'codex'):
            session = 'parser-error-' + harness
            path, payload, sp = self.error_session(session, harness)
            path.write_text('{broken json}\n')
            command = shlex.join(['orca', 'orchestration', 'send', '--from', 'terminal-B', '--type', 'escalation',
                                  '--subject', 'Drift', '--body', 'Stop exact assignment.', '--task-id', 'task_actual', '--dispatch-id', 'ctx_actual'])
            bad_commands = [command.replace('terminal-B', 'worker-terminal'), command.replace('escalation', 'worker_done'),
                            command + ' --to run_other', command + ' --json', command + ' --body duplicate',
                            command.replace('--subject', '--body'), command + '; pwd', command + ' | pwd',
                            command.replace('Drift', '$(pwd)'), command.replace('Drift', '`pwd`'), command + "'",
                            command.replace('Drift', 'x' * 48001), command.replace('Drift', '界' * 16001),
                            f'python3 -I {r.SCRIPT} session-status --session another',
                            f'python3 -I {r.SCRIPT} checkpoint --session {session}', 'pwd']
            bad_commands.extend(command.replace('Drift', ch) for ch in ('*', '?', '[x]', '{x,y}', '~', '\n', '\r', '\x00'))
            with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}), patch.object(r, 'invoke') as invoke:
                for name in ('Bash', 'exec_command'):
                    for cmd in bad_commands:
                        with self.subTest(harness=harness, name=name, command=cmd[:150]):
                            data = {'command' if name == 'Bash' else 'cmd': cmd}
                            with self.assertRaises(ValueError):
                                r.hook(self.root, {**payload, 'tool_name': name, 'tool_input': data})
                for name in ('Write', 'Edit', 'Skill', 'Agent', 'ScheduleWakeup'):
                    with self.assertRaises(ValueError):
                        r.hook(self.root, {**payload, 'tool_name': name})
                for name, data in [(None, {}), (12, {}), ('Read', []), ('Bash', None),
                                   ('exec_command', {'cmd': []}), ('Bash', {'command': 5})]:
                    with self.assertRaises(ValueError):
                        r.hook(self.root, {**payload, 'tool_name': name, 'tool_input': data})
                with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': ''}):
                    with self.assertRaises(ValueError):
                        r.hook(self.root, {**payload, 'tool_name': 'Bash', 'tool_input': {'command': command}})
                invoke.assert_not_called()
            self.assertEqual(len(r.load(sp)['boundaries']), 1)

    def test_scan_error_cli_veto_and_nonblocking_stop_keep_host_contracts(self):
        for harness in ('claude', 'codex'):
            session = 'cli-error-' + harness
            path, payload, sp = self.error_session(session, harness)
            path.write_text('{broken json}\n')
            env = {**os.environ, 'ORCA_TERMINAL_HANDLE': 'terminal-B', 'PATH': ''}
            argv = [sys.executable, '-B', '-I', str(Path(r.__file__)), '--root', str(self.root), 'hook', '--harness', harness]
            for event, name in [('PreCompact', 'Edit'), ('PreToolUse', 'Edit'), ('PreToolUse', 'Read'),
                                ('Stop', 'Edit'), ('Stop', 'Edit')]:
                data = {**payload, 'hook_event_name': event, 'tool_name': name,
                        'stop_hook_active': True, 'stopHookActive': True}
                proc = subprocess.run(argv, input=json.dumps(data), capture_output=True, text=True, env=env, timeout=10)
                with self.subTest(harness=harness, event=event, name=name):
                    expected = 2 if (event == 'PreToolUse' and name == 'Edit') or (event == 'PreCompact' and harness == 'claude') else 0
                    self.assertEqual(proc.returncode, expected, proc.stderr)
                    if event == 'PreCompact' and harness == 'codex':
                        self.assertIs(json.loads(proc.stdout)['continue'], False)
                    elif expected == 0:
                        self.assertEqual(json.loads(proc.stdout), {})
            self.assertEqual(r.load(sp)['terminal_drift']['actual_terminal'], 'terminal-B')
            self.assertEqual(len(r.load(sp)['boundaries']), 1)
            self.assertFalse(r.load(sp.with_suffix('.scan-error.json'))['scan_verified'])

    def test_diagnostic_storage_failure_preserves_recovery_and_saved_authority(self):
        import contextlib
        import errno
        import io
        import shlex
        for harness in ('claude', 'codex'):
            for fault in ('directory', errno.ENOSPC, errno.EIO):
                session = f'diagnostic-{harness}-{fault}'
                path, payload, sp = self.error_session(session, harness)
                with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}):
                    r.hook(self.root, payload)
                    state = r.load(sp)
                    state.update(phase='notifying', request={'id': 'retained'},
                                 checkpoint={'notes_sha256': 'retained'}, notify_error='Unknown delivery',
                                 hook_implementation_sha256='0' * 64)
                    r.atomic(sp, state)
                    saved = sp.read_bytes()
                    notes = r.notes_path(self.root, session)
                    notes.parent.mkdir(exist_ok=True)
                    notes.write_text('Retained notes; no repeat notification.')
                    path.write_text('{broken json}\n')
                    sidecar = sp.with_suffix('.scan-error.json')
                    if fault == 'directory':
                        sidecar.mkdir()
                    atomic = r.atomic
                    def write(target, value):
                        if target == sidecar and fault != 'directory':
                            raise OSError(fault, 'Synthetic diagnostic storage failure')
                        return atomic(target, value)
                    command = shlex.join(['orca', 'orchestration', 'send', '--from', 'terminal-B',
                                          '--type', 'escalation', '--subject', 'Blocked', '--body', 'Stop exact assignment.',
                                          '--task-id', 'task_actual', '--dispatch-id', 'ctx_actual'])
                    log = io.StringIO()
                    with patch.object(r, 'atomic', side_effect=write), contextlib.redirect_stderr(log), patch.object(r, 'invoke') as invoke:
                        for name, data in [('Read', {}), ('Bash', {'command': f'python3 -I {r.SCRIPT} session-status --session {session}'}),
                                           ('Bash', {'command': command}), ('exec_command', {'cmd': command})]:
                            self.assertEqual(r.hook(self.root, {**payload, 'tool_name': name, 'tool_input': data}), {})
                        for event in ('PreToolUse', 'PreCompact'):
                            with self.assertRaises(ValueError):
                                r.hook(self.root, {**payload, 'hook_event_name': event})
                        for recursive in (False, True):
                            self.assertEqual(r.hook(self.root, {**payload, 'hook_event_name': 'Stop',
                                                               'stop_hook_active': recursive, 'stopHookActive': recursive}), {})
                        invoke.assert_not_called()
                    self.assertIn('scan diagnostic not persisted:', log.getvalue())
                    self.assertIn('Rotation scan failed:', log.getvalue())
                    self.assertEqual(sp.read_bytes(), saved)
                    self.assertEqual(notes.read_text(), 'Retained notes; no repeat notification.')
                    self.assertTrue(sidecar.is_dir() if fault == 'directory' else not sidecar.exists())

    def test_first_drift_commit_failure_and_programming_fault_never_allow_recovery(self):
        import errno
        for harness in ('claude', 'codex'):
            session = 'commit-failure-' + harness
            path, payload, sp = self.error_session(session, harness)
            saved = sp.read_bytes()
            atomic = r.atomic
            def unwritable(target, value):
                if target == sp:
                    raise OSError(errno.ENOSPC, 'Synthetic authority storage failure')
                return atomic(target, value)
            with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}), patch.object(r, 'invoke') as invoke:
                with patch.object(r, 'atomic', side_effect=unwritable), patch.object(r, 'scan') as scan:
                    for name in ('Read', 'Edit'):
                        with self.assertRaises(OSError):
                            r.hook(self.root, {**payload, 'tool_name': name})
                    scan.assert_not_called()
                self.assertEqual(sp.read_bytes(), saved)
                self.assertFalse(sp.with_suffix('.scan-error.json').exists())
                with patch.object(r, 'scan', side_effect=TypeError('Unrelated programming fault')):
                    with self.assertRaisesRegex(TypeError, 'Unrelated programming fault'):
                        r.hook(self.root, {**payload, 'tool_name': 'Read'})
                self.assertFalse(sp.with_suffix('.scan-error.json').exists())
                invoke.assert_not_called()

    def assert_saved_state_host_veto(self, payload, harness, events=('PreToolUse', 'PreCompact')):
        import contextlib
        import io
        for event in events:
            stdout, stderr = io.StringIO(), io.StringIO()
            argv = ['rotation', '--root', str(self.root), 'hook', '--harness', harness]
            with patch.object(sys, 'argv', argv), patch.object(sys, 'stdin', io.StringIO(json.dumps({**payload, 'hook_event_name': event}))), contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                if harness == 'codex' and event == 'PreCompact':
                    with self.assertRaises(SystemExit) as raised:
                        r.main()
                    self.assertEqual(raised.exception.code, 0)
                    value = json.loads(stdout.getvalue())
                    self.assertIs(value['continue'], False)
                    self.assertTrue(value['stopReason'])
                else:
                    with self.assertRaises(SystemExit) as raised:
                        r.main()
                    self.assertEqual(raised.exception.code, 2)
                    self.assertEqual(stdout.getvalue(), '')
            self.assertIn('hook failed:', stderr.getvalue())
            self.assertNotIn('Traceback', stderr.getvalue())

    def test_consumed_saved_field_shapes_and_coherent_drift_reject_before_scan(self):
        for harness in ('claude', 'codex'):
            session = 'field-shapes-' + harness
            path, payload, sp = self.error_session(session, harness)
            with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'terminal-B'}):
                r.hook(self.root, payload)
                saved = r.load(sp)
                cursor, drift = saved['transcript_cursor'], saved['terminal_drift']
                mutations = [{'schema_version': True}, {'epoch': 1.0}, {'threshold': True}, {'phase': []},
                             {'created_at': 'invalid'}, {'created_at': '2026-01-01T00:00:00'}, {'terminal': []},
                             {'hook_events': None}, {'hook_events': 'PreToolUse'}, {'hook_events': {}},
                             {'hook_events': [[]]}, {'hook_events': [None]}, {'hook_events': ['unsupported']},
                             {'request': []}, {'checkpoint': []}, {'boundaries': [{}]},
                             {'harness': []}, {'transcript_session_verified': 1},
                             {'transcript_cursor': {**cursor, 'inode': -1}},
                             {'transcript_cursor': {**cursor, 'inode': 0}},
                             {'transcript_cursor': {**cursor, 'inode': True}},
                             {'transcript_cursor': {**cursor, 'offset': True}},
                             {'transcript_cursor': {**cursor, 'path': []}},
                             {'terminal_drift': {k: v for k, v in drift.items() if k != 'actual_terminal'}},
                             {'terminal_drift': {**drift, 'event': ''}},
                             {'terminal_drift': {**drift, 'event': []}},
                             {'terminal_drift': {**drift, 'actual_terminal': []}},
                             {'terminal_drift': {**drift, 'actual_terminal': saved['terminal']}},
                             {'terminal_drift': {**drift, 'detected_at': 'invalid'}}]
                for mutation in mutations:
                    with self.subTest(harness=harness, mutation=mutation):
                        r.atomic(sp, {**saved, **mutation})
                        before = sp.read_bytes()
                        # A valid transcript must not silently repair malformed authority.
                        with patch.object(r, 'scan') as scan:
                            self.assert_saved_state_host_veto(payload, harness)
                            scan.assert_not_called()
                        self.assertEqual(sp.read_bytes(), before)
                        self.assertFalse(sp.with_suffix('.scan-error.json').exists())
                # Missing optional history is supported for legacy records; observed
                # absent callers are explicitly present null/empty, never omitted.
                for actual in (None, ''):
                    legacy = {**saved, 'terminal_drift': {**drift, 'actual_terminal': actual}}
                    legacy.pop('hook_events')
                    r.atomic(sp, legacy)
                    path.write_text('{broken json}\n')
                    before = sp.read_bytes()
                    self.assertEqual(r.hook(self.root, {**payload, 'tool_name': 'Read'}), {})
                    self.assertEqual(sp.read_bytes(), before)
                    sp.with_suffix('.scan-error.json').unlink()

    def test_coordinator_routing_and_tool_input_shapes_use_host_denial(self):
        saved = r.load(self.path)
        for harness in ('claude', 'codex'):
            for mutation in ({'retired_sessions': None}, {'retired_sessions': [{}]}, {'boundaries': None},
                             {'boundaries': [[]]}, {'phase': []}, {'threshold': True}, {'epoch': []}):
                r.atomic(self.path, {**saved, **mutation})
                before = self.path.read_bytes()
                self.assert_saved_state_host_veto(self.payload(), harness)
                self.assertEqual(self.path.read_bytes(), before)
            r.atomic(self.path, saved)
            for tool, data in ((None, {}), ('Read', []), ('Bash', {'command': []}), ('exec_command', {'cmd': {}})):
                self.assert_saved_state_host_veto({**self.payload(), 'tool_name': tool, 'tool_input': data}, harness,
                                                  events=('PreToolUse',))

    def test_no_state_is_noop_and_bad_state_fails_tool_hook(self):
        self.path.unlink()
        self.assertEqual(r.hook(self.root, self.payload()), {})
        self.path.write_text('{bad')
        result = subprocess.run([sys.executable, '-I', str(Path(r.__file__)), '--root', str(self.root), 'hook'],
                                input=json.dumps(self.payload()), capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertIn('hook failed', result.stderr)


class RunRotationTests(unittest.TestCase):
    # Reuse fixture helpers only; all original tests above remain intact.
    setUp_base = RotationTests.setUp

    def transcript_for(self, session):
        if session not in self.transcripts:
            path = self.root / (session + '.jsonl')
            path.write_text(json.dumps({'type': 'user', 'sessionId': session, 'timestamp': r.now()}) + '\n')
            self.transcripts[session] = path
        return self.transcripts[session]

    append = RotationTests.append
    boundary = RotationTests.boundary
    payload = RotationTests.payload

    def setUp(self):
        self.setUp_base()
        self.run = 'run_new'
        self.current_terminal = 'coord-terminal'
        self.navigation = '.voiceport/handoffs/current.json'
        nav = self.root / self.navigation
        nav.parent.mkdir()
        r.atomic(nav, {'run_id': self.run, 'product_task_id': 'unbound', 'product_attempt_id': 'unbound',
                       'role_predecessor': 'retained', 'freeze': {'id': 'frozen', 'sha256': 'unchanged'}})
        self.calls = []
        self.orca = patch.object(r, 'invoke', side_effect=self.receipt)
        self.orca.start()
        self.addCleanup(self.orca.stop)

    def receipt(self, root, argv):
        self.calls.append(argv)
        if argv[:2] in (['orchestration', 'run-current'], ['orchestration', 'run-show']):
            return {'ok': True, 'result': {'run': {'id': self.run,
                    'coordinator_handle': self.current_terminal,
                    'consumer_generation': 2 if self.current_terminal == 'coord-terminal' else 3}}}
        if argv[:2] == ['terminal', 'show']:
            terminal = argv[3]
            return {'ok': True, 'result': {'terminal': {'handle': terminal, 'connected': True,
                    'orphaned': False, 'worktreePath': str(root.resolve()), 'incarnationId': terminal + '-incarnation'}}}
        if argv[:2] == ['agent', 'launch']:
            state = r.load(r.coordinator_path(root, self.run))
            self.assertEqual(state['phase'], 'launching')
            self.assertIn('launch_started_at', state['request'])
            self.assertIn('launch_argv', state['request'])
            return installed_launch_receipt()
        if argv[:2] == ['terminal', 'close']:
            self.assertIn('close_started_at', r.load(r.coordinator_path(root, self.run))['request'])
            return {'ok': True, 'result': {'close': {'handle': argv[3], 'ptyKilled': True}}}
        self.fail('unexpected mock Orca operation: ' + repr(argv))

    def boot(self, session='coord', terminal='coord-terminal'):
        os.environ['ORCA_TERMINAL_HANDLE'] = terminal
        r.hook(self.root, self.payload(session=session, name='Read'))
        return r.load(r.session_path(self.root, session))

    def register(self):
        self.boot()
        return r.register(self.root, 'coord', self.run, str(self.transcript_for('coord')),
                          'deliver current goal', 'continue authorized work', self.navigation)

    def retire(self):
        self.register()
        for i in range(4):
            self.boundary(str(i), 'coord')
        r.hook(self.root, self.payload(session='coord'))
        self.append({'sessionId': 'coord', 'message': {'content': [
            {'type': 'tool_use', 'name': 'ScheduleWakeup', 'id': 'stop', 'input': {'stop': True}}]}}, 'coord')
        self.append({'sessionId': 'coord', 'message': {'content': [
            {'type': 'tool_result', 'tool_use_id': 'stop', 'content': 'Loop stopped'}]}}, 'coord')
        return r.retire(self.root, 'coord', str(self.transcript_for('coord')), self.run)

    def launch(self):
        self.retire()
        r.launch(self.root, 'coord', self.run)
        return self.saved()['request']['token']

    def saved(self):
        return r.load(r.coordinator_path(self.root, self.run))

    def successor(self):
        token = self.launch()
        self.boot('next', 'next-terminal')
        self.current_terminal = 'next-terminal'  # Mock explicit same-Run run-use receipt.
        return r.claim(self.root, 'next', token, self.run)

    def test_absent_global_lease_dispatches_real_worker_accounting(self):
        self.path.unlink()
        os.environ['ORCA_TERMINAL_HANDLE'] = 'worker-terminal'
        r.hook(self.root, self.payload(session='new-worker', name='Read'))
        self.assertTrue(r.session_path(self.root, 'new-worker').exists(), 'hook must account without a global lease')
        proof = r.load(r.session_path(self.root, 'new-worker'))
        self.assertFalse(self.path.exists())
        self.assertEqual(proof['boundaries'], [])
        self.assertEqual(proof['hook_events'], ['PreToolUse'])
        self.assertTrue(proof['transcript_session_verified'])
        for i in range(4):
            self.boundary(str(i), 'new-worker')
        blocked = r.hook(self.root, self.payload(session='new-worker'))
        self.assertEqual(blocked['hookSpecificOutput']['permissionDecision'], 'deny')
        self.assertFalse(self.calls)

    def test_new_run_registration_preserves_unrelated_lease_and_counts(self):
        before = self.path.read_bytes(), self.path.stat().st_mtime_ns
        self.boot()
        self.boundary('prior', 'coord')
        r.hook(self.root, self.payload(session='coord', name='Read'))
        state = r.register(self.root, 'coord', self.run, str(self.transcript_for('coord')),
                           'goal', 'loop', self.navigation)
        self.assertEqual(state['boundaries'], ['prior'])
        count = len(self.calls)
        self.assertEqual(r.hook(self.root, self.payload(session='coord')), {})
        self.assertEqual(len(self.calls), count)  # No Orca lookups on hooks.
        self.assertEqual((self.path.read_bytes(), self.path.stat().st_mtime_ns), before)
        self.assertEqual(r.load(r.session_path(self.root, 'coord'))['coordinator_run'], self.run)

    def test_spoofed_wrong_terminal_run_and_unverified_registration_fail(self):
        self.boot()
        sp = r.session_path(self.root, 'coord')
        original = sp.read_bytes()
        for mutation in ({'terminal': 'wrong'}, {'hook_events': []}, {'hook_implementation_sha256': 'stale'},
                         {'terminal_drift': {'stored_terminal': 'coord-terminal', 'actual_terminal': 'other',
                           'event': 'PreToolUse', 'detected_at': r.now()}}):
            with self.subTest(mutation=mutation):
                r.atomic(sp, {**json.loads(original), **mutation})
                with self.assertRaises(ValueError):
                    r.register(self.root, 'coord', self.run, str(self.transcript_for('coord')), 'g', 'l', self.navigation)
                self.assertFalse(r.coordinator_path(self.root, self.run).exists())
        sp.write_bytes(original)
        self.current_terminal = 'actual-other-coordinator'
        with self.assertRaisesRegex(ValueError, 'current Run coordinator'):
            r.register(self.root, 'coord', self.run, str(self.transcript_for('coord')), 'g', 'l', self.navigation)
        self.current_terminal = 'coord-terminal'
        with self.assertRaisesRegex(ValueError, 'lineage mismatch'):
            r.register(self.root, 'coord', 'run_wrong', str(self.transcript_for('coord')), 'g', 'l', self.navigation)
        with self.assertRaises(OSError):
            r.register(self.root, 'spoofed-session', self.run, str(self.transcript_for('coord')), 'g', 'l', self.navigation)

    def test_registration_checks_actual_bound_run_and_terminal_receipt(self):
        self.boot()
        def wrong(root, argv):
            result = self.receipt(root, argv)
            if argv[:2] == ['orchestration', 'run-current']:
                result['result']['run']['id'] = 'run_other'
            return result
        with patch.object(r, 'invoke', side_effect=wrong):
            with self.assertRaisesRegex(ValueError, 'bound'):
                r.register(self.root, 'coord', self.run, str(self.transcript_for('coord')), 'g', 'l', self.navigation)
        def wrong_terminal(root, argv):
            result = self.receipt(root, argv)
            if argv[:2] == ['terminal', 'show']:
                result['result']['terminal']['worktreePath'] = '/wrong'
            return result
        with patch.object(r, 'invoke', side_effect=wrong_terminal):
            with self.assertRaisesRegex(ValueError, 'worktree'):
                r.register(self.root, 'coord', self.run, str(self.transcript_for('coord')), 'g', 'l', self.navigation)

    def test_last_round_can_register_then_retire_launch_but_cannot_work(self):
        self.boot()
        for i in range(4):
            self.boundary(str(i), 'coord')
        command = (f'python3 -I {r.SCRIPT} register --session coord --run {self.run} '
                   f'--transcript {self.transcript_for("coord")} --goal goal --loop-prompt loop '
                   f'--navigation {self.navigation}')
        self.assertEqual(r.hook(self.root, self.payload(session='coord', name='Bash', data={'command': command})), {})
        file_command = command.replace('--loop-prompt loop', '--loop-prompt-file .claude/loop.md')
        self.assertEqual(r.hook(self.root, self.payload(session='coord', name='Bash', data={'command': file_command})), {})
        (self.root / '.claude').mkdir()
        (self.root / '.claude/loop.md').write_text('original authorized prompt\nsecond line\n')
        argv = ['rotation', '--root', str(self.root), *r.shlex.split(file_command)[3:]]
        with patch.object(sys, 'argv', argv), patch.object(r, 'register', return_value={}) as registered, patch('builtins.print'):
            r.main()
        self.assertEqual(registered.call_args.args[5], 'original authorized prompt\nsecond line\n')
        r.register(self.root, 'coord', self.run, str(self.transcript_for('coord')), 'g', 'l', self.navigation)
        for name, data in [('Edit', {}), ('ScheduleWakeup', {'delaySeconds': 1200}),
                           ('Bash', {'command': 'orca orchestration check'}), ('Bash', {'command': 'python3 test.py'})]:
            self.assertEqual(r.hook(self.root, self.payload(session='coord', name=name, data=data))
                             ['hookSpecificOutput']['permissionDecision'], 'deny')
        for name, data in [('Read', {}), ('ScheduleWakeup', {'stop': True}),
                           ('Bash', {'command': f'python3 -I {r.SCRIPT} retire --session coord --run {self.run} --transcript x'}),
                           ('exec_command', {'cmd': f'python3 -I {r.SCRIPT} status --run {self.run}'})]:
            self.assertEqual(r.hook(self.root, self.payload(session='coord', name=name, data=data)), {})
        result = r.hook(self.root, self.payload('PreCompact', session='coord'))
        self.assertEqual(r.codex_output({'hook_event_name': 'PreCompact'}, result)['continue'], False)
        self.assertEqual(result['decision'], 'block')
        self.assertEqual(self.saved()['boundaries'], ['0', '1', '2', '3'])

    def test_unknown_launch_is_persisted_before_call_and_never_duplicated(self):
        self.retire()
        calls = []
        def timeout(root, argv):
            if argv[:2] == ['agent', 'launch']:
                calls.append(argv)
                self.assertEqual(self.saved()['phase'], 'launching')
                self.assertIn('launch_argv', self.saved()['request'])
                raise subprocess.TimeoutExpired('orca', 45)
            return self.receipt(root, argv)
        with patch.object(r, 'invoke', side_effect=timeout):
            with self.assertRaisesRegex(ValueError, 'unknown'):
                r.launch(self.root, 'coord', self.run)
            with self.assertRaisesRegex(ValueError, 'already attempted'):
                r.launch(self.root, 'coord', self.run)
        self.assertEqual(len(calls), 1)
        self.assertIn('launch_error', self.saved()['request'])

    def test_successor_requires_initial0_current_proof_after_launch(self):
        token = self.launch()
        self.boot('next', 'next-terminal')
        self.current_terminal = 'next-terminal'
        sp = r.session_path(self.root, 'next')
        original = sp.read_bytes()
        for mutation in ({'boundaries': ['prior']}, {'created_at': '2020-01-01T00:00:00+00:00'},
                         {'hook_events': []}, {'hook_implementation_sha256': 'old'}, {'terminal': 'coord-terminal'}):
            r.atomic(sp, {**json.loads(original), **mutation})
            with self.assertRaises(ValueError):
                r.claim(self.root, 'next', token, self.run)
            self.assertEqual(self.saved()['owner'], 'coord')
        sp.write_bytes(original)
        transcript_bytes = self.transcript_for('next').read_bytes()
        self.transcript_for('next').write_text(json.dumps({'sessionId': 'unrelated'}) + '\n')
        with self.assertRaisesRegex(ValueError, 'another session'):
            r.claim(self.root, 'next', token, self.run)
        self.transcript_for('next').write_bytes(transcript_bytes)
        claim = r.claim(self.root, 'next', token, self.run)
        self.assertTrue(claim['paused'])
        self.assertEqual(claim['owner'], 'next')
        self.assertEqual(self.saved()['successor_verified']['proof']['boundaries'], [])
        self.assertNotEqual(self.saved()['terminal'], 'coord-terminal')

    def test_successor_wrong_run_owner_and_old_session_cannot_claim(self):
        token = self.launch()
        with self.assertRaisesRegex(ValueError, 'stale'):
            r.claim(self.root, 'coord', token, self.run)
        self.boot('next', 'next-terminal')
        with self.assertRaisesRegex(ValueError, 'current Run coordinator'):
            r.claim(self.root, 'next', token, self.run)
        self.assertEqual(self.saved()['owner'], 'coord')

    def test_claim_preserves_navigation_pause_fencing_and_exact_close(self):
        legacy = self.path.read_bytes()
        nav = (self.root / self.navigation).read_bytes()
        self.successor()
        packet = r.load(self.root / self.saved()['request']['packet'])
        self.assertEqual(packet['run_id'], self.run)
        self.assertEqual(packet['product_attempt_id'], 'unbound')
        self.assertEqual(packet['tasks'], {})
        self.assertEqual(packet['coordination']['freeze']['sha256'], 'unchanged')
        self.assertTrue(packet['paused'])
        self.assertEqual(r.hook(self.root, self.payload(session='next'))['hookSpecificOutput']['permissionDecision'], 'deny')
        with self.assertRaisesRegex(ValueError, 'exact retiring'):
            r.close_retired(self.root, 'next', self.run, 'unrelated-terminal')
        self.assertFalse(any(c[:2] == ['terminal', 'close'] for c in self.calls))
        r.close_retired(self.root, 'next', self.run, 'coord-terminal')
        self.assertEqual(r.hook(self.root, self.payload(session='next')), {})
        os.environ['ORCA_TERMINAL_HANDLE'] = 'coord-terminal'
        self.assertEqual(r.hook(self.root, self.payload(session='coord'))['hookSpecificOutput']['permissionDecision'], 'deny')
        self.assertEqual(self.path.read_bytes(), legacy)
        self.assertEqual((self.root / self.navigation).read_bytes(), nav)

    def test_unknown_close_latches_no_duplicate_and_denies_product(self):
        self.successor()
        calls = []
        def timeout(root, argv):
            if argv[:2] == ['terminal', 'close']:
                calls.append(argv)
                self.assertIn('close_started_at', self.saved()['request'])
                raise subprocess.TimeoutExpired('orca', 45)
            return self.receipt(root, argv)
        with patch.object(r, 'invoke', side_effect=timeout):
            with self.assertRaisesRegex(ValueError, 'unknown'):
                r.close_retired(self.root, 'next', self.run, 'coord-terminal')
            with self.assertRaisesRegex(ValueError, 'never retry'):
                r.close_retired(self.root, 'next', self.run, 'coord-terminal')
        self.assertEqual(len(calls), 1)
        self.assertIn('close_error', self.saved()['request'])
        self.assertEqual(r.hook(self.root, self.payload(session='next'))['hookSpecificOutput']['permissionDecision'], 'deny')

    def test_success_envelope_without_exact_stop_proof_remains_unknown(self):
        self.successor()
        def unknown(root, argv):
            if argv[:2] == ['terminal', 'close']:
                return {'ok': True, 'result': {'close': {'handle': argv[3], 'ptyKilled': False}}}
            return self.receipt(root, argv)
        with patch.object(r, 'invoke', side_effect=unknown):
            with self.assertRaisesRegex(ValueError, 'unknown'):
                r.close_retired(self.root, 'next', self.run, 'coord-terminal')
        self.assertFalse(self.saved()['request']['close_confirmed'])
        self.assertIn('close_receipt', self.saved()['request'])
        self.assertEqual(r.hook(self.root, self.payload(session='next'))['hookSpecificOutput']['permissionDecision'], 'deny')

    def test_changed_retiring_incarnation_prevents_close(self):
        self.successor()
        def changed(root, argv):
            receipt = self.receipt(root, argv)
            if argv[:2] == ['terminal', 'show'] and argv[3] == 'coord-terminal':
                receipt['result']['terminal']['incarnationId'] = 'replacement'
            return receipt
        with patch.object(r, 'invoke', side_effect=changed):
            with self.assertRaisesRegex(ValueError, 'identity changed'):
                r.close_retired(self.root, 'next', self.run, 'coord-terminal')
        self.assertFalse(any(c[:2] == ['terminal', 'close'] for c in self.calls))

    def test_scoped_drift_remains_latched_and_does_not_rebind(self):
        self.register()
        os.environ['ORCA_TERMINAL_HANDLE'] = 'wrong-terminal'
        result = r.hook(self.root, self.payload(session='coord'))
        self.assertEqual(result['hookSpecificOutput']['permissionDecision'], 'deny')
        os.environ['ORCA_TERMINAL_HANDLE'] = 'coord-terminal'
        self.assertEqual(r.hook(self.root, self.payload(session='coord'))['hookSpecificOutput']['permissionDecision'], 'deny')
        self.assertEqual(self.saved()['terminal'], 'coord-terminal')
        with self.assertRaisesRegex(ValueError, 'drift'):
            r.configure(self.root, 'coord', 5, run_id=self.run)

    def test_old_initial0_transcript_cannot_be_resumed_as_fresh_successor(self):
        token = self.launch()
        path = self.transcript_for('next')
        path.write_text(json.dumps({'sessionId': 'next', 'timestamp': '2020-01-01T00:00:00+00:00'}) + '\n')
        self.boot('next', 'next-terminal')
        self.current_terminal = 'next-terminal'
        with self.assertRaisesRegex(ValueError, 'fresh initial0'):
            r.claim(self.root, 'next', token, self.run)
        self.assertEqual(self.saved()['owner'], 'coord')
        # A missing transcript timestamp cannot stand in for fresh session proof.
        r.session_path(self.root, 'next').unlink()
        path.write_text(json.dumps({'sessionId': 'next'}) + '\n')
        self.boot('next', 'next-terminal')
        with self.assertRaisesRegex(ValueError, 'fresh initial0'):
            r.claim(self.root, 'next', token, self.run)

    def test_absent_global_cli_hook_creates_independent_state_in_both_harnesses(self):
        self.path.unlink()
        for harness in ('claude', 'codex'):
            sid = 'cli-' + harness
            transcript = self.transcript_for(sid)
            if harness == 'codex':
                transcript.write_text(json.dumps({'type': 'session_meta', 'timestamp': r.now(), 'payload': {'id': sid}}) + '\n')
            cmd = [sys.executable, '-B', '-I', str(Path(r.__file__)), '--root', str(self.root), 'hook', '--harness', harness]
            result = subprocess.run(cmd, input=json.dumps(self.payload(session=sid, name='Read')),
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            proof = r.load(r.session_path(self.root, sid))
            self.assertEqual(proof['boundaries'], [])
            self.assertEqual(proof['harness'], harness)
            self.assertEqual(proof['hook_events'], ['PreToolUse'])
        self.assertFalse(self.path.exists())

    def test_native_scoped_final_round_keeps_codex_veto_and_admin_gate(self):
        os.environ['ORCA_TERMINAL_HANDLE'] = 'coord-terminal'
        sid = 'native-coord'
        transcript = self.transcript_for(sid)
        transcript.write_text(json.dumps({'type': 'session_meta', 'timestamp': r.now(), 'payload': {'id': sid}}) + '\n')
        payload = {**self.payload(session=sid), 'rotation_harness': 'codex'}
        r.hook(self.root, payload)
        r.register(self.root, sid, self.run, str(transcript), 'g', 'l', self.navigation)
        for i in range(4):
            self.append({'type': 'compacted', 'payload': {'window_id': str(i)}}, sid)
        result = r.hook(self.root, {**payload, 'hook_event_name': 'PreCompact'})
        self.assertEqual(r.codex_output({'hook_event_name': 'PreCompact'}, result)['continue'], False)
        command = f'python3 -I {r.SCRIPT} status --run {self.run}'
        self.assertEqual(r.hook(self.root, {**payload, 'tool_name': 'exec_command', 'tool_input': {'cmd': command}}), {})
        self.assertEqual(self.saved()['boundaries'], ['codex:0', 'codex:1', 'codex:2', 'codex:3'])

    def test_corrupt_scoped_authority_fails_closed_without_touching_legacy(self):
        self.register()
        path = r.coordinator_path(self.root, self.run)
        before = path.read_bytes()
        legacy = self.path.read_bytes()
        for mutation in ({'identity_receipt': None}, {'retired_sessions': [{}]}, {'threshold': True},
                         {'run_id': 'run_other'}, {'terminal': 'wrong'}, {'phase': 'invalid'}):
            r.atomic(path, {**json.loads(before), **mutation})
            with self.assertRaises(ValueError):
                r.hook(self.root, self.payload(session='coord'))
            self.assertEqual(self.path.read_bytes(), legacy)
        path.write_bytes(before)
        transcript = self.transcript_for('coord')
        transcript.write_text('{bad\n')
        with self.assertRaises(ValueError):
            r.hook(self.root, self.payload(session='coord'))
        self.assertEqual(path.read_bytes(), before)

    def test_unknown_launch_retains_raw_receipt_and_allows_only_bounded_inspection(self):
        self.retire()
        observation = {'argv': ['agent', 'launch'], 'exit': 1, 'stdout': 'request_id=fixture-only', 'stderr': 'unknown'}
        def refused(root, argv):
            if argv[:2] == ['agent', 'launch']:
                raise r.InvocationError('unknown delivery', observation)
            return self.receipt(root, argv)
        with patch.object(r, 'invoke', side_effect=refused):
            with self.assertRaisesRegex(ValueError, 'unknown'):
                r.launch(self.root, 'coord', self.run)
        self.assertEqual(self.saved()['request']['launch_failure'], observation)
        for command in (f'python3 -I {r.SCRIPT} status --run {self.run}',
                        f'orca orchestration run-show --id {self.run} --full --json',
                        'orca terminal show --terminal term_fixture --json',
                        f'orca terminal list --worktree path:{self.root.resolve()} --limit 20 --json'):
            self.assertEqual(r.hook(self.root, self.payload(session='coord', name='Bash', data={'command': command})), {})
        for command in ('orca agent launch --agent claude', 'orca terminal close --terminal term_fixture',
                        'orca orchestration run-use --id run_other', 'orca terminal list --json'):
            result = r.hook(self.root, self.payload(session='coord', name='Bash', data={'command': command}))
            self.assertEqual(result['hookSpecificOutput']['permissionDecision'], 'deny')

    def test_navigation_changed_run_oversized_and_wrong_stop_transcript_refuse_retirement(self):
        self.retire()
        state = self.saved()
        state['phase'] = 'requested'
        state['retired_sessions'] = []
        r.atomic(r.coordinator_path(self.root, self.run), state)
        wrong = self.transcript_for('coord').with_suffix('.copied')
        wrong.write_bytes(self.transcript_for('coord').read_bytes())
        with self.assertRaisesRegex(ValueError, 'differs'):
            r.retire(self.root, 'coord', str(wrong), self.run)
        nav = self.root / self.navigation
        r.atomic(nav, {'run_id': 'run_unrelated', 'product_task_id': 'unbound', 'product_attempt_id': 'unbound'})
        with self.assertRaisesRegex(ValueError, 'mismatch'):
            r.retire(self.root, 'coord', str(self.transcript_for('coord')), self.run)
        nav.write_text(' ' * 48001)
        with self.assertRaisesRegex(ValueError, '48KB'):
            r.retire(self.root, 'coord', str(self.transcript_for('coord')), self.run)
        self.assertEqual(self.saved()['phase'], 'requested')

    def test_installed_delivery_outcomes_confirm_once(self):
        self.retire()
        retired = copy.deepcopy(self.saved())
        for outcome in ('journaled', 'handed-to-terminal'):
            with self.subTest(outcome=outcome):
                r.atomic(r.coordinator_path(self.root, self.run), retired)
                receipt = installed_launch_receipt(outcome)
                with patch.object(r, 'invoke', side_effect=lambda root, argv:
                                  receipt if argv[:2] == ['agent', 'launch'] else self.receipt(root, argv)):
                    error = None
                    try:
                        r.launch(self.root, 'coord', self.run)
                    except ValueError as caught:
                        error = str(caught)
                    self.assertIsNone(error, 'installed result.prompt must confirm delivery')
                    self.assertTrue(self.saved()['request']['delivery_confirmed'])
                    self.assertEqual(self.saved()['request']['delivery_status'], outcome)
                    self.assertEqual(self.saved()['request']['launch_receipt'], receipt)
                    with self.assertRaisesRegex(ValueError, 'already attempted'):
                        r.launch(self.root, 'coord', self.run)

    def test_bad_delivery_envelopes_remain_latched_with_distinct_status(self):
        self.retire()
        retired = copy.deepcopy(self.saved())
        cases = [
            ('nested-only', {'ok': True, 'result': {'launch': {'prompt': {'outcome': 'journaled'}}}}, 'missing'),
            ('missing', {'ok': True, 'result': {}}, 'missing'),
            ('missing-outcome', {'ok': True, 'result': {'prompt': {}}}, 'missing'),
            ('missing-ok', {'result': {'prompt': {'outcome': 'journaled'}}}, 'malformed'),
            ('false-ok', {'ok': False, 'result': {'prompt': {'outcome': 'journaled'}}}, 'error'),
            ('error', {**installed_launch_receipt(), 'error': {'code': 'refused'}}, 'error'),
            ('result-error', {'ok': True, 'result': {'error': 'refused', 'prompt': {'outcome': 'journaled'}}}, 'error'),
            ('prompt-error', {'ok': True, 'result': {'prompt': {'outcome': 'journaled', 'error': 'refused'}}}, 'error'),
            ('unknown', installed_launch_receipt('queued'), 'unknown'),
            ('not-delivered', installed_launch_receipt('not-delivered'), 'unknown'),
            ('unconfirmed', installed_launch_receipt('unconfirmed'), 'unknown'),
        ]
        for field, value, status in (('delivery', None, 'malformed'), ('delivery', [], 'malformed'),
                                     ('delivery', 'draft', 'unknown'), ('delivery', 'other', 'unknown'),
                                     ('messageId', None, 'malformed'), ('messageId', [], 'malformed')):
            receipt = installed_launch_receipt()
            receipt['result']['prompt'][field] = value
            cases.append(('bad-' + field + repr(value), receipt, status))
        for field in ('delivery', 'messageId'):
            receipt = installed_launch_receipt()
            del receipt['result']['prompt'][field]
            cases.append(('missing-' + field, receipt, 'missing'))
        for value in (None, [], 'bad', 1, True):
            cases.append(('bad-root-' + repr(value), value, 'malformed'))
            cases.append(('bad-result-' + repr(value), {'ok': True, 'result': value}, 'malformed'))
            cases.append(('bad-prompt-' + repr(value), {'ok': True, 'result': {'prompt': value}}, 'malformed'))
            cases.append(('bad-outcome-' + repr(value), installed_launch_receipt(value),
                          'unknown' if isinstance(value, str) else 'malformed'))
        for name, receipt, status in cases:
            with self.subTest(name=name):
                r.atomic(r.coordinator_path(self.root, self.run), retired)
                launches = []
                def invoke(root, argv):
                    if argv[:2] == ['agent', 'launch']:
                        launches.append(argv)
                        return receipt
                    return self.receipt(root, argv)
                with patch.object(r, 'invoke', side_effect=invoke):
                    with self.assertRaisesRegex(ValueError, 'prompt delivery'):
                        r.launch(self.root, 'coord', self.run)
                    with self.assertRaisesRegex(ValueError, 'already attempted'):
                        r.launch(self.root, 'coord', self.run)
                request = self.saved()['request']
                self.assertFalse(request['delivery_confirmed'])
                self.assertEqual(request['delivery_status'], status)
                self.assertEqual(request['launch_receipt'], receipt)
                self.assertEqual(len(launches), 1)
                self.assertEqual(self.saved()['phase'], 'launching')

    def test_installed_receipt_after_fresh_claim_updates_request_and_history(self):
        self.retire()
        receipt = installed_launch_receipt()
        def invoke(root, argv):
            if argv[:2] != ['agent', 'launch']:
                return self.receipt(root, argv)
            self.boot('next', 'next-terminal')
            self.current_terminal = 'next-terminal'
            r.claim(self.root, 'next', self.saved()['request']['token'], self.run)
            self.assertIn('hookSpecificOutput', r.hook(self.root, self.payload(session='next', name='Edit')))
            with self.assertRaisesRegex(ValueError, 'close requires'):
                r.close_retired(self.root, 'next', self.run, 'coord-terminal')
            os.environ['ORCA_TERMINAL_HANDLE'] = 'coord-terminal'
            return receipt
        with patch.object(r, 'invoke', side_effect=invoke):
            r.launch(self.root, 'coord', self.run)
        os.environ['ORCA_TERMINAL_HANDLE'] = 'next-terminal'
        saved = self.saved()
        for record in (saved['request'], saved['history'][-1]):
            self.assertEqual(record['launch_receipt'], receipt)
            self.assertEqual(record['delivery_status'], 'journaled')
            self.assertTrue(record['delivery_confirmed'])
        self.assertEqual(saved['owner'], 'next')
        self.assertIn('hookSpecificOutput', r.hook(self.root, self.payload(session='next', name='Edit')))
        r.close_retired(self.root, 'next', self.run, 'coord-terminal')
        self.assertEqual(r.hook(self.root, self.payload(session='next', name='Edit')), {})
        for i in range(4):
            self.boundary('next-' + str(i), 'next')
        r.hook(self.root, self.payload(session='next'))
        self.assertNotIn('delivery_confirmed', self.saved()['request'])
        self.assertEqual(self.saved()['history'][-1]['launch_receipt'], receipt)
        self.assertTrue(self.saved()['history'][-1]['close_confirmed'])

    def test_second_rotation_does_not_reuse_old_closure_or_verification(self):
        self.successor()
        r.close_retired(self.root, 'next', self.run, 'coord-terminal')
        for i in range(4):
            self.boundary('next-' + str(i), 'next')
        blocked = r.hook(self.root, self.payload(session='next'))
        self.assertEqual(blocked['hookSpecificOutput']['permissionDecision'], 'deny')
        self.assertEqual(self.saved()['phase'], 'requested')
        self.assertNotIn('launch_started_at', self.saved()['request'])
        self.assertTrue(self.saved()['history'][-1]['delivery_confirmed'])
        self.assertTrue(self.saved()['history'][-1]['close_confirmed'])
        self.assertEqual(self.saved()['history'][-1]['close_receipt']['result']['close']['handle'], 'coord-terminal')
        command = f'python3 -I {r.SCRIPT} retire --session next --run {self.run} --transcript x'
        self.assertEqual(r.hook(self.root, self.payload(session='next', name='Bash', data={'command': command})), {})


if __name__ == '__main__':
    unittest.main()
