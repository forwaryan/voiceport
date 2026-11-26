"""Run only synthetic Shell, Files and menu fixtures in a dedicated hosted Aqua guest."""
import argparse
import json
import math
import os
import platform
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

SOURCE_COMMIT = "4975037a7ecc2df7bf1c3b3f6adcd3a9475b7249"
SOURCE_TREE = "75009746671df933edd1d6d7ca21fc7b50ace00b"
REF = "refs/heads/ci/voiceport-native-4975037a7ecc-r2"
OVERLAY_PATHS = ('.github/workflows/voiceport-native-once.yml', 'crates/voiceport-macos/examples/control_product_fixture.rs', 'crates/voiceport-macos/fixtures/hosted_aqua_entry.rs', 'crates/voiceport-macos/src/app.rs', 'crates/voiceport-macos/src/app/control/fixture.rs', 'crates/voiceport-macos/src/app/file_transcription.rs', 'crates/voiceport-macos/src/app/file_transcription_fixture.rs', 'crates/voiceport-macos/src/app/menu_accept_fixture.rs', 'crates/voiceport-macos/src/app/menu_surface.rs', 'crates/voiceport-macos/src/app/update_ui.rs', 'crates/voiceport-macos/src/app/workbench/accept_shell_gui.rs', 'crates/voiceport-macos/src/app/workbench/fixture.rs', 'crates/voiceport-macos/tests/shortcut_native.rs', 'scripts/hosted_native_console.m', 'scripts/hosted_native_menu.swift', 'scripts/hosted_native_routes.py', 'scripts/hosted_native_transport.py')
OVERLAY_PATHS = (*OVERLAY_PATHS, 'crates/voiceport-macos/src/app/control.rs',
                 'crates/voiceport-macos/src/app/dictation_workflows.rs',
                 'crates/voiceport-macos/src/app/menu_source_accept.rs', 'crates/voiceport-macos/src/app/quick_panel.rs',
                 'crates/voiceport-macos/src/app/recent_dictations/workbench.rs',
                 'crates/voiceport-macos/src/app/recent_dictations/workbench/fixture.rs',
                 'crates/voiceport-macos/src/app/status_overlay_fixture.rs',
                 'crates/voiceport-macos/tests/stop_failure.rs')
CONTROL_NAME = "status-overlay-focus-fixture"
CONTROL_BUNDLE_ID = "org.voiceport.status-overlay-focus-fixture"
CONTROL_ATTESTED = "status-overlay-focus: launcher Aqua session and fixture bundle verified"
CONTROL_FIELDS = ("active", "key_window_present", "key_window_is_target", "target_is_key", "ax_is_editor",
                  "responder_is_editor", "caret_is_expected")
CONTROL_BASELINE = "status-overlay-focus: positive native baseline Focus { " + ", ".join(
    f"{field}: true" for field in CONTROL_FIELDS) + " }"
CONTROL_MISSING_BASELINE = "status-overlay-focus: native baseline unavailable after activation request: "
CONTROL_STAGES = tuple(f"status-overlay-focus: {stage} reduce_motion={reduced} preserved native focus"
                       for stage, reduced in (
                           ("Preparing", "false"), ("Capture(Opening)", "false"),
                           ("Capture(Recording)", "false"), ("Capture(Recording)", "true"),
                           ("Capture(Recovering)", "false"), ("Capture(Resumed)", "false"),
                           ("Capture(Finalizing)", "false"), ("Transcribing", "false"),
                           ("Delivering", "false"),
                       ))
CONTROL_CLOSING = "status-overlay-focus: cancellation and quit preserved key window, AX identity, responder and caret"
CONTROL_CONDITIONS = {
    "status-overlay-focus: refused before AppKit (the launcher has no active regular Aqua desktop application)":
        "launcher_regular_desktop",
    "status-overlay-focus: AppKit rejected the regular activation policy": "regular_policy",
    "status-overlay-focus: no NSScreen / WindowServer fixture surface": "window_server_surface",
    "status-overlay-focus: authorized Aqua session is no longer current": "active_aqua_session",
    "status-overlay-focus: AppKit exited before the focus checks completed": "appkit_event_loop",
}
SOURCE_CLEANUP = "source-probe: isolated preference domain removed"
SOURCE_ABORT = "source-probe: abort cleanup complete"
LEASE_ENDED = "native-routes: supervisor lease ended"
SOURCE_PHASES = {
    "attachAuthorization", "firstTick", "identity", "origin", "initialState", "statusItem", "statusHit",
    "visiblePopover", "focusWindow", "localDisclosure", "localFocus", "localClick", "localState",
    "focusRestored", "closePopover", "reopen", "externalClick", "externalModal", "externalDecision",
}
WINDOW_FIELDS = ("active", "frontmost", "visible", "can_key", "is_key", "key_present", "owned_key")
FOCUS_FIELDS = {"start": ("accessory", "active", "frontmost"), "first": WINDOW_FIELDS, "final": WINDOW_FIELDS}
SHELL_MARKER = "shell_gui: seven real mounts, key window, native event dispatch, two appearances and reopen completed"
FILES_MARKER = "PASS native file entries, drop=programmatic-item, key and sheet parent, page reuse and both exports"
MENU_MARKER = "menu-probe: owned surface closed"
# All menu bundles enter the same Debug-only basename guard in app.rs.
MENU_EXECUTABLE = "voiceport-menu-fixture"
UNAVAILABLE = {
    "shell": {
        "shell_gui_environment native AppKit refused Accessory activation policy": "accessory_policy",
        "shell_gui_environment missing positive key-window baseline": "positive_key_window",
        "shell_gui_environment keyboard navigation does not reach all controls": "full_keyboard_access",
    },
    "files": {
        "files_gui_unavailable accessory_policy": "accessory_policy",
        "files_gui_unavailable positive_key_window": "positive_key_window",
    },
}
PAGES = dict(result=1, history=2, files=3, sources=5, device=6, settings=7, updates=9)


