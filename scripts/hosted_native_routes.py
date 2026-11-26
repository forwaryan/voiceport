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
REF = "refs/heads/ci/voiceport-native-4975037a7ecc-r3"
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
MENU_ABORT = "menu-probe: abort cleanup complete"
MENU_SCENES = ("warm", "blocked", "stall")
MENU_PHASES = {"attachAuthorization", "ready", "identity", "statusItem", "statusHit", "visiblePopover",
               "visibleContextMenu", "menuAction", "hidden", "updatesLink", "budget"}
MENU_EVENTS = {"left_up", "right_up", "other", "none"}
MENU_AX_FIELDS = ("frontmost", "disclosure", "center", "owned_window_at_point", "exact_hit", "enabled")
MENU_NATIVE_FIELDS = ("sampled", "shown", "visible_window", "app_active")
MENU_ATTEMPT_STATES = {"reached", "cleanup_failed", "conditions_unmet", "driver_failed", "receipt_failed", "pass"}
MENU_ATTEMPT_BOOLEANS = {"ready", "driver_clean", "driver_timed_out", "app_exit_observed", "transport_cleanup",
                         "graceful_cleanup", "abort_cleanup", "lease_ended", "app_timed_out"}
MENU_ATTEMPT_EXITS = {"driver_exit", "app_exit", "opener_exit"}
APP_STATES = {"completed", "launch_error", "timeout", "interrupted"}
# The workflow is limited to 45 minutes. Leave time for checkout, receipt upload and teardown.
ROUTE_BUDGET_SECONDS = 42 * 60
CLEANUP_RESERVE_SECONDS = 12
PRODUCT_OUTCOMES = ("positive_control", "shell", "files", "history", "device", "menu", "sources")
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
WINDOW_CASES = ("shell", "files", "history", "device", "shell_manual", "files_manual")
SHELL_INITIAL_MARKER = "shell_gui_initial_key passed=true"
FILES_INITIAL_MARKER = "files_gui_key owned_window=true"
HISTORY_MARKER = "shell_gui_ipc_cleanup socket_removed=true"
DEVICE_MARKER = "shell_gui_device_complete deep_links_and_tab=true"
SIDEBAR_MARKER = "shell_gui_sidebar seven_pages_both_directions_two_appearances=true"
FILES_CLEANUP = "files_gui_cleanup window_closed=true socket_removed=true"
LIFECYCLE_FIELDS = ("running", "launched", "activated", "target_became_key")
PAGES = dict(result=1, history=2, files=3, sources=5, device=6, settings=7, updates=9)


class GuestUnavailable(ValueError):
    pass


def require(condition, kind=ValueError):
    if not condition:
        raise kind


def capture(*argv, timeout=10):
    return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                          text=True, timeout=timeout, check=True).stdout.strip()


class BudgetExhausted(RuntimeError):
    pass


def remaining_time(deadline, timeout, *, require_full=False):
    remaining = deadline - time.monotonic() - CLEANUP_RESERVE_SECONDS
    if remaining <= 0 or (require_full and remaining < timeout):
        raise BudgetExhausted
    return min(timeout, remaining)


def read_source(capture_command=capture):
    head = os.environ["GITHUB_SHA"]
    require(re.fullmatch(r"[a-f0-9]{40}", head) and os.environ.get("GITHUB_REF") == REF
            and os.environ.get("GITHUB_EVENT_NAME") == "push" and os.environ.get("GITHUB_RUN_ATTEMPT") == "1")
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_bytes())
    require(event.get("ref") == REF and event.get("after") == head and event.get("created") is True
            and event.get("deleted") is False and event["repository"]["id"] == 1445329392)
    require(capture_command("git", "rev-list", "--parents", "-n", "1", "HEAD").split() == [head, SOURCE_COMMIT])
    require(capture_command("git", "rev-parse", f"{SOURCE_COMMIT}^{{tree}}") == SOURCE_TREE)
    require(set(capture_command("git", "diff", "--name-only", "--no-renames", SOURCE_COMMIT, head).splitlines()) == set(OVERLAY_PATHS))
    require(not capture_command("git", "status", "--porcelain=v1", "--untracked-files=all"))
    return capture_command("git", "rev-parse", "HEAD^{tree}")


