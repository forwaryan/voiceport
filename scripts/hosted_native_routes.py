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

SOURCE_COMMIT = "b7f4e9ae3e5cb540bbe2f07bcd9c46bb8b9d00ba"
SOURCE_TREE = "f80a879fd7f5d1f9e435b8cf41fcce449334f953"
REF = "refs/heads/ci/voiceport-native-b7f4e9ae3e5c-aqua-diag-task-3574accb228f"
WORKFLOW_REF = f"forwaryan/voiceport/.github/workflows/voiceport-native-once.yml@{REF}"
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
THEMES = ("NSAppearanceNameAqua", "NSAppearanceNameDarkAqua")
DEVICE_ALIASES = {("app", "open", "microphone"): "app-open-microphone", ("microphone", "open"): "microphone-open"}
KEYBOARD_CELLS = {
    "shell": {(page, row, theme, None) for page, row in PAGES.items() for theme in THEMES},
    "device": {("device", PAGES["device"], theme, route) for route in DEVICE_ALIASES.values() for theme in THEMES},
}
KEYBOARD_COORDINATES = ("page", "row", "theme", "route")
KEYBOARD_COUNTS = ("declared_controls", "hidden_controls", "disabled_controls", "tab_steps", "backtab_steps")
KEYBOARD_FIELDS = {*KEYBOARD_COORDINATES, *KEYBOARD_COUNTS, "key_window"}
KEYBOARD_READ = "shell_gui_environment full_keyboard_access="


class GuestUnavailable(ValueError):
    pass


def require(condition, kind=ValueError):
    if not condition:
        raise kind


def console_marker(stderr):
    fields = re.fullmatch(
        r"item23 uid_matches=([01]) graphics=1 remote=0 sdk_on_console=(true|false|missing|invalid) "
        r"login_done=(true|false|missing|invalid) screen_lock=(true|false|missing|invalid) desktop_frontmost=([01])\n",
        stderr or "",
    )
    if fields:
        uid, on_console, login_done, locked, desktop = fields.groups()
        session_ok = uid == "1" and on_console == login_done == "true" and locked in {"missing", "false"}
        return session_ok, desktop == "1"
    return None


CONSOLE_REJECTIONS = {(True, False): "desktop", (False, True): "session", (False, False): "session_and_desktop"}


def console_rejection(stderr):
    if stderr == "item23 caller is outside a non-root local graphical console session\n":
        return "caller"
    return CONSOLE_REJECTIONS.get(console_marker(stderr), "unclassified")


HANDOFF_FLAGS = ("present", "active", "regular", "named", "excluded", "session_eligible", "caller_asid_matches")


def handoff_predicates(flags):
    if (type(flags) is not dict or flags.keys() != set(HANDOFF_FLAGS)
            or any(type(value) is not bool for value in flags.values())):
        return None
    present, active, regular, named, excluded, session, _ = (flags[key] for key in HANDOFF_FLAGS)
    if (not present and any((active, regular, named, excluded))) or (excluded and not named):
        return None
    desktop = present and active and regular and named and not excluded
    return session, desktop


def parse_console_handoff(stderr, exit_code):
    first, newline, detail = (stderr or "").partition("\n")
    match = re.fullmatch("item23 handoff " + " ".join(f"{name}=([01])" for name in HANDOFF_FLAGS) + "\n", detail)
    if match:
        flags = {name: bit == "1" for name, bit in zip(HANDOFF_FLAGS, match.groups(), strict=True)}
        predicates = handoff_predicates(flags)
        if predicates is not None and predicates == console_marker(first + newline):
            rejection = CONSOLE_REJECTIONS.get(predicates)
            if exit_code == (78 if rejection else 0):
                return flags, rejection
    return None