class GuestUnavailable(ValueError):
    pass


def require(condition, kind=ValueError):
    if not condition:
        raise kind


def capture(*argv, timeout=10):
    return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                          text=True, timeout=timeout, check=True).stdout.strip()


def read_source():
    head = os.environ["GITHUB_SHA"]
    require(re.fullmatch(r"[a-f0-9]{40}", head) and os.environ.get("GITHUB_REF") == REF
            and os.environ.get("GITHUB_EVENT_NAME") == "push" and os.environ.get("GITHUB_RUN_ATTEMPT") == "1")
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_bytes())
    require(event.get("ref") == REF and event.get("after") == head and event.get("created") is True
            and event.get("deleted") is False and event["repository"]["id"] == 1445329392)
    require(capture("git", "rev-list", "--parents", "-n", "1", "HEAD").split() == [head, SOURCE_COMMIT])
    require(capture("git", "rev-parse", f"{SOURCE_COMMIT}^{{tree}}") == SOURCE_TREE)
    require(set(capture("git", "diff", "--name-only", "--no-renames", SOURCE_COMMIT, head).splitlines()) == set(OVERLAY_PATHS))
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


def rows(text, prefix):
    return [json.loads(line[len(prefix):]) for line in text.splitlines() if line.startswith(prefix)]


def control_proof(text, success):
    lines = text.splitlines()
    policy = [line for line in lines if line.startswith("status-overlay-focus: activation_policy ")]
    require(len(policy) == 1 and re.fullmatch(
        r"status-overlay-focus: activation_policy initial=[0-2] current=0 request=(already-regular|accepted)",
        policy[0]))
    require(tuple(line for line in lines if line.endswith("preserved native focus")) == CONTROL_STAGES)
    expected = (CONTROL_BASELINE, *CONTROL_STAGES, CONTROL_CLOSING, success)
    require(all(lines.count(line) == 1 for line in expected))
    order = [lines.index(line) for line in (policy[0], *expected)]
    require(order == sorted(order))


def control_condition(stderr):
    reasons = [CONTROL_CONDITIONS[line] for line in stderr.splitlines() if line in CONTROL_CONDITIONS]
    reasons.extend("positive_key_ax_caret" for line in stderr.splitlines() if line.startswith(CONTROL_MISSING_BASELINE))
    return reasons[0] if len(reasons) == 1 else "native_bootstrap"


def control_missing_focus(stderr):
    lines = [line.removeprefix(CONTROL_MISSING_BASELINE) for line in stderr.splitlines()
             if line.startswith(CONTROL_MISSING_BASELINE)]
    pattern = r"Focus \{ " + ", ".join(f"{field}: (true|false)" for field in CONTROL_FIELDS) + r" \}"
    if len(lines) == 1 and (match := re.fullmatch(pattern, lines[0])):
        return {name: value == "true" for name, value in zip(CONTROL_FIELDS, match.groups(), strict=True)}
    return None


