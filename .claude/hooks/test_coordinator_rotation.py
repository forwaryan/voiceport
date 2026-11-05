"""Lifecycle checks use temporary projects and mocked Orca; never launch real agents."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('rotation', Path(__file__).with_name('coordinator_rotation.py'))
r = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(r)


class RotationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / '.voiceport').mkdir()
        self.path = self.root / '.voiceport' / r.STATE_NAME
        self.transcript = self.root / 'session.jsonl'
        self.transcript.touch()
        self.state = {'schema_version': 1, 'owner': 'old', 'epoch': 1, 'run_id': 'run_one',
                      'phase': 'active', 'threshold': 2, 'boundaries': [], 'retired_sessions': [],
                      'request': None, 'history': [], 'goal': 'deliver', 'loop_prompt': 'continue safely'}
        r.atomic(self.path, self.state)
        r.atomic(self.root / '.voiceport/coordination.json', {'run_id': 'run_one',
                 'developer': {'dispatch': 'developer'}, 'verifier': {'dispatch': 'verifier'},
                 'freeze': {'sha256': 'unchanged'}, 'product_attempt': 'same-attempt'})
        r.atomic(self.root / '.voiceport/loop-state.json', {'paused': True, 'tasks': {
            'VP-013': {'status': 'running', 'attempts': [{'id': 'same-attempt'}]}}})

    def append(self, value):
        with self.transcript.open('a') as out:
            out.write(json.dumps(value) + '\n')

    def boundary(self, identity, session='old'):
        self.append({'subtype': 'compact_boundary', 'uuid': identity, 'sessionId': session})

    def payload(self, event='PreToolUse', session='old', name='Edit', data=None):
        return {'hook_event_name': event, 'session_id': session, 'transcript_path': str(self.transcript),
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

    def test_worker_hooks_never_write_lease_or_request_rotation(self):
        before = self.path.read_bytes(), self.path.stat().st_mtime_ns
        self.boundary('one', 'worker')
        self.assertEqual(r.hook(self.root, self.payload(session='worker')), {})
        self.assertEqual((self.path.read_bytes(), self.path.stat().st_mtime_ns), before)

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
        with patch.object(r, 'invoke', return_value={'ok': True, 'result': {'launch': {'prompt': {'outcome': 'journaled'}}}}):
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
        with patch.object(r, 'invoke', return_value={'ok': True, 'result': {'launch': {'prompt': {'outcome': 'handed-to-terminal'}}}}):
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
            return {'ok': True, 'result': {'launch': {'prompt': {'outcome': 'journaled'}}}}
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

    def test_no_state_is_noop_and_bad_state_fails_tool_hook(self):
        self.path.unlink()
        self.assertEqual(r.hook(self.root, self.payload()), {})
        self.path.write_text('{bad')
        result = subprocess.run([sys.executable, '-I', str(Path(r.__file__)), '--root', str(self.root), 'hook'],
                                input=json.dumps(self.payload()), capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn('hook failed', result.stderr)


if __name__ == '__main__':
    unittest.main()