def guest_boot(capture_command=capture):
    require(os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
            and os.environ.get("RUNNER_OS") == "macOS" and os.environ.get("RUNNER_ARCH") == "ARM64"
            and sys.platform == "darwin" and platform.machine() == "arm64" and os.geteuid() != 0,
            GuestUnavailable)
    version = capture_command("/usr/bin/sw_vers", "-productVersion")
    require(re.fullmatch(r"15\.\d+(?:\.\d+)?", version) and
            capture_command("/usr/sbin/sysctl", "-n", "kern.hv_vmm_present") == "1", GuestUnavailable)
    boot = capture_command("/usr/sbin/sysctl", "-n", "kern.bootsessionuuid")
    require(re.fullmatch(r"[a-fA-F0-9-]{36}", boot), GuestUnavailable)
    return boot


def same_aqua(console, capture_command=capture):
    try:
        uid, asid = capture_command(str(console)).split()
        require(uid.isdecimal() and int(uid) == os.geteuid() and asid.isdecimal(), GuestUnavailable)
        record = capture_command("/bin/launchctl", "print", f"gui/{uid}")
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


def tab_cycles(cycles, expected):
    require(len(cycles) == len(expected) and {(r["page"], r["row"], r["theme"], r["route"]) for r in cycles} == expected)
    require(all(r["key_window"] is True and type(r["window_number"]) is int and r["window_number"] > 0
                and r["tab_steps"] == r["backtab_steps"] == len(r["first_responder_cycle"]) > 1
                and r["page_ax"]["ax_id"] == f"voiceport.workbench.page.{r['page']}"
                and r["page_ax"]["ax_role"] == "AXGroup" and r["page_ax"]["ax_label"] for r in cycles))


def shell_sidebar_proof(text):
    cli = rows(text, "shell_gui_ipc ")
    aliases = {("app", "open", "file"), ("app", "open", "files"), ("file", "open")}
    matched = [row for row in cli if tuple(row["argv"]) in aliases]
    require(len(matched) == 3 and {tuple(row["argv"]) for row in matched} == aliases)
    require(all(r["exit"] == 0 and r["reply"]["data"] == {
        "page": "file", "state": "user_action_required", "next": "choose_audio_file"} for r in matched))
    require(text.splitlines().count(SIDEBAR_MARKER) == 1)
    [reopen] = rows(text, "shell_gui_reopen ")
    require(reopen["page"] == "sources" and type(reopen["window_number"]) is int and reopen["window_number"] > 0
            and (reopen["sidebar_responder"] is True or reopen["previous_page_responder"] is True))
    return {"seven_sidebar_pages_two_appearances": True, "files_entry_count": 3, "same_window_reopened": True}


def shell_proof(text):
    uses = shell_sidebar_proof(text)
    cycles = rows(text, "shell_gui_tab ")
    themes = ("NSAppearanceNameAqua", "NSAppearanceNameDarkAqua")
    tab_cycles(cycles, {(page, row, theme, None) for page, row in PAGES.items() for theme in themes})
    require(text.splitlines().count(SHELL_MARKER) == 1)
    return dict(uses, page_appearance_cycles=len(cycles))


def device_route_proof(text):
    aliases = {("app", "open", "microphone"): "app-open-microphone", ("microphone", "open"): "microphone-open"}
    cli = rows(text, "shell_gui_ipc ")
    matched = [r for r in cli if tuple(r["argv"]) in aliases]
    require([tuple(r["argv"]) for r in matched] == list(aliases))
    require(all(r["exit"] == 0 and r["reply"]["status"] == "ok"
                and r["reply"]["data"] == {"page": "microphone", "shown": True} for r in matched))
    lines = text.splitlines()
    markers = [f"shell_gui_device_route {route} owned_window=true sidebar_responder=true" for route in aliases.values()]
    require([line for line in lines if line.startswith("shell_gui_device_route ")] == markers)
    [reopened] = rows(text, "shell_gui_ipc_reopen ")
    require(reopened["same_window"] is True and reopened["key"] is True and reopened["sidebar_responder"] is True
            and reopened["page"] == "device" and type(reopened["window_number"]) is int and reopened["window_number"] > 0)
    return {"device_alias_count": len(matched), "same_key_window_reopened": True}


