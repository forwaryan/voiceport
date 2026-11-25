//! File selection and result controls. The drop pasteboard is read only during a drop.
use super::*;
use crate::file_transcription::{self as service, ExportFormat, Input, Plan, State};
use block2::RcBlock;
use objc2::{Message, rc::Weak};
use objc2_foundation::{NSArray, NSURL};
use std::{collections::HashSet, ffi::CStr, os::unix::ffi::OsStrExt, path::PathBuf};

const SINGLE_FILE: service::Error =
    service::Error::new("invalid_file_count", "请只选择一个本地音频文件。");
const FILE_HINT: &str = "拖入 WAV、M4A 或 MP3";

pub(super) struct Page {
    window: Retained<NSWindow>,
    choose: Retained<NSButton>,
    file_label: Retained<NSTextField>,
    source: Retained<NSPopUpButton>,
    destination: Retained<NSTextField>,
    start: Retained<NSButton>,
    cancel: Retained<NSButton>,
    activity: Retained<NSTextField>,
    notice: Retained<NSTextField>,
    result: Retained<NSTextView>,
    save_text: Retained<NSButton>,
    save_markdown: Retained<NSButton>,
    panel: RefCell<Option<Retained<NSSavePanel>>>,
    input: RefCell<Option<Input>>,
    plan: RefCell<Option<Plan>>,
    language: RefCell<String>,
    operation: RefCell<Option<String>>,
    displayed_result: RefCell<Option<String>>,
    displayed_error: Cell<Option<&'static str>>,
    #[cfg(debug_assertions)]
    fixture: RefCell<Option<FixtureChoices>>,
}

fn set_label(field: &NSTextField, value: &str) {
    let value = NSString::from_str(value);
    if *field.stringValue() != *value {
        field.setStringValue(&value);
    }
}

fn disambiguate_source_titles(rows: &mut [(String, String)]) {
    let key = |title: &str| {
        NSString::from_str(title)
            .precomposedStringWithCanonicalMapping()
            .to_string()
    };
    let mut used = HashSet::new();
    for (id, title) in rows {
        if used.insert(key(title)) {
            continue;
        }
        let qualified = format!("{title} · {id}");
        title.clone_from(&qualified);
        let mut number = 2;
        while !used.insert(key(title)) {
            // A configured name may already include the ID and a number.
            *title = format!("{qualified} ({number})");
            number += 1;
        }
    }
}

fn path_from_url(url: &NSURL) -> Option<PathBuf> {
    if !url.isFileURL()
        || url.user().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.host().is_none_or(|host| {
            let host = host.to_string();
            host.is_empty() || host.eq_ignore_ascii_case("localhost")
        })
    {
        return None;
    }
    let mut buffer = [0; libc::PATH_MAX as usize];
    // On success NSURL writes a NUL-terminated filesystem path into this buffer.
    if !unsafe {
        url.getFileSystemRepresentation_maxLength(
            std::ptr::NonNull::from(&mut buffer[0]),
            buffer.len(),
        )
    } {
        return None;
    }
    let bytes = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_bytes();
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(bytes));
    path.is_absolute().then_some(path)
}

fn single_url(urls: &NSArray<NSURL>) -> Result<PathBuf, service::Error> {
    if urls.count() != 1 {
        return Err(SINGLE_FILE);
    }
    path_from_url(&urls.objectAtIndex(0)).ok_or(SINGLE_FILE)
}

fn dropped_url(items: &NSArray<NSPasteboardItem>) -> Result<PathBuf, service::Error> {
    if items.count() != 1 {
        return Err(SINGLE_FILE);
    }
    let url = items
        .objectAtIndex(0)
        .stringForType(unsafe { NSPasteboardTypeFileURL })
        .and_then(|value| NSURL::URLWithString(&value))
        .ok_or(SINGLE_FILE)?;
    path_from_url(&url).ok_or(SINGLE_FILE)
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Weak<Delegate>]
    pub(super) struct DropArea;
    unsafe impl NSObjectProtocol for DropArea {}
    unsafe impl NSDraggingDestination for DropArea {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if info.draggingSourceOperationMask().contains(NSDragOperation::Copy) {
                NSDragOperation::Copy
            } else {
                NSDragOperation::None
            }
        }
        #[unsafe(method(performDragOperation:))]
        fn perform_drop(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            info.draggingSourceOperationMask().contains(NSDragOperation::Copy)
                && self.accept_drop(info.draggingPasteboard().pasteboardItems().as_deref())
        }
    }
);

