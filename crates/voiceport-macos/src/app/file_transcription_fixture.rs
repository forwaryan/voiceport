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
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
use voiceport_core::CancellationToken;

const TEXT: &str = "合成音频结果\nSynthetic transcript";
static CALLS: AtomicUsize = AtomicUsize::new(0);

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
    let page = owner.file_page();
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
        format!("去向：{tail_origin}（本机服务，可能转发） · English")
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
    wait_for_result(owner, page);
    assert_no_request(&wrong);
    assert_no_request(&tail);
    owner.with_controller(|c| {
        let id = page.operation.borrow();
        let (status, text) = c.file_jobs.result(id.as_ref().unwrap()).unwrap();
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
    assert!(!page.window.isVisible());
    assert_eq!(fs::read(input).unwrap(), crate::plugin_runtime::sample());
    println!(
        "PASS AppKit source identities and unique titles; missing/stale ID clears plan; 0 requests before consent, 1 to tail and 0 to other sources; delivery:none; input unchanged"
    );
}

fn wait_for_result(owner: &Delegate, page: &Page) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        owner.with_controller(|c| c.file_jobs.poll());
        owner.refresh_file_page();
        if page.save_text.isEnabled() {
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
    let page = owner.file_page();
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
    let start = || {
        assert!(unsafe {
            app.sendAction_to_from(
                page.start.action().unwrap(),
                Some(&owner),
                Some(&*page.start),
            )
        });
        assert!(!page.start.isEnabled());
        assert!(!page.choose.isEnabled());
        assert!(page.cancel.isEnabled());
        wait_for_result(&owner, page);
        assert_eq!(page.result.string().to_string(), TEXT);
        owner.with_controller(|c| {
            let id = page.operation.borrow();
            let (status, text) = c.file_jobs.result(id.as_ref().unwrap()).unwrap();
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
    start();
    let first_operation = page.operation.borrow().clone();
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    let txt = folder.path().join("result.txt");
    let md = folder.path().join("result.md");
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

    let item = NSPasteboardItem::new();
    assert!(
        item.setString_forType(&input_url.absoluteString().unwrap(), unsafe {
            NSPasteboardTypeFileURL
        })
    );
    assert!(area.accept_drop(Some(&NSArray::from_slice(&[&*item]))));
    assert!(page.start.isEnabled());
    assert!(!area.accept_drop(Some(&NSArray::from_slice(&[&*item, &*item]))));
    assert!(page.notice.stringValue().to_string().contains("一个"));
    assert!(!page.start.isEnabled());
    assert_eq!(page.result.string().to_string(), TEXT);
    assert!(area.accept_drop(Some(&NSArray::from_slice(&[&*item]))));
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    start();
    assert_ne!(first_operation, *page.operation.borrow());
    assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    assert!(!page.window.isVisible());
    assert_eq!(fs::read(&input).unwrap(), audio);
    println!(
        "PASS AppKit chooser/drop -> decoded file -> authorized fixture -> selectable result -> explicit TXT/Markdown; no-clobber; input unchanged; delivery:none; dictation generation unchanged"
    );
    source_selection(&app, &input);
}