def device_proof(text):
    uses = device_route_proof(text)
    aliases = {("app", "open", "microphone"): "app-open-microphone", ("microphone", "open"): "microphone-open"}
    cycles = rows(text, "shell_gui_tab ")
    themes = ("NSAppearanceNameAqua", "NSAppearanceNameDarkAqua")
    tab_cycles(cycles, {("device", 6, theme, route) for route in aliases.values() for theme in themes})
    [reopened] = rows(text, "shell_gui_ipc_reopen ")
    require({row["window_number"] for row in cycles} == {reopened["window_number"]})
    current = None
    for line in text.splitlines():
        if line.startswith("shell_gui_ipc "):
            current = aliases.get(tuple(json.loads(line.removeprefix("shell_gui_ipc "))["argv"]))
        elif line.startswith("shell_gui_tab "):
            require(json.loads(line.removeprefix("shell_gui_tab "))["route"] == current)
    require(text.splitlines().count(DEVICE_MARKER) == 1)
    return dict(uses, device_alias_appearance_cycles=len(cycles))


def history_proof(text):
    [history] = rows(text, "shell_gui_history_order_out ")
    require(history == dict(opener="native-programmatic", reopen="cli-app-show", key_baseline=True,
                            popover_shown=True, hidden_without_minimize=True, same_key_window=True,
                            body_selection_feedback_cleared_while_hidden_and_reopened=True,
                            original_ids_and_bodies=2, same_id_expandable=True, clipboard_writes=0))
    return history


def initial_proof(text, marker):
    require(text.splitlines().count(marker) == 1)
    return {"owned_key_window": True}


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
    family = "files" if case.startswith("files") else "shell"
    if not closed(check) or check["exit_code"] != 78 or check["raw_wait_status"] != 78 << 8:
        return None
    lines = stderr.splitlines()
    reasons = [UNAVAILABLE[family][line] for line in lines if line in UNAVAILABLE[family]]
    if len(reasons) == 1:
        return reasons[0]
    if not reasons and sum(line.startswith("native-routes: refused before AppKit (") for line in lines) == 1:
        return "native_bootstrap"
    return None


def focus_diagnostics(stderr):
    observations = {}
    for phase, fields in FOCUS_FIELDS.items():
        pattern = "hosted-focus " + phase + " " + " ".join(f"{field}=(true|false)" for field in fields)
        lines = [line for line in stderr.splitlines() if line.startswith(f"hosted-focus {phase} ")]
        if len(lines) == 1 and (match := re.fullmatch(pattern, lines[0])):
            observations[phase] = {name: value == "true" for name, value in zip(fields, match.groups(), strict=True)}
    return observations


def enum_diagnostic(text, prefix, values):
    rows = [line.removeprefix(prefix) for line in text.splitlines() if line.startswith(prefix)]
    return rows[0] if len(rows) == 1 and rows[0] in values else None


def source_driver_phase(stderr):
    return enum_diagnostic(stderr, "source-driver-phase ", SOURCE_PHASES)


def bool_diagnostic(text, prefix, fields):
    rows = [line for line in text.splitlines() if line.startswith(prefix)]
    pattern = prefix + " ".join(f"{field}=(true|false)" for field in fields)
    if len(rows) == 1 and (match := re.fullmatch(pattern, rows[0])):
        return {field: value == "true" for field, value in zip(fields, match.groups(), strict=True)}
    return None


def menu_cleanup(scene, app, app_text, app_stderr):
    lease_ended = LEASE_ENDED in app_stderr.splitlines()
    lines = app_text.splitlines()
    graceful = launch_passed(app) and not lease_ended and lines.count(MENU_MARKER) == 1
    abort_marker = SOURCE_ABORT if scene == "sources" else MENU_ABORT
    if scene == "sources":
        graceful = graceful and lines.count(SOURCE_CLEANUP) == 1 and lines.index(SOURCE_CLEANUP) < lines.index(MENU_MARKER)
    aborted = graceful and lines.count(abort_marker) == 1 and lines.index(abort_marker) < lines.index(MENU_MARKER)
    if scene == "sources":
        aborted = aborted and lines.index(SOURCE_CLEANUP) < lines.index(SOURCE_ABORT)
    graceful = graceful and (abort_marker not in lines or aborted)
    return graceful, aborted, lease_ended


