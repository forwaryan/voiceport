// Included privately by app/workbench/fixture.rs in a copy of the integrated tree.
use super::*;
use crate::app::{hosted_focus_observation, pump_hosted_window, run_hosted_window};
use objc2_foundation::{NSArray, NSPoint};
use serde_json::json;
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn observe(app: &NSApplication, mut condition: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(3);
    while !condition() {
        if Instant::now() >= end {
            return false;
        }
        pump_hosted_window(app, 0.02);
    }
    true
}

fn wait_for(app: &NSApplication, condition: impl FnMut() -> bool, name: &str) {
    assert!(observe(app, condition), "AppKit did not reach {name}");
}

fn send(window: &NSWindow, text: &str, key: u16, modifiers: NSEventModifierFlags) {
    let unmodified = NSString::from_str(if key == 48 { "\t" } else { text });
    let text = NSString::from_str(text);
    let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::KeyDown, NSPoint::ZERO, modifiers, 0., window.windowNumber(), None,
        &text, &unmodified, false, key,
    ).unwrap();
    window.sendEvent(&event);
}

fn focus_is(window: &NSWindow, view: &NSView) -> bool {
    let actual = window.firstResponder();
    let responder: &NSResponder = view;
    actual.as_deref() == Some(responder)
        || view
            .downcast_ref::<NSControl>()
            .and_then(NSControl::currentEditor)
            .is_some_and(|editor| {
                let field_editor: &NSResponder = &editor;
                actual.as_deref() == Some(field_editor)
            })
}

fn location(view: &NSView) -> serde_json::Value {
    json!({
        "ax_id": NSAccessibility::accessibilityIdentifier(view).map(|s| s.to_string()),
        "ax_label": NSAccessibility::accessibilityLabel(view).map(|s| s.to_string()),
        "ax_role": NSAccessibility::accessibilityRole(view).map(|s| s.to_string()),
    })
}

fn inspect_tab(
    app: &NSApplication,
    window: &NSWindow,
    shell: &Workbench,
    page: Page,
    route: Option<&str>,
) {
    let sidebar: &NSView = shell.sidebar();
    assert!(window.isKeyWindow() && app.isActive());
    assert!(focus_is(window, sidebar), "{page:?}: sidebar entry");
    let pages = shell.ivars().pages.borrow();
    let mounted = &pages[&page];
    let mut current = sidebar.retain();
    let mut forward = vec![current.clone()];
    for _ in 0..=mounted.key_views.len() {
        let next = unsafe { current.nextValidKeyView() }.expect("incomplete key-view loop");
        send(window, "\t", 48, NSEventModifierFlags::empty());
        assert!(
            focus_is(window, &next),
            "{page:?}: Tab to {}",
            location(&next)
        );
        if &*next == sidebar {
            break;
        }
        assert!(
            next.isDescendantOf(&mounted.view),
            "{page:?}: Tab left the page"
        );
        assert!(!next.isHiddenOrHasHiddenAncestor());
        assert!(!forward.contains(&next), "{page:?}: duplicate key view");
        forward.push(next.clone());
        current = next;
    }
    assert!(forward.len() > 1, "{page:?}: no reachable page controls");
    assert!(
        focus_is(window, sidebar),
        "{page:?}: forward cycle did not return"
    );
    for expected in forward.iter().rev() {
        send(window, "\u{19}", 48, NSEventModifierFlags::Shift);
        assert!(
            focus_is(window, expected),
            "{page:?}: Shift-Tab to {}",
            location(expected)
        );
    }
    assert!(window.isVisible() && window.isKeyWindow() && app.isActive());
    assert_eq!(app.keyWindow().as_deref(), Some(window));
    let page_ax = location(&mounted.view);
    assert_eq!(
        page_ax["ax_id"],
        format!("voiceport.workbench.page.{}", page.identifier())
    );
    assert_eq!(page_ax["ax_label"], page.title(shell.ivars().language));
    assert_eq!(page_ax["ax_role"], "AXGroup");
    println!(
        "shell_gui_tab {}",
        json!({"page":page.identifier(), "row":shell.sidebar().selectedRow(), "route":route,
        "window_number":window.windowNumber(), "tab_steps":forward.len(), "backtab_steps":forward.len(),
        "theme":window.appearance().unwrap().name().to_string(),
        "key_window":window.isKeyWindow(), "page_ax":page_ax,
        "first_responder_cycle":forward.iter().map(|key| location(key)).collect::<Vec<_>>() })
    );
}

