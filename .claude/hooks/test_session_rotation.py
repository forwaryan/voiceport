"""Isolated top-level rotation and scheduler regressions; no live Orca side effects."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('rotation', Path(__file__).with_name('coordinator_rotation.py'))
r = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(r)


class SessionRotationTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        (self.root / '.voiceport').mkdir()
        self.source = 'direct'
        self.calls = []
        self.sequence = 0
        self.contract = {'goal': 'finish the authorized task', 'loop_prompt': '/loop 20m preserve the exact original prompt',
                         'authorization': 'local source changes only; no recording, upload, commit or push',
                         'paused': False, 'active_tools': []}
        r.atomic(self.root / '.voiceport/loop-state.json', {'paused': True, 'tasks': {'VP-013': {'attempt': 'retained'}}})
        r.atomic(self.root / '.voiceport/coordination.json', {'run_id': 'run_unrelated', 'workers': {}, 'freeze': 'retained'})
        self.protected = {p: p.read_bytes() for p in (self.root / '.voiceport').glob('*.json')}
        env = patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'term_direct'})
        env.start()
        self.addCleanup(env.stop)
        invoke = patch.object(r, 'invoke', side_effect=self.receipt)
        invoke.start()
        self.addCleanup(invoke.stop)
        self.boot()

    def transcript(self, session='direct'):
        path = self.root / (session + '.jsonl')
        if not path.exists():
            path.write_text(json.dumps({'sessionId': session, 'timestamp': r.now()}) + '\n')
        return path

    def append(self, row, session='direct'):
        with self.transcript(session).open('a') as out:
            out.write(json.dumps(row) + '\n')

    def payload(self, name='Read', data=None, session='direct', event='PreToolUse'):
        return {'hook_event_name': event, 'session_id': session, 'transcript_path': str(self.transcript(session)),
                'tool_name': name, 'tool_input': data or {}}

    def boot(self, session='direct', terminal='term_direct'):
        os.environ['ORCA_TERMINAL_HANDLE'] = terminal
        return r.hook(self.root, self.payload(session=session))

    def saved(self, session='direct'):
        return r.load(r.session_path(self.root, session))

    def receipt(self, root, argv):
        self.calls.append(argv)
        if argv[:2] == ['orchestration', 'run-current']:
            return {'ok': True, 'result': {'run': None}}
        if argv[:2] == ['orchestration', 'worker-list']:
            return {'ok': True, 'result': {'workers': [], 'scope': {'source': 'all'}, 'page': {'hasMore': False, 'nextCursor': None}}}
        if argv[:2] == ['terminal', 'show']:
            handle = argv[3]
            return {'ok': True, 'result': {'terminal': {'handle': handle, 'connected': True, 'orphaned': False,
                    'worktreePath': str(root.resolve()), 'incarnationId': handle + '-inc', 'agentIdentity': 'claude'}}}
        if argv[:2] == ['agent', 'launch']:
            self.assertIn(self.saved(self.source)['phase'], ('launching', 'claimed'))
            self.assertIn('launch_started_at', self.saved(self.source)['request'])
            return {'ok': True, 'result': {'outcome': {'kind': 'terminal', 'handle': 'term_next'},
                    'prompt': {'delivery': 'submit', 'outcome': 'handed-to-terminal'}}}
        if argv[:2] == ['terminal', 'close']:
            self.assertIn('close_started_at', self.saved(self.source)['request'])
            return {'ok': True, 'result': {'close': {'handle': argv[3], 'ptyKilled': True}}}
        self.fail('unexpected Orca mutation: ' + repr(argv))

    def enroll(self):
        path = r.contract_path(self.root, self.source)
        r.atomic(path, self.contract)
        return r.session_register(self.root, self.source, path.relative_to(self.root).as_posix())

    def request(self):
        for index in range(4):
            self.append({'sessionId': self.source, 'subtype': 'compact_boundary', 'uuid': f'{self.source}-{index}'}, self.source)
        r.hook(self.root, self.payload(session=self.source))

    def tool(self, name, data, result=None, error=False, session='direct'):
        self.sequence += 1
        call = f'call-{self.sequence}'
        self.append({'sessionId': session, 'timestamp': r.now(), 'message': {'content': [
            {'type': 'tool_use', 'id': call, 'name': name, 'input': data}]}}, session)
        item = {'type': 'tool_result', 'tool_use_id': call, 'content': 'host result'}
        if error:
            item['is_error'] = True
        self.append({'sessionId': session, 'toolUseResult': result, 'message': {'content': [item]}}, session)
        return call

    def stop(self, session='direct'):
        return self.tool('ScheduleWakeup', {'stop': True}, {'stopped': True, 'cancelledWakeups': 0}, session=session)

    def retire(self):
        self.enroll()
        self.request()
        r.checkpoint_notes(self.root, self.source, 'Original scope, logs, no active tools, remaining work and user boundaries retained.')
        self.stop(self.source)
        return r.session_retire(self.root, self.source)

    def launch(self):
        self.retire()
        r.session_launch(self.root, self.source)
        return self.saved(self.source)['request']['token']

    def claim(self):
        token = self.launch()
        self.boot('next', 'term_next')
        return r.session_claim(self.root, 'next', self.source, token)

    def test_standalone_drift_opens_only_exact_recovery_and_owned_shutdown(self):
        self.enroll()
        self.tool('CronCreate', {'cron': '0 * * * *', 'prompt': 'original task'}, {'id': 'owned'})
        os.environ['ORCA_TERMINAL_HANDLE'] = 'term_conflict'
        r.hook(self.root, self.payload('Edit'))
        before = self.saved()
        command = f'python3 -I {r.SCRIPT} session-recover --session direct'
        allowed = [('Bash', {'command': command}), ('exec_command', {'cmd': command}),
                   ('ScheduleWakeup', {'stop': True}), ('CronList', {}), ('CronDelete', {'id': 'owned'})]
        for name, data in allowed:
            self.assertEqual(r.hook(self.root, self.payload(name, data)), {}, (name, data))
        for bad in (command + '; cargo test', command + ' --root /tmp', command.replace('direct', 'other'),
                    command + ' --session direct', 'orca agent launch --agent claude --json'):
            result = r.hook(self.root, self.payload('Bash', {'command': bad}))
            self.assertEqual(result['hookSpecificOutput']['permissionDecision'], 'deny', bad)
        for name, data in [('CronCreate', {'cron': '* * * * *'}), ('CronDelete', {'id': 'foreign'}),
                           ('ScheduleWakeup', {'stop': True, 'delaySeconds': 1200}), ('Skill', {'skill': 'orca-cli'})]:
            self.assertEqual(r.hook(self.root, self.payload(name, data))['hookSpecificOutput']['permissionDecision'], 'deny')
        after = self.saved()
        for key in ('terminal', 'boundaries', 'terminal_drift', 'standalone'):
            self.assertEqual(after[key], before[key])
        self.assertIn('session-recover --session direct', r.terminal_drift_hint(after))

    def test_recovery_gate_does_not_grant_workers_or_returned_terminal_a_rescue(self):
        command = f'python3 -I {r.SCRIPT} session-recover --session direct'
        os.environ['ORCA_TERMINAL_HANDLE'] = 'term_conflict'
        self.assertEqual(r.hook(self.root, self.payload('Bash', {'command': command}))['hookSpecificOutput']['permissionDecision'], 'deny')
        proof = self.saved()
        proof['standalone'] = {'contract': self.contract}
        self.assertTrue(r.standalone_recovery_candidate(proof))
        proof['coordinator_run'] = 'run_existing'
        self.assertFalse(r.standalone_recovery_candidate(proof))
        del proof['coordinator_run']
        proof['harness'] = 'codex'
        self.assertFalse(r.standalone_recovery_candidate(proof))
        proof['harness'] = 'claude'
        os.environ['ORCA_TERMINAL_HANDLE'] = 'term_direct'
        self.assertFalse(r.standalone_recovery_candidate(proof))

    def test_recovery_entry_rejects_healthy_session_without_external_mutations(self):
        before = self.saved()
        self.calls.clear()
        with self.assertRaises(ValueError):
            r.session_recover(self.root, self.source, rotation=r)
        self.assertFalse(self.calls)
        self.assertEqual(self.saved(), before)

    def test_drift_stop_reminds_once_only_when_unpaused_and_rescue_not_attempted(self):
        self.enroll()
        r.atomic(self.root / '.voiceport/loop-state.json', {'paused': False})
        os.environ['ORCA_TERMINAL_HANDLE'] = 'term_conflict'
        r.hook(self.root, self.payload('Edit'))
        stop = self.payload(event='Stop')
        self.assertEqual(r.hook(self.root, stop)['decision'], 'block')
        self.assertEqual(r.hook(self.root, {**stop, 'stop_hook_active': True}), {})
        r.atomic(self.root / '.voiceport/loop-state.json', {'paused': True})
        self.assertEqual(r.hook(self.root, stop), {})
        r.atomic(self.root / '.voiceport/loop-state.json', {'paused': False})
        with r.session_transaction(self.root, self.source) as state:
            state['inherited_pause'] = True
        self.assertEqual(r.hook(self.root, stop), {})
        with r.session_transaction(self.root, self.source) as state:
            state['inherited_pause'] = False
        r.atomic(self.root / '.voiceport/session-recoveries/direct.json', {'management_launch_started_at': r.now()})
        self.assertEqual(r.hook(self.root, stop), {})

    def test_startup_before_transcript_defers_without_manufacturing_proof(self):
        session = 'not-yet-written'
        path = self.root / (session + '.jsonl')
        payload = {'session_id': session, 'hook_event_name': 'SessionStart', 'transcript_path': str(path)}
        result = r.hook(self.root, payload)
        self.assertIn('first real PreToolUse', result['hookSpecificOutput']['additionalContext'])
        self.assertFalse(r.session_path(self.root, session).exists())
        payload.update(hook_event_name='PreToolUse', tool_name='Read', tool_input={})
        with self.assertRaises(FileNotFoundError):
            r.hook(self.root, payload)
        self.assertFalse(r.session_path(self.root, session).exists())
        self.boot(session, 'term_new')
        proof = self.saved(session)
        self.assertTrue(proof['transcript_session_verified'])
        self.assertEqual(proof['boundaries'], [])
        self.assertNotIn('SessionStart', proof['hook_events'])

    def test_voluntary_handoff_keeps_real_accounting_and_uses_normal_fences(self):
        self.enroll()
        before = self.saved()
        request = r.session_request(self.root, self.source, 'bounded live lifecycle rehearsal')
        after = self.saved()
        self.assertEqual(after['boundaries'], before['boundaries'])
        self.assertEqual(after['threshold'], 5)
        self.assertEqual(request['trigger'], 'voluntary')
        self.assertEqual(request['completed_compactions_at_request'], 0)
        self.assertEqual(after['phase'], 'requested')
        self.assertEqual(r.session_request(self.root, self.source, 'another reason'), request)
        self.assertEqual(r.hook(self.root, self.payload('Edit'))['hookSpecificOutput']['permissionDecision'], 'deny')
        r.checkpoint_notes(self.root, self.source, 'Harmless rehearsal; no active tools; preserve scope.')
        self.stop()
        r.session_retire(self.root, self.source)
        r.session_launch(self.root, self.source)
        self.assertEqual(self.saved()['boundaries'], [])
        self.assertEqual(self.saved()['phase'], 'launching')

    def test_voluntary_request_rejects_unregistered_or_unsettled_predecessor(self):
        with self.assertRaisesRegex(ValueError, 'registered active'):
            r.session_request(self.root, self.source, 'early handoff')
        self.claim()
        with self.assertRaisesRegex(ValueError, 'prior predecessor'):
            r.session_request(self.root, 'next', 'cannot rotate before predecessor exit')
        self.assertEqual(self.saved('next')['phase'], 'active')
        self.assertEqual(self.saved()['phase'], 'claimed')

    def test_voluntary_request_never_overwrites_natural_threshold_request(self):
        self.enroll()
        self.request()
        before = self.saved()
        self.assertEqual(r.session_request(self.root, self.source, 'late manual request'), before['request'])
        self.assertEqual(self.saved()['boundaries'], before['boundaries'])
        for reason in ('', 'x' * 2001):
            with self.assertRaisesRegex(ValueError, 'nonempty reason'):
                r.session_request(self.root, self.source, reason)

    def test_direct_final_round_recovers_without_inventing_worker_registration(self):
        self.request()
        before = self.saved()['boundaries']
        stop = self.payload('ScheduleWakeup', {'stop': True})
        self.assertEqual(r.hook(self.root, stop), {})
        r.checkpoint_notes(self.root, 'direct', 'Existing historical notes remain intact.')
        with self.assertRaisesRegex(ValueError, 'one exact registered worker'):
            r.checkpoint(self.root, 'direct')
        path = r.contract_path(self.root, 'direct')
        self.assertEqual(r.hook(self.root, self.payload('Write', {'file_path': str(path)})), {})
        command = f'python3 -I {r.SCRIPT} session-register --session direct --contract .voiceport/handoffs/session-contract-direct.json'
        self.assertEqual(r.hook(self.root, self.payload('Bash', {'command': command})), {})
        self.enroll()
        self.stop()
        r.session_retire(self.root, 'direct')
        r.session_launch(self.root, 'direct')
        self.assertEqual(self.saved()['boundaries'], before)
        self.assertEqual(self.saved()['phase'], 'launching')
        for path, raw in self.protected.items():
            self.assertEqual(path.read_bytes(), raw)

    def test_loop_cannot_start_until_contract_and_identity_are_registered(self):
        for name, data in [('ScheduleWakeup', {'delaySeconds': 1200}), ('CronCreate', {'cron': '*/20 * * * *', 'prompt': 'task'})]:
            self.assertEqual(r.hook(self.root, self.payload(name, data))['hookSpecificOutput']['permissionDecision'], 'deny')
        self.enroll()
        self.assertEqual(r.hook(self.root, self.payload('ScheduleWakeup', {'delaySeconds': 1200})), {})

    def test_registration_rejects_bound_run_managed_or_incomplete_membership(self):
        cases = [{'run': {'id': 'run_one'}}, {'workers': [{'agentTerminalHandle': 'term_direct'}], 'scope': {'source': 'all'}, 'page': {'hasMore': False, 'nextCursor': None}},
                 {'workers': [], 'scope': {'source': 'bound'}, 'nextCursor': None}, {'workers': [], 'scope': {'source': 'all'}}]
        for result in cases:
            original = self.receipt
            def overridden(root, argv):
                match = ['orchestration', 'run-current'] if 'run' in result else ['orchestration', 'worker-list']
                return {'ok': True, 'result': result} if argv[:2] == match else original(root, argv)
            with self.subTest(result=result), patch.object(r, 'invoke', side_effect=overridden):
                with self.assertRaises(ValueError):
                    self.enroll()
            self.assertNotIn('standalone', self.saved())

    def test_worker_membership_checks_later_pages_and_stale_registry(self):
        seen = []
        def pages(root, argv):
            if argv[:2] == ['orchestration', 'worker-list']:
                seen.append(argv)
                result = {'workers': [], 'scope': {'source': 'all'}, 'page': {'hasMore': True, 'nextCursor': 'page2'}} if '--cursor' not in argv else {
                    'workers': [{'agentTerminalHandle': 'term_direct', 'terminalState': 'retained'}], 'scope': {'source': 'all'}, 'page': {'hasMore': False, 'nextCursor': None}}
                return {'ok': True, 'result': result}
            return self.receipt(root, argv)
        with patch.object(r, 'invoke', side_effect=pages), self.assertRaisesRegex(ValueError, 'managed worker'):
            self.enroll()
        self.assertEqual(len(seen), 2)
        r.atomic(self.root / '.voiceport/coordination.json', {'workers': {'reviewer': {'terminal': 'term_direct'}}})
        with self.assertRaisesRegex(ValueError, 'registered worker'):
            self.enroll()

    def test_contract_requires_full_scope_and_remains_immutable(self):
        self.contract.pop('authorization')
        with self.assertRaisesRegex(ValueError, 'contract requires'):
            self.enroll()
        self.contract['authorization'] = 'local changes only'
        self.enroll()
        self.contract['loop_prompt'] = 'expanded unapproved objective'
        with self.assertRaisesRegex(ValueError, 'immutable'):
            self.enroll()

    def test_roundtrip_preserves_pause_scope_and_waits_for_exact_exit(self):
        result = self.claim()
        self.assertTrue(result['paused'])
        old = self.saved()
        packet = r.load(self.root / old['request']['packet'])
        self.assertEqual(packet['contract'], self.contract)
        self.assertEqual(self.saved('next')['boundaries'], [])
        self.assertTrue(self.saved('next')['inherited_pause'])
        for name, data in [('Edit', {}), ('ScheduleWakeup', {'delaySeconds': 1200}), ('CronCreate', {'cron': '* * * * *'})]:
            self.assertEqual(r.hook(self.root, self.payload(name, data, session='next'))['hookSpecificOutput']['permissionDecision'], 'deny')
        command = f'python3 -I {r.SCRIPT} session-close --session next --source direct --terminal term_direct'
        self.assertEqual(r.hook(self.root, self.payload('Bash', {'command': command}, session='next')), {})
        r.session_close(self.root, 'next', 'direct', 'term_direct')
        self.assertEqual(r.hook(self.root, self.payload('Edit', session='next')), {})
        self.boot()
        self.assertEqual(r.hook(self.root, self.payload('Edit'))['hookSpecificOutput']['permissionDecision'], 'deny')
        self.assertEqual(len([c for c in self.calls if c[:2] == ['terminal', 'close']]), 1)
        for path, raw in self.protected.items():
            self.assertEqual(path.read_bytes(), raw)

    def test_launch_timeout_and_bad_delivery_are_latched(self):
        self.retire()
        def timeout(root, argv):
            if argv[:2] == ['agent', 'launch']:
                raise subprocess.TimeoutExpired(argv, 45)
            return self.receipt(root, argv)
        with patch.object(r, 'invoke', side_effect=timeout):
            with self.assertRaisesRegex(ValueError, 'unknown'):
                r.session_launch(self.root, 'direct')
            with self.assertRaisesRegex(ValueError, 'already attempted'):
                r.session_launch(self.root, 'direct')
        self.assertIn('launch_failure', self.saved()['request'])
        self.assertEqual(self.saved()['phase'], 'launching')

    def test_claim_requires_real_fresh_session_token_and_untampered_notes(self):
        token = self.launch()
        with self.assertRaisesRegex(ValueError, 'different session'):
            r.session_claim(self.root, 'direct', 'direct', token)
        self.boot('next', 'term_next')
        with self.assertRaisesRegex(ValueError, 'token'):
            r.session_claim(self.root, 'next', 'direct', 'wrong')
        self.append({'sessionId': 'next', 'subtype': 'compact_boundary', 'uuid': 'used'}, 'next')
        with self.assertRaisesRegex(ValueError, 'fresh initial0'):
            r.session_claim(self.root, 'next', 'direct', token)
        self.boot('fresh', 'term_next')
        r.notes_path(self.root, 'direct').write_text('Tampered after retirement')
        with self.assertRaisesRegex(ValueError, 'notes changed'):
            r.session_claim(self.root, 'fresh', 'direct', token)
        self.assertEqual(self.saved()['phase'], 'launching')

    def test_successor_cannot_close_unrelated_target_or_wrong_launch_handle(self):
        self.claim()
        with self.assertRaisesRegex(ValueError, 'exact predecessor'):
            r.session_close(self.root, 'next', 'direct', 'term_other')
        with r.session_transaction(self.root, 'direct') as old:
            old['request']['launch_receipt']['result']['outcome']['handle'] = 'term_unrelated'
        with self.assertRaisesRegex(ValueError, 'verified launch target'):
            r.session_close(self.root, 'next', 'direct', 'term_direct')
        self.assertFalse(any(c[:2] == ['terminal', 'close'] for c in self.calls))

    def test_unknown_close_remains_fenced_and_never_repeated(self):
        self.claim()
        def uncertain(root, argv):
            return {'ok': True, 'result': {}} if argv[:2] == ['terminal', 'close'] else self.receipt(root, argv)
        with patch.object(r, 'invoke', side_effect=uncertain):
            with self.assertRaisesRegex(ValueError, 'does not prove'):
                r.session_close(self.root, 'next', 'direct', 'term_direct')
            with self.assertRaisesRegex(ValueError, 'never retry'):
                r.session_close(self.root, 'next', 'direct', 'term_direct')
        self.assertEqual(r.hook(self.root, self.payload('Edit', session='next'))['hookSpecificOutput']['permissionDecision'], 'deny')

    def test_claim_racing_launch_receipt_cannot_work_until_delivery_and_close(self):
        self.retire()
        def racing(root, argv):
            if argv[:2] != ['agent', 'launch']:
                return self.receipt(root, argv)
            token = self.saved()['request']['token']
            self.boot('next', 'term_next')
            r.session_claim(self.root, 'next', 'direct', token)
            with self.assertRaisesRegex(ValueError, 'verified launch target'):
                r.session_close(self.root, 'next', 'direct', 'term_direct')
            os.environ['ORCA_TERMINAL_HANDLE'] = 'term_direct'
            return self.receipt(root, argv)
        with patch.object(r, 'invoke', side_effect=racing):
            r.session_launch(self.root, 'direct')
        self.assertEqual(self.saved()['phase'], 'claimed')
        self.assertTrue(self.saved()['request']['delivery_confirmed'])
        self.boot('next', 'term_next')
        r.session_close(self.root, 'next', 'direct', 'term_direct')

    def test_successive_rotations_preserve_old_evidence_and_get_new_budget(self):
        self.claim()
        r.session_close(self.root, 'next', 'direct', 'term_direct')
        old = r.session_path(self.root, 'direct').read_bytes()
        self.source = 'next'
        self.request()
        self.stop('next')
        r.checkpoint_notes(self.root, 'next', 'Second-generation bounded notes; no active tools.')
        r.session_retire(self.root, 'next')
        r.session_launch(self.root, 'next')
        self.assertEqual(r.session_path(self.root, 'direct').read_bytes(), old)
        self.assertEqual(len(self.saved('next')['boundaries']), 4)
        self.assertNotIn('close_confirmed', self.saved('next')['request'])

    def test_final_admin_parser_does_not_open_shell_or_product_operations(self):
        self.enroll()
        self.request()
        good = f'python3 -I {r.SCRIPT} session-retire --session direct'
        for tool, key in [('Bash', 'command'), ('exec_command', 'cmd')]:
            self.assertEqual(r.hook(self.root, self.payload(tool, {key: good})), {})
            for bad in (good + '; pwd', good + ' --session other', good.replace('direct', 'other'),
                        'orca agent launch --agent claude', 'orca terminal close --terminal term_direct'):
                self.assertEqual(r.hook(self.root, self.payload(tool, {key: bad}))['hookSpecificOutput']['permissionDecision'], 'deny')
        r.checkpoint_notes(self.root, 'direct', 'Bounded notes, no active tools; scheduling shutdown still required.')
        with self.assertRaisesRegex(ValueError, 'successfully'):
            r.session_retire(self.root, 'direct')

    def test_stop_receipt_requires_structured_success_and_no_later_dynamic_restart(self):
        for result, error in [(None, False), ({'stopped': False}, False), ({'stopped': True}, True)]:
            self.tool('ScheduleWakeup', {'stop': True}, result, error)
            with self.assertRaisesRegex(ValueError, 'ScheduleWakeup'):
                r.stop_receipt(self.transcript(), 'direct')
        self.stop()
        self.assertEqual(r.stop_receipt(self.transcript(), 'direct')['kind'], 'dynamic')
        self.tool('ScheduleWakeup', {'delaySeconds': 1200}, {'scheduledFor': 123})
        with self.assertRaisesRegex(ValueError, 'latest scheduling'):
            r.stop_receipt(self.transcript(), 'direct')

    def test_cron_final_gate_only_deletes_proven_owned_jobs(self):
        self.enroll()
        self.tool('CronCreate', {'cron': '*/20 * * * *', 'prompt': 'exact authorized prompt'}, {'id': 'owned'})
        self.tool('CronList', {}, {'jobs': [{'id': 'owned'}, {'id': 'foreign'}]})
        self.request()
        for name, data in [('CronList', {}), ('CronDelete', {'id': 'owned'}), ('ScheduleWakeup', {'stop': True})]:
            self.assertEqual(r.hook(self.root, self.payload(name, data)), {})
        for name, data in [('CronDelete', {'id': 'foreign'}), ('CronDelete', {'id': 'owned', 'force': True}),
                           ('ScheduleWakeup', {'stop': True, 'prompt': 'renew'}), ('CronCreate', {'prompt': 'renew'})]:
            self.assertEqual(r.hook(self.root, self.payload(name, data))['hookSpecificOutput']['permissionDecision'], 'deny')

    def test_cron_shutdown_needs_all_deletes_then_final_list(self):
        for job in ('one', 'two'):
            self.tool('CronCreate', {'cron': '*/20 * * * *', 'prompt': job}, {'id': job})
        self.stop()
        with self.assertRaisesRegex(ValueError, 'CronDelete'):
            r.stop_receipt(self.transcript(), 'direct')
        self.tool('CronDelete', {'id': 'one'}, {'id': 'one'})
        self.tool('CronDelete', {'id': 'two'}, {'id': 'wrong'})
        with self.assertRaisesRegex(ValueError, 'CronDelete'):
            r.stop_receipt(self.transcript(), 'direct')
        # Explicitly failed delete can be retried. An unknown malformed result is
        # deliberately not cleared by a later call without reconciliation.
        self.transcript().write_text(self.transcript().read_text().replace('"id": "wrong"', '"id": "two"'))
        with self.assertRaisesRegex(ValueError, 'CronList'):
            r.stop_receipt(self.transcript(), 'direct')
        self.tool('CronList', {}, {'jobs': [{'id': 'foreign'}]})
        result = r.stop_receipt(self.transcript(), 'direct')
        self.assertEqual(result['kind'], 'mixed')
        self.assertEqual(result['cron_jobs']['one']['prompt'], 'one')
        self.tool('CronCreate', {'cron': '* * * * *', 'prompt': 'third'}, {'id': 'three'})
        with self.assertRaisesRegex(ValueError, 'CronDelete'):
            r.stop_receipt(self.transcript(), 'direct')

    def test_cron_only_shutdown_and_failed_deletion_recovery(self):
        self.tool('CronCreate', {'cron': '*/20 * * * *', 'prompt': 'exact'}, {'id': 'owned'})
        self.tool('CronDelete', {'id': 'owned'}, None, error=True)
        self.tool('CronList', {}, {'jobs': [{'id': 'owned'}]})
        with self.assertRaisesRegex(ValueError, 'CronDelete'):
            r.stop_receipt(self.transcript(), 'direct')
        self.tool('CronDelete', {'id': 'owned'}, {'id': 'owned'})
        self.tool('CronList', {}, {'jobs': []})
        self.assertEqual(r.stop_receipt(self.transcript(), 'direct')['kind'], 'cron')

    def test_cron_ownership_and_pending_mutations_survive_transcript_truncation(self):
        self.enroll()
        self.tool('CronCreate', {'cron': '* * * * *', 'prompt': 'owned'}, {'id': 'retained'})
        r.hook(self.root, self.payload())
        self.transcript().write_text(json.dumps({'sessionId': 'direct', 'timestamp': r.now()}) + '\n')
        self.stop()
        r.hook(self.root, self.payload())
        self.assertEqual(r.hook(self.root, self.payload('CronDelete', {'id': 'retained'})), {})
        with self.assertRaisesRegex(ValueError, 'CronDelete'):
            r.stop_receipt(self.transcript(), 'direct', self.saved()['scheduler'])
        self.tool('CronDelete', {'id': 'retained'}, {'id': 'retained'})
        self.tool('CronList', {}, {'jobs': []})
        r.hook(self.root, self.payload())
        self.assertEqual(r.stop_receipt(self.transcript(), 'direct', self.saved()['scheduler'])['kind'], 'mixed')
        self.append({'sessionId': 'direct', 'message': {'content': [
            {'type': 'tool_use', 'name': 'CronCreate', 'id': 'unknown-create', 'input': {'prompt': 'unknown'}}]}})
        r.hook(self.root, self.payload())
        self.transcript().write_text(json.dumps({'sessionId': 'direct', 'timestamp': r.now()}) + '\n')
        self.stop()
        self.tool('CronList', {}, {'jobs': []})
        r.hook(self.root, self.payload())
        with self.assertRaisesRegex(ValueError, 'CronDelete'):
            r.stop_receipt(self.transcript(), 'direct', self.saved()['scheduler'])

    def test_full_worker_schema_rejects_concise_rows_and_inconsistent_pagination(self):
        for result in ({'workers': [{'terminal': 'term_direct'}], 'scope': {'source': 'all'},
                        'page': {'hasMore': False, 'nextCursor': None}},
                       {'workers': [], 'scope': {'source': 'all'}, 'page': {'hasMore': True, 'nextCursor': None}}):
            def wrong(root, argv):
                return {'ok': True, 'result': result} if argv[:2] == ['orchestration', 'worker-list'] else self.receipt(root, argv)
            with patch.object(r, 'invoke', side_effect=wrong), self.assertRaises(ValueError):
                self.enroll()
            self.assertNotIn('standalone', self.saved())

    def test_registration_rejects_unsettled_tools_and_symlink_contract(self):
        self.contract['active_tools'] = ['unsettled-background-handle']
        with self.assertRaisesRegex(ValueError, 'settle active tools'):
            self.enroll()
        path = r.contract_path(self.root, 'direct')
        path.unlink()
        target = self.root / 'external.json'
        target.write_text(json.dumps(self.contract))
        path.symlink_to(target)
        with self.assertRaisesRegex(ValueError, 'regular private file'):
            r.session_register(self.root, 'direct', path.relative_to(self.root).as_posix())

    def test_wrong_launch_target_cannot_consume_transfer_token(self):
        token = self.launch()
        self.boot('wrong', 'term_unrelated')
        with self.assertRaisesRegex(ValueError, 'verified launch target'):
            r.session_claim(self.root, 'wrong', 'direct', token)
        self.assertEqual(self.saved()['phase'], 'launching')
        self.boot('next', 'term_next')
        r.session_claim(self.root, 'next', 'direct', token)

    def test_old_initial0_transcript_cannot_claim_a_new_launch(self):
        self.transcript('old-context')
        token = self.launch()
        self.boot('old-context', 'term_next')
        with self.assertRaisesRegex(ValueError, 'fresh initial0'):
            r.session_claim(self.root, 'old-context', 'direct', token)

    def test_legacy_upgrade_preserves_history_and_enters_verified_run_path(self):
        self.request()
        legacy = {'schema_version': 1, 'owner': 'direct', 'epoch': 3, 'run_id': 'run_legacy',
                  'phase': 'requested', 'threshold': 5, 'boundaries': self.saved()['boundaries'],
                  'retired_sessions': ['retired-owner'], 'history': [{'id': 'prior-transfer'}],
                  'request': {'id': 'current-request', 'from_session': 'direct'},
                  'goal': 'original goal', 'loop_prompt': 'original full prompt'}
        r.atomic(r.coordinator_path(self.root), legacy)
        r.atomic(self.root / '.voiceport/coordination.json', {'run_id': 'run_legacy', 'workers': {}, 'freeze': 'retained'})
        r.atomic(self.root / '.voiceport/loop-state.json', {'paused': True, 'tasks': {
            'VP-013': {'status': 'running', 'attempts': [{'id': 'same-attempt'}]}}})
        command = f'python3 -I {r.SCRIPT} upgrade-legacy --session direct --run run_legacy'
        self.assertEqual(r.hook(self.root, self.payload('Bash', {'command': command})), {})
        def coordinator(root, argv):
            if argv[:2] in (['orchestration', 'run-current'], ['orchestration', 'run-show']):
                return {'ok': True, 'result': {'run': {'id': 'run_legacy', 'coordinator_handle': 'term_direct', 'consumer_generation': 7}}}
            return self.receipt(root, argv)
        with patch.object(r, 'invoke', side_effect=coordinator):
            r.upgrade_legacy(self.root, 'direct', 'run_legacy')
        upgraded = r.load(r.coordinator_path(self.root, 'run_legacy'))
        for field in ('epoch', 'boundaries', 'history', 'request', 'goal', 'loop_prompt', 'retired_sessions'):
            self.assertEqual(upgraded[field], legacy[field])
        self.assertEqual(r.load(r.coordinator_path(self.root))['migrated_run'], 'run_legacy')
        self.assertEqual(self.saved()['coordinator_run'], 'run_legacy')
        self.assertEqual(r.load(self.root / upgraded['navigation'])['product_attempt_id'], 'same-attempt')
        self.assertEqual(r.hook(self.root, self.payload('Edit'))['hookSpecificOutput']['permissionDecision'], 'deny')
        with self.assertRaisesRegex(ValueError, 'upgraded'):
            r.launch(self.root, 'direct')

    def test_cli_rotation_requests_terminal_and_rejects_structured_delivery(self):
        self.retire()
        def structured(root, argv):
            if argv[:2] == ['agent', 'launch']:
                self.assertEqual(argv[argv.index('--surface') + 1], 'terminal')
                return {'ok': True, 'result': {'outcome': {'kind': 'structured', 'handle': 'agent_1'},
                        'prompt': {'delivery': 'submit', 'outcome': 'journaled', 'messageId': 'msg_1'}}}
            return self.receipt(root, argv)
        with patch.object(r, 'invoke', side_effect=structured):
            with self.assertRaisesRegex(ValueError, 'prompt delivery'):
                r.session_launch(self.root, 'direct')
        self.assertEqual(self.saved()['request']['delivery_status'], 'non-terminal')
        self.assertFalse(self.saved()['request']['delivery_confirmed'])
        with self.assertRaisesRegex(ValueError, 'already attempted'):
            r.session_launch(self.root, 'direct')

    def test_dynamic_scheduler_provenance_survives_truncation_and_cron_shutdown(self):
        self.enroll()
        self.tool('ScheduleWakeup', {'delaySeconds': 1200, 'prompt': 'original task'}, {'scheduledFor': 1234})
        self.tool('CronCreate', {'cron': '* * * * *', 'prompt': 'owned'}, {'id': 'cron'})
        r.hook(self.root, self.payload())
        self.transcript().write_text(json.dumps({'sessionId': 'direct', 'timestamp': r.now()}) + '\n')
        self.tool('CronDelete', {'id': 'cron'}, {'id': 'cron'})
        self.tool('CronList', {}, {'jobs': []})
        self.request()
        r.checkpoint_notes(self.root, 'direct', 'All actual work settled; scheduler proof pending.')
        with self.assertRaisesRegex(ValueError, 'ScheduleWakeup'):
            r.session_retire(self.root, 'direct')
        self.stop()
        r.hook(self.root, self.payload())
        r.session_retire(self.root, 'direct')
        self.assertEqual(self.saved()['request']['loop_stop']['kind'], 'mixed')

    def test_truncated_earlier_stop_cannot_erase_later_persisted_schedule(self):
        self.stop()
        earlier_stop = self.transcript().read_text()
        self.tool('ScheduleWakeup', {'delaySeconds': 1200}, {'scheduledFor': 1234})
        r.hook(self.root, self.payload())
        self.transcript().write_text(earlier_stop)
        r.hook(self.root, self.payload())
        with self.assertRaisesRegex(ValueError, 'ScheduleWakeup'):
            r.stop_receipt(self.transcript(), 'direct', self.saved()['scheduler'])

    def test_native_scheduler_is_explicitly_unsupported_not_false_success(self):
        path = self.root / 'native.jsonl'
        path.write_text(json.dumps({'type': 'session_meta', 'payload': {'id': 'native'}}) + '\n')
        with self.assertRaisesRegex(ValueError, 'unverified'):
            r.stop_receipt(path, 'native')

    def test_standalone_identity_drift_fences_and_does_not_rebind(self):
        self.enroll()
        with patch.dict(os.environ, {'ORCA_TERMINAL_HANDLE': 'term_wrong'}):
            self.assertEqual(r.hook(self.root, self.payload('Edit'))['hookSpecificOutput']['permissionDecision'], 'deny')
            with self.assertRaisesRegex(ValueError, 'ORCA_TERMINAL_HANDLE'):
                r.session_retire(self.root, 'direct')
        self.assertEqual(r.hook(self.root, self.payload('Edit'))['hookSpecificOutput']['permissionDecision'], 'deny')
        self.assertEqual(self.saved()['terminal'], 'term_direct')


if __name__ == '__main__':
    unittest.main()