def menu_attempt(scene, app, driver, app_text, app_stderr, driver_stderr, ready):
    graceful, aborted, lease_ended = menu_cleanup(scene, app, app_text, app_stderr)
    return dict(outcome="cleanup_failed", ready=ready, app_status=app["status"],
                app_exit=app["exit_code"], app_exit_observed=app["app_exit_observed"],
                opener_exit=app["opener_exit_code"], transport_cleanup=app["cleanup_confirmed"],
                graceful_cleanup=graceful, abort_cleanup=aborted, lease_ended=lease_ended,
                driver_exit=driver.get("exit_code"), driver_clean=clean(driver),
                app_timed_out=app.get("timed_out") is True,
                driver_timed_out=driver.get("timed_out") is True,
                driver_phase=(source_driver_phase(driver_stderr) if scene == "sources" else
                              enum_diagnostic(driver_stderr, "menu-driver-phase ", MENU_PHASES)),
                first_event=enum_diagnostic(app_stderr, "menu-native-first-event ", MENU_EVENTS),
                native_surface=bool_diagnostic(app_stderr, "menu-native-first-surface ", MENU_NATIVE_FIELDS),
                ax_first_left=bool_diagnostic(driver_stderr, "menu-ax-first-left ", MENU_AX_FIELDS))


def menu_driver_outcome(attempt):
    require(attempt["graceful_cleanup"] and attempt["driver_clean"])
    code = attempt["driver_exit"]
    require(code is not None and attempt["abort_cleanup"] == (code != 0))
    if code == 0:
        return "evaluate_proof"
    return "conditions_unmet" if code == 77 else "driver_failed"


def review_source_exit(report, app, driver, app_text, app_stderr, driver_stderr, first_tick):
    lines = app_text.splitlines()
    driver_exit = driver.get("exit_code")
    cleaned, aborted_cleanly, lease_ended = menu_cleanup("sources", app, app_text, app_stderr)
    report.update(sources="failure", source_app_exit=app["exit_code"], source_driver_exit=driver_exit,
                  source_first_tick=first_tick, source_cleanup=cleaned, source_abort_cleanup=aborted_cleanly,
                  source_lease_ended=lease_ended, source_driver_phase=source_driver_phase(driver_stderr))
    require(cleaned and clean(driver) and driver_exit is not None)
    if driver_exit == 0:
        require(first_tick and SOURCE_ABORT not in lines)
        return "evaluate_proof"
    require(aborted_cleanly)
    return "permissions_unmet" if driver_exit == 77 else "failed_cleanly"


def lifecycle_diagnostics(stderr):
    observations = {}
    for phase in FOCUS_FIELDS:
        prefix = f"hosted-lifecycle {phase} "
        pattern = prefix + " ".join(f"{field}=(true|false)" for field in LIFECYCLE_FIELDS)
        lines = [line for line in stderr.splitlines() if line.startswith(prefix)]
        if len(lines) == 1 and (match := re.fullmatch(pattern, lines[0])):
            observations[phase] = {name: value == "true" for name, value in zip(LIFECYCLE_FIELDS, match.groups(), strict=True)}
    return observations


def review_window_attempt(report, case, result, text, stderr, proof, socket_removed):
    report.setdefault("window_attempts", {})[case] = dict(
        status=result["status"], app_exit=result["exit_code"], opener_exit=result["opener_exit_code"],
        app_exit_observed=result["app_exit_observed"], timed_out=result["timed_out"],
        transport_cleanup=result["cleanup_confirmed"], socket_removed=socket_removed,
    )
    if not result["cleanup_confirmed"] or not socket_removed:
        report[case] = "failure"
        report[f"{case}_exit"] = result["exit_code"]
        raise ValueError("window cleanup not confirmed")
    review_route(report, case, result, text, stderr, proof)


