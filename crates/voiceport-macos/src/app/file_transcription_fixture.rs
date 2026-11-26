//! Offscreen AppKit fixture for native actions, decoding, loopback and export.
use super::*;
use crate::{
    local_models::CHATGPT,
    plugin_process::Custody,
    plugins::{HTTP, Source, Store},
};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};
use voiceport_core::CancellationToken;

const TEXT: &str = "合成音频结果\nSynthetic transcript";
static CALLS: AtomicUsize = AtomicUsize::new(0);
static RELEASE_CANCELLED: AtomicBool = AtomicBool::new(false);

fn transcribe(
    plan: &Plan,
    audio: &Path,
    _: &CancellationToken,
    _: &Custody,
) -> Result<String, service::Error> {
    assert_eq!(plan.target.destination, service::Destination::ChatGpt);
    assert_eq!(plan.target.source, CHATGPT);
    assert_eq!(plan.target.language, "en");
    assert_eq!(audio.file_name().unwrap(), "upload.wav");
    assert_eq!(
        fs::metadata(audio).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(audio.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(fs::read(audio).unwrap(), crate::plugin_runtime::sample());
    CALLS.fetch_add(1, Ordering::SeqCst);
    Ok(TEXT.into())
}

fn known_failure(
    plan: &Plan,
    audio: &Path,
    cancel: &CancellationToken,
    custody: &Custody,
) -> Result<String, service::Error> {
    transcribe(plan, audio, cancel, custody)?;
    Err(service::Error::new("transcription_error", "合成来源失败。"))
}

fn delayed_result(
    plan: &Plan,
    audio: &Path,
    cancel: &CancellationToken,
    custody: &Custody,
) -> Result<String, service::Error> {
    let text = transcribe(plan, audio, cancel, custody)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !RELEASE_CANCELLED.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(4));
    }
    assert!(cancel.is_cancelled());
    Ok(text)
}

fn operation_id(page: &Page) -> String {
    page.operation.borrow().as_ref().unwrap().id.clone()
}

fn assert_no_request(listener: &TcpListener) {
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
}