def capture(*argv, timeout=10, observation=None, console_sample=None):
    try:
        result = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                                text=True, timeout=timeout, check=True)
    except subprocess.CalledProcessError as error:
        if observation is not None:
            observation.update(status="completed", exit=error.returncode, direct_child_reaped=True)
            if observation["probe"] == "console_probe" and error.returncode == 78:
                observation["console_rejection"] = console_rejection(error.stderr)
                if console_sample is not None and (handoff := parse_console_handoff(error.stderr, error.returncode)):
                    flags, rejection = handoff
                    observation["console_rejection"] = rejection
                    console_sample["flags"] = flags
        raise
    except subprocess.TimeoutExpired:
        if observation is not None:
            # subprocess.run on Darwin kills and waits for the direct child only.
            observation.update(status="timeout", direct_child_reaped=True)
        raise
    except (OSError, ValueError):
        if observation is not None:
            observation["status"] = "error"
        raise
    if observation is not None:
        observation.update(status="completed", exit=result.returncode, direct_child_reaped=True)
        if observation["probe"] == "console_probe" and console_sample is not None:
            if handoff := parse_console_handoff(result.stderr, result.returncode):
                console_sample["flags"], _ = handoff
    return result.stdout.strip()


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
            and os.environ.get("GITHUB_EVENT_NAME") == "push" and os.environ.get("GITHUB_RUN_ATTEMPT") == "1"
            and os.environ.get("GITHUB_WORKFLOW_REF") == WORKFLOW_REF
            and os.environ.get("GITHUB_WORKFLOW_SHA") == head)
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_bytes())
    require(event.get("ref") == REF and event.get("after") == head and event.get("created") is True
            and event.get("deleted") is False and event["repository"]["id"] == 1445329392)
    require(capture_command("git", "rev-list", "--parents", "-n", "1", "HEAD").split() == [head, SOURCE_COMMIT])
    require(capture_command("git", "rev-parse", f"{SOURCE_COMMIT}^{{tree}}") == SOURCE_TREE)
    require(set(capture_command("git", "diff", "--name-only", "--no-renames", SOURCE_COMMIT, head).splitlines()) == set(OVERLAY_PATHS))
    require(not capture_command("git", "status", "--porcelain=v1", "--untracked-files=all"))
    return capture_command("git", "rev-parse", "HEAD^{tree}")


def guest_boot(capture_command=capture, *, observe=lambda _phase: None):
    observe("runner_identity")
    require(os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
            and os.environ.get("RUNNER_OS") == "macOS" and os.environ.get("RUNNER_ARCH") == "ARM64"
            and sys.platform == "darwin" and platform.machine() == "arm64" and os.geteuid() != 0,
            GuestUnavailable)
    observe("os_version")
    version = capture_command("/usr/bin/sw_vers", "-productVersion")
    require(re.fullmatch(r"15\.\d+(?:\.\d+)?", version), GuestUnavailable)
    observe("virtualization")
    require(capture_command("/usr/sbin/sysctl", "-n", "kern.hv_vmm_present") == "1", GuestUnavailable)
    observe("boot_identity")
    boot = capture_command("/usr/sbin/sysctl", "-n", "kern.bootsessionuuid")
    require(re.fullmatch(r"[a-fA-F0-9-]{36}", boot), GuestUnavailable)
    return boot


def same_aqua(console, capture_command=capture, *, observe=lambda _phase: None):
    try:
        observe("console_probe")
        output = capture_command(str(console))
        observe("console_identity")
        uid, asid = output.split()
        require(uid.isdecimal() and int(uid) == os.geteuid() and asid.isdecimal(), GuestUnavailable)
        observe("gui_query")
        record = capture_command("/bin/launchctl", "print", f"gui/{uid}")
        observe("gui_context")
        contexts = re.findall(r"(?ms)^\s*security context\s*=\s*\{([^{}]*)\}", record)
        require(len(contexts) == 1, GuestUnavailable)
        observe("gui_asid_record")
        values = re.findall(r"(?m)^\s*asid\s*=\s*(0[xX][0-9a-fA-F]+|[0-9]+)\s*$", contexts[0])
        require(len(values) == 1, GuestUnavailable)
        token = values[0]
        observe("gui_asid_match")
        require(int(token, 16 if token.lower().startswith("0x") else 10) == int(asid), GuestUnavailable)
        return asid
    except (subprocess.SubprocessError, ValueError) as exc:
        raise GuestUnavailable from exc


def attest_aqua(boot, asid, console, capture_command=capture, *, observe=lambda _phase: None):
    current_boot = guest_boot(capture_command, observe=observe)
    observe("boot_continuity")
    require(current_boot == boot, GuestUnavailable)
    current_asid = same_aqua(console, capture_command, observe=observe)
    observe("aqua_continuity")
    require(current_asid == asid, GuestUnavailable)
    observe("passed")


def elapsed_us(start):
    return (time.monotonic_ns() - start) // 1000


def ordered_handoff_times(*times):
    return all(type(value) is int and 0 <= value <= (1 << 40) for value in times) and list(times) == sorted(times)