def review_route(report, case, result, text, stderr, proof):
    focus = focus_diagnostics(stderr)
    life = lifecycle_diagnostics(stderr)
    report[f"{case}_focus"] = focus
    report[f"{case}_lifecycle"] = life
    report[f"{case}_exit"] = result["exit_code"]
    report[case] = "failure"
    if case in ("shell", "device"):
        prefix = "shell_gui_environment full_keyboard_access="
        values = [line.removeprefix(prefix) for line in text.splitlines() if line.startswith(prefix)]
        if values in (["true"], ["false"]):
            report[f"{case}_full_keyboard_access"] = values == ["true"]
    reason = unavailable_reason(case, result, stderr)
    if not reason and not launch_passed(result):
        return
    if reason in ("accessory_policy", "native_bootstrap"):
        report[case] = "conditions_unmet"
        report[f"{case}_condition"] = reason
        return
    target = focus.get("final", {})
    positive_key = all(target.get(field) is True for field in ("active", "visible", "is_key", "owned_key"))
    expected_running = not case.endswith("_manual")
    cleanup = FILES_CLEANUP if case.startswith("files") else HISTORY_MARKER
    try:
        lines = text.splitlines()
        first_cli = "hosted-first-cli accepted=true mounted=true"
        require(lines.count(first_cli) == lines.count(cleanup) == 1 and lines.index(first_cli) < lines.index(cleanup))
        require(set(focus) == set(life) == set(FOCUS_FIELDS))
        require(focus["start"]["accessory"] and all(row["running"] == expected_running for row in life.values()))
        for field in ("launched", "activated", "target_became_key"):
            require([life[phase][field] for phase in FOCUS_FIELDS] == sorted(life[phase][field] for phase in FOCUS_FIELDS))
        if expected_running:
            require(life["start"]["launched"])
        report[f"{case}_first_cli"] = True
        if reason == "positive_key_window":
            require(not positive_key)
        else:
            require(positive_key and target["key_present"])
        if reason == "full_keyboard_access":
            require(case in ("shell", "device") and report.get(f"{case}_full_keyboard_access") is False
                    and not rows(text, "shell_gui_tab "))
            partial = shell_sidebar_proof if case == "shell" else device_route_proof
            report[f"{case}_evidence"] = partial(text)
        elif not reason:
            if case in ("shell", "device"):
                require(report.get(f"{case}_full_keyboard_access") is True)
            report[f"{case}_evidence"] = proof(text)
    except (ValueError, TypeError, KeyError):
        report[case] = "receipt_failed"
    else:
        report[case] = "conditions_unmet" if reason else "pass"
        if reason:
            report[f"{case}_condition"] = reason


BOOLEAN_DIAGNOSTICS = frozenset({
    "positive_control_cleanup", "budget_exhausted", "shell_full_keyboard_access", "device_full_keyboard_access", "menu_preflight_pass",
    "source_first_tick", "source_cleanup", "source_abort_cleanup", "source_lease_ended",
    *(f"{case}_first_cli" for case in WINDOW_CASES),
})
EXIT_DIAGNOSTICS = frozenset({
    "positive_control_attest_exit", "positive_control_exit", "positive_control_opener_exit",
    "source_app_exit", "source_driver_exit", *(f"{case}_exit" for case in WINDOW_CASES),
})
FOCUS_DIAGNOSTICS = frozenset(f"{case}_focus" for case in WINDOW_CASES)
LIFECYCLE_DIAGNOSTICS = frozenset(f"{case}_lifecycle" for case in WINDOW_CASES)
DIAGNOSTIC_ENUMS = {
    "positive_control": {"not_run", "build_failed", "failure", "conditions_unmet", "pass"},
    "positive_control_condition": set(CONTROL_CONDITIONS.values()) | {"positive_key_ax_caret", "native_bootstrap"},
    "window_launch": {"foreground_requested"},
    "source_driver_phase": SOURCE_PHASES | {None},
    **{case: {"not_run", "build_failed", "failure", "receipt_failed", "conditions_unmet", "pass"} for case in WINDOW_CASES},
    **{f"{case}_condition": {"accessory_policy", "native_bootstrap", "positive_key_window", "full_keyboard_access"}
       for case in WINDOW_CASES},
}
RECEIPT_KEYS = {
    "schema", "result", "stage", "source_tree", "execution_tree", "menu", "sources",
    "external_ax", "voiceover", "finder", "menu_permissions", "menu_scenes", "source_proof", "positive_control_focus",
    "menu_attempts", "window_attempts", *(f"{case}_evidence" for case in WINDOW_CASES),
} | BOOLEAN_DIAGNOSTICS | EXIT_DIAGNOSTICS | FOCUS_DIAGNOSTICS | LIFECYCLE_DIAGNOSTICS | DIAGNOSTIC_ENUMS.keys()


