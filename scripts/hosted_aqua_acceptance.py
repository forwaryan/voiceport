"""Run the fixed VoicePort synthetic focus and offscreen navigation cases on hosted Aqua."""
import argparse
from contextlib import redirect_stderr, redirect_stdout
from hashlib import sha256
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile

SOURCE_CHANGES = {
    ("A", ".github/workflows/voiceport-aqua-once.yml"),
    ("A", "scripts/hosted_aqua_acceptance.py"),
    ("A", "scripts/hosted_aqua_console.m"),
    ("M", "scripts/verify_status_overlay_focus.py"),
    ("M", "crates/voiceport-macos/src/app/status_overlay_fixture.rs"),
    ("M", "crates/voiceport-macos/tests/stop_failure.rs"),
}
STAGES = (
    ("Preparing", False), ("Capture(Opening)", False), ("Capture(Recording)", False),
    ("Capture(Recording)", True), ("Capture(Recovering)", False), ("Capture(Resumed)", False),
    ("Capture(Finalizing)", False), ("Transcribing", False), ("Delivering", False),
)
BASELINE = (
    "status-overlay-focus: positive native baseline Focus { active: true, key_window_present: true, "
    "key_window_is_target: true, target_is_key: true, ax_is_editor: true, responder_is_editor: true, "
    "caret_is_expected: true }"
)
STAGE_LINES = tuple(f"status-overlay-focus: {stage} reduce_motion={str(reduced).lower()} preserved native focus"
                    for stage, reduced in STAGES)
CLOSING = "status-overlay-focus: cancellation and quit preserved key window, AX identity, responder and caret"
SUCCESS = "status-overlay-focus: passed positive native key/AX baseline and all overlay transitions"
WORKBENCH = "workbench_native: native groups, bilingual names, selection, key loop, appearance and production window reuse passed offscreen"
GLOBAL_SKIPPED = "shortcut_native: skipped global registration in isolated UI mode"


class GuestUnavailable(ValueError):
    pass


def require(condition, kind=ValueError):
    if not condition:
        raise kind


def capture(*argv, timeout=10):
    return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                          text=True, timeout=timeout, check=True).stdout.strip()


def read_source(expected_tree, expected_ref):
    require(all(re.fullmatch(r"[a-f0-9]{40}", value) for value in
                (expected_tree, os.environ.get("GITHUB_SHA", ""))))
    require(expected_ref == f"refs/heads/ci/voiceport-aqua-{expected_tree[:12]}"
            and os.environ.get("GITHUB_EVENT_NAME") == "push"
            and os.environ.get("GITHUB_REF") == expected_ref
            and os.environ.get("GITHUB_RUN_ATTEMPT") == "1")
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_bytes())
    require(event.get("created") is True and event.get("deleted") is False and event.get("ref") == expected_ref)
    head, parent = capture("git", "rev-list", "--parents", "-n", "1", "HEAD").split()
    require(head == os.environ["GITHUB_SHA"] and event.get("after") == head
            and capture("git", "rev-parse", f"{parent}^{{tree}}") == expected_tree)
    require(set(tuple(line.split("\t")) for line in capture(
        "git", "diff", "--no-renames", "--name-status", parent, head).splitlines()) == SOURCE_CHANGES)
    require(not capture("git", "status", "--porcelain=v1", "--untracked-files=all"))
    return capture("git", "rev-parse", "HEAD^{tree}")


def guest_boot():
    require(os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
            and os.environ.get("RUNNER_OS") == "macOS" and os.environ.get("RUNNER_ARCH") == "ARM64"
            and sys.platform == "darwin" and platform.machine() == "arm64" and os.geteuid() != 0,
            GuestUnavailable)
    version = capture("/usr/bin/sw_vers", "-productVersion")
    require(re.fullmatch(r"15\.\d+(?:\.\d+)?", version) and
            capture("/usr/sbin/sysctl", "-n", "kern.hv_vmm_present") == "1", GuestUnavailable)
    boot = capture("/usr/sbin/sysctl", "-n", "kern.bootsessionuuid")
    require(re.fullmatch(r"[a-fA-F0-9-]{36}", boot), GuestUnavailable)
    return boot