def attest_aqua_with_trace(boot, asid, console, bounded_capture, trace, *, exit_timing=None):
    timing = sample = None
    if exit_timing is not None and {"app_exit_observed_ns", "opener_reaped_ns", "transport_closed_ns",
                                    "socket_dir_removed"} <= exit_timing.keys():
        origin = exit_timing["app_exit_observed_ns"]
        opener = (exit_timing["opener_reaped_ns"] - origin) // 1000
        closed = (exit_timing["transport_closed_ns"] - origin) // 1000
        removed = exit_timing["socket_dir_removed"]
        if ordered_handoff_times(opener, closed) and (removed is None or type(removed) is bool):
            trace["post_control"] = timing = dict(opener_reaped_us=opener, transport_closed_us=closed,
                                                  socket_dir_removed=removed)

    def observe(phase):
        trace["gate"] = phase

    def tracked_capture(*argv, timeout=10):
        nonlocal sample
        trace["last_child"] = child = dict(probe=trace["gate"], status="pending", exit=None,
                                           direct_child_reaped=None)
        if timing is None or child["probe"] != "console_probe":
            return bounded_capture(*argv, timeout=timeout, observation=child)
        started = elapsed_us(origin)
        decoded = {}
        flags = None
        try:
            output = bounded_capture(*argv, "--describe-handoff", asid, timeout=timeout,
                                     observation=child, console_sample=decoded)
        except subprocess.CalledProcessError:
            flags = decoded.get("flags")
            raise
        else:
            identity = output.split()
            if ((observed := decoded.get("flags")) is not None and len(identity) == 2
                    and observed["caller_asid_matches"] == (identity[1] == asid)):
                flags = observed
            return output
        finally:
            if flags is not None:
                finished = elapsed_us(origin)
                if ordered_handoff_times(opener, closed, started, finished):
                    sample = dict(capture_started_us=started, capture_finished_us=finished,
                                  child=dict(child), flags=flags)

    try:
        attest_aqua(boot, asid, console, tracked_capture, observe=observe)
    finally:
        # Publish only after the timestamps and the original Guard trace are available.
        if sample is not None and console_sample_matches_trace(trace, sample):
            timing["console_sample"] = sample


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


def keyboard_navigation(text, *, before_first_cli=False):
    reads = [(index, line.removeprefix(KEYBOARD_READ)) for index, line in enumerate(text.splitlines())
             if line.startswith(KEYBOARD_READ)]
    if len(reads) != 1 or reads[0][1] not in {"true", "false"}:
        return None
    index, value = reads[0]
    if before_first_cli and any(line.startswith(("shell_gui_ipc ", "hosted-first-cli "))
                                for line in text.splitlines()[:index]):
        return None
    return value == "true"


def validate_keyboard_matrix(matrix, case):
    expected = KEYBOARD_CELLS[case]
    require(type(matrix) is list and len(matrix) == len(expected))
    coordinates = set()
    for cell in matrix:
        require(type(cell) is dict and cell.keys() == KEYBOARD_FIELDS)
        require(type(cell["row"]) is int and cell["key_window"] is True)
        require(type(cell["page"]) is str and type(cell["theme"]) is str
                and (cell["route"] is None or type(cell["route"]) is str))
        require(all(type(cell[field]) is int and 0 <= cell[field] <= 128 for field in KEYBOARD_COUNTS))
        eligible = cell["declared_controls"] - cell["hidden_controls"] - cell["disabled_controls"]
        require(eligible >= 1 and cell["tab_steps"] == cell["backtab_steps"] == eligible + 1)
        coordinates.add(tuple(cell[field] for field in KEYBOARD_COORDINATES))
    require(coordinates == expected)


def tab_cycles(cycles, case):
    matrix = [{field: row[field] for field in KEYBOARD_FIELDS} for row in cycles]
    validate_keyboard_matrix(matrix, case)
    require(all(type(r["window_number"]) is int and r["window_number"] > 0
                and r["tab_steps"] == len(r["first_responder_cycle"])
                and r["page_ax"]["ax_id"] == f"voiceport.workbench.page.{r['page']}"
                and r["page_ax"]["ax_role"] == "AXGroup" and r["page_ax"]["ax_label"] for r in cycles))
    require(len({row["window_number"] for row in cycles}) == 1)
    return matrix


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
    matrix = tab_cycles(cycles, "shell")
    [reopened] = rows(text, "shell_gui_reopen ")
    require({row["window_number"] for row in cycles} == {reopened["window_number"]})
    require(text.splitlines().count(SHELL_MARKER) == 1)
    return dict(uses, page_appearance_cycles=len(matrix), keyboard_cycles=matrix)