fn cli_call(
    owner: &crate::app::Delegate,
    cli: &Path,
    socket: &Path,
    words: &[&str],
) -> serde_json::Value {
    let mut child = Command::new(cli)
        .args(["--json", "--socket"])
        .arg(socket)
        .args(words)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(6);
    while child.try_wait().unwrap().is_none() {
        owner.poll_control();
        pump_hosted_window(&NSApplication::sharedApplication(owner.mtm()), 0.01);
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            panic!("private CLI timed out: {words:?}");
        }
    }
    let output = child.wait_with_output().unwrap();
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    println!(
        "shell_gui_ipc {}",
        json!({"argv":words, "exit":output.status.code(), "reply":reply})
    );
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{words:?}: {reply}"
    );
    assert_eq!(reply["status"], "ok");
    reply["data"].clone()
}

fn open_from_cli(owner: &crate::app::Delegate, app: &NSApplication, scene: &str, full_keyboard_access: bool) -> i32 {
    use objc2_foundation::NSBundle;
    use voiceport_control::transport::{Server, path_from_env};
    let cli = std::env::var_os("VOICEPORT_SHELL_ACCEPT_CLI").expect("private CLI path");
    let cli = Path::new(&cli);
    assert!(cli.is_absolute() && cli.is_file());
    let socket = path_from_env().expect("explicit private socket");
    assert_eq!(
        std::env::var_os("VOICEPORT_CONTROL_SOCKET").as_deref(),
        Some(socket.as_os_str())
    );
    assert!(!socket.exists());
    owner.ivars().control.borrow_mut().server = Some(Server::bind(&socket).unwrap());
    let status = cli_call(owner, cli, &socket, &["status"]);
    assert_eq!(status["pid"], std::process::id());
    assert_eq!(
        status["bundle_path"],
        NSBundle::mainBundle().bundlePath().to_string()
    );
    assert_eq!(status["socket"], socket.to_string_lossy().as_ref());
    let open = |words: &[&str], page: Page| {
        let data = cli_call(owner, cli, &socket, words);
        let window = owner.ivars().window.get().unwrap();
        let shell = owner.ivars().workbench.get().unwrap();
        let key = || {
            window.isVisible()
                && window.isKeyWindow()
                && app.isActive()
                && app.keyWindow().as_deref() == Some(&**window)
        };
        if words == ["app", "show"] && page == Page::Result {
            assert_eq!(data["shown"], true);
            assert_eq!(shell.ivars().selected.get(), Some(Page::Result));
            assert!(!shell.ivars().pages.borrow()[&page].view.isHidden());
            println!("hosted-first-cli accepted=true mounted=true");
            hosted_focus_observation(owner, "first", app, Some(window));
            let positive_key = observe(app, key);
            hosted_focus_observation(owner, "final", app, Some(window));
            if !positive_key {
                eprintln!("shell_gui_environment missing positive key-window baseline");
                return false;
            }
        } else {
            wait_for(app, key, "key window after private CLI");
        }
        assert_eq!(
            shell.ivars().selected.get(),
            Some(page),
            "private CLI visible page"
        );
        assert!(!shell.ivars().pages.borrow()[&page].view.isHidden());
        if page == Page::Files {
            assert_eq!(data["page"], "file");
            assert_eq!(data["state"], "user_action_required");
            assert_eq!(data["next"], "choose_audio_file");
        } else {
            assert_eq!(data["shown"], true);
        }
        if let Some(expected) = match words {
            ["app", "show", "--settings"] | ["app", "open", "settings"] => Some("settings"),
            ["app", "open", "microphone"] | ["microphone", "open"] => Some("microphone"),
            ["app", "open", "result"] => Some("result"),
            _ => None,
        } {
            assert_eq!(data["page"], expected);
        }
        true
    };
    if !open(&["app", "show"], Page::Result) {
        return 78;
    }
    if scene == "probe" {
        println!("shell_gui_initial_key passed=true");
        return 0;
    }
    let window = owner.ivars().window.get().unwrap();
    let shell = owner.ivars().workbench.get().unwrap();
    let sidebar: &NSView = shell.sidebar();
    if scene == "history" {
        crate::app::recent_dictations::workbench::fixture::prove_visible_quick_panel_hide(owner, app, || {
            assert!(open(&["app", "show"], Page::History));
        });
        return 0;
    }
    if scene == "shell" {
        for (words, page) in [
            (&["app", "show", "--settings"][..], Page::Settings),
            (&["app", "open", "file"][..], Page::Files),
            (&["app", "open", "settings"][..], Page::Settings),
            (&["file", "open"][..], Page::Files),
            (&["app", "open", "settings"][..], Page::Settings),
            (&["app", "open", "files"][..], Page::Files),
            (&["app", "open", "result"][..], Page::Result),
        ] {
            assert!(open(words, page));
        }
        return inspect_shell(owner, app, full_keyboard_access);
    }
    assert_eq!(scene, "device");
    for (words, route) in [
        (&["app", "open", "microphone"][..], "app-open-microphone"),
        (&["microphone", "open"][..], "microphone-open"),
    ] {
        assert!(open(&["app", "open", "settings"], Page::Settings));
        assert!(open(words, Page::Device));
        assert!(std::ptr::eq(
            &**window,
            &**owner.ivars().window.get().unwrap()
        ));
        assert_eq!(shell.sidebar().selectedRow(), 6);
        assert!(
            focus_is(window, sidebar),
            "{route}: sidebar first responder"
        );
        println!("shell_gui_device_route {route} owned_window=true sidebar_responder=true");
        if full_keyboard_access {
            let original = window.appearance();
            for theme in unsafe { [NSAppearanceNameAqua, NSAppearanceNameDarkAqua] } {
                window.setAppearance(Some(&NSAppearance::appearanceNamed(theme).unwrap()));
                inspect_tab(app, window, shell, Page::Device, Some(route));
            }
            window.setAppearance(original.as_deref());
        }
    }
    window.performClose(None);
    wait_for(app,
        || !window.isVisible() && !window.isKeyWindow() && app.keyWindow().is_none(),
        "closed window before private CLI reopen",
    );
    assert!(open(&["app", "show"], Page::Device));
    let reopened = app.keyWindow().expect("key window after app show");
    assert_eq!(
        &*reopened, &**window,
        "app show should reopen the same NSWindow"
    );
    println!(
        "shell_gui_ipc_reopen {}",
        json!({"same_window":true,
        "window_number":reopened.windowNumber(), "page":"device", "key":reopened.isKeyWindow(),
        "sidebar_responder":focus_is(&reopened, sidebar)})
    );
    assert!(focus_is(window, sidebar));
    if !full_keyboard_access {
        eprintln!("shell_gui_environment keyboard navigation does not reach all controls");
        return 78;
    }
    println!("shell_gui_device_complete deep_links_and_tab=true");
    0
}

