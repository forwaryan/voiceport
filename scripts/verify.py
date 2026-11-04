#!/usr/bin/env python3
"""Offline release verification. No microphone, credential contents, cloud calls, or paste events."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import sys
import uuid

ROOT = Path(__file__).resolve().parents[1]


def source_fingerprint():
    digest = hashlib.sha256()
    paths = [ROOT / "Package.swift", ROOT / "THIRD_PARTY_NOTICES.md"]
    for folder in ("Sources", "Tests", "resources", "scripts", "automation"):
        paths.extend(path for path in (ROOT / folder).rglob("*")
                     if path.is_file() and "__pycache__" not in path.parts)
    for path in sorted(paths):
        digest.update(str(path.relative_to(ROOT)).encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app-smoke", action="store_true", help="also launch the menu app in no-I/O smoke mode")
    args = parser.parse_args()
    evidence = ROOT / ".voiceport/evidence" / (datetime.now().strftime("%Y%m%d-%H%M%S-") + uuid.uuid4().hex[:6])
    evidence.mkdir(parents=True, mode=0o700)
    cli = ".build/release/voiceport-cli"
    checks = [
        (["bash", "scripts/test_core.sh"], 0, "0 failures"),
        (["bash", "scripts/test_app.sh"], 0, "0 failures"),
        (["python3", "-I", "-m", "unittest", "discover", "-s", "Tests/LoopTests", "-v"], 0, "OK"),
        (["swift", "build", "-c", "release"], 0, None),
        ([cli, "--help"], 0, "voiceport-cli transcribe"),
        ([cli, "demo"], 0, "离线模拟，未调用云端"),
        ([cli, "doctor"], 0, "Codex 登录文件"),
        ([cli, "not-a-command"], 1, "参数无效"),
        ([cli, "transcribe", "--language"], 1, "参数无效"),
        (["bash", "scripts/package_app.sh", "--skip-build"], 0, "Built "),
        (["git", "diff", "--check"], 0, None),
    ]
    if args.app_smoke:
        checks.append((["dist/VoicePort.app/Contents/MacOS/VoicePort", "--smoke-test"], 0, "menu ready"))
    report = {"started_at": datetime.now(timezone.utc).isoformat(), "system": platform.platform(),
              "source_sha256": source_fingerprint(), "checks": [],
              "unverified": ["real microphone", "cloud account and billing", "Orca and other apps receiving text", "Codex recognition parity"]}
    success = True
    for index, (command, expected_code, marker) in enumerate(checks):
        print("Running:", " ".join(command), flush=True)
        try:
            result = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                    timeout=240 if command[:2] == ["swift", "build"] else 120)
            output, code = result.stdout, result.returncode
        except subprocess.TimeoutExpired as error:
            output, code = error.stdout or b"", 124
        decoded = output.decode(errors="replace")
        passed = code == expected_code and (marker is None or marker in decoded)
        logfile = evidence / f"{index + 1:02d}.log"
        logfile.write_bytes(output)
        report["checks"].append({"command": command, "exit_code": code, "expected_exit_code": expected_code,
                                  "required_output": marker, "passed": passed, "log": str(logfile.relative_to(ROOT))})
        print(decoded[-9000:], end="", flush=True)
        if not passed:
            success = False
            break
    report["source_sha256_at_finish"] = source_fingerprint()
    if report["source_sha256"] != report["source_sha256_at_finish"]:
        success = False
        report["failure"] = "Source changed during verification; rerun affected checks."
    report["passed"] = success
    report["finished_at"] = datetime.now(timezone.utc).isoformat()
    report_path = evidence / "report.json"
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print("Evidence:", report_path.relative_to(ROOT), flush=True)
    return 0 if success else 1


if __name__ == "__main__":
    sys.exit(main())