def device_route_proof(text):
    aliases = DEVICE_ALIASES
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
    aliases = DEVICE_ALIASES
    cycles = rows(text, "shell_gui_tab ")
    matrix = tab_cycles(cycles, "device")
    [reopened] = rows(text, "shell_gui_ipc_reopen ")
    require({row["window_number"] for row in cycles} == {reopened["window_number"]})
    current = None
    for line in text.splitlines():
        if line.startswith("shell_gui_ipc "):
            current = aliases.get(tuple(json.loads(line.removeprefix("shell_gui_ipc "))["argv"]))
        elif line.startswith("shell_gui_tab "):
            require(json.loads(line.removeprefix("shell_gui_tab "))["route"] == current)
    require(text.splitlines().count(DEVICE_MARKER) == 1)
    return dict(uses, device_alias_appearance_cycles=len(matrix), keyboard_cycles=matrix)


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


def menu_side(scene, ordinal):
    if scene == "sources":
        return "left" if 1 <= ordinal <= 3 else None
    if ordinal == 63:
        return {"warm": "left", "blocked": "right"}.get(scene)
    if 1 <= ordinal <= 62:
        return "left" if ordinal == 1 or 3 <= ordinal <= 32 else "right"
    return None


def menu_trace_row(scene, origin, row):
    require(type(row) is list and len(row) >= 2 and type(row[0]) is int and type(row[1]) is str)
    ordinal, phase, *fields = row
    side = menu_side(scene, ordinal)
    require(side is not None or origin == "native" and ordinal == 0)

    def relative(value):
        return type(value) is int and 0 <= value <= 140_000

    if origin == "native":
        require(fields and relative(fields[0]))
        values = fields[1:]
        if phase == "selector":
            require(len(values) == 1 and type(values[0]) is str and values[0] in MENU_EVENTS)
        elif phase == "route":
            suffixes = ("unmatched",) if ordinal == 0 else ("matched", "unmatched")
            allowed = {f"{kind}_{suffix}" for kind in MENU_EVENTS for suffix in suffixes}
            require(len(values) == 1 and type(values[0]) is str and values[0] in allowed)
        elif phase == "surface":
            require(len(values) == 5 and all(type(value) is bool for value in values))
        else:
            require(phase in {"popup_enter", "tracking_begin", "tracking_end", "popup_return",
                              "activation_request", "became_active"} and values == ["none"])
    elif phase == "begin":
        require(fields == [side])
    elif phase == "posted":
        require(not fields)
    elif phase == "postmortem":
        require(ordinal == 1 and len(fields) == 2 and all(relative(value) for value in fields)
                and fields[0] <= fields[1])
    else:
        gates = {"none", "center", "onScreen", "exactHit", "actionable"}
        gates |= {"frontmost", "disclosure"} if side == "left" else {"menuItem", "caption", "enabled"}
        require(phase in {"first", "last", "best"} and len(fields) == 4
                and type(fields[0]) is str and fields[0] in gates and all(relative(value) for value in fields[1:3])
                and fields[1] <= fields[2] and type(fields[3]) is bool)


def validate_menu_trace(scene, origin, trace):
    require(type(trace) is dict and trace.keys() == {"state", "rows"}
            and type(trace["state"]) is str and trace["state"] in {"missing", "malformed", "captured"}
            and type(trace["rows"]) is list and len(trace["rows"]) <= 512
            and bool(trace["rows"]) == (trace["state"] == "captured"))
    seen = set()
    for row in trace["rows"]:
        menu_trace_row(scene, origin, row)
        ordinal, phase = row[:2]
        if origin == "driver":
            require((ordinal, phase) not in seen)
            seen.add((ordinal, phase))
    if origin == "driver":
        ordinals = {ordinal for ordinal, _ in seen}
        require(all((ordinal, "begin") in seen for ordinal in ordinals))
        require(ordinals == set(range(1, len(ordinals) + 1)))