def validate_menu_attempts(attempts):
    require(type(attempts) is dict and attempts.keys() <= {"sources", *MENU_SCENES})
    for scene, row in attempts.items():
        require(type(row) is dict and type(row.get("outcome")) is str and row["outcome"] in MENU_ATTEMPT_STATES)
        if row["outcome"] == "reached":
            require(row.keys() == {"outcome"})
            continue
        enums = {"app_status": APP_STATES, "first_event": MENU_EVENTS,
                 "driver_phase": SOURCE_PHASES if scene == "sources" else MENU_PHASES}
        vectors = {"native_surface": MENU_NATIVE_FIELDS, "ax_first_left": MENU_AX_FIELDS}
        require(row.keys() == {"outcome"} | MENU_ATTEMPT_BOOLEANS | MENU_ATTEMPT_EXITS | enums.keys() | vectors.keys())
        for key in MENU_ATTEMPT_BOOLEANS:
            require(type(row[key]) is bool)
        for key in MENU_ATTEMPT_EXITS:
            require(row[key] is None or type(row[key]) is int and -127 <= row[key] <= 255)
        for key, allowed in enums.items():
            require((key != "app_status" and row[key] is None) or type(row[key]) is str and row[key] in allowed)
        for key, fields in vectors.items():
            value = row[key]
            require(value is None or type(value) is dict and value.keys() == set(fields)
                    and all(type(item) is bool for item in value.values()))


def validate_window_attempts(attempts):
    require(type(attempts) is dict and attempts.keys() <= set(WINDOW_CASES))
    flags = {"app_exit_observed", "timed_out", "transport_cleanup", "socket_removed"}
    exits = {"app_exit", "opener_exit"}
    for row in attempts.values():
        require(type(row) is dict and row.keys() == flags | exits | {"status"})
        require(type(row["status"]) is str and row["status"] in APP_STATES)
        require(all(type(row[key]) is bool for key in flags))
        require(all(row[key] is None or type(row[key]) is int and -127 <= row[key] <= 255 for key in exits))