def shell_proof(text):
    cycles = rows(text, "shell_gui_tab ")
    observations = [row for row in cycles if row["route"] is None]
    expected = {(page, row, theme) for page, row in PAGES.items() for theme in ("NSAppearanceNameAqua", "NSAppearanceNameDarkAqua")}
    require(len(observations) == len(expected) and {(r["page"], r["row"], r["theme"]) for r in observations} == expected)
    require(all(r["key_window"] is True and type(r["window_number"]) is int and r["window_number"] > 0
                and r["tab_steps"] == r["backtab_steps"] == len(r["first_responder_cycle"]) > 1
                and r["page_ax"]["ax_id"] == f"voiceport.workbench.page.{r['page']}"
                and r["page_ax"]["ax_role"] == "AXGroup" and r["page_ax"]["ax_label"] for r in cycles))
    cli = rows(text, "shell_gui_ipc ")
    files = {("app", "open", "file"), ("app", "open", "files"), ("file", "open")}
    matched = [r for r in cli if tuple(r["argv"]) in files]
    require(len(matched) == 3 and {tuple(r["argv"]) for r in matched} == files)
    require(all(r["exit"] == 0 and r["reply"]["data"] == {
        "page": "file", "state": "user_action_required", "next": "choose_audio_file"} for r in matched))
    lines = text.splitlines()
    aliases = {("app", "open", "microphone"): "app-open-microphone", ("microphone", "open"): "microphone-open"}
    matched = [r for r in cli if tuple(r["argv"]) in aliases]
    require(len(matched) == 2 and {tuple(r["argv"]) for r in matched} == set(aliases))
    require(all(r["exit"] == 0 and r["reply"]["status"] == "ok"
                and r["reply"]["data"] == {"page": "microphone", "shown": True} for r in matched))
    device = [row for row in cycles if row["route"] is not None]
    require(len(device) == 4 and {(r["route"], r["page"], r["row"], r["theme"]) for r in device} ==
            {(route, "device", 6, theme) for route in aliases.values()
             for theme in ("NSAppearanceNameAqua", "NSAppearanceNameDarkAqua")})
    require(len({r["window_number"] for r in device}) == 1)
    current_alias = None
    for line in lines:
        if line.startswith("shell_gui_ipc "):
            argv = json.loads(line.removeprefix("shell_gui_ipc "))["argv"]
            current_alias = aliases.get(tuple(argv))
        elif line.startswith("shell_gui_tab "):
            route = json.loads(line.removeprefix("shell_gui_tab "))["route"]
            require(route is None or route == current_alias)
    [reopened] = rows(text, "shell_gui_ipc_reopen ")
    require(reopened["same_window"] is True and reopened["key"] is True and reopened["sidebar_responder"] is True
            and reopened["page"] == "device" and type(reopened["window_number"]) is int and reopened["window_number"] > 0)
    [history] = rows(text, "shell_gui_history_order_out ")
    require(history == dict(opener="native-programmatic", reopen="cli-app-show", key_baseline=True,
                            popover_shown=True, hidden_without_minimize=True, same_key_window=True,
                            body_selection_feedback_cleared_while_hidden_and_reopened=True,
                            original_ids_and_bodies=2, same_id_expandable=True, clipboard_writes=0))
    require(lines.count("shell_gui_ipc_cleanup socket_removed=true") == 1
            and len(rows(text, "shell_gui_reopen ")) == 1 and lines.count(SHELL_MARKER) == 1)
    return {"page_appearance_cycles": len(observations), "same_window_reopened": True, "files_entry_count": 3,
            "device_alias_count": len(matched), "device_alias_appearance_cycles": len(device),
            "history_order_out": history}


def files_proof(text, drop="programmatic-item"):
    require(drop in ("programmatic-item", "finder"))
    expected = [f"files_gui_sheet {kind} {status}" for kind in ("open", "txt", "markdown") for status in ("attached", "cleared")]
    lines = text.splitlines()
    require([line for line in lines if line.startswith("files_gui_sheet ")] == expected)
    marker = FILES_MARKER.replace("drop=programmatic-item", f"drop={drop}")
    require(lines.count("files_gui_key owned_window=true") == 1 and lines.count(marker) == 1
            and lines.count("files_gui_cleanup window_closed=true socket_removed=true") == 1)
    require([line for line in lines if line.startswith("PASS native file entries,")] == [marker])
    sequence = ["files_gui_key owned_window=true", *expected, marker,
                "files_gui_cleanup window_closed=true socket_removed=true"]
    require([lines.index(line) for line in sequence] == sorted(lines.index(line) for line in sequence))
    return {"sheet_count": 3, "drop": drop, "owned_key_window": True}


def source_proof(text, native_text):
    data = [json.loads(line) for line in text.splitlines()]
    require(len(data) == 6 and all(row.get("scene") == "sources" for row in data))
    a, b, external = "fixture-local-a", "fixture-local-b", "fixture-external"
    for row, (prior, selected, focus, writes) in zip(data[:3], [(a, b, "source_row", 2), (b, a, "disclosure", 3),
                                                               (a, b, "disclosure", 4)], strict=True):
        require(row == {"kind": "local_source", "scene": "sources", "from": prior, "to": selected,
                        "entry_focus": focus, "same_popover": True, "collapsed": True, "current_label": True,
                        "native_focus": True, "ax_focus": True, "preference_writes": writes, "delivery": "hid-mouse"})
    require(data[3] == {"kind": "source_reopen", "scene": "sources", "saved_local_b": True})
    for row, (operation, persisted, writes, delivery) in zip(
            data[4:], [("cancel", b, 0, "hid-escape"), ("confirm", external, 1, "hid-mouse")], strict=True):
        require(row == {"kind": "external_source", "scene": "sources", "operation": operation,
                        "stable_external_id": True, "destination": "synthetic-loopback", "focused_modal": True,
                        "popover_hidden_before_modal": True, "unchanged_before_decision": True, "both_closed": True,
                        "persisted": persisted, "decision_writes": writes, "external_calls": 0, "delivery": delivery})
    require(native_text.splitlines().count(SOURCE_CLEANUP) == 1)
    return {"local_switches": 3, "native_and_ax_focus": True, "saved_after_reopen": True,
            "isolated_preference_domain_removed": True, "cancel_writes": 0, "confirm_writes": 1, "external_calls": 0}