fn source_selection(app: &NSApplication, input: &Path) {
    let owner = Delegate::new(app.mtm());
    owner.ivars().controller.replace(Some(Controller::new()));
    owner.with_controller(|c| {
        c.language = "en".into();
        c.transcript = "untouched dictation result".into();
    });
    let wrong = TcpListener::bind("127.0.0.1:0").unwrap();
    let tail = TcpListener::bind("127.0.0.1:0").unwrap();
    wrong.set_nonblocking(true).unwrap();
    tail.set_nonblocking(true).unwrap();
    let wrong_origin = format!("http://{}", wrong.local_addr().unwrap());
    let tail_origin = format!("http://{}", tail.local_addr().unwrap());
    let store = Store {
        sources: [
            ("first", "ChatGPT · clash", &wrong_origin),
            ("second", "ChatGPT · clash (2)", &wrong_origin),
            ("clash", "ChatGPT", &wrong_origin),
            ("tail", "Tail", &tail_origin),
        ]
        .into_iter()
        .map(|(id, name, origin)| Source {
            id: id.into(),
            name: name.into(),
            plugin_id: HTTP.into(),
            plugin_version: "1".into(),
            enabled: true,
            config: BTreeMap::from([
                ("endpoint".into(), format!("{origin}/transcribe/{id}")),
                ("model".into(), format!("model-{id}")),
            ]),
            credential_refs: BTreeMap::new(),
        })
        .collect(),
        ..Store::default()
    };
    let config = input.with_file_name("synthetic-sources.json");
    store.save(&config).unwrap();
    owner.with_controller(|c| c.catalog.add_configured(&Store::load(&config).unwrap()));
    let owner = &*owner;
    let page = owner.file_page_with_bounds(rect(0., 0., PAGE_WIDTH, PAGE_HEIGHT));
    page.fixture.borrow_mut().replace(FixtureChoices::default());
    let choose = || {
        let input = NSURL::fileURLWithPath(&NSString::from_str(input.to_str().unwrap()));
        page.fixture.borrow_mut().as_mut().unwrap().open = Some(NSArray::from_slice(&[&*input]));
        assert!(unsafe {
            app.sendAction_to_from(
                page.choose.action().unwrap(),
                Some(owner),
                Some(&*page.choose),
            )
        });
    };
    let menu = || page.source.menu().unwrap();
    let select = |index| {
        let row = page.source.itemAtIndex(index).unwrap();
        page.source.selectItem(Some(&row));
        menu().performActionForItemAtIndex(index);
    };
    let start = || {
        assert!(unsafe {
            app.sendAction_to_from(
                page.start.action().unwrap(),
                Some(owner),
                Some(&*page.start),
            )
        });
    };
    choose();
    let index = page.source.indexOfItemWithTitle(ns_string!("Tail"));
    select(index);
    let actual = page.plan.borrow().as_ref().unwrap().target.clone();
    assert_no_request(&wrong);
    assert_no_request(&tail);
    assert!(page.operation.borrow().is_none());
    println!(
        "AppKit selection={:?} index={index} items={} prepared={} destination={:?} requests_before_confirm=0",
        page.source.titleOfSelectedItem().unwrap().to_string(),
        page.source.numberOfItems(),
        actual.source,
        page.destination
            .stringValue()
            .to_string()
            .replace(&tail_origin, "tail")
            .replace(&wrong_origin, "other"),
    );
    assert_eq!(
        actual.source, "tail",
        "the selected Tail row must prepare its own source"
    );
    for (index, (id, title)) in [
        (CHATGPT, "ChatGPT"),
        ("first", "ChatGPT · clash"),
        ("second", "ChatGPT · clash (2)"),
        ("clash", "ChatGPT · clash (3)"),
        ("tail", "Tail"),
    ]
    .into_iter()
    .enumerate()
    {
        select(index as isize);
        assert_eq!(
            page.source.titleOfSelectedItem().unwrap().to_string(),
            title
        );
        assert_eq!(picker::selected_id(&page.source).as_deref(), Some(id));
        assert_eq!(page.plan.borrow().as_ref().unwrap().target.source, id);
    }
    assert_eq!(page.source.numberOfItems(), 5);
    assert_eq!(page.start.title().to_string(), "同意发送并转写");
    assert_eq!(
        page.destination.stringValue().to_string(),
        format!("音频去向：{tail_origin}（本机服务，可能转发） · English")
    );
    assert!(page.start.isEnabled());

    // A lost identity between selection and consent must discard the old plan.
    let row = page.source.selectedItem().unwrap();
    unsafe { row.setRepresentedObject(None) };
    start();
    assert!(page.plan.borrow().is_none() && page.operation.borrow().is_none());
    assert!(!page.start.isEnabled());
    unsafe { row.setRepresentedObject(Some(ns_string!("gone"))) };
    select(index);
    assert!(page.plan.borrow().is_none() && page.operation.borrow().is_none());
    assert!(!page.start.isEnabled());
    assert!(
        page.destination
            .stringValue()
            .to_string()
            .contains("不可用")
    );
    choose();
    select(index);
    assert!(page.start.isEnabled());
    assert_no_request(&wrong);
    assert_no_request(&tail);

    start();
    let deadline = Instant::now() + Duration::from_secs(5);
    let connection = loop {
        assert_no_request(&wrong);
        match tail.accept() {
            Ok((connection, _)) => break connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "no request reached the selected source"
                );
                std::thread::sleep(Duration::from_millis(4));
            }
            Err(error) => panic!("loopback accept: {error}"),
        }
    };
    connection.set_nonblocking(false).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut connection = BufReader::new(connection);
    let mut headers = String::new();
    loop {
        let offset = headers.len();
        assert_ne!(connection.read_line(&mut headers).unwrap(), 0);
        if &headers[offset..] == "\r\n" {
            break;
        }
    }
    assert!(headers.starts_with("POST /transcribe/tail HTTP/1.1\r\n"));
    let length = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    let mut body = vec![0; length];
    connection.read_exact(&mut body).unwrap();
    assert!(
        body.windows(b"model-tail".len())
            .any(|part| part == b"model-tail")
    );
    let result = serde_json::json!({"text": TEXT}).to_string();
    write!(
        connection.get_mut(),
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{result}",
        result.len()
    )
    .unwrap();
    wait_for_state(owner, page, State::Ready);
    assert_no_request(&wrong);
    assert_no_request(&tail);
    owner.with_controller(|c| {
        let (status, text) = c.file_jobs.result(&operation_id(page)).unwrap();
        assert_eq!(status.target, actual);
        assert_eq!(status.delivery, "none");
        assert_eq!(text, Some(TEXT));
        assert_eq!(c.state.generation(), 0);
        assert_eq!(c.state.phase, Phase::Idle);
        assert_eq!(c.input_method(), None);
        assert!(c.gui_dictation_receipt().is_none());
        assert_eq!(c.transcript, "untouched dictation result");
        assert_eq!(c.source_id, CHATGPT);
    });
    assert_eq!(page.result.string().to_string(), TEXT);
    assert!(page.view.window().is_none());
    assert_eq!(fs::read(input).unwrap(), crate::plugin_runtime::sample());
    println!(
        "PASS AppKit source identities and unique titles; missing/stale ID clears plan; 0 requests before consent, 1 to tail and 0 to other sources; delivery:none; input unchanged"
    );
}

