#!/usr/bin/env python3
"""Opt-in ChatGPT-account WebRTC handshake probe; no microphone or API key.

Default invocation is an offline description and never opens an auth file.
--connect submits exactly one SDP offer to the fixed first-party Codex route.
An accepted handshake is not proof of live transcription or ASR-only support.
Run with python3 -I. Do not execute code from downloaded research directories.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import ssl
import stat
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

ENDPOINT = "https://chatgpt.com/backend-api/codex/realtime/calls?intent=quicksilver&architecture=avas"
UPSTREAM_COMMIT = "7924f95421a326bb66426a3ce232b72550948470"
MAX_AUTH_BYTES = 1_000_000
MAX_SDP_BYTES = 65_536
MAX_RESPONSE_BYTES = 131_072
SESSION = {
    "model": "gpt-live-1.1-codex",
    "instructions": "This is an isolated connection test. Do not invoke tools or delegate tasks.",
    "audio": {"output": {"voice": "cove"}},
    "delegation": {"type": "client", "ack_filler": False},
}


class ProbeError(Exception):
    """Only fixed, content-free classifications may cross the CLI boundary."""


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def read_capped(path: Path, maximum: int) -> bytes:
    with path.open("rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > maximum:
            raise ProbeError("invalid_input_file")
        data = stream.read(maximum + 1)
    if len(data) > maximum:
        raise ProbeError("input_too_large")
    return data


def auth_path() -> Path:
    root = os.environ.get("CODEX_HOME")
    return (Path(root) if root else Path.home() / ".codex") / "auth.json"


def credentials(path: Path) -> tuple[str, str]:
    try:
        obj = json.loads(read_capped(path, MAX_AUTH_BYTES))
        if obj.get("auth_mode", "chatgpt") != "chatgpt":
            raise ProbeError("chatgpt_file_login_required")
        token = obj["tokens"]["access_token"]
        account = obj["tokens"]["account_id"]
        for value in (token, account):
            if not isinstance(value, str) or not value or not all(0x21 <= ord(c) <= 0x7E for c in value):
                raise ProbeError("invalid_chatgpt_credentials")
        return token, account
    except ProbeError:
        raise
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        raise ProbeError("chatgpt_file_login_unavailable") from None


def proxy_url() -> str | None:
    explicit = os.environ.get("VOICEPORT_HTTPS_PROXY")
    value = explicit if explicit else urllib.request.getproxies().get("https")
    if not value:
        return None
    parsed = urllib.parse.urlsplit(value)
    if (parsed.scheme not in ("http", "https") or not parsed.hostname
            or parsed.username is not None or parsed.password is not None
            or parsed.path not in ("", "/") or parsed.query or parsed.fragment):
        raise ProbeError("unsupported_proxy_configuration")
    return value


def response_summary(status: int, headers, body: bytes) -> dict:
    result = {
        "http_status": status,
        "sdp_answer": False,
        "classification": "http_error",
        "response_bytes": len(body),
    }
    if status in (200, 201) and body.startswith(b"v=0"):
        result.update(classification="handshake_accepted", sdp_answer=True)
    elif status in (301, 302, 303, 307, 308):
        result["classification"] = "redirect_refused"
    elif status == 401:
        result["classification"] = "account_authentication_rejected"
    elif status == 403:
        result["classification"] = "account_or_edge_access_denied"
    elif status == 429:
        result["classification"] = "rate_or_usage_limit"
    elif status in (400, 404, 422):
        result["classification"] = "route_model_or_protocol_rejected"
    elif status >= 500:
        result["classification"] = "service_unavailable"
    elif status in (200, 201):
        result["classification"] = "unexpected_response_format"
    result["edge_challenge"] = headers.get("cf-mitigated", "").lower() == "challenge"
    # Do not echo upstream messages, account identifiers, Location, or headers.
    # Restrict service diagnostics to allowlisted codes, never arbitrary strings.
    allowed_codes = {"invalid_api_key", "invalid_auth", "unauthorized", "forbidden",
                     "model_not_found", "invalid_request_error", "rate_limit_exceeded",
                     "insufficient_quota", "unsupported_model", "access_denied"}
    try:
        error = json.loads(body).get("error")
        if isinstance(error, dict):
            for key in ("code", "type"):
                if error.get(key) in allowed_codes:
                    result["service_error_" + key] = error[key]
    except (ValueError, AttributeError, TypeError):
        pass
    return result


def call_reference(location: str) -> str | None:
    """Extract only the opaque identifier; never follow a Location URL."""
    parts = urllib.parse.urlsplit(location)
    if parts.scheme and (parts.scheme != "https" or parts.netloc not in ("chatgpt.com", "api.openai.com")):
        return None
    if parts.netloc and not parts.scheme:
        return None
    for segment in reversed(parts.path.split("/")):
        if re.fullmatch(r"rtc_[A-Za-z0-9_-]{1,180}", segment):
            return segment
        if re.fullmatch(r"[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}", segment):
            return segment
    return None


def connect(offer: Path, output: Path, auth: Path | None = None) -> dict:
    # Reserve a new output directory before reading credentials or networking.
    output.mkdir(mode=0o700, parents=False, exist_ok=False)
    report = {
        "schema_version": 1, "endpoint": ENDPOINT,
        "upstream_commit": UPSTREAM_COMMIT, "auth": "existing_chatgpt_file_login",
        "api_key_used": False, "microphone_requested": False, "audio_uploaded": False,
        "request_count": 0, "retries": 0, "redirects_followed": 0,
        "transcription_verified": False,
    }
    started = time.monotonic()
    try:
        sdp = read_capped(offer, MAX_SDP_BYTES).decode("utf-8")
        if not sdp.startswith("v=0") or "m=audio " not in sdp or "m=application " not in sdp:
            raise ProbeError("invalid_webrtc_offer")
        proxy = proxy_url()
        token, account = credentials(auth if auth is not None else auth_path())
        session_id = str(uuid.uuid4())
        request = urllib.request.Request(
            ENDPOINT, method="POST",
            data=json.dumps({"sdp": sdp, "session": SESSION}).encode(),
            headers={"Authorization": "Bearer " + token, "ChatGPT-Account-Id": account,
                     "Content-Type": "application/json", "Accept": "application/sdp",
                     "User-Agent": "VoicePort-Realtime-Probe/0.1", "originator": "voiceport_realtime_probe",
                     "openai-alpha": "quicksilver=v2", "x-session-id": session_id},
        )
        handlers = [NoRedirect(), urllib.request.ProxyHandler({"https": proxy} if proxy else {}),
                    urllib.request.HTTPSHandler(context=ssl.create_default_context())]
        opener = urllib.request.build_opener(*handlers)
        report["request_count"] = 1
        try:
            response = opener.open(request, timeout=20)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            body = response.read(MAX_RESPONSE_BYTES + 1)
            if len(body) > MAX_RESPONSE_BYTES:
                raise ProbeError("response_too_large")
            report.update(response_summary(response.code, response.headers, body))
            reference = call_reference(response.headers.get("Location", "")) if report["sdp_answer"] else None
        if reference is not None:
            # Private join reference, never copied into stdout or report.json.
            dest = output / "call-reference.json"
            with dest.open("x") as stream:
                os.chmod(dest, 0o600)
                json.dump({"call_id": reference, "session_id": session_id}, stream)
        report["call_reference_saved"] = reference is not None
        if report["sdp_answer"]:
            dest = output / "answer.sdp"
            with dest.open("xb") as stream:
                os.chmod(dest, 0o600)
                stream.write(body)
    except ProbeError as error:
        report["classification"] = str(error)
    except (OSError, ValueError, urllib.error.URLError, TimeoutError):
        # Transport exception strings may contain request data: never print them.
        report["classification"] = "transport_or_input_error"
    report["elapsed_seconds"] = round(time.monotonic() - started, 3)
    dest = output / "report.json"
    with dest.open("x") as stream:
        os.chmod(dest, 0o600)
        json.dump(report, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    return report


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--connect", action="store_true", help="Submit one SDP offer with existing ChatGPT login")
    parser.add_argument("--offer", type=Path, help="Own WebRTC audio+data SDP; never a recording")
    parser.add_argument("--output", type=Path, help="New output directory under an existing parent")
    args = parser.parse_args(argv)
    if not args.connect:
        print(json.dumps({"mode": "offline", "endpoint": ENDPOINT, "model": SESSION["model"],
                          "auth_read": False, "network": False, "api_key_used": False,
                          "upstream_commit": UPSTREAM_COMMIT}, ensure_ascii=False))
        return 0
    if args.offer is None or args.output is None:
        parser.error("--connect requires --offer and --output")
    try:
        report = connect(args.offer, args.output)
    except OSError:
        print(json.dumps({"classification": "output_directory_unavailable", "request_count": 0}))
        return 1
    print(json.dumps(report, ensure_ascii=False))
    return 0 if report.get("classification") == "handshake_accepted" else 1


if __name__ == "__main__":
    raise SystemExit(main())