pub(super) fn run(mtm: MainThreadMarker) {
    assert_eq!(
        std::env::var("VOICEPORT_TEST_ISOLATED_UI").as_deref(),
        Ok("1")
    );
    let bundle_id = objc2_foundation::NSBundle::mainBundle().bundleIdentifier();
    if bundle_id.as_deref()
        != Some(objc2_foundation::ns_string!(
            "org.voiceport.workbench-focus-fixture"
        ))
    {
        eprintln!("shell_gui_environment dedicated test bundle required");
        std::process::exit(78);
    }
    let app = NSApplication::sharedApplication(mtm);
    assert!(app.keyWindow().is_none() && !app.windows().iter().any(|window| window.isVisible()));
    let requested = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let actual = app.activationPolicy();
    println!("shell_gui_environment activation_request={requested} active_policy={actual:?}");
    if actual != NSApplicationActivationPolicy::Accessory {
        eprintln!("shell_gui_environment native AppKit refused Accessory activation policy");
        std::process::exit(78);
    }
    let scene = std::env::var("VOICEPORT_HOSTED_WORKBENCH_SCENE").unwrap_or_else(|_| "shell".into());
    assert!(matches!(scene.as_str(), "probe" | "shell" | "history" | "device"));
    let socket = voiceport_control::transport::path_from_env().expect("explicit private socket");
    let owner = crate::app::Delegate::new(mtm);
    owner.ivars().controller.replace(Some(crate::controller::Controller::new()));
    let code = run_hosted_window(&owner, &app, move |owner, app| {
        assert_eq!(app.activationPolicy(), NSApplicationActivationPolicy::Accessory);
        hosted_focus_observation(owner, "start", app, None);
        let full_keyboard_access = app.isFullKeyboardAccessEnabled();
        println!("shell_gui_environment full_keyboard_access={full_keyboard_access}");
        open_from_cli(owner, app, &scene, full_keyboard_access)
    });
    wait_for(&app, || !socket.exists(), "owned IPC socket removal");
    println!("shell_gui_ipc_cleanup socket_removed=true");
    if code != 0 {
        std::process::exit(code);
    }
}