def menu_proof(text, scene):
    data = [json.loads(line) for line in text.splitlines()]
    require(all(row.get("scene") == scene for row in data))
    def of_kind(kind):
        return [row for row in data if row.get("kind") == kind]

    require(sorted(r["button"] for r in of_kind("first_click")) == ["left", "right"])
    require(sorted(r["button"] for r in of_kind("summary")) == ["left", "right"] and len(of_kind("click")) == 60)
    metrics = {}
    for button in ("left", "right"):
        samples = [r for r in of_kind("click") if r["button"] == button]
        require([r["iteration"] for r in samples] == list(range(1, 31)))
        times = sorted(r["ms"] for r in samples)
        require(all(math.isfinite(ms) and ms >= 0 for ms in times))
        summary = next(r for r in of_kind("summary") if r["button"] == button)
        p50, p95 = times[14], times[28]
        require((summary["n"], summary["p50_ms"], summary["p95_ms"], summary["over_budget"])
                == (30, p50, p95, p95 > 100))
        require(p95 >= 390 if scene == "stall" else p95 <= 100)
        metrics[button] = {"n": 30, "p50_ms": p50, "p95_ms": p95}
    if scene == "warm":
        hidden, = of_kind("hidden")
        require(hidden["render_calls"] == 0 and hidden["seconds"] >= 60)
    elif scene == "blocked":
        tracking, = of_kind("tracking_update")
        require(tracking["row_stable"] is True and tracking["queries_completed"] == 2)
    else:
        stall, = of_kind("stall_receipts")
        require(stall["count"] == 62)
    if scene != "stall":
        link, = of_kind("updates_deep_link")
        require(link["focused_workbench"] is True and link["button"] == ("left" if scene == "warm" else "right"))
    return metrics


def clean(check):
    return (check.get("cleanup_confirmed") is True and check.get("left_descendants") is False
            and check.get("interrupted") is False and check.get("timed_out") is False)


def closed(check):
    return (check["status"] == "completed" and clean(check) and check["app_exit_observed"] is True
            and check["exit_source"] == "Darwin EVFILT_PROC NOTE_EXITSTATUS" and check["opener_exit_code"] == 0)


def launch_passed(check):
    return closed(check) and check["passed"] is True and check["raw_wait_status"] == check["exit_code"] == 0


def unavailable_reason(case, check, stderr):
    if closed(check) and check["exit_code"] == 78:
        reasons = [UNAVAILABLE[case][line] for line in stderr.splitlines() if line in UNAVAILABLE[case]]
        return reasons[0] if len(reasons) == 1 else "native_bootstrap"
    return None


def focus_diagnostics(stderr):
    observations = {}
    for phase, fields in FOCUS_FIELDS.items():
        pattern = "hosted-focus " + phase + " " + " ".join(f"{field}=(true|false)" for field in fields)
        lines = [line for line in stderr.splitlines() if line.startswith(f"hosted-focus {phase} ")]
        if len(lines) == 1 and (match := re.fullmatch(pattern, lines[0])):
            observations[phase] = {name: value == "true" for name, value in zip(fields, match.groups(), strict=True)}
    return observations


def source_driver_phase(stderr):
    phases = [line.removeprefix("source-driver-phase ") for line in stderr.splitlines()
              if line.startswith("source-driver-phase ")]
    return phases[0] if len(phases) == 1 and phases[0] in SOURCE_PHASES else None


def review_source_exit(report, app, driver, app_text, app_stderr, driver_stderr, first_tick):
    lines = app_text.splitlines()
    driver_exit = driver.get("exit_code")
    lease_ended = LEASE_ENDED in app_stderr.splitlines()
    cleaned = (launch_passed(app) and not lease_ended and lines.count(SOURCE_CLEANUP) == 1
               and lines.count(MENU_MARKER) == 1 and lines.index(SOURCE_CLEANUP) < lines.index(MENU_MARKER))
    aborted_cleanly = (cleaned and lines.count(SOURCE_ABORT) == 1
                       and lines.index(SOURCE_CLEANUP) < lines.index(SOURCE_ABORT) < lines.index(MENU_MARKER))
    report.update(sources="failure", source_app_exit=app["exit_code"], source_driver_exit=driver_exit,
                  source_first_tick=first_tick, source_cleanup=cleaned, source_abort_cleanup=aborted_cleanly,
                  source_lease_ended=lease_ended, source_driver_phase=source_driver_phase(driver_stderr))
    require(cleaned and clean(driver) and driver_exit is not None)
    if driver_exit == 0:
        require(first_tick and SOURCE_ABORT not in lines)
        return "evaluate_proof"
    require(aborted_cleanly)
    return "permissions_unmet" if driver_exit == 77 else "failed_cleanly"