def same_aqua(console):
    try:
        uid, asid = capture(str(console)).split()
        require(uid.isdecimal() and int(uid) == os.geteuid() and asid.isdecimal(), GuestUnavailable)
        record = capture("/bin/launchctl", "print", f"gui/{uid}")
        contexts = re.findall(r"(?ms)^\s*security context\s*=\s*\{([^{}]*)\}", record)
        require(len(contexts) == 1, GuestUnavailable)
        values = re.findall(r"(?m)^\s*asid\s*=\s*(0[xX][0-9a-fA-F]+|[0-9]+)\s*$", contexts[0])
        require(len(values) == 1, GuestUnavailable)
        token = values[0]
        require(int(token, 16 if token.lower().startswith("0x") else 10) == int(asid), GuestUnavailable)
        return asid
    except (subprocess.SubprocessError, ValueError) as exc:
        raise GuestUnavailable from exc


def focus_sequence(stdout):
    lines = stdout.splitlines()
    require(lines.count(BASELINE) == 1 and lines.count(CLOSING) == 1 and lines.count(SUCCESS) == 1)
    require(re.search(r"(?m)^status-overlay-focus: activation_policy initial=[0-2] current=0 request=(already-regular|accepted)$", stdout))
    require(tuple(line for line in lines if line.endswith("preserved native focus")) == STAGE_LINES)
    ordered = [lines.index(expected) for expected in (BASELINE, *STAGE_LINES, CLOSING, SUCCESS)]
    require(ordered == sorted(ordered))


def clean_exit(check):
    return (check.get("status") == "completed" and check.get("passed") is True
            and check.get("exit_code") == 0 and check.get("cleanup_confirmed") is True
            and check.get("left_descendants") is False and check.get("interrupted") is False
            and check.get("timed_out") is False)


def clean_launch(check):
    return (clean_exit(check) and check.get("app_exit_observed") is True and check.get("opener_exit_code") == 0
            and check.get("raw_wait_status") == 0 and check.get("exit_source") == "Darwin EVFILT_PROC NOTE_EXITSTATUS")


def review_focus(directory):
    receipt = json.loads((directory / "report.json").read_bytes())
    require(receipt.get("status") == "completed" and receipt.get("passed") is True and receipt.get("exit_code") == 0
            and "active_check" not in receipt and not list(directory.glob("bundle.*")))
    checks = receipt["checks"]
    require(len(checks) == 3 and all(clean_exit(check) for check in checks) and clean_launch(checks[-1]))
    logs = {}
    for name, check in zip(("build", "aqua", "focus"), checks):
        for stream in ("stdout", "stderr"):
            path = directory / f"{name}.{stream}.log"
            require(Path(check[stream]) == path and not path.is_symlink())
            data = path.read_bytes()
            require((len(data), sha256(data).hexdigest()) == (check[f"{stream}_bytes"], check[f"{stream}_sha256"]))
            if stream == "stdout" and name != "build":
                logs[name] = data.decode()
    require("status-overlay-focus: launcher Aqua session and fixture bundle verified" in logs["aqua"])
    focus_sequence(logs["focus"])


def self_test():
    text = "\n".join(("status-overlay-focus: activation_policy initial=0 current=0 request=already-regular",
                      BASELINE, *STAGE_LINES, CLOSING, SUCCESS))
    focus_sequence(text)
    for bad in (text.replace("target_is_key: true", "target_is_key: false"),
                text.replace(STAGE_LINES[0], STAGE_LINES[1]), text.replace(CLOSING, ""),
                text.replace("current=0", "current=2")):
        try:
            focus_sequence(bad)
        except ValueError:
            continue
        raise AssertionError("bad focus evidence accepted")
    check = dict(status="completed", passed=True, exit_code=0, cleanup_confirmed=True, left_descendants=False,
                 interrupted=False, timed_out=False, app_exit_observed=True, opener_exit_code=0,
                 raw_wait_status=0, exit_source="Darwin EVFILT_PROC NOTE_EXITSTATUS")
    assert clean_launch(check)
    for key, value in (("raw_wait_status", None), ("opener_exit_code", 1), ("cleanup_confirmed", False)):
        assert not clean_launch(dict(check, **{key: value}))
    print("Hosted receipt controls passed (synthetic only)")
    return 0