fn inspect_shell(owner: &crate::app::Delegate, app: &NSApplication, full_keyboard_access: bool) -> i32 {
    let window = owner.ivars().window.get().unwrap();
    let shell = owner.ivars().workbench.get().unwrap();
    let sidebar: &NSView = shell.sidebar();
    let is_key = || {
        app.isActive()
            && window.isVisible()
            && window.isKeyWindow()
            && app.keyWindow().as_deref() == Some(&**window)
    };
    wait_for(app, is_key, "visible production key window");
    assert!(focus_is(window, sidebar), "first open sidebar responder");
    let themes = unsafe { [NSAppearanceNameAqua, NSAppearanceNameDarkAqua] };
    let names = NSArray::from_slice(&themes);
    let order = [
        (1, Page::Result),
        (2, Page::History),
        (3, Page::Files),
        (5, Page::Sources),
        (6, Page::Device),
        (7, Page::Settings),
        (9, Page::Updates),
    ];
    let modifiers = NSEventModifierFlags::Function | NSEventModifierFlags::NumericPad;
    for (index, (row, page)) in order.into_iter().enumerate() {
        if index > 0 {
            send(window, "\u{f701}", 125, modifiers);
        }
        assert_eq!(shell.ivars().selected.get(), Some(page), "Down at {page:?}");
        assert_eq!(shell.sidebar().selectedRow(), row);
        assert!(is_key() && focus_is(window, sidebar));
        for theme in themes {
            window.setAppearance(Some(&NSAppearance::appearanceNamed(theme).unwrap()));
            let view = shell.ivars().pages.borrow()[&page].view.clone();
            for target in [sidebar, &*view] {
                assert_eq!(
                    target
                        .effectiveAppearance()
                        .bestMatchFromAppearancesWithNames(&names)
                        .as_deref(),
                    Some(theme)
                );
            }
            if full_keyboard_access {
                inspect_tab(app, window, shell, page, None);
            }
        }
    }
    for (row, page) in order.into_iter().rev().skip(1) {
        send(window, "\u{f700}", 126, modifiers);
        assert_eq!(shell.ivars().selected.get(), Some(page), "Up at {page:?}");
        assert_eq!(shell.sidebar().selectedRow(), row);
        assert!(is_key() && focus_is(window, sidebar));
    }

    println!("shell_gui_sidebar seven_pages_both_directions_two_appearances=true");
    owner.show_workbench_page(Page::Sources);
    assert!(is_key() && focus_is(window, sidebar));
    let focused = full_keyboard_access.then(|| {
        let focused = unsafe { sidebar.nextValidKeyView() }.expect("source page key view");
        assert_ne!(&*focused, sidebar);
        send(window, "\t", 48, NSEventModifierFlags::empty());
        assert!(focus_is(window, &focused));
        focused
    });
    owner.show_workbench_page(Page::Sources);
    let sidebar_or_previous = || {
        focused.as_ref().is_some_and(|view| focus_is(window, view)) || focus_is(window, sidebar)
    };
    assert!(focused.as_ref().map_or_else(|| focus_is(window, sidebar), |view| focus_is(window, view)),
        "same page should preserve its responder");
    window.performClose(None);
    wait_for(app,
        || !window.isVisible() && !window.isKeyWindow() && app.keyWindow().is_none(),
        "closed window",
    );
    owner.show_window();
    wait_for(app, is_key, "reopened production key window");
    assert_eq!(shell.ivars().selected.get(), Some(Page::Sources));
    assert_eq!(shell.sidebar().selectedRow(), 5);
    assert!(sidebar_or_previous(), "unexpected reopened responder: {:?}", window.firstResponder());
    println!(
        "shell_gui_reopen {}",
        json!({"page":"sources", "window_number": window.windowNumber(),
        "sidebar_responder":focus_is(window, sidebar),
        "previous_page_responder":focused.as_ref().is_some_and(|view| focus_is(window, view))})
    );
    window.performClose(None);
    wait_for(app,
        || !window.isVisible() && !window.isKeyWindow(),
        "closed test window",
    );
    if !full_keyboard_access {
        eprintln!("shell_gui_environment keyboard navigation does not reach all controls");
        return 78;
    }
    println!(
        "shell_gui: seven real mounts, key window, native event dispatch, two appearances and reopen completed"
    );
    0
}