impl DropArea {
    pub(super) fn new(owner: &Delegate, frame: NSRect) -> Retained<Self> {
        let area = Self::alloc(owner.mtm()).set_ivars(Weak::from_retained(&owner.retain()));
        let area: Retained<Self> = unsafe { msg_send![super(area), initWithFrame: frame] };
        area.registerForDraggedTypes(&NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]));
        area
    }

    fn accept_drop(&self, items: Option<&NSArray<NSPasteboardItem>>) -> bool {
        let Some(owner) = self.ivars().load() else {
            return false;
        };
        if owner.ivars().quitting.get() {
            return false;
        }
        let selection = items.ok_or(SINGLE_FILE).and_then(dropped_url);
        owner
            .present_file_page()
            .accept_selection(&owner, selection)
    }
}

impl Page {
    fn build(owner: &Delegate) -> Self {
        let mtm = owner.mtm();
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0., 0., 572., 436.),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                true,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setTitle(ns_string!("音频文件转写"));
        let area = DropArea::new(owner, rect(0., 0., 572., 436.));
        window.setContentView(Some(&area));
        let choose = owner.button(
            &area,
            "选取音频…",
            sel!(chooseFileTranscription:),
            18.,
            385.,
            120.,
        );
        let file_label = owner.label(&area, FILE_HINT, rect(150., 393., 402., 20.), 13.);
        let source_label = owner.label(&area, "来源", rect(24., 354., 74., 22.), 13.);
        let source = owner.popup(
            &area,
            &[],
            sel!(selectFileTranscriptionSource:),
            rect(95., 348., 451., 30.),
        );
        source_label.setAccessibilityIdentifier(Some(ns_string!("voiceport.file.source-label")));
        source.setAccessibilityLabel(Some(ns_string!("文件转写来源")));
        let destination = owner.label(&area, "", rect(24., 314., 524., 31.), 12.);
        let start = owner.button(
            &area,
            "开始转写",
            sel!(startFileTranscription:),
            18.,
            278.,
            168.,
        );
        let cancel = owner.button(
            &area,
            "取消转写",
            sel!(cancelFileTranscription:),
            194.,
            278.,
            110.,
        );
        let activity = owner.label(&area, "", rect(317., 284., 231., 22.), 13.);
        activity.setAccessibilityIdentifier(Some(ns_string!("voiceport.file.state")));
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(24., 88., 524., 179.));
        scroll.setHasVerticalScroller(true);
        scroll.setBorderType(NSBorderType::BezelBorder);
        let result = NSTextView::initWithFrame(NSTextView::alloc(mtm), rect(0., 0., 524., 179.));
        result.setEditable(false);
        result.setSelectable(true);
        result.setRichText(false);
        result.setImportsGraphics(false);
        result.setAutomaticLinkDetectionEnabled(false);
        result.setFont(Some(&NSFont::systemFontOfSize(13.)));
        result.setTextContainerInset(NSSize::new(8., 7.));
        result.setVerticallyResizable(true);
        result.setHorizontallyResizable(false);
        result.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        if let Some(container) = unsafe { result.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        result.setAccessibilityLabel(Some(ns_string!("文件转写结果")));
        result.setAccessibilityIdentifier(Some(ns_string!("voiceport.file.result")));
        scroll.setDocumentView(Some(&result));
        area.addSubview(&scroll);
        let save_text = owner.button(
            &area,
            "导出 TXT…",
            sel!(exportFileTranscriptionText:),
            18.,
            47.,
            132.,
        );
        let save_markdown = owner.button(
            &area,
            "导出 Markdown…",
            sel!(exportFileTranscriptionMarkdown:),
            158.,
            47.,
            166.,
        );
        let notice = owner.label(&area, "", rect(24., 9., 524., 32.), 12.);
        notice.setAccessibilityIdentifier(Some(ns_string!("voiceport.file.notice")));
        window.setInitialFirstResponder(Some(&choose));
        let key_views: [&NSView; 7] = [
            &choose,
            &source,
            &start,
            &cancel,
            &result,
            &save_text,
            &save_markdown,
        ];
        for (previous, next) in key_views
            .into_iter()
            .zip(key_views.into_iter().cycle().skip(1))
        {
            unsafe {
                previous.setNextKeyView(Some(next));
            }
        }
        window.setAutorecalculatesKeyViewLoop(false);
        start.setEnabled(false);
        cancel.setEnabled(false);
        source.setEnabled(false);
        save_text.setEnabled(false);
        save_markdown.setEnabled(false);
        window.center();
        Self {
            window,
            choose,
            file_label,
            source,
            destination,
            start,
            cancel,
            activity,
            notice,
            result,
            save_text,
            save_markdown,
            panel: RefCell::new(None),
            input: RefCell::new(None),
            plan: RefCell::new(None),
            language: RefCell::new(String::new()),
            operation: RefCell::new(None),
            displayed_result: RefCell::new(None),
            displayed_error: Cell::new(None),
            #[cfg(debug_assertions)]
            fixture: RefCell::new(None),
        }
    }

    fn accept_selection(
        &self,
        owner: &Delegate,
        selection: Result<PathBuf, service::Error>,
    ) -> bool {
        if owner.ivars().quitting.get() || !self.choose.isEnabled() {
            return false;
        }
        let path = match selection.and_then(|path| Input::open(&path).map(|input| (path, input))) {
            Ok((path, input)) => {
                *self.input.borrow_mut() = Some(input);
                path
            }
            Err(error) => {
                self.input.borrow_mut().take();
                self.plan.borrow_mut().take();
                set_label(&self.file_label, FILE_HINT);
                set_label(&self.destination, "");
                owner.with_controller(|controller| self.refresh(controller));
                set_label(&self.notice, error.message);
                return false;
            }
        };
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let display: String = name
            .chars()
            .map(|c| if c.is_control() { '�' } else { c })
            .take(96)
            .collect();
        set_label(&self.file_label, &display);
        set_label(&self.notice, "");
        owner.with_controller(|controller| {
            *self.language.borrow_mut() = controller.language.clone();
            let current_title = controller
                .catalog
                .entries
                .iter()
                .find(|entry| entry.id == controller.source_id)
                .map_or("当前听写来源", |entry| entry.title.as_str());
            let mut rows = vec![(controller.source_id.clone(), current_title.to_owned())];
            rows.extend(
                service::sources(&controller.catalog, &controller.language)
                    .into_iter()
                    .filter(|target| target.source != controller.source_id)
                    .map(|target| (target.source, target.source_name)),
            );
            disambiguate_source_titles(&mut rows);
            picker::set_rows(&self.source, &rows);
        });
        self.select_source(owner);
        true
    }

    fn select_source(&self, owner: &Delegate) {
        if self.input.borrow().is_none() {
            return;
        }
        owner.with_controller(|controller| {
            let plan = picker::selected_id(&self.source)
                .ok_or("所选来源不可用，请重新选择。")
                .and_then(|id| {
                    service::Plan::resolve(&controller.catalog, &id, &self.language.borrow())
                        .map_err(|error| error.message)
                });
            match plan {
                Ok(plan) => {
                    let label = plan.target.destination.label();
                    let language = voiceport_core::language::LANGUAGES
                        .iter()
                        .find(|(code, _)| *code == plan.target.language)
                        .map_or("自动检测", |(_, name)| name);
                    set_label(&self.destination, &format!("去向：{label} · {language}"));
                    let title = if plan.target.destination.requires_confirmation() {
                        "同意发送并转写"
                    } else {
                        "开始转写"
                    };
                    self.start.setTitle(&NSString::from_str(title));
                    *self.plan.borrow_mut() = Some(plan);
                }
                Err(message) => {
                    set_label(&self.destination, message);
                    self.start.setTitle(ns_string!("开始转写"));
                    self.plan.borrow_mut().take();
                }
            }
            self.refresh(controller);
        });
    }

    fn start(&self, owner: &Delegate) {
        if owner.ivars().quitting.get() || self.panel.borrow().is_some() {
            return;
        }
        if self.plan.borrow().as_ref().is_some_and(|plan| {
            picker::selected_id(&self.source).as_deref() != Some(plan.target.source.as_str())
        }) {
            // A changed selection needs its own destination prompt and consent.
            self.select_source(owner);
            return;
        }
        owner.with_controller(|controller| {
            if controller.busy() || self.plan.borrow().is_none() {
                return;
            }
            let Some(input) = self.input.borrow_mut().take() else {
                return;
            };
            let plan = self.plan.borrow_mut().take().expect("selected file source");
            match controller.start_prepared_file(input, plan, true) {
                Ok(status) => {
                    controller.file_jobs.keep_for_page(&status.operation);
                    *self.operation.borrow_mut() = Some(status.operation);
                    set_label(&self.notice, "");
                }
                Err(error) => set_label(&self.notice, error.message),
            }
            self.refresh(controller);
        });
    }

    fn refresh(&self, controller: &Controller) {
        let operation = self.operation.borrow();
        let current = operation
            .as_deref()
            .and_then(|id| controller.file_jobs.result(id).ok());
        let status = current.map(|(status, _)| status);
        let pending =
            status.is_some_and(|status| matches!(status.state, State::Working | State::Cancelling));
        let has_result = status.is_some_and(|status| status.state == State::Ready);
        let can_select = !pending && self.panel.borrow().is_none();
        self.choose.setEnabled(can_select);
        self.source
            .setEnabled(can_select && self.input.borrow().is_some());
        self.start.setEnabled(
            can_select
                && !controller.busy()
                && self.input.borrow().is_some()
                && self.plan.borrow().is_some(),
        );
        let cleanup = status
            .and_then(|status| status.error.as_ref())
            .is_some_and(|error| error.code == "cleanup_pending");
        let cancel_title = if cleanup {
            ns_string!("检查退出状态")
        } else {
            ns_string!("取消转写")
        };
        if *self.cancel.title() != *cancel_title {
            self.cancel.setTitle(cancel_title);
        }
        self.cancel
            .setEnabled(cleanup || status.is_some_and(|status| status.state == State::Working));
        self.save_text.setEnabled(can_select && has_result);
        self.save_markdown.setEnabled(can_select && has_result);
        let activity = match status {
            Some(status) => match status.state {
                State::Working => "正在转写…",
                State::Cancelling => "正在取消…",
                State::Ready if self.input.borrow().is_some() => "显示上次结果",
                State::Ready => "转写完成",
                State::Failed => "转写失败",
                State::Cancelled => "已取消",
            },
            None if controller.busy() => "正在处理其他任务…",
            None => "",
        };
        set_label(&self.activity, activity);
        let error = status.and_then(|status| status.error.as_ref());
        if self.displayed_error.replace(error.map(|error| error.code))
            != error.map(|error| error.code)
            && let Some(error) = error
        {
            set_label(&self.notice, error.message);
        }
        let desired = if has_result {
            operation.as_deref()
        } else {
            None
        };
        let mut displayed = self.displayed_result.borrow_mut();
        if displayed.as_deref() != desired {
            let text = current.and_then(|(_, text)| text).unwrap_or_default();
            self.result.setString(&NSString::from_str(text));
            *displayed = desired.map(str::to_owned);
        }
    }

    fn finish_open(&self, owner: &Delegate, urls: &NSArray<NSURL>) {
        self.accept_selection(owner, single_url(urls));
    }

    fn finish_export(
        &self,
        owner: &Delegate,
        operation: &str,
        url: Option<&NSURL>,
        format: ExportFormat,
    ) {
        let Some(path) = url.and_then(path_from_url) else {
            set_label(&self.notice, "请选择本地保存路径。");
            return;
        };
        owner.with_controller(|controller| {
            let message = match controller.file_jobs.export(operation, &path, format) {
                Ok(()) => "已导出转写结果。",
                Err(error) => error.message,
            };
            set_label(&self.notice, message);
            self.refresh(controller);
        });
    }
}