def validate_diagnostics(report):
    require(report.keys() <= RECEIPT_KEYS)
    if "menu_attempts" in report:
        validate_menu_attempts(report["menu_attempts"])
    if "window_attempts" in report:
        validate_window_attempts(report["window_attempts"])
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
    for key in report.keys() & LIFECYCLE_DIAGNOSTICS:
        life = report[key]
        require(type(life) is dict and life.keys() <= FOCUS_FIELDS.keys())
        for row in life.values():
            require(type(row) is dict and row.keys() == set(LIFECYCLE_FIELDS)
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


def runtime_result(report):
    outcomes = [report[key] for key in PRODUCT_OUTCOMES]
    if all(value == "pass" for value in outcomes):
        return "pass", 0
    if all(value == "pass" or value.startswith("conditions_unmet") for value in outcomes):
        return "conditions_unmet", 78
    return "failure", 1


def prior_hard_failure(report):
    # A later change of Aqua session must not turn a measured failure into exit 78.
    failed = {"failure", "build_failed", "receipt_failed", "preflight_failed", "driver_build_failed"}
    cases = (*WINDOW_CASES, "positive_control", "sources")
    if any(report[key] in failed for key in cases):
        return True
    if report["menu"] in failed - {"failure"}:
        return True
    return any(row["outcome"] in {"driver_failed", "receipt_failed", "cleanup_failed"}
               for row in report.get("menu_attempts", {}).values())


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
                  menu="not_run", sources="not_run", **dict.fromkeys(WINDOW_CASES, "not_run"),
                  external_ax="not_run", voiceover="not_run", finder="not_run")
    rc = 1
    deadline = time.monotonic() + ROUTE_BUDGET_SECONDS

    def bounded_capture(*argv, timeout=10):
        limit = remaining_time(deadline, timeout)
        try:
            return capture(*argv, timeout=limit)
        except subprocess.TimeoutExpired as error:
            if limit < timeout:
                raise BudgetExhausted from error
            raise

    try:
        report["execution_tree"] = read_source(bounded_capture)
        report["source_tree"] = SOURCE_TREE
        report["stage"] = "guest-preflight"
        boot = guest_boot(bounded_capture)
        with tempfile.TemporaryDirectory(prefix="vp-native.", dir="/private/tmp") as temporary:
            scratch = Path(temporary)
            target = scratch / "target"
            console = scratch / "console"
            bounded_capture("/usr/bin/xcrun", "clang", "-fobjc-arc", "-Wall", "-Wextra", "-Werror",
                    "scripts/hosted_native_console.m", "-framework", "AppKit", "-framework", "CoreGraphics",
                    "-framework", "Security", "-o", str(console), timeout=30)
            asid = same_aqua(console, bounded_capture)
            env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_BUILD_JOBS="2", CARGO_INCREMENTAL="0",
                       CARGO_PROFILE_TEST_DEBUG="0", CARGO_PROFILE_DEV_DEBUG="0")
            app_env = dict(VOICEPORT_TEST_ISOLATED_UI="1", VOICEPORT_SOURCES_PATH=str(scratch / "sources.json"))

            def check(name, argv, timeout):
                limit = remaining_time(deadline, timeout)
                result, _ = run_check(argv, scratch / f"{name}.stdout", scratch / f"{name}.stderr",
                                      cwd=root, env=env, timeout=limit)
                if limit < timeout and result["timed_out"]:
                    report["budget_exhausted"] = True
                return result

            def attest():
                require(guest_boot(bounded_capture) == boot and same_aqua(console, bounded_capture) == asid, GuestUnavailable)

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
                # Native measurements always get their original full per-scene timeout.
                remaining_time(deadline, timeout, require_full=True)
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
            bounded_capture("rustup", "toolchain", "install", "1.95.0", "--profile", "minimal", timeout=180)
            bounded_capture("cargo", "+1.95.0", "fetch", "--locked", "--target", "aarch64-apple-darwin", timeout=240)
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

                report["window_launch"] = "foreground_requested"
                if shell_build["passed"]:
                    executable = artifact(scratch / "shell-build.stdout", target, "shortcut_native", "test")
                    binary = bundle(scratch / "shell-bundle", executable, "voiceport-shell-fixture",
                                    "org.voiceport.workbench-focus-fixture", True)
                    for case, scene, loop, marker, proof in (
                        ("shell_manual", "probe", "manual", SHELL_INITIAL_MARKER, lambda text: initial_proof(text, SHELL_INITIAL_MARKER)),
                        ("shell", "shell", "app-run", SHELL_MARKER, shell_proof),
                        ("history", "history", "app-run", HISTORY_MARKER, history_proof),
                        ("device", "device", "app-run", DEVICE_MARKER, device_proof),
                    ):
                        report["stage"] = case
                        socket = scratch / f"{case}.sock"
                        result, text = launch(binary, case, ["--hosted-shell", boot, asid], {
                            "VOICEPORT_SHELL_ACCEPTANCE": "isolated-gui", "VOICEPORT_HOSTED_WORKBENCH_SCENE": scene,
                            "VOICEPORT_HOSTED_WINDOW_LOOP": loop, "VOICEPORT_SHELL_ACCEPT_CLI": str(cli),
                            "VOICEPORT_CONTROL_SOCKET": str(socket),
                        }, marker, 90, foreground=True)
                        review_window_attempt(report, case, result, text,
                                              (scratch / case / "focus.stderr.log").read_text(errors="replace"),
                                              proof, not socket.exists())
                else:
                    for case in ("shell_manual", "shell", "history", "device"):
                        report[case] = "build_failed"

                executable = artifact(scratch / "files-build.stdout", target, "control_product_fixture", "example")
                binary = bundle(scratch / "files-bundle", executable, "voiceport-files-fixture",
                                "org.voiceport.files-acceptance-fixture", True)
                for case, loop, marker, proof in (
                    ("files_manual", "manual", FILES_INITIAL_MARKER, lambda text: initial_proof(text, FILES_INITIAL_MARKER)),
                    ("files", "app-run", FILES_MARKER, files_proof),
                ):
                    report["stage"] = case
                    files_dir = scratch / f"{case}-data"
                    files_dir.mkdir(mode=0o700)
                    result, text = launch(binary, case, [str(cli), "--hosted-files", boot, asid], {
                        "VOICEPORT_HOSTED_FILES_DIR": str(files_dir), "VOICEPORT_HOSTED_WINDOW_LOOP": loop,
                    }, marker, 90, foreground=True)
                    review_window_attempt(report, case, result, text,
                                          (scratch / case / "focus.stderr.log").read_text(errors="replace"),
                                          proof, not (files_dir / "ipc/control.sock").exists())

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
                        report["menu_attempts"] = {}
                        for scene in ("sources", *MENU_SCENES):
                            scene_binary, variables = binary, {}
                            if scene == "sources":
                                domain = "org.voiceport.source-acceptance-fixture." + uuid.uuid4().hex
                                scene_binary = bundle(scratch / "sources-bundle", executable,
                                                      MENU_EXECUTABLE, domain, True)
                                variables = {"VOICEPORT_NATIVE_SOURCE_DOMAIN": domain}
                            report["stage"] = "source-switch" if scene == "sources" else f"menu-{scene}"
                            report["menu_attempts"][scene] = {"outcome": "reached"}
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
                                if result.get("exit_code") is not None and clean(result):
                                    # Keep the lease open for owned AppKit cleanup after any clean driver exit.
                                    (data / "abort").touch(exist_ok=False)
                                    return
                                raise ValueError("menu driver did not finish cleanly")

                            result, native_text = launch(scene_binary, f"menu-{scene}",
                                                         ["--private-menu-accept", str(data), scene, boot, asid],
                                                         variables, MENU_MARKER, 60 if scene == "sources" else 130, ready)
                            driver_log = scratch / f"{scene}.stderr"
                            driver_stderr = driver_log.read_text(errors="replace") if driver_log.is_file() else ""
                            app_stderr = (scratch / f"menu-{scene}/focus.stderr.log").read_text(errors="replace")
                            ready_observed = (data / "ready").is_file()
                            attempt = menu_attempt(scene, result, driver_result, native_text, app_stderr,
                                                   driver_stderr, ready_observed)
                            report["menu_attempts"][scene] = attempt
                            if scene == "sources":
                                # Preserve the original preference-domain and abort-order proof.
                                review_source_exit(report, result, driver_result, native_text, app_stderr,
                                                   driver_stderr, ready_observed)
                            outcome = menu_driver_outcome(attempt)
                            if outcome == "conditions_unmet":
                                attempt["outcome"] = outcome
                                record_permissions_missing(scratch / f"{scene}.stdout")
                                if any(report["menu_attempts"].get(prior, {}).get("outcome") in {"driver_failed", "receipt_failed"}
                                       for prior in MENU_SCENES):
                                    report["menu"] = "failure"
                                break
                            if outcome == "driver_failed":
                                attempt["outcome"] = outcome
                                continue
                            try:
                                driver_text = (scratch / f"{scene}.stdout").read_text()
                                if scene == "sources":
                                    report["source_proof"] = source_proof(driver_text, native_text)
                                    report["sources"] = "pass"
                                else:
                                    report["menu_scenes"][scene] = menu_proof(driver_text, scene)
                            except (OSError, ValueError, TypeError, KeyError):
                                attempt["outcome"] = "receipt_failed"
                                if scene == "sources":
                                    report["sources"] = "receipt_failed"
                            else:
                                attempt["outcome"] = "pass"
                        if all(scene in report["menu_scenes"] for scene in MENU_SCENES):
                            report["menu"] = "pass"
            report["stage"] = "source-recheck"
            require(read_source(bounded_capture) == report["execution_tree"])
            report["stage"] = "complete"
            report["result"], rc = runtime_result(report)
    except BudgetExhausted:
        report["budget_exhausted"] = True
    except GuestUnavailable:
        if not prior_hard_failure(report):
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
