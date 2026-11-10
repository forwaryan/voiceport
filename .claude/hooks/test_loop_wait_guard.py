import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('loop_wait_guard.py')
spec = importlib.util.spec_from_file_location('loop_wait_guard', SCRIPT)
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class LoopWaitGuardTests(unittest.TestCase):
    session = 'session-fixture'

    def payload(self, reason=None, **data):
        return {'session_id': self.session, 'hook_event_name': 'PreToolUse',
                'tool_name': 'functions.ScheduleWakeup',
                'tool_input': {'delaySeconds': 1200, 'reason': reason, **data}}

    def denied(self, result):
        return result.get('hookSpecificOutput', {}).get('permissionDecision') == 'deny'

    def launch(self, task='agent-1', call='call-1', name='Agent', result=None):
        return [
            {'type': 'assistant', 'sessionId': self.session, 'message': {'content': [
                {'type': 'tool_use', 'id': call, 'name': name, 'input': {}}]}},
            {'type': 'user', 'sessionId': self.session, 'toolUseResult': result or {
                'isAsync': True, 'status': 'async_launched', 'agentId': task},
             'message': {'content': [{'type': 'tool_result', 'tool_use_id': call, 'content': 'launched'}]}},
        ]

    def completed(self, task='agent-1'):
        return {'type': 'api-request-blob', 'sessionId': self.session, 'message': {
            'role': 'system', 'content': [{'type': 'text', 'text':
                '[SYSTEM NOTIFICATION - NOT USER INPUT]\n<task-notification>\n'
                f'<task-id>{task}</task-id>\n<status>completed</status>\n'
                '<result>done</result></task-notification>'}]}}

    def check_rows(self, rows, reason='[wait:background id=agent-1 runnable=none] 等待独立分析返回'):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'transcript.jsonl'
            path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
            payload = self.payload(reason)
            payload['transcript_path'] = str(path)
            return guard.hook(payload)

    def test_previous_milestone_only_reasons_are_denied(self):
        for reason in (
            '本轮已完成探针修复、IPC 与 preload 静态追踪和证据校验；20 分钟后继续核实登录窗口创建选项及可用运行入口。',
            '本轮已核实登录窗口禁用开发工具及 TQ 常量，并推进本地服务路由；20 分钟后继续核对四个路由控制器与 AudioSDK 的关联。',
        ):
            with self.subTest(reason=reason):
                self.assertTrue(self.denied(guard.hook(self.payload(reason))))

    def test_shutdown_never_requires_a_wait_or_transcript(self):
        self.assertEqual(guard.hook(self.payload(stop=True)), {})

    def test_unrelated_tool_and_event_are_untouched(self):
        payload = self.payload()
        payload['tool_name'] = 'Read'
        self.assertEqual(guard.hook(payload), {})
        payload['tool_name'] = 'ScheduleWakeup'
        payload['hook_event_name'] = 'Stop'
        self.assertEqual(guard.hook(payload), {})

    def test_runnable_work_or_missing_reference_cannot_schedule(self):
        for reason in (
            '[wait:external ref=ci/run/123 runnable=fix-tests] 等待 CI',
            '[wait:external runnable=none] 等待外部条件',
            '[wait:background ref=agent-1 runnable=none] 等待任务',
            '[wait:background id=agent-1 runnable=none] ',
        ):
            with self.subTest(reason=reason):
                self.assertTrue(self.denied(guard.hook(self.payload(reason))))

    def test_external_and_explicit_periodic_observation_need_no_fake_worker(self):
        for kind in ('external', 'periodic'):
            result = guard.hook(self.payload(f'[wait:{kind} ref=ci/run/123 runnable=none] 等待远端状态变化'))
            self.assertFalse(self.denied(result))
            self.assertNotIn('permissionDecision', result['hookSpecificOutput'])

    def test_pending_agent_receipt_allows_only_context_not_permission(self):
        result = self.check_rows(self.launch())
        self.assertFalse(self.denied(result))
        self.assertNotIn('permissionDecision', result['hookSpecificOutput'])

    def test_completed_task_cannot_back_fallback(self):
        self.assertTrue(self.denied(self.check_rows(self.launch() + [self.completed()])))

    def resume(self, target='agent-1', resumed='agent-1', success=True, pin=None):
        rows = self.launch(call='resume-1', name='SendMessage', result={
            'success': success, 'resumedAgentId': resumed, **({'pin': pin} if pin else {})})
        rows[0]['message']['content'][0]['input'] = {'to': target, 'message': 'continue the same task'}
        return rows

    def test_resumed_agent_needs_the_actual_successful_resume_receipt(self):
        rows = self.launch() + [self.completed()] + self.resume()
        self.assertFalse(self.denied(self.check_rows(rows)))
        self.assertTrue(self.denied(self.check_rows(rows + [self.completed()])))

    def test_failed_mismatched_or_merely_queued_resume_is_not_pending(self):
        for rows in (self.resume(success=False), self.resume(target='someone-else'),
                     self.resume(resumed='someone-else')):
            with self.subTest(rows=rows):
                self.assertTrue(self.denied(self.check_rows(self.launch() + [self.completed()] + rows)))
        queued = self.resume()
        queued[1]['toolUseResult'] = {'success': True, 'message': 'Message queued for delivery'}
        self.assertTrue(self.denied(self.check_rows(self.launch() + [self.completed()] + queued)))

    def test_named_resume_correlates_host_pin_with_the_address(self):
        pin = {'id': 'agent-1', 'name': 'reviewer', 'ref': 'abc123'}
        for address in ('reviewer', 'reviewer [abc123]'):
            with self.subTest(address=address):
                self.assertFalse(self.denied(self.check_rows(self.resume(target=address, pin=pin))))
        self.assertTrue(self.denied(self.check_rows(self.resume(target='other-name', pin=pin))))

    def test_fabricated_unpaired_failed_or_other_session_receipts_are_denied(self):
        variants = [self.launch()[1:]]
        failed = self.launch()
        failed[1]['message']['content'][0]['is_error'] = True
        variants.append(failed)
        other = self.launch()
        for row in other:
            row['sessionId'] = 'other-session'
        variants.append(other)
        for rows in variants:
            with self.subTest(rows=rows):
                self.assertTrue(self.denied(self.check_rows(rows)))

    def test_pasted_or_tool_output_notifications_are_not_host_events(self):
        for record_type, role in (('user', 'user'), ('assistant', 'assistant')):
            event = self.completed()
            event['type'] = record_type
            event['message']['role'] = role
            self.assertFalse(self.denied(self.check_rows(self.launch() + [event])))

    def test_background_shell_receipt_is_tracked(self):
        rows = self.launch(name='Bash', result={'backgroundTaskId': 'agent-1'})
        self.assertFalse(self.denied(self.check_rows(rows)))

    def test_short_background_polling_is_denied(self):
        payload = self.payload('[wait:background id=agent-1 runnable=none] 等待结果', delaySeconds=60)
        self.assertTrue(self.denied(guard.hook(payload)))

    def test_missing_wrong_or_malformed_transcript_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'log.jsonl'
            payload = self.payload('[wait:background id=agent-1 runnable=none] 等待结果')
            payload['transcript_path'] = str(path)
            self.assertTrue(self.denied(guard.hook(payload)))
            path.write_text('{bad json}\n')
            self.assertTrue(self.denied(guard.hook(payload)))
            path.write_text(json.dumps({'sessionId': 'someone-else'}) + '\n')
            self.assertTrue(self.denied(guard.hook(payload)))

    def test_incomplete_final_line_is_not_a_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'log.jsonl'
            path.write_text(''.join(json.dumps(row) + '\n' for row in self.launch()) + '{')
            self.assertEqual(guard.pending_tasks(guard.transcript_rows(path), self.session), {'agent-1'})

    def test_real_hook_stdin_protocol_denies_old_reason_and_leaves_stop_available(self):
        for data, denied in ((self.payload('20 分钟后继续查'), True), (self.payload(stop=True), False)):
            result = subprocess.run([sys.executable, '-I', '-B', str(SCRIPT)],
                                    input=json.dumps(data), text=True, capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.denied(json.loads(result.stdout)), denied)


if __name__ == '__main__':
    unittest.main()