def menu_trace(scene, origin, text):
    prefix = f"menu-{origin}-click "
    lines = [line.removeprefix(prefix) for line in text.splitlines() if line.startswith(prefix)]
    if not lines:
        return {"state": "missing", "rows": []}
    if len(lines) > 512 or any(len(line) > 96 for line in lines):
        return {"state": "malformed", "rows": []}
    parsed = [[part == "true" if part in {"true", "false"} else
               int(part) if re.fullmatch(r"0|[1-9][0-9]{0,5}", part) else part
               for part in line.split(" ")] for line in lines]
    trace = {"state": "captured", "rows": parsed}
    try:
        validate_menu_trace(scene, origin, trace)
    except ValueError:
        return {"state": "malformed", "rows": []}
    return trace


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
    driver_trace = menu_trace(scene, "driver", driver_stderr)
    ax_first_left = bool_diagnostic(driver_stderr, "menu-ax-first-left ", MENU_AX_FIELDS)
    if ax_first_left is not None:
        ax_state = "captured"
    elif any(line.startswith("menu-ax-first-left ") for line in driver_stderr.splitlines()):
        ax_state = "malformed"
    elif any(row[:3] == [1, "last", "actionable"] for row in driver_trace["rows"]):
        ax_state = "not_applicable"
    else:
        ax_state = "missing"
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
                ax_first_left=ax_first_left, ax_first_left_state=ax_state, driver_trace=driver_trace,
                native_trace=menu_trace(scene, "native", app_stderr))


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


def review_window_attempt(report, case, result, text, stderr, proof, socket_removed, *, attest):
    report.setdefault("window_attempts", {})[case] = dict(
        status=result["status"], app_exit=result["exit_code"], opener_exit=result["opener_exit_code"],
        app_exit_observed=result["app_exit_observed"], timed_out=result["timed_out"],
        transport_cleanup=result["cleanup_confirmed"], socket_removed=socket_removed,
    )
    report[case] = "failure"
    report[f"{case}_exit"] = result["exit_code"]
    require(result["cleanup_confirmed"] is True and socket_removed is True)
    attest()
    review_route(report, case, result, text, stderr, proof)


def review_route(report, case, result, text, stderr, proof):
    focus = focus_diagnostics(stderr)
    life = lifecycle_diagnostics(stderr)
    report[f"{case}_focus"] = focus
    report[f"{case}_lifecycle"] = life
    report[f"{case}_exit"] = result["exit_code"]
    report[case] = "failure"
    if case == "shell_manual" and closed(result):
        keyboard = keyboard_navigation(text, before_first_cli=True)
        report["keyboard_preflight"] = {None: "unavailable", False: "disabled", True: "enabled"}[keyboard]
    if case in KEYBOARD_CELLS:
        keyboard = keyboard_navigation(text, before_first_cli=True)
        if keyboard is not None:
            report[f"{case}_full_keyboard_access"] = keyboard
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
            if case in KEYBOARD_CELLS:
                require(report.get("shell_manual") == "pass" and report.get("keyboard_preflight") == "enabled"
                        and report.get(f"{case}_full_keyboard_access") is True)
            report[f"{case}_evidence"] = proof(text)
    except (ValueError, TypeError, KeyError):
        report[case] = "receipt_failed"
    else:
        report[case] = "conditions_unmet" if reason else "pass"
        if reason:
            report[f"{case}_condition"] = reason


