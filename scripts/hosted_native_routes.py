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
REF = "refs/heads/ci/voiceport-native-4975037a7ecc"
OVERLAY_PATHS = ('.github/workflows/voiceport-native-once.yml', 'crates/voiceport-macos/examples/control_product_fixture.rs', 'crates/voiceport-macos/fixtures/hosted_aqua_entry.rs', 'crates/voiceport-macos/src/app.rs', 'crates/voiceport-macos/src/app/control/fixture.rs', 'crates/voiceport-macos/src/app/file_transcription.rs', 'crates/voiceport-macos/src/app/file_transcription_fixture.rs', 'crates/voiceport-macos/src/app/menu_accept_fixture.rs', 'crates/voiceport-macos/src/app/menu_surface.rs', 'crates/voiceport-macos/src/app/update_ui.rs', 'crates/voiceport-macos/src/app/workbench/accept_shell_gui.rs', 'crates/voiceport-macos/src/app/workbench/fixture.rs', 'crates/voiceport-macos/tests/shortcut_native.rs', 'scripts/hosted_native_console.m', 'scripts/hosted_native_menu.swift', 'scripts/hosted_native_routes.py', 'scripts/hosted_native_transport.py')
OVERLAY_PATHS = (*OVERLAY_PATHS, 'crates/voiceport-macos/src/app/control.rs',
                 'crates/voiceport-macos/src/app/dictation_workflows.rs',
                 'crates/voiceport-macos/src/app/menu_source_accept.rs', 'crates/voiceport-macos/src/app/quick_panel.rs',
                 'crates/voiceport-macos/src/app/recent_dictations/workbench.rs',
                 'crates/voiceport-macos/src/app/recent_dictations/workbench/fixture.rs')
SOURCE_CLEANUP = "source-probe: isolated preference domain removed"
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


def review_route(report, case, result, text, stderr, proof):
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
    paths = {Path(row["executable"]).resolve(strict=True) for row in records
             if row.get("reason") == "compiler-artifact" and row.get("executable")
             and row["target"]["name"] == name and row["target"]["kind"] == [kind]}
    require(len(paths) == 1)
    executable = paths.pop()
    require(executable.is_relative_to(target.resolve()) and executable.is_file())
    return executable