fn wait_for_state(owner: &Delegate, page: &Page, state: State) {
    let operation = operation_id(page);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut current = None;
        owner.with_controller(|c| {
            c.file_jobs.poll();
            current = Some(c.file_jobs.status(&operation).unwrap().state);
        });
        owner.refresh_file_page();
        if current == Some(state) {
            break;
        }
        assert!(Instant::now() < deadline, "{}", page.notice.stringValue());
        std::thread::sleep(Duration::from_millis(4));
    }
}

pub fn isolated_file_transcription_regression(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    let owner = Delegate::new(mtm);
    owner
        .ivars()
        .controller
        .borrow_mut()
        .replace(Controller::new());
    owner.with_controller(|c| {
        c.language = "en".into();
        c.transcript = "untouched dictation result".into();
        c.file_jobs.use_test_transcriber(transcribe);
    });
    let bounds = rect(0., 0., 540., 392.);
    let page = owner.file_page_with_bounds(bounds);
    let view = page.content_view();
    assert_eq!(view.frame(), bounds);
    // The view is retained on the main thread and has not been mounted yet.
    assert!(unsafe { view.superview() }.is_none() && view.window().is_none());
    let host = NSView::initWithFrame(NSView::alloc(mtm), bounds);
    host.addSubview(&view);
    let [
        choose,
        source,
        start_control,
        cancel,
        result,
        text,
        markdown,
    ] = page.key_views();
    assert!(choose.isDescendantOf(&view) && source.isDescendantOf(&view));
    assert!(start_control.isDescendantOf(&view) && cancel.isDescendantOf(&view));
    assert!(
        result.isDescendantOf(&view)
            && text.isDescendantOf(&view)
            && markdown.isDescendantOf(&view)
    );
    assert!(
        page.result
            .enclosingScrollView()
            .unwrap()
            .frame()
            .size
            .height
            > 100.
    );
    page.fixture.borrow_mut().replace(FixtureChoices::default());
    let folder = tempfile::tempdir().unwrap();
    let input = folder.path().join("synthetic input.wav");
    let audio = crate::plugin_runtime::sample();
    fs::write(&input, &audio).unwrap();
    let url = |path: &Path| NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
    let input_url = url(&input);
    let area = DropArea::new(&owner, rect(0., 0., 572., 408.));
    let dictation = dictation_page::Page::build(mtm, &owner, &area);
    let choose = dictation.open_file.action().unwrap();
    assert_eq!(choose, sel!(chooseFileTranscription:));
    assert_eq!(page.choose.action(), Some(choose));
    page.fixture.borrow_mut().as_mut().unwrap().open =
        Some(NSArray::from_slice(&[&*input_url, &*input_url]));
    assert!(unsafe { app.sendAction_to_from(choose, Some(&owner), Some(&*page.choose)) });
    assert!(page.notice.stringValue().to_string().contains("一个"));
    assert!(!page.start.isEnabled());
    page.fixture.borrow_mut().as_mut().unwrap().open = Some(NSArray::from_slice(&[&*input_url]));
    assert!(unsafe { app.sendAction_to_from(choose, Some(&owner), Some(&*dictation.open_file)) });
    assert!(page.start.isEnabled());
    assert_eq!(page.start.title().to_string(), "同意发送并转写");
    assert!(
        page.destination
            .stringValue()
            .to_string()
            .contains("chatgpt.com")
    );
    assert!(
        page.destination
            .stringValue()
            .to_string()
            .contains("English")
    );
    assert_eq!(CALLS.load(Ordering::SeqCst), 0);
    assert!(page.operation.borrow().is_none());
    assert!(page.result.isSelectable() && !page.result.isEditable());
    assert!(!page.save_text.isEnabled());
    let trigger = |button: &NSButton| {
        assert!(unsafe {
            app.sendAction_to_from(button.action().unwrap(), Some(&owner), Some(button))
        });
    };
    let launch = || {
        trigger(&page.start);
        assert!(!page.start.isEnabled());
        assert!(!page.choose.isEnabled());
        assert!(page.cancel.isEnabled());
        assert!(!page.save_text.isEnabled() && !page.save_markdown.isEnabled());
        assert_eq!(page.activity.stringValue().to_string(), "正在转写…");
    };
    let start = || {
        launch();
        wait_for_state(&owner, page, State::Ready);
        assert!(page.save_text.isEnabled() && page.save_markdown.isEnabled());
        assert_eq!(page.result.string().to_string(), TEXT);
        owner.with_controller(|c| {
            let (status, text) = c.file_jobs.result(&operation_id(page)).unwrap();
            assert_eq!(status.delivery, "none");
            assert_eq!(text, Some(TEXT));
            assert_eq!(c.state.generation(), 0);
            assert_eq!(c.state.phase, Phase::Idle);
            assert_eq!(c.input_method(), None);
            assert!(c.gui_dictation_receipt().is_none());
            assert_eq!(c.transcript, "untouched dictation result");
            assert_eq!(c.source_id, CHATGPT);
        });
    };
    let txt = folder.path().join("result.txt");
    let md = folder.path().join("result.md");
    start();
    let first_operation = operation_id(page);
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    assert!(!txt.exists() && !md.exists());
    assert_eq!(
        page.result_caption.stringValue().to_string(),
        "synthetic input.wav · 仅在此页显示"
    );
    let save = |path: &Path, button: &NSButton| {
        page.fixture.borrow_mut().as_mut().unwrap().save = Some(url(path));
        assert!(unsafe {
            app.sendAction_to_from(button.action().unwrap(), Some(&owner), Some(button))
        });
    };
    save(&txt, &page.save_text);
    assert_eq!(fs::read_to_string(&txt).unwrap(), TEXT);
    save(&md, &page.save_markdown);
    assert_eq!(
        fs::read_to_string(&md).unwrap(),
        "# 转写结果\n\n    合成音频结果\n    Synthetic transcript\n"
    );
    fs::write(&txt, "existing TXT").unwrap();
    save(&txt, &page.save_text);
    assert_eq!(fs::read_to_string(&txt).unwrap(), "existing TXT");
    assert_eq!(page.result.string().to_string(), TEXT);
    assert!(page.notice.stringValue().to_string().contains("无法保存"));

    let next_input = folder.path().join("next synthetic.wav");
    fs::write(&next_input, &audio).unwrap();
    let item = NSPasteboardItem::new();
    assert!(
        item.setString_forType(&url(&next_input).absoluteString().unwrap(), unsafe {
            NSPasteboardTypeFileURL
        })
    );
    assert!(area.accept_drop(Some(&NSArray::from_slice(&[&*item]))));
    assert!(page.start.isEnabled());
    assert_eq!(
        page.file_label.stringValue().to_string(),
        "next synthetic.wav"
    );
    assert_eq!(page.activity.stringValue().to_string(), "待转写");
    assert_eq!(
        page.result_caption.stringValue().to_string(),
        "synthetic input.wav · 仅在此页显示"
    );
    assert!(!area.accept_drop(Some(&NSArray::from_slice(&[&*item, &*item]))));
    assert!(page.notice.stringValue().to_string().contains("一个"));
    assert!(!page.start.isEnabled());
    assert_eq!(page.result.string().to_string(), TEXT);
    assert!(area.accept_drop(Some(&NSArray::from_slice(&[&*item]))));
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    start();
    assert_ne!(first_operation, operation_id(page));
    assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    assert_eq!(
        page.result_caption.stringValue().to_string(),
        "next synthetic.wav · 仅在此页显示"
    );

    let choose_url = |input: &NSURL| {
        page.fixture.borrow_mut().as_mut().unwrap().open = Some(NSArray::from_slice(&[input]));
        trigger(&page.choose);
    };
    choose_url(&input_url);
    owner.with_controller(|c| c.file_jobs.use_test_transcriber(known_failure));
    launch();
    let failed = operation_id(page);
    wait_for_state(&owner, page, State::Failed);
    assert_eq!(page.activity.stringValue().to_string(), "转写失败");
    assert_eq!(page.notice.stringValue().to_string(), "合成来源失败。");
    assert_eq!(page.start.title().to_string(), "同意发送并重试");
    assert!(page.start.isEnabled());
    assert!(!page.save_text.isEnabled() && page.result.string().is_empty());
    assert_eq!(CALLS.load(Ordering::SeqCst), 3);

    owner.with_controller(|c| c.file_jobs.use_test_transcriber(transcribe));
    start();
    assert_ne!(failed, operation_id(page));
    assert_eq!(CALLS.load(Ordering::SeqCst), 4);
    assert_eq!(
        page.result_caption.stringValue().to_string(),
        "synthetic input.wav · 仅在此页显示"
    );
    assert!(page.notice.stringValue().is_empty());

    choose_url(&input_url);
    owner.with_controller(|c| c.file_jobs.use_test_transcriber(delayed_result));
    launch();
    let deadline = Instant::now() + Duration::from_secs(5);
    while CALLS.load(Ordering::SeqCst) < 5 {
        assert!(
            Instant::now() < deadline,
            "transcriber did not enter before cancel"
        );
        std::thread::sleep(Duration::from_millis(4));
    }
    trigger(&page.cancel);
    assert_eq!(page.activity.stringValue().to_string(), "正在取消…");
    assert!(!page.choose.isEnabled() && !page.start.isEnabled() && !page.cancel.isEnabled());
    RELEASE_CANCELLED.store(true, Ordering::SeqCst);
    wait_for_state(&owner, page, State::Cancelled);
    assert_eq!(CALLS.load(Ordering::SeqCst), 5);
    assert_eq!(page.start.title().to_string(), "同意发送并重试");
    assert!(page.start.isEnabled());
    assert!(page.notice.stringValue().to_string().contains("无法撤回"));
    assert!(!page.save_text.isEnabled() && page.result.string().is_empty());

    owner.with_controller(|c| c.file_jobs.use_test_transcriber(transcribe));
    let corrupt_input = folder.path().join("corrupt.wav");
    fs::write(&corrupt_input, b"not WAV audio").unwrap();
    choose_url(&url(&corrupt_input));
    launch();
    wait_for_state(&owner, page, State::Failed);
    assert!(!page.start.isEnabled());
    assert!(page.notice.stringValue().to_string().contains("完整解码"));
    assert_eq!(CALLS.load(Ordering::SeqCst), 5);

    choose_url(&url(&next_input));
    fs::write(&next_input, b"changed after selection").unwrap();
    launch();
    wait_for_state(&owner, page, State::Failed);
    assert!(!page.start.isEnabled());
    assert!(
        page.notice
            .stringValue()
            .to_string()
            .contains("文件在转写前发生变化")
    );
    assert_eq!(CALLS.load(Ordering::SeqCst), 5);
    assert!(page.view.window().is_none());
    println!(
        "PASS file workbench draft/result separation, explicit retry with original descriptor, cancellation discards late text, corrupt/changed audio requires reselection"
    );
    assert_eq!(fs::read(&input).unwrap(), audio);
    println!(
        "PASS AppKit chooser/drop -> decoded file -> authorized fixture -> selectable result -> explicit TXT/Markdown; no-clobber; input unchanged; delivery:none; dictation generation unchanged"
    );
    source_selection(&app, &input);
}