BOOLEAN_DIAGNOSTICS = frozenset({
    "positive_control_cleanup", "budget_exhausted", "shell_full_keyboard_access", "device_full_keyboard_access", "menu_preflight_pass",
    "source_first_tick", "source_cleanup", "source_abort_cleanup", "source_lease_ended", "final_source_recheck",
    *(f"{case}_first_cli" for case in WINDOW_CASES),
})
EXIT_DIAGNOSTICS = frozenset({
    "positive_control_attest_exit", "positive_control_exit", "positive_control_opener_exit",
    "source_app_exit", "source_driver_exit", *(f"{case}_exit" for case in WINDOW_CASES),
})
FOCUS_DIAGNOSTICS = frozenset(f"{case}_focus" for case in WINDOW_CASES)
LIFECYCLE_DIAGNOSTICS = frozenset(f"{case}_lifecycle" for case in WINDOW_CASES)
AQUA_TRACE_KEYS = ("window_aqua", "menu_aqua")
DIAGNOSTIC_ENUMS = {
    "positive_control": {"not_run", "build_failed", "failure", "conditions_unmet", "pass"},
    "positive_control_condition": set(CONTROL_CONDITIONS.values()) | {"positive_key_ax_caret", "native_bootstrap"},
    "window_launch": {"foreground_requested"},
    "keyboard_preflight": {"enabled", "disabled", "unavailable"},
    "voiceover": {"not_run"},
    "source_driver_phase": SOURCE_PHASES | {None},
    **{case: {"not_run", "build_failed", "failure", "receipt_failed", "conditions_unmet", "pass"} for case in WINDOW_CASES},
    **{f"{case}_condition": {"accessory_policy", "native_bootstrap", "positive_key_window", "full_keyboard_access"}
       for case in WINDOW_CASES},
}
RECEIPT_KEYS = {
    "schema", "result", "stage", "source_tree", "execution_tree", "menu", "sources",
    "external_ax", "voiceover", "finder", "menu_permissions", "menu_scenes", "source_proof", "positive_control_focus",
    "menu_attempts", "window_attempts", *AQUA_TRACE_KEYS, *(f"{case}_evidence" for case in WINDOW_CASES),
} | BOOLEAN_DIAGNOSTICS | EXIT_DIAGNOSTICS | FOCUS_DIAGNOSTICS | LIFECYCLE_DIAGNOSTICS | DIAGNOSTIC_ENUMS.keys()

MENU_AQUA_PROBES = {"os_version", "virtualization", "boot_identity", "console_probe", "gui_query"}
MENU_AQUA_GATES = MENU_AQUA_PROBES | {
    "runner_identity", "boot_continuity", "console_identity", "gui_context", "gui_asid_record",
    "gui_asid_match", "aqua_continuity", "passed",
}


def validate_aqua_child(child):
    keys = {"probe", "status", "exit", "direct_child_reaped"}
    require(type(child) is dict and keys <= child.keys() <= keys | {"console_rejection"})
    require(type(child["probe"]) is str and child["probe"] in MENU_AQUA_PROBES)
    status, exit_code = child["status"], child["exit"]
    require(type(status) is str and status in {"pending", "not_started", "completed", "timeout", "error"})
    require(type(exit_code) is int and -127 <= exit_code <= 255 if status == "completed" else exit_code is None)
    require(child["direct_child_reaped"] is (True if status in {"completed", "timeout"} else None))
    if "console_rejection" in child:
        reason = child["console_rejection"]
        require(child["probe"] == "console_probe" and exit_code == 78 and type(reason) is str
                and reason in {"caller", "session", "desktop", "session_and_desktop", "unclassified"})


def console_sample_matches_trace(trace, sample):
    gate, last = trace["gate"], trace.get("last_child", {})
    child = sample["child"]
    if gate == "console_probe":
        return child["exit"] == 78 and child == last
    if child["exit"] != 0:
        return False
    if gate == "gui_query":
        return last.get("probe") == "gui_query" or child == last
    return (gate in {"gui_context", "gui_asid_record", "gui_asid_match", "aqua_continuity", "passed"}
            and last.get("probe") == "gui_query" and last["exit"] == 0
            and (gate != "passed" or sample["flags"]["caller_asid_matches"]))


