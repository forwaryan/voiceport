"""Offline controls for the opt-in account handshake probe; synthetic inputs only."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

MODULE = Path(__file__).resolve().parents[2] / "scripts/chatgpt_realtime_probe.py"
spec = importlib.util.spec_from_file_location("chatgpt_realtime_probe", MODULE)
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class Response:
    def __init__(self, code, body, headers=None):
        self.code, self.body, self.headers = code, body, headers or {}

    def read(self, maximum):
        return self.body[:maximum]

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False


class ProbeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.offer = self.root / "offer.sdp"
        self.offer.write_text("v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n")
        self.auth = self.root / "auth.json"
        self.auth.write_text(json.dumps({"auth_mode": "chatgpt", "tokens": {
            "access_token": "SYNTHETIC_TOKEN", "account_id": "SYNTHETIC_ACCOUNT"}}))
        self.output = self.root / "result"

    def test_default_never_reads_auth_or_networks(self):
        with mock.patch.object(probe, "credentials", side_effect=AssertionError), \
                mock.patch.object(probe.urllib.request, "build_opener", side_effect=AssertionError), \
                contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(probe.main([]), 0)
        self.assertEqual(json.loads(out.getvalue())["mode"], "offline")

    def run_response(self, response):
        opener = mock.Mock()
        opener.open.return_value = response
        with mock.patch.object(probe, "proxy_url", return_value=None), \
                mock.patch.object(probe.urllib.request, "build_opener", return_value=opener):
            report = probe.connect(self.offer, self.output, self.auth)
        return report, opener

    def test_one_fixed_first_party_request_and_no_secret_in_report(self):
        report, opener = self.run_response(Response(201, b"v=0\r\ns=synthetic-answer\r\n"))
        self.assertEqual(report["classification"], "handshake_accepted")
        self.assertFalse(report["transcription_verified"])
        opener.open.assert_called_once()
        request = opener.open.call_args.args[0]
        self.assertEqual(request.full_url, probe.ENDPOINT)
        self.assertEqual(request.get_header("Authorization"), "Bearer SYNTHETIC_TOKEN")
        self.assertFalse(report["audio_uploaded"])
        saved = (self.output / "report.json").read_text()
        self.assertNotIn("SYNTHETIC_TOKEN", saved)
        self.assertNotIn("SYNTHETIC_ACCOUNT", saved)
        self.assertEqual((self.output / "answer.sdp").stat().st_mode & 0o777, 0o600)

    def test_private_call_reference_never_enters_report(self):
        location = "https://api.openai.com/v1/realtime/calls/rtc_SYNTHETIC_PRIVATE"
        report, _ = self.run_response(Response(201, b"v=0\r\ns=answer\r\n", {"Location": location}))
        ref_path = self.output / "call-reference.json"
        self.assertTrue(report["call_reference_saved"])
        self.assertEqual(json.loads(ref_path.read_text())["call_id"], "rtc_SYNTHETIC_PRIVATE")
        self.assertEqual(ref_path.stat().st_mode & 0o777, 0o600)
        self.assertNotIn("rtc_SYNTHETIC_PRIVATE", json.dumps(report))
        self.assertIsNone(probe.call_reference("https://other.example/calls/rtc_STOLEN"))
        self.assertIsNone(probe.call_reference("//other.example/calls/rtc_STOLEN"))
        self.assertIsNone(probe.call_reference("/calls/rtc_bad%0D%0A"))

    def test_redirect_never_forwards_credentials(self):
        self.assertIsNone(probe.NoRedirect().redirect_request(None, None, 302, "", {}, "https://other.example/"))
        report, opener = self.run_response(Response(302, b"SYNTHETIC_TOKEN", {"Location": "https://other.example/"}))
        self.assertEqual(report["classification"], "redirect_refused")
        self.assertEqual(opener.open.call_count, 1)
        self.assertNotIn("other.example", json.dumps(report))
        self.assertFalse((self.output / "answer.sdp").exists())

    def test_denial_and_malicious_error_details_are_redacted(self):
        report, _ = self.run_response(Response(403, json.dumps({"error": {
            "code": "SYNTHETIC_TOKEN", "type": "forbidden", "message": "SYNTHETIC_ACCOUNT"}}).encode(),
            {"cf-mitigated": "challenge"}))
        self.assertEqual(report["classification"], "account_or_edge_access_denied")
        self.assertTrue(report["edge_challenge"])
        self.assertEqual(report["service_error_type"], "forbidden")
        self.assertNotIn("SYNTHETIC", json.dumps(report))

    def test_api_key_auth_rejected_before_network(self):
        self.auth.write_text('{"auth_mode":"apikey","OPENAI_API_KEY":"SYNTHETIC"}')
        with mock.patch.object(probe, "proxy_url", return_value=None), \
                mock.patch.object(probe.urllib.request, "build_opener", side_effect=AssertionError):
            report = probe.connect(self.offer, self.output, self.auth)
        self.assertEqual(report["request_count"], 0)
        self.assertEqual(report["classification"], "chatgpt_file_login_required")

    def test_oversize_response_does_not_save_body(self):
        report, _ = self.run_response(Response(200, b"x" * (probe.MAX_RESPONSE_BYTES + 1)))
        self.assertEqual(report["classification"], "response_too_large")
        self.assertFalse((self.output / "answer.sdp").exists())

    def test_existing_output_blocks_network_and_auth(self):
        self.output.mkdir()
        with mock.patch.object(probe, "credentials", side_effect=AssertionError), \
                self.assertRaises(FileExistsError):
            probe.connect(self.offer, self.output, self.auth)

    def test_proxy_credentials_and_bad_offer_rejected(self):
        with mock.patch.dict(probe.os.environ, {"VOICEPORT_HTTPS_PROXY": "http://user:secret@localhost:1234"}), \
                self.assertRaises(probe.ProbeError):
            probe.proxy_url()
        self.offer.write_text("not an audio offer")
        report = probe.connect(self.offer, self.output, self.auth)
        self.assertEqual(report["request_count"], 0)
        self.assertEqual(report["classification"], "invalid_webrtc_offer")


if __name__ == "__main__":
    unittest.main()