// Appended only to the private copy of app/file_transcription_fixture.rs.
pub(in crate::app) fn isolated_file_entry_routes(
    owner: &Delegate,
    open_with_cli: impl Fn(),
    folder: &Path,
) -> bool {
    use crate::app::pump_hosted_window;

    fn await_native(app: &NSApplication, timeout: u64, mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(timeout);
        while !predicate() {
            assert!(Instant::now() < deadline, "native window or sheet not observed");
            pump_hosted_window(app, 0.01);
        }
    }

    let app = NSApplication::sharedApplication(owner.mtm());
    assert_eq!(app.activationPolicy(), NSApplicationActivationPolicy::Accessory);
    hosted_focus_observation(owner, "start", &app, None);
    owner.with_controller(|c| {
        c.language = "en".into();
        c.file_jobs.use_test_transcriber(transcribe);
    });
    assert!(owner.ivars().file_page.get().is_none());
    open_with_cli();
    let page = owner.file_page();
    let window = page.view.window().expect("CLI must mount the file page");
    assert!(std::ptr::eq(&*window, &**owner.ivars().window.get().unwrap()));
    assert!(!page.view.isHidden());
    println!("hosted-first-cli accepted=true mounted=true");
    hosted_focus_observation(owner, "first", &app, Some(&window));
    let key_deadline = Instant::now() + Duration::from_secs(5);
    while !(app.isActive() && window.isKeyWindow() && window.isVisible()
        && app.keyWindow().as_deref() == Some(&*window))
    {
        if Instant::now() >= key_deadline {
            hosted_focus_observation(owner, "final", &app, Some(&window));
            window.close();
            assert!(!window.isVisible());
            eprintln!("files_gui_unavailable positive_key_window");
            return false;
        }
        pump_hosted_window(&app, 0.01);
    }
    hosted_focus_observation(owner, "final", &app, Some(&window));
    println!("files_gui_key owned_window=true");
    if std::env::var("VOICEPORT_HOSTED_WINDOW_LOOP").as_deref() == Ok("manual") {
        return true;
    }
    assert!(!page.view.isHidden());
    assert!(page.fixture.borrow().is_none());
    assert!(page.panel.borrow().is_none());
    assert!(page.operation.borrow().is_none());

    let shell = owner.ivars().workbench.get().unwrap();
    shell.select(workbench::Page::Result);
    assert!(page.view.isHidden());
    owner.build_context_menu();
    let menu = owner.ivars().context_menu.get().unwrap();
    let item = menu
        .itemArray()
        .iter()
        .find(|item| item.action() == Some(sel!(showFileTranscription:)))
        .expect("file entry in the actual context menu");
    let target = item.target().unwrap();
    assert!(unsafe { app.sendAction_to_from(item.action().unwrap(), Some(&target), Some(&item)) });
    assert!(!page.view.isHidden());
    assert!(std::ptr::eq(page, owner.file_page()));
    assert!(window.attachedSheet().is_none());

    let trigger = |button: &NSButton| {
        assert!(unsafe { app.sendAction_to_from(button.action().unwrap(), Some(owner), Some(button)) });
    };
    let check_sheet = |button: &NSButton, kind: &str| {
        await_native(&app, 5, || window.isKeyWindow());
        trigger(button);
        await_native(&app, 5, || window.attachedSheet().is_some());
        let sheet = window.attachedSheet().unwrap();
        let panel = page.panel.borrow().clone().expect("production panel retained");
        let panel_window: &NSWindow = &panel;
        assert!(std::ptr::eq(&*sheet, panel_window));
        assert!(std::ptr::eq(&*sheet.sheetParent().unwrap(), &*window));
        println!("files_gui_sheet {kind} attached");
        owner.close_file_sheet();
        await_native(&app, 5, || window.attachedSheet().is_none() && page.panel.borrow().is_none());
        println!("files_gui_sheet {kind} cleared");
    };
    check_sheet(&page.choose, "open");
    assert!(page.selection.borrow().is_none());

    let input = folder.join("mounted-drop.wav");
    let audio = crate::plugin_runtime::sample();
    fs::write(&input, &audio).unwrap();
    let url = NSURL::fileURLWithPath(&NSString::from_str(input.to_str().unwrap()));
    let item = NSPasteboardItem::new();
    assert!(item.setString_forType(&url.absoluteString().unwrap(), unsafe {
        NSPasteboardTypeFileURL
    }));
    assert!(page.view.accept_drop(Some(&NSArray::from_slice(&[&*item]))));
    assert_eq!(page.file_label.stringValue().to_string(), "mounted-drop.wav");
    assert!(page.operation.borrow().is_none());
    assert_eq!(CALLS.load(Ordering::SeqCst), 0);
    shell.select(workbench::Page::Settings);
    open_with_cli();
    assert!(std::ptr::eq(page, owner.file_page()));
    assert_eq!(page.file_label.stringValue().to_string(), "mounted-drop.wav");
    assert_eq!(page.start.title().to_string(), "同意发送并转写");

    trigger(&page.start);
    wait_for_state(owner, page, State::Ready);
    let operation = operation_id(page);
    shell.select(workbench::Page::Result);
    open_with_cli();
    assert!(std::ptr::eq(page, owner.file_page()));
    assert_eq!(operation_id(page), operation);
    assert_eq!(page.result.string().to_string(), TEXT);
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    check_sheet(&page.save_text, "txt");
    check_sheet(&page.save_markdown, "markdown");

    for (format, expected) in [
        (ExportFormat::Text, TEXT),
        (ExportFormat::Markdown, "# 转写结果\n\n    合成音频结果\n    Synthetic transcript\n"),
    ] {
        let path = folder.join(format!("file.{}", format.extension()));
        assert!(!path.exists(), "sheet cancellation must leave outputs absent");
        let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
        page.finish_export(owner, &operation, Some(&url), format);
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
        fs::write(&path, b"occupied\n").unwrap();
        page.finish_export(owner, &operation, Some(&url), format);
        assert_eq!(fs::read(&path).unwrap(), b"occupied\n");
        assert!(page.notice.stringValue().to_string().contains("无法保存"));
        assert_eq!(page.result.string().to_string(), TEXT);
    }
    assert_eq!(fs::read(input).unwrap(), audio);
    window.close();
    await_native(&app, 5, || !window.isVisible() && !window.isKeyWindow() && window.attachedSheet().is_none());
    println!("PASS native file entries, drop=programmatic-item, key and sheet parent, page reuse and both exports");
    true
}