def review_route(report, case, result, text, stderr, proof):
    report[f"{case}_focus"] = focus_diagnostics(stderr)
    if case == "shell":
        prefix = "shell_gui_environment full_keyboard_access="
        values = [line.removeprefix(prefix) for line in text.splitlines() if line.startswith(prefix)]
        if values in (["true"], ["false"]):
            report["shell_full_keyboard_access"] = values == ["true"]
    report[f"{case}_exit"] = result["exit_code"]
    report[case] = "failure"
    reason = unavailable_reason(case, result, stderr)
    if reason:
        report[case] = "conditions_unmet"
        report[f"{case}_condition"] = reason
    elif launch_passed(result):
        try:
            report[f"{case}_evidence"] = proof(text)
        except (ValueError, TypeError, KeyError):
            report[case] = "receipt_failed"
        else:
            report[case] = "pass"


BOOLEAN_DIAGNOSTICS = frozenset({
    "positive_control_cleanup", "shell_full_keyboard_access", "menu_preflight_pass",
    "source_first_tick", "source_cleanup", "source_abort_cleanup", "source_lease_ended",
})
EXIT_DIAGNOSTICS = frozenset({
    "positive_control_attest_exit", "positive_control_exit", "positive_control_opener_exit",
    "source_app_exit", "source_driver_exit", "shell_exit", "files_exit",
})
FOCUS_DIAGNOSTICS = frozenset({"shell_focus", "files_focus"})
DIAGNOSTIC_ENUMS = {
    "positive_control": {"not_run", "build_failed", "failure", "conditions_unmet", "pass"},
    "positive_control_condition": set(CONTROL_CONDITIONS.values()) | {"positive_key_ax_caret", "native_bootstrap"},
    "window_launch": {"foreground_requested"},
    "source_driver_phase": SOURCE_PHASES | {None},
}
RECEIPT_KEYS = {
    "schema", "result", "stage", "source_tree", "execution_tree", "shell", "files", "menu", "sources",
    "external_ax", "voiceover", "finder", "shell_condition", "files_condition", "shell_evidence",
    "files_evidence", "menu_permissions", "menu_scenes", "source_proof", "positive_control_focus",
} | BOOLEAN_DIAGNOSTICS | EXIT_DIAGNOSTICS | FOCUS_DIAGNOSTICS | DIAGNOSTIC_ENUMS.keys()


def validate_diagnostics(report):
    require(report.keys() <= RECEIPT_KEYS)
    for key in report.keys() & BOOLEAN_DIAGNOSTICS:
        require(type(report[key]) is bool)
    for key in report.keys() & EXIT_DIAGNOSTICS:
        value = report[key]
        require(value is None or type(value) is int and -127 <= value <= 255)
    for key in report.keys() & DIAGNOSTIC_ENUMS.keys():
        value = report[key]
        require((value is None or type(value) is str) and value in DIAGNOSTIC_ENUMS[key])
    if "positive_control_focus" in report:
        row = report["positive_control_focus"]
        require(type(row) is dict and row.keys() == set(CONTROL_FIELDS)
                and all(type(value) is bool for value in row.values()))
    for key in report.keys() & FOCUS_DIAGNOSTICS:
        focus = report[key]
        require(type(focus) is dict and focus.keys() <= FOCUS_FIELDS.keys())
        for phase, row in focus.items():
            require(type(row) is dict and row.keys() == set(FOCUS_FIELDS[phase])
                    and all(type(value) is bool for value in row.values()))


def bundle(directory, executable, bin_name, bundle_id, accessory):
    contents = directory / "Fixture.app" / "Contents"
    app = contents / "MacOS" / bin_name
    app.parent.mkdir(parents=True)
    shutil.copy2(executable, app)
    with (contents / "Info.plist").open("xb") as output:
        plistlib.dump(dict(CFBundleExecutable=bin_name, CFBundleIdentifier=bundle_id, CFBundleName=bin_name,
                           CFBundlePackageType="APPL", CFBundleVersion="1", LSUIElement=accessory), output)
    return app.resolve(strict=True)