impl Delegate {
    fn file_page(&self) -> &Page {
        self.ivars().file_page.get_or_init(|| Page::build(self))
    }

    fn present_file_page(&self) -> &Page {
        let page = self.file_page();
        self.close_quick_panel();
        self.with_controller(|controller| page.refresh(controller));
        #[cfg(debug_assertions)]
        if page.fixture.borrow().is_some() {
            return page;
        }
        page.window.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
        page
    }

    pub(super) fn show_file_transcription(&self) {
        if !self.ivars().quitting.get() {
            self.present_file_page();
        }
    }

    pub(super) fn choose_file_transcription(&self) {
        if self.ivars().quitting.get() {
            return;
        }
        let page = self.present_file_page();
        if !page.choose.isEnabled() {
            return;
        }
        #[cfg(debug_assertions)]
        if let Some(fixture) = page.fixture.borrow_mut().as_mut() {
            if let Some(choice) = fixture.open.take() {
                page.finish_open(self, &choice);
            }
            return;
        }
        let panel = NSOpenPanel::openPanel(self.mtm());
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        panel.setResolvesAliases(false);
        panel.setTreatsFilePackagesAsDirectories(false);
        panel.setMessage(Some(ns_string!("WAV、M4A 或 MP3，最多 20 MB / 180 秒。")));
        #[allow(deprecated)]
        panel.setAllowedFileTypes(Some(&NSArray::from_slice(&[
            ns_string!("wav"),
            ns_string!("m4a"),
            ns_string!("mp3"),
        ])));
        *page.panel.borrow_mut() = Some(panel.clone().into_super());
        let owner = self.retain();
        let selected_panel = panel.clone();
        panel.beginSheetModalForWindow_completionHandler(
            &page.window,
            &RcBlock::new(move |response| {
                let page = owner.file_page();
                page.panel.borrow_mut().take();
                owner.with_controller(|controller| page.refresh(controller));
                if response == NSModalResponseOK {
                    page.finish_open(&owner, &selected_panel.URLs());
                }
            }),
        );
    }