def validate_aqua_trace(trace, *, window=False, conditions_unmet=False):
    optional = {"last_child", "post_control"} if window else {"last_child"}
    required = {"gate", "scratch_removed"}
    require(type(trace) is dict and required <= trace.keys() <= required | optional)
    require(type(trace["gate"]) is str and trace["gate"] in MENU_AQUA_GATES)
    require(trace["scratch_removed"] is None or type(trace["scratch_removed"]) is bool)
    if "last_child" in trace:
        validate_aqua_child(trace["last_child"])
    if "post_control" not in trace:
        return
    row = trace["post_control"]
    required = {"opener_reaped_us", "transport_closed_us", "socket_dir_removed"}
    require(type(row) is dict and required <= row.keys() <= required | {"console_sample"})
    require(row["socket_dir_removed"] is None or type(row["socket_dir_removed"]) is bool)
    times = [row["opener_reaped_us"], row["transport_closed_us"]]
    if "console_sample" in row:
        sample = row["console_sample"]
        require(type(sample) is dict and sample.keys() == {"capture_started_us", "capture_finished_us", "child", "flags"})
        times.extend((sample["capture_started_us"], sample["capture_finished_us"]))
        child, flags = sample["child"], sample["flags"]
        validate_aqua_child(child)
        predicates = handoff_predicates(flags)
        require(child["probe"] == "console_probe" and predicates is not None)
        rejection = CONSOLE_REJECTIONS.get(predicates)
        require(child["exit"] == (78 if rejection else 0) and child.get("console_rejection") == rejection)
        require(console_sample_matches_trace(trace, sample))
        if conditions_unmet and trace["gate"] == "aqua_continuity":
            require(not flags["caller_asid_matches"])
    require(ordered_handoff_times(*times))


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
        trace_keys = {"driver_trace", "native_trace", "ax_first_left_state"}
        old_keys = {"outcome"} | MENU_ATTEMPT_BOOLEANS | MENU_ATTEMPT_EXITS | enums.keys() | vectors.keys()
        require(row.keys() in (old_keys, old_keys | trace_keys))
        if trace_keys <= row.keys():
            for origin in ("driver", "native"):
                validate_menu_trace(scene, origin, row[f"{origin}_trace"])
            state = row["ax_first_left_state"]
            require(type(state) is str and state in {"missing", "malformed", "captured", "not_applicable"}
                    and (state == "captured") == (row["ax_first_left"] is not None))
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
    for key in AQUA_TRACE_KEYS:
        if key in report:
            trace = report[key]
            validate_aqua_trace(trace, window=key == "window_aqua", conditions_unmet=report.get("result") == "conditions_unmet")
            if "post_control" in trace:
                require(report.get("positive_control") == "pass")
    for case in KEYBOARD_CELLS:
        evidence = report.get(f"{case}_evidence", {})
        require(type(evidence) is dict)
        if report.get(case) == "pass":
            require(report.get("shell_manual") == "pass" and report.get("keyboard_preflight") == "enabled"
                    and report.get(f"{case}_full_keyboard_access") is True)
            require(report.get(f"{case}_first_cli") is True and report.get(f"{case}_exit") == 0)
            final_focus = report.get(f"{case}_focus", {}).get("final", {})
            require(all(final_focus.get(field) is True for field in ("active", "visible", "is_key", "key_present", "owned_key")))
            require(report.get("window_attempts", {}).get(case) == dict(
                status="completed", app_exit=0, opener_exit=0, app_exit_observed=True,
                timed_out=False, transport_cleanup=True, socket_removed=True))
            validate_keyboard_matrix(evidence.get("keyboard_cycles"), case)
        else:
            require("keyboard_cycles" not in evidence)
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
    if prior_hard_failure(report):
        return "failure", 1
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


