#!/usr/bin/env python3
"""Opt-in, read-only observer for an existing ChatGPT-account AVAS v3 call.

No API key, microphone, audio upload, session update, tool execution, or retries.
The fixed sideband URL follows the pinned official Codex implementation. A
successful WebSocket connection does not establish transcription-only support.
Run with python3 -I. Requires websocket-client only for --observe.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
from pathlib import Path
import time
import urllib.parse
import uuid

_spec = importlib.util.spec_from_file_location("voiceport_probe", Path(__file__).with_name("chatgpt_realtime_probe.py"))
probe = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(probe)
SIDEBAND_BASE = "wss://api.openai.com/v1/live/"
MAX_EVENTS = 100
MAX_EVENT_BYTES = 65_536
OBSERVE_SECONDS = 25


def event_summary(event: dict) -> dict:
    """Save transcript events, never arbitrary session/error response bodies."""
    known = {"session.created", "session.started", "session.updated", "turn.started", "turn.done",
             "input_audio.speech_started", "input_audio.speech_stopped", "input_audio_buffer.speech_started",
             "input_audio_buffer.speech_stopped", "input_transcript.added", "output_transcript.added",
             "conversation.item.input_audio_transcription.delta", "conversation.item.input_audio_transcription.completed",
             "error", "delegation.started", "delegation.requested"}
    kind = event.get("type")
    kind = kind if isinstance(kind, str) else "other_event"
    result = {"type": kind if kind in known else "other_event",
              "received_at_ms": round(time.time() * 1000)}
    if kind in {"input_transcript.added", "output_transcript.added",
                "conversation.item.input_audio_transcription.delta", "conversation.item.input_audio_transcription.completed"}:
        for key in ("text", "transcript", "delta"):
            value = event.get(key)
            if isinstance(value, str):
                result[key] = value[:4000]
        item = event.get("item")
        if isinstance(item, dict) and isinstance(item.get("text"), str):
            # AVAS v3's added events contain one incremental fragment in item.text.
            result["text"] = item["text"][:4000]
    if kind == "turn.done":
        turn = event.get("turn")
        if isinstance(turn, dict) and turn.get("role") in ("user", "assistant") and isinstance(turn.get("transcript"), str):
            result.update(role=turn["role"], text=turn["transcript"][:4000])
    if kind == "error":
        error = event.get("error")
        if isinstance(error, dict):
            for key in ("code", "type"):
                if error.get(key) in {"invalid_request_error", "invalid_auth", "unauthorized", "forbidden", "rate_limit_exceeded"}:
                    result["service_error_" + key] = error[key]
    return result


def observe(reference: Path, output: Path, auth: Path | None = None, ready=None) -> dict:
    output.mkdir(mode=0o700, parents=False, exist_ok=False)
    report = {"schema_version": 1, "classification": "not_connected", "websocket_connected": False,
              "endpoint_base": SIDEBAND_BASE, "api_key_used": False, "microphone_requested": False,
              "audio_uploaded": False, "application_messages_sent": 0, "request_count": 0,
              "retries": 0, "redirects_followed": 0, "ignored_event_count": 0, "events": []}
    ws = None
    try:
        ref = json.loads(probe.read_capped(reference, 4096))
        call_id, session_id = ref["call_id"], ref["session_id"]
        if not isinstance(call_id, str) or probe.call_reference("/" + call_id) != call_id:
            raise probe.ProbeError("invalid_call_reference")
        if not isinstance(session_id, str) or str(uuid.UUID(session_id)) != session_id:
            raise probe.ProbeError("invalid_call_reference")
        proxy = probe.proxy_url()
        options = {"timeout": 10, "redirect_limit": 0, "suppress_origin": True}
        if proxy:
            parsed = urllib.parse.urlsplit(proxy)
            if parsed.scheme != "http":
                raise probe.ProbeError("unsupported_sideband_proxy")
            options.update(http_proxy_host=parsed.hostname, http_proxy_port=parsed.port or 80,
                           http_proxy_timeout=10, proxy_type="http")
        import websocket
        token, account = probe.credentials(auth if auth is not None else probe.auth_path())
        options["header"] = {"Authorization": "Bearer " + token, "ChatGPT-Account-Id": account,
                             "originator": "voiceport_realtime_probe", "openai-alpha": "quicksilver=v2",
                             "x-session-id": session_id, "User-Agent": "VoicePort-Realtime-Probe/0.1"}
        report["request_count"] = 1
        try:
            ws = websocket.create_connection(SIDEBAND_BASE + call_id, **options)
        except websocket.WebSocketBadStatusException as error:
            status = error.status_code
            report.update(http_status=status, classification={401: "account_authentication_rejected", 403: "account_or_edge_access_denied",
                          404: "call_not_active_or_unavailable", 429: "rate_or_usage_limit"}.get(status, "websocket_handshake_rejected"))
        if ws is not None:
            if ws.getstatus() != 101:
                raise probe.ProbeError("websocket_upgrade_not_accepted")
            report.update(websocket_connected=True, classification="observing")
            if ready:
                ready()
            end = time.monotonic() + OBSERVE_SECONDS
            while (time.monotonic() < end and len(report["events"]) < MAX_EVENTS
                   and report["ignored_event_count"] < 1000):
                ws.settimeout(min(2, max(0.01, end - time.monotonic())))
                try:
                    data = ws.recv()
                except websocket.WebSocketTimeoutException:
                    continue
                if not data:
                    report["classification"] = "upstream_closed"
                    break
                if len(data) > MAX_EVENT_BYTES:
                    raise probe.ProbeError("sideband_event_too_large")
                event = json.loads(data)
                if isinstance(event, dict):
                    summary = event_summary(event)
                    if summary["type"] == "other_event":
                        report["ignored_event_count"] += 1
                    else:
                        report["events"].append(summary)
            else:
                report["classification"] = "observation_complete"
    except probe.ProbeError as error:
        report["classification"] = str(error)
    except ImportError:
        report["classification"] = "websocket_dependency_unavailable"
    except Exception:
        # Exception strings can contain auth-bearing headers or response bodies.
        report["classification"] = "sideband_transport_or_input_error"
    finally:
        if ws is not None:
            try:
                ws.close(timeout=1)
            except Exception:
                pass
    dest = output / "report.json"
    with dest.open("x") as stream:
        os.chmod(dest, 0o600)
        json.dump(report, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    return report


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--observe", action="store_true")
    parser.add_argument("--call-reference", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    if not args.observe:
        print(json.dumps({"mode": "offline", "auth_read": False, "network": False,
                          "endpoint_base": SIDEBAND_BASE, "api_key_used": False}))
        return 0
    if args.call_reference is None or args.output is None:
        parser.error("--observe requires --call-reference and --output")
    try:
        report = observe(args.call_reference, args.output,
                         ready=lambda: print(json.dumps({"status": "sideband_ready"}), flush=True))
    except OSError:
        print(json.dumps({"classification": "output_directory_unavailable", "request_count": 0}))
        return 1
    # Transcripts belong in the private report, not terminal output.
    print(json.dumps({k: v for k, v in report.items() if k != "events"} | {"event_count": len(report["events"])}))
    return 0 if (report["websocket_connected"]
                 and report["classification"] in {"observation_complete", "upstream_closed"}) else 1


if __name__ == "__main__":
    raise SystemExit(main())