def artifact(log, target, name, kind):
    records = (json.loads(line) for line in log.read_text().splitlines() if line.startswith("{"))
    matches = [row for row in records if row.get("reason") == "compiler-artifact" and row.get("executable")
               and row["target"]["name"] == name and row["target"]["kind"] == [kind]]
    require(matches and all(row["profile"]["debug_assertions"] is True for row in matches))
    paths = {Path(row["executable"]).resolve(strict=True) for row in matches}
    require(len(paths) == 1)
    executable = paths.pop()
    require(executable.is_relative_to(target.resolve()) and executable.is_file())
    return executable


def hosted_main():
    root = Path.cwd().resolve()
    require(root == Path(__file__).resolve().parent.parent)
    require(os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", GuestUnavailable)
    sys.path.insert(0, str(root / "scripts"))
    from hosted_native_transport import PASS_MARKER as CONTROL_PASS, launch_services_focus
    from verification_run import (
        full_suite_lock,
        run_check,
        termination_signals,
        write_report,
    )

    receipt_dir = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True) / "voiceport-native-receipt"
    receipt_dir.mkdir(mode=0o700)
    report = dict(schema=1, result="failure", stage="source", positive_control="not_run",
                  shell="not_run", files="not_run", menu="not_run", sources="not_run",
                  external_ax="not_run", voiceover="not_run", finder="not_run")
    rc = 1
    try:
        report["execution_tree"] = read_source()
        report["source_tree"] = SOURCE_TREE
        report["stage"] = "guest-preflight"
        boot = guest_boot()
        with tempfile.TemporaryDirectory(prefix="vp-native.", dir="/private/tmp") as temporary:
            scratch = Path(temporary)
            target = scratch / "target"
            console = scratch / "console"
            capture("/usr/bin/xcrun", "clang", "-fobjc-arc", "-Wall", "-Wextra", "-Werror",
                    "scripts/hosted_native_console.m", "-framework", "AppKit", "-framework", "CoreGraphics",
                    "-framework", "Security", "-o", str(console), timeout=30)
            asid = same_aqua(console)
            env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_BUILD_JOBS="2", CARGO_INCREMENTAL="0",
                       CARGO_PROFILE_TEST_DEBUG="0", CARGO_PROFILE_DEV_DEBUG="0")
            app_env = dict(VOICEPORT_TEST_ISOLATED_UI="1", VOICEPORT_SOURCES_PATH=str(scratch / "sources.json"))

            def check(name, argv, timeout):
                return run_check(argv, scratch / f"{name}.stdout", scratch / f"{name}.stderr",
                                 cwd=root, env=env, timeout=timeout)[0]

            def attest():
                require(guest_boot() == boot and same_aqua(console) == asid, GuestUnavailable)

            def record_permissions_missing(log):
                permissions = json.loads(log.read_text())
                require(set(permissions) == {"ax", "event_post"} and all(type(value) is bool for value in permissions.values())
                        and not all(permissions.values()))
                report["menu_permissions"] = permissions
                report["menu"] = "conditions_unmet_ax_or_event_post"
                if report["sources"] == "not_run" or report.get("source_driver_exit") == 77:
                    report["sources"] = "conditions_unmet_ax_or_event_post"

            def launch(binary, case, arguments, variables, marker, timeout, ready=None, *, foreground=False):
                attest()
                directory = scratch / case
                directory.mkdir(mode=0o700)
                result = launch_services_focus(
                    binary, arguments, directory, env, app_env=dict(app_env, **variables),
                    timeout_seconds=timeout, pass_marker=marker, on_ready=ready, foreground=foreground)
                return result, (directory / "focus.stdout.log").read_text(errors="replace")

            def run_positive_control(flags):
                report["stage"] = "positive-control-build"
                compiled = check("control-build", ["cargo", "test", *flags, "-p", "voiceport-macos",
                                                   "--test", "stop_failure", "--no-run"], 600)
                if not compiled["passed"] or not clean(compiled):
                    report["positive_control"] = "build_failed"
                    raise ValueError("positive control build did not finish cleanly")
                executable = artifact(scratch / "control-build.stdout", target, "stop_failure", "test")
                binary = bundle(scratch / "control-bundle", executable, CONTROL_NAME, CONTROL_BUNDLE_ID, False)
                arguments = ["--status-overlay-focus", "--authorized-isolated-vm", boot, asid]
                report.update(stage="positive-control-attest", positive_control="failure")
                attest()
                attested = check("control-attest", [binary, *arguments, "--attest-aqua"], 8)
                report["positive_control_attest_exit"] = attested["exit_code"]
                require(attested["status"] == "completed" and clean(attested))
                if attested["exit_code"] == 78:
                    report.update(positive_control="conditions_unmet", positive_control_condition=control_condition(
                        (scratch / "control-attest.stderr").read_text(errors="replace")))
                    raise GuestUnavailable
                require(attested["passed"] and (scratch / "control-attest.stdout").read_text().splitlines().count(
                    CONTROL_ATTESTED) == 1)

                # Reuse the proven Regular LaunchServices path before the Accessory routes.
                report["stage"] = "positive-control-key"
                result, text = launch(binary, "control", arguments, {}, CONTROL_PASS, 20)
                report.update(positive_control_exit=result["exit_code"],
                              positive_control_opener_exit=result["opener_exit_code"],
                              positive_control_cleanup=clean(result))
                if closed(result) and result["exit_code"] == 78:
                    stderr = (scratch / "control/focus.stderr.log").read_text(errors="replace")
                    report.update(positive_control="conditions_unmet", positive_control_condition=control_condition(stderr))
                    if (focus := control_missing_focus(stderr)) is not None:
                        report["positive_control_focus"] = focus
                    raise GuestUnavailable
                require(launch_passed(result))
                control_proof(text, CONTROL_PASS)
                report.update(positive_control="pass", positive_control_focus=dict.fromkeys(CONTROL_FIELDS, True))

            report["stage"] = "dependencies"
            capture("rustup", "toolchain", "install", "1.95.0", "--profile", "minimal", timeout=180)
            capture("cargo", "+1.95.0", "fetch", "--locked", "--target", "aarch64-apple-darwin", timeout=240)
            attest()
            env["CARGO_NET_OFFLINE"] = "true"
            with full_suite_lock(root), termination_signals():
                flags = ["--offline", "--locked", "-j", "2", "--message-format=json"]
                run_positive_control(flags)

                report["stage"] = "menu-driver-preflight"
                attest()
                driver = scratch / "menu-click"
                compiled = check("swift", ["/usr/bin/xcrun", "swiftc", "-warnings-as-errors",
                                          "scripts/hosted_native_menu.swift", "-o", driver], 60)
                require(clean(compiled))
                menu_ready = False
                if compiled["passed"]:
                    preflight = check("menu-preflight", [driver, "--preflight", boot, asid], 5)
                    require(clean(preflight))
                    menu_ready = preflight["passed"]
                    report["menu_preflight_pass"] = menu_ready
                    report["menu"] = "preflight_pass" if menu_ready else "preflight_failed"
                    if preflight["exit_code"] == 77:
                        record_permissions_missing(scratch / "menu-preflight.stdout")
                else:
                    report["menu"] = "driver_build_failed"

                report["stage"] = "build-shell"
                shell_build = check("shell-build", ["cargo", "test", *flags, "-p", "voiceport-macos",
                                                    "--test", "shortcut_native", "--no-run"], 240)
                require(clean(shell_build))
                report["stage"] = "build-files-cli"
                common = check("files-build", ["cargo", "build", *flags, "-p", "voiceport-macos", "-p", "voiceport-cli",
                                               "--example", "control_product_fixture", "--bin", "voiceport"], 240)
                require(common["passed"] and clean(common))
                cli = artifact(scratch / "files-build.stdout", target, "voiceport", "bin")

                report["stage"] = "shell"
                report["window_launch"] = "foreground_requested"
                if shell_build["passed"]:
                    executable = artifact(scratch / "shell-build.stdout", target, "shortcut_native", "test")
                    binary = bundle(scratch / "shell-bundle", executable, "voiceport-shell-fixture",
                                    "org.voiceport.workbench-focus-fixture", True)
                    socket = scratch / "shell.sock"
                    shell, text = launch(binary, "shell", ["--hosted-shell", boot, asid], {
                        "VOICEPORT_SHELL_ACCEPTANCE": "isolated-gui", "VOICEPORT_SHELL_ACCEPT_FILES_ALIAS": "1",
                        "VOICEPORT_SHELL_ACCEPT_CLI": str(cli), "VOICEPORT_CONTROL_SOCKET": str(socket),
                    }, SHELL_MARKER, 90, foreground=True)
                    require(shell["cleanup_confirmed"] is True and not socket.exists())
                    review_route(report, "shell", shell, text,
                                 (scratch / "shell/focus.stderr.log").read_text(errors="replace"), shell_proof)
                else:
                    report["shell"] = "build_failed"

                report["stage"] = "files"
                executable = artifact(scratch / "files-build.stdout", target, "control_product_fixture", "example")
                binary = bundle(scratch / "files-bundle", executable, "voiceport-files-fixture",
                                "org.voiceport.files-acceptance-fixture", True)
                files_dir = scratch / "file-data"
                files_dir.mkdir(mode=0o700)
                files, text = launch(binary, "files", [str(cli), "--hosted-files", boot, asid], {
                    "VOICEPORT_HOSTED_FILES_DIR": str(files_dir),
                }, FILES_MARKER, 90, foreground=True)
                require(files["cleanup_confirmed"] is True and not (files_dir / "ipc/control.sock").exists())
                review_route(report, "files", files, text,
                             (scratch / "files/focus.stderr.log").read_text(errors="replace"), files_proof)

                if menu_ready:
                    report["stage"] = "menu-build"
                    menu_build = check("menu-build", ["cargo", "build", *flags, "-p", "voiceport-macos",
                                                      "--bin", "voiceport-macos"], 300)
                    require(clean(menu_build))
                    report["menu"] = "build_failed"
                    if menu_build["passed"]:
                        report["stage"] = "menu"
                        executable = artifact(scratch / "menu-build.stdout", target, "voiceport-macos", "bin")
                        binary = bundle(scratch / "menu-bundle", executable, MENU_EXECUTABLE,
                                        "org.voiceport.menu-acceptance-fixture", True)
                        report["menu"] = "failure"
                        report["menu_scenes"] = {}
                        for scene in ("sources", "warm", "blocked", "stall"):
                            scene_binary, variables = binary, {}
                            if scene == "sources":
                                domain = "org.voiceport.source-acceptance-fixture." + uuid.uuid4().hex
                                scene_binary = bundle(scratch / "sources-bundle", executable,
                                                      MENU_EXECUTABLE, domain, True)
                                variables = {"VOICEPORT_NATIVE_SOURCE_DOMAIN": domain}
                            report["stage"] = "source-switch" if scene == "sources" else f"menu-{scene}"
                            data = scratch / f"menu-data-{scene}"
                            data.mkdir(mode=0o700)
                            driver_result = {}

                            def ready(pid, deadline, scene=scene, scene_binary=scene_binary,
                                      data=data, driver_result=driver_result):
                                limit = min(120, deadline - time.monotonic() - 5)
                                require(limit > 0)
                                app_log = scratch / f"menu-{scene}/focus.stdout.log"
                                result = check(scene, [driver, "--attach", str(pid), scene_binary, data, app_log,
                                                       boot, asid, scene], limit)
                                driver_result.update(result)
                                if result["passed"] and clean(result):
                                    (data / "finish").touch(exist_ok=False)
                                    return
                                if scene == "sources" and clean(result):
                                    # Keep the lease open while the owned AppKit process closes its surfaces
                                    # and deletes the synthetic preferences, even after a driver failure.
                                    (data / "abort").touch(exist_ok=False)
                                    return
                                raise ValueError("menu driver did not finish cleanly")

                            result, native_text = launch(scene_binary, f"menu-{scene}",
                                                         ["--private-menu-accept", str(data), scene, boot, asid],
                                                         variables, MENU_MARKER, 60 if scene == "sources" else 130, ready)
                            if scene == "sources":
                                driver_log = scratch / "sources.stderr"
                                driver_stderr = driver_log.read_text(errors="replace") if driver_log.is_file() else ""
                                source_outcome = review_source_exit(
                                    report, result, driver_result, native_text,
                                    (scratch / "menu-sources/focus.stderr.log").read_text(errors="replace"),
                                    driver_stderr, (data / "ready").is_file(),
                                )
                                if source_outcome == "failed_cleanly":
                                    continue
                            else:
                                require(result["cleanup_confirmed"] is True and clean(driver_result))
                            if driver_result["exit_code"] == 77:
                                record_permissions_missing(scratch / f"{scene}.stdout")
                                break
                            require(launch_passed(result))
                            driver_text = (scratch / f"{scene}.stdout").read_text()
                            if scene == "sources":
                                try:
                                    report["source_proof"] = source_proof(driver_text, native_text)
                                except (ValueError, TypeError, KeyError):
                                    report["sources"] = "receipt_failed"
                                else:
                                    report["sources"] = "pass"
                            else:
                                report["menu_scenes"][scene] = menu_proof(driver_text, scene)
                        else:
                            report["menu"] = "pass"
            report["stage"] = "source-recheck"
            require(read_source() == report["execution_tree"])
            report["stage"] = "complete"
            outcomes = [report[key] for key in ("positive_control", "shell", "files", "menu", "sources")]
            if outcomes == ["pass"] * len(outcomes):
                report["result"], rc = "pass", 0
            elif all(value == "pass" or value.startswith("conditions_unmet") for value in outcomes):
                report["result"], rc = "conditions_unmet", 78
    except GuestUnavailable:
        report["result"], rc = "conditions_unmet", 78
    except (OSError, ValueError, TypeError, KeyError, subprocess.SubprocessError, KeyboardInterrupt):
        pass
    finally:
        try:
            validate_diagnostics(report)
        except ValueError:
            report, rc = dict(schema=1, result="failure", stage="receipt-whitelist"), 1
        write_report(receipt_dir / "receipt.json", report)
        print(json.dumps(report, sort_keys=True))
    return rc


if __name__ == "__main__":
    argparse.ArgumentParser(description=__doc__).parse_args()
    try:
        sys.exit(hosted_main())
    except GuestUnavailable:
        print("hosted-native-routes conditions_unmet stage=runner-identity")
        sys.exit(78)
    except (OSError, ValueError, KeyError):
        print("hosted-native-routes failure stage=runner-setup")
        sys.exit(1)