def validate_receipt(report, execution_tree):
    validate_diagnostics(report)
    require(type(report.get("schema")) is int and report["schema"] == 1)
    if execution_tree is None:
        require(report.get("stage") == "source" and report.get("result") == "failure"
                and "source_tree" not in report and "execution_tree" not in report)
    else:
        require(re.fullmatch(r"[a-f0-9]{40}", execution_tree)
                and report.get("source_tree") == SOURCE_TREE and report.get("execution_tree") == execution_tree)


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
    execution_tree = None
    deadline = time.monotonic() + ROUTE_BUDGET_SECONDS
    command_clean = menu_scene_clean = True

    def bounded_capture(*argv, timeout=10, observation=None, console_sample=None):
        nonlocal command_clean
        try:
            limit = remaining_time(deadline, timeout)
        except BudgetExhausted:
            if observation is not None:
                observation["status"] = "not_started"
            raise
        command_clean = False
        try:
            output = capture(*argv, timeout=limit, observation=observation, console_sample=console_sample)
        except subprocess.CalledProcessError:
            command_clean = True  # The direct probe exited and was reaped.
            raise
        except subprocess.TimeoutExpired as error:
            if limit < timeout:
                raise BudgetExhausted from error
            raise
        command_clean = True
        return output

    try:
        execution_tree = read_source(bounded_capture)
        report["execution_tree"] = execution_tree
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
                nonlocal command_clean
                limit = remaining_time(deadline, timeout)
                command_clean = False
                result, _ = run_check(argv, scratch / f"{name}.stdout", scratch / f"{name}.stderr",
                                      cwd=root, env=env, timeout=limit)
                command_clean = clean(result)
                if limit < timeout and result["timed_out"]:
                    report["budget_exhausted"] = True
                return result

            def attest():
                attest_aqua(boot, asid, console, bounded_capture)

            def record_permissions_missing(log):
                permissions = json.loads(log.read_text())
                require(set(permissions) == {"ax", "event_post"} and all(type(value) is bool for value in permissions.values())
                        and not all(permissions.values()))
                report["menu_permissions"] = permissions
                report["menu"] = "conditions_unmet_ax_or_event_post"
                if report["sources"] == "not_run" or report.get("source_driver_exit") == 77:
                    report["sources"] = "conditions_unmet_ax_or_event_post"

            def launch(binary, case, arguments, variables, marker, timeout, ready=None, *,
                       foreground=False, exit_timing=None):
                nonlocal menu_scene_clean
                attest()
                # Native measurements always get their original full per-scene timeout.
                remaining_time(deadline, timeout, require_full=True)
                directory = scratch / case
                directory.mkdir(mode=0o700)
                if ready is not None:
                    menu_scene_clean = False
                result = launch_services_focus(
                    binary, arguments, directory, env, app_env=dict(app_env, **variables),
                    timeout_seconds=timeout, pass_marker=marker, on_ready=ready, foreground=foreground,
                    exit_timing=exit_timing)
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
                exit_timing = {}
                result, text = launch(binary, "control", arguments, {}, CONTROL_PASS, 20,
                                      exit_timing=exit_timing)
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
                return exit_timing

            report["stage"] = "dependencies"
            bounded_capture("rustup", "toolchain", "install", "1.95.0", "--profile", "minimal", timeout=180)
            bounded_capture("cargo", "+1.95.0", "fetch", "--locked", "--target", "aarch64-apple-darwin", timeout=240)
            attest()
            env["CARGO_NET_OFFLINE"] = "true"
            with full_suite_lock(root), termination_signals():
                flags = ["--offline", "--locked", "-j", "2", "--message-format=json"]
                exit_timing = run_positive_control(flags)

                report["stage"] = "window-aqua"
                report["window_aqua"] = window_aqua = dict(gate="runner_identity", scratch_removed=None)
                attest_aqua_with_trace(boot, asid, console, bounded_capture, window_aqua, exit_timing=exit_timing)

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
                                              proof, not socket.exists(), attest=attest)
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
                                          proof, not (files_dir / "ipc/control.sock").exists(), attest=attest)

                report["stage"] = "window-source-recheck"
                require(read_source(bounded_capture) == report["execution_tree"])

                report["stage"] = "menu-driver-preflight"
                report["menu_aqua"] = menu_aqua = dict(gate="runner_identity", scratch_removed=None)
                attest_aqua_with_trace(boot, asid, console, bounded_capture, menu_aqua)
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
                            menu_scene_clean = attempt["graceful_cleanup"] and attempt["driver_clean"]
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
    except BudgetExhausted:
        report["budget_exhausted"] = True
    except GuestUnavailable:
        if not prior_hard_failure(report):
            report["result"], rc = "conditions_unmet", 78
    except KeyboardInterrupt:
        command_clean = False
    except (OSError, ValueError, TypeError, KeyError, subprocess.SubprocessError):
        pass
    else:
        report["stage"] = "source-recheck"
    finally:
        aqua_traces = [report[key] for key in AQUA_TRACE_KEYS if key in report]
        if aqua_traces:
            try:
                scratch.lstat()
            except FileNotFoundError:
                scratch_removed = True
            except OSError:
                scratch_removed = None
            else:
                scratch_removed = False
            for trace in aqua_traces:
                trace["scratch_removed"] = scratch_removed
        if "menu_aqua" in report:
            # The menu gate is reached only after the window source seal. Keep any late failure stage.
            if report["menu_aqua"]["scratch_removed"] is True and command_clean and menu_scene_clean:
                try:
                    remaining_time(deadline, 10)
                    require(read_source(bounded_capture) == execution_tree)
                except BudgetExhausted:
                    report.update(result="failure", budget_exhausted=True)
                    rc = 1
                except KeyboardInterrupt:
                    report["result"], rc = "failure", 1
                except (OSError, ValueError, TypeError, KeyError, subprocess.SubprocessError):
                    report.update(result="failure", final_source_recheck=False)
                    rc = 1
                else:
                    report["final_source_recheck"] = True
                    if report["stage"] == "source-recheck":
                        report["stage"] = "complete"
                        report["result"], rc = runtime_result(report)
        try:
            validate_receipt(report, execution_tree)
        except (ValueError, TypeError, KeyError):
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