    pub(super) fn select_file_transcription_source(&self) {
        self.file_page().select_source(self);
    }

    pub(super) fn start_file_transcription(&self) {
        self.file_page().start(self);
    }

    pub(super) fn cancel_file_transcription(&self) {
        let page = self.file_page();
        self.with_controller(|controller| {
            if let Some(operation) = page.operation.borrow().as_deref() {
                let _ = controller.file_jobs.cancel(operation);
            }
            page.refresh(controller);
        });
    }

    pub(super) fn export_file_transcription(&self, format: ExportFormat) {
        let page = self.file_page();
        if self.ivars().quitting.get() || !page.save_text.isEnabled() {
            return;
        }
        let Some(operation) = page.operation.borrow().clone() else {
            return;
        };
        #[cfg(debug_assertions)]
        if let Some(fixture) = page.fixture.borrow_mut().as_mut() {
            if let Some(choice) = fixture.save.take() {
                page.finish_export(self, &operation, Some(&choice), format);
            }
            return;
        }
        let panel = NSSavePanel::savePanel(self.mtm());
        panel.setCanCreateDirectories(true);
        let extension = NSString::from_str(format.extension());
        panel.setNameFieldStringValue(&NSString::from_str(&format!(
            "转写结果.{}",
            format.extension()
        )));
        panel.setMessage(Some(ns_string!("请使用新文件名。")));
        #[allow(deprecated)]
        panel.setAllowedFileTypes(Some(&NSArray::from_slice(&[&*extension])));
        *page.panel.borrow_mut() = Some(panel.clone());
        let owner = self.retain();
        let selected_panel = panel.clone();
        panel.beginSheetModalForWindow_completionHandler(
            &page.window,
            &RcBlock::new(move |response| {
                let page = owner.file_page();
                page.panel.borrow_mut().take();
                if response == NSModalResponseOK && !owner.ivars().quitting.get() {
                    page.finish_export(&owner, &operation, selected_panel.URL().as_deref(), format);
                }
                owner.with_controller(|controller| page.refresh(controller));
            }),
        );
    }

    pub(super) fn refresh_file_page(&self) {
        if let Some(page) = self.ivars().file_page.get() {
            self.with_controller(|controller| page.refresh(controller));
        }
    }

    pub(super) fn close_file_sheet(&self) {
        if let Some(page) = self.ivars().file_page.get() {
            let panel = page.panel.borrow().clone();
            if let Some(panel) = panel {
                unsafe {
                    panel.cancel(None);
                }
            }
        }
    }
}

#[cfg(debug_assertions)]
#[derive(Default)]
struct FixtureChoices {
    open: Option<Retained<NSArray<NSURL>>>,
    save: Option<Retained<NSURL>>,
}

#[cfg(debug_assertions)]
#[path = "file_transcription_fixture.rs"]
mod fixture;
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use fixture::isolated_file_transcription_regression;
