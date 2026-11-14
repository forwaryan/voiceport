"""No live networking: test fixed sideband auth, redaction, and stop boundaries."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest import mock
import uuid

MODULE = Path(__file__).resolve().parents[2] / "scripts/chatgpt_realtime_observer.py"
spec = importlib.util.spec_from_file_location("chatgpt_realtime_observer", MODULE)
observer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(observer)


class ObserverTests(unittest.TestCase):
    def setUp(self):
        # websocket-client is optional. Keep the offline suite independent of
        # site-packages, and fail closed if a test forgets to stub a connection.
        websocket = types.ModuleType("websocket")
        websocket.create_connection = mock.Mock(side_effect=AssertionError("unexpected connection"))
        websocket.WebSocketBadStatusException = type("WebSocketBadStatusException", (Exception,), {})
        websocket.WebSocketTimeoutException = type("WebSocketTimeoutException", (Exception,), {})
        module_patch = mock.patch.dict(sys.modules, {"websocket": websocket})
        module_patch.start()
        self.addCleanup(module_patch.stop)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.reference = self.root / "reference.json"
        self.reference.write_text(json.dumps({"call_id": "rtc_SYNTHETIC_PRIVATE", "session_id": str(uuid.uuid4())}))
        self.output = self.root / "observed"
        self.auth = self.root / "auth.json"
        self.auth.write_text(json.dumps({"auth_mode": "chatgpt", "tokens": {
            "access_token": "SYNTHETIC_TOKEN", "account_id": "SYNTHETIC_ACCOUNT"}}))

    def test_offline_does_not_read_credentials_or_connect(self):
        with mock.patch.object(observer, "observe", side_effect=AssertionError), contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(observer.main([]), 0)
        self.assertFalse(json.loads(out.getvalue())["network"])

    def test_missing_optional_dependency_is_reported_before_credentials(self):
        with mock.patch.dict(sys.modules, {"websocket": None}), \
                mock.patch.object(observer.probe, "proxy_url", return_value=None), \
                mock.patch.object(observer.probe, "credentials", side_effect=AssertionError) as credentials:
            report = observer.observe(self.reference, self.output, self.auth)
        self.assertEqual(report["classification"], "websocket_dependency_unavailable")
        self.assertEqual(report["request_count"], 0)
        credentials.assert_not_called()

    def test_observer_uses_fixed_url_and_sends_no_application_messages(self):
        ws = mock.Mock()
        ws.getstatus.return_value = 101
        ws.recv.side_effect = [json.dumps({"type": "input_transcript.added", "text": "synthetic text"}), ""]
        ready = mock.Mock()
        with mock.patch.object(observer.probe, "proxy_url", return_value=None), \
                mock.patch("websocket.create_connection", return_value=ws) as connect:
            report = observer.observe(self.reference, self.output, self.auth, ready)
        connect.assert_called_once()
        self.assertEqual(connect.call_args.args[0], "wss://api.openai.com/v1/live/rtc_SYNTHETIC_PRIVATE")
        self.assertEqual(connect.call_args.kwargs["redirect_limit"], 0)
        self.assertEqual(connect.call_args.kwargs["header"]["Authorization"], "Bearer SYNTHETIC_TOKEN")
        ws.send.assert_not_called()
        ready.assert_called_once()
        self.assertTrue(report["websocket_connected"])
        self.assertEqual(report["events"][0]["text"], "synthetic text")
        saved = (self.output / "report.json").read_text()
        for private in ("SYNTHETIC_TOKEN", "SYNTHETIC_ACCOUNT", "rtc_SYNTHETIC_PRIVATE"):
            self.assertNotIn(private, saved)
        self.assertEqual((self.output / "report.json").stat().st_mode & 0o777, 0o600)

    def test_cli_fails_when_observation_errors_after_connecting(self):
        cases = [
            ("malformed", "{invalid_json", "sideband_transport_or_input_error"),
            ("oversized", "x" * (observer.MAX_EVENT_BYTES + 1), "sideband_event_too_large"),
            ("transport", OSError("SYNTHETIC_PRIVATE_ERROR"), "sideband_transport_or_input_error"),
        ]
        for name, received, classification in cases:
            with self.subTest(name=name):
                ws = mock.Mock()
                ws.getstatus.return_value = 101
                ws.recv.side_effect = [received]
                output = self.root / name
                with mock.patch.object(observer.probe, "proxy_url", return_value=None), \
                        mock.patch.object(observer.probe, "auth_path", return_value=self.auth), \
                        mock.patch("websocket.create_connection", return_value=ws), \
                        contextlib.redirect_stdout(io.StringIO()) as stdout:
                    code = observer.main(["--observe", "--call-reference", str(self.reference),
                                          "--output", str(output)])
                report = json.loads((output / "report.json").read_text())
                self.assertTrue(report["websocket_connected"])
                self.assertEqual(report["classification"], classification)
                self.assertEqual(code, 1)
                self.assertEqual(json.loads(stdout.getvalue().splitlines()[-1])["classification"], classification)
                self.assertNotIn("SYNTHETIC_PRIVATE_ERROR", stdout.getvalue())
                ws.close.assert_called_once()

    def test_cli_succeeds_when_observation_finishes_normally(self):
        for name, received, classification in [
            ("closed", "", "upstream_closed"),
            ("complete", json.dumps({"type": "session.started"}), "observation_complete"),
        ]:
            with self.subTest(name=name):
                ws = mock.Mock()
                ws.getstatus.return_value = 101
                ws.recv.side_effect = [received]
                output = self.root / name
                with mock.patch.object(observer.probe, "proxy_url", return_value=None), \
                        mock.patch.object(observer.probe, "auth_path", return_value=self.auth), \
                        mock.patch.object(observer, "MAX_EVENTS", 1), \
                        mock.patch("websocket.create_connection", return_value=ws), \
                        contextlib.redirect_stdout(io.StringIO()):
                    code = observer.main(["--observe", "--call-reference", str(self.reference),
                                          "--output", str(output)])
                report = json.loads((output / "report.json").read_text())
                self.assertEqual(report["classification"], classification)
                self.assertEqual(code, 0)
                ws.close.assert_called_once()

    def test_upstream_errors_and_session_fields_are_redacted(self):
        summary = observer.event_summary({"type": "error", "error": {"message": "SYNTHETIC_TOKEN", "type": "forbidden"}})
        self.assertEqual(summary["type"], "error")
        self.assertNotIn("SYNTHETIC_TOKEN", json.dumps(summary))
        summary = observer.event_summary({"type": "session.started", "credentials": "SYNTHETIC_TOKEN"})
        self.assertNotIn("SYNTHETIC_TOKEN", json.dumps(summary))
        summary = observer.event_summary({"type": {"unexpected": "object"}})
        self.assertEqual(summary["type"], "other_event")

    def test_v3_nested_fragments_and_role_are_preserved(self):
        first = observer.event_summary({"type": "input_transcript.added", "item": {"text": "这是"}})
        second = observer.event_summary({"type": "input_transcript.added", "item": {"text": "实时测试。"}})
        self.assertEqual(first["text"] + second["text"], "这是实时测试。")
        done = observer.event_summary({"type": "turn.done", "turn": {"role": "user", "transcript": "这是实时测试。"}})
        self.assertEqual(done["role"], "user")
        self.assertEqual(done["text"], "这是实时测试。")
        self.assertNotIn("text", observer.event_summary({"type": "session.started", "item": {"text": "SYNTHETIC_SECRET"}}))

    def test_invalid_reference_rejected_before_credentials(self):
        self.reference.write_text(json.dumps({"call_id": "../../other.example", "session_id": str(uuid.uuid4())}))
        with mock.patch.object(observer.probe, "credentials", side_effect=AssertionError):
            report = observer.observe(self.reference, self.output, self.auth)
        self.assertEqual(report["request_count"], 0)
        self.assertEqual(report["classification"], "invalid_call_reference")

    def test_existing_output_rejected_before_credentials(self):
        self.output.mkdir()
        with mock.patch.object(observer.probe, "credentials", side_effect=AssertionError), self.assertRaises(FileExistsError):
            observer.observe(self.reference, self.output, self.auth)

    def test_redirect_upgrade_is_not_observed(self):
        ws = mock.Mock()
        ws.getstatus.return_value = 302
        with mock.patch.object(observer.probe, "proxy_url", return_value=None), \
                mock.patch("websocket.create_connection", return_value=ws):
            report = observer.observe(self.reference, self.output, self.auth)
        ws.recv.assert_not_called()
        self.assertEqual(report["classification"], "websocket_upgrade_not_accepted")


if __name__ == "__main__":
    unittest.main()