def hosted_main():
    root = Path.cwd().resolve()
    require(root == Path(__file__).resolve().parent.parent)
    require(os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", GuestUnavailable)
    private = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
    env = dict(os.environ, CARGO_NET_OFFLINE="true", CARGO_BUILD_JOBS="2", CARGO_INCREMENTAL="0",
               CARGO_PROFILE_TEST_DEBUG="0", CARGO_PROFILE_DEV_DEBUG="0", VOICEPORT_TEST_ISOLATED_UI="1")
    receipt_dir = private / "voiceport-aqua-receipt"
    receipt_dir.mkdir(mode=0o700)
    sys.path.insert(0, str(root / "scripts"))
    from verification_run import full_suite_lock, run_check, termination_signals, write_report

    report = dict(schema=1, result="failure", stage="source", focus="not_run", workbench="not_run")
    rc = 1
    try:
        expected = os.environ["VP_AQUA_SOURCE_TREE"]
        ref = os.environ["VP_AQUA_REF"]
        report["execution_tree"] = read_source(expected, ref)
        report["source_tree"] = expected
        with tempfile.TemporaryDirectory(prefix="voiceport-aqua.", dir=private) as directory:
            scratch = Path(directory)
            report["stage"] = "runner-preflight"
            boot = guest_boot()
            console = scratch / "console"
            report["stage"] = "probe-build"
            capture("/usr/bin/xcrun", "clang", "-fobjc-arc", "-Wall", "-Wextra", "-Werror",
                    "scripts/hosted_aqua_console.m", "-framework", "AppKit", "-framework", "CoreGraphics",
                    "-framework", "Security", "-o", str(console), timeout=30)
            report["stage"] = "aqua-preflight"
            asid = same_aqua(console)
            report["stage"] = "dependencies"
            capture("rustup", "toolchain", "install", "1.95.0", "--profile", "minimal", timeout=180)
            capture("cargo", "+1.95.0", "fetch", "--locked", "--target", "aarch64-apple-darwin", timeout=240)
            report["stage"] = "aqua-recheck"
            require(boot == guest_boot() and asid == same_aqua(console), GuestUnavailable)
            report["stage"] = "focus"
            from verify_status_overlay_focus import main as focus_main

            argv = sys.argv
            try:
                sys.argv = ["verify_status_overlay_focus.py", "--launch-services", "--authorized-isolated-vm", boot, asid]
                with (scratch / "focus.stdout").open("x") as out, (scratch / "focus.stderr").open("x") as err:
                    with redirect_stdout(out), redirect_stderr(err):
                        focus_rc = focus_main()
            finally:
                sys.argv = argv
            report["focus_exit"] = focus_rc
            if focus_rc == 78:
                report["focus"] = "conditions_unmet"
                raise GuestUnavailable
            require(focus_rc == 0)
            focus = Path((scratch / "focus.stdout").read_text().strip()).resolve(strict=True)
            require(focus.parent == root / ".voiceport" / "evidence" and focus.name.startswith("status-overlay-focus."))
            review_focus(focus)
            report.update(focus="pass", key_ax_caret=True, stage_count=len(STAGES),
                          app_exit_observed=True, focus_cleanup=True)
            report["stage"] = "workbench-offscreen"
            env["CARGO_TARGET_DIR"] = str(root / ".voiceport" / "overlay-target")
            with full_suite_lock(root), termination_signals():
                result, _ = run_check(["cargo", "test", "--offline", "--locked", "-p", "voiceport-macos",
                                       "--test", "shortcut_native", "--test", "dictation_page_native"],
                                      scratch / "nav.stdout", scratch / "nav.stderr", cwd=root, env=env,
                                      timeout=360, marker=WORKBENCH)
            report["workbench_exit"] = result["exit_code"]
            require(clean_exit(result) and GLOBAL_SKIPPED in (scratch / "nav.stdout").read_text())
            report["workbench"] = "offscreen_pass"
            report["stage"] = "source-recheck"
            require(read_source(expected, ref) == report["execution_tree"])
            report.update(result="pass", stage="complete")
            rc = 0
    except GuestUnavailable:
        report["result"] = "conditions_unmet"
        rc = 78
    except (OSError, ValueError, subprocess.SubprocessError, KeyError, KeyboardInterrupt):
        pass  # Public logs carry only fixed classifications and observation fields.
    finally:
        write_report(receipt_dir / "receipt.json", report)
        print(json.dumps(report, sort_keys=True))
    return rc


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    try:
        sys.exit(self_test() if args.self_test else hosted_main())
    except GuestUnavailable:
        print("hosted-aqua conditions-unmet stage=runner-identity")
        sys.exit(78)
    except (OSError, ValueError, KeyError):
        print("hosted-aqua failure stage=runner-setup")
        sys.exit(1)