def hosted_main():
    root = Path.cwd().resolve()
    require(root == Path(__file__).resolve().parent.parent)
    require(os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", GuestUnavailable)
    sys.path.insert(0, str(root / "scripts"))
    from hosted_native_transport import launch_services_focus
    from verification_run import (
        full_suite_lock,
        run_check,
        termination_signals,
        write_report,
    )

    receipt_dir = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True) / "voiceport-native-receipt"
    receipt_dir.mkdir(mode=0o700)
    report = dict(schema=1, result="failure", stage="source", shell="not_run", files="not_run", menu="not_run",
                  sources="not_run", external_ax="not_run", voiceover="not_run", finder="not_run")
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
                if report["sources"] == "not_run":
                    report["sources"] = "conditions_unmet_ax_or_event_post"

            def launch(binary, case, arguments, variables, marker, timeout, ready=None):
                attest()
                directory = scratch / case
                directory.mkdir(mode=0o700)
                result = launch_services_focus(
                    binary, arguments, directory, env, app_env=dict(app_env, **variables),
                    timeout_seconds=timeout, pass_marker=marker, on_ready=ready)
                return result, (directory / "focus.stdout.log").read_text(errors="replace")

            report["stage"] = "menu-driver-preflight"
            driver = scratch / "menu-click"
            compiled = check("swift", ["/usr/bin/xcrun", "swiftc", "-warnings-as-errors",
                                      "scripts/hosted_native_menu.swift", "-o", driver], 60)
            require(clean(compiled))
            menu_ready = False
            if compiled["passed"]:
                preflight = check("menu-preflight", [driver, "--preflight", boot, asid], 5)
                require(clean(preflight))
                menu_ready = preflight["passed"]
                report["menu"] = "preflight_pass" if menu_ready else "preflight_failed"
                if preflight["exit_code"] == 77:
                    record_permissions_missing(scratch / "menu-preflight.stdout")
            else:
                report["menu"] = "driver_build_failed"

            report["stage"] = "dependencies"
            capture("rustup", "toolchain", "install", "1.95.0", "--profile", "minimal", timeout=180)
            capture("cargo", "+1.95.0", "fetch", "--locked", "--target", "aarch64-apple-darwin", timeout=240)
            attest()
            env["CARGO_NET_OFFLINE"] = "true"
            with full_suite_lock(root), termination_signals():
                flags = ["--offline", "--locked", "-j", "2", "--message-format=json"]
                report["stage"] = "build-shell"
                shell_build = check("shell-build", ["cargo", "test", *flags, "-p", "voiceport-macos",
                                                    "--test", "shortcut_native", "--no-run"], 600)
                require(clean(shell_build))
                report["stage"] = "build-files-cli"
                common = check("files-build", ["cargo", "build", *flags, "-p", "voiceport-macos", "-p", "voiceport-cli",
                                               "--example", "control_product_fixture", "--bin", "voiceport"], 240)
                require(common["passed"] and clean(common))
                cli = artifact(scratch / "files-build.stdout", target, "voiceport", "bin")

                report["stage"] = "shell"
                if shell_build["passed"]:
                    executable = artifact(scratch / "shell-build.stdout", target, "shortcut_native", "test")
                    binary = bundle(scratch / "shell-bundle", executable, "voiceport-shell-fixture",
                                    "org.voiceport.workbench-focus-fixture", True)
                    socket = scratch / "shell.sock"
                    shell, text = launch(binary, "shell", ["--hosted-shell", boot, asid], {
                        "VOICEPORT_SHELL_ACCEPTANCE": "isolated-gui", "VOICEPORT_SHELL_ACCEPT_FILES_ALIAS": "1",
                        "VOICEPORT_SHELL_ACCEPT_CLI": str(cli), "VOICEPORT_CONTROL_SOCKET": str(socket),
                    }, SHELL_MARKER, 90)
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
                }, FILES_MARKER, 90)
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
                                raise ValueError("menu driver did not finish cleanly")

                            result, native_text = launch(scene_binary, f"menu-{scene}",
                                                         ["--private-menu-accept", str(data), scene, boot, asid],
                                                         variables, MENU_MARKER, 60 if scene == "sources" else 130, ready)
                            require(result["cleanup_confirmed"] is True and clean(driver_result))
                            if scene == "sources":
                                report["source_app_exit"] = result["exit_code"]
                                report["source_driver_exit"] = driver_result["exit_code"]
                            if driver_result["exit_code"] == 77:
                                record_permissions_missing(scratch / f"{scene}.stdout")
                                break
                            require(launch_passed(result))
                            driver_text = (scratch / f"{scene}.stdout").read_text()
                            if scene == "sources":
                                report["source_proof"] = source_proof(driver_text, native_text)
                                report["sources"] = "pass"
                            else:
                                report["menu_scenes"][scene] = menu_proof(driver_text, scene)
                        else:
                            report["menu"] = "pass"
            report["stage"] = "source-recheck"
            require(read_source() == report["execution_tree"])
            report["stage"] = "complete"
            outcomes = [report[key] for key in ("shell", "files", "menu", "sources")]
            if outcomes == ["pass"] * 4:
                report["result"], rc = "pass", 0
            elif all(value == "pass" or value.startswith("conditions_unmet") for value in outcomes):
                report["result"], rc = "conditions_unmet", 78
    except GuestUnavailable:
        report["result"], rc = "conditions_unmet", 78
    except (OSError, ValueError, TypeError, KeyError, subprocess.SubprocessError, KeyboardInterrupt):
        pass
    finally:
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
