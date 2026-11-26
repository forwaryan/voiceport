//! File selection and result controls. The drop pasteboard is read only during a drop.
use super::*;
use crate::file_transcription::{self as service, ExportFormat, Input, Plan, State};
use block2::RcBlock;
use objc2::{Message, rc::Weak};
use objc2_foundation::{NSArray, NSURL};
use std::{collections::HashSet, ffi::CStr, os::unix::ffi::OsStrExt, path::PathBuf};

const SINGLE_FILE: service::Error =
    service::Error::new("invalid_file_count", "请只选择一个本地音频文件。");
const FILE_HINT: &str = "拖入 WAV、M4A 或 MP3 · 最多 20 MB / 180 秒";
const RESULT_HINT: &str = "文字保留在此页，导出时才保存";
const PAGE_WIDTH: f64 = 572.;
const PAGE_HEIGHT: f64 = 436.;

struct Selection {
    input: Input,
    name: String,
    attempted: bool,
}

struct PageOperation {
    id: String,
    file_name: String,
}

pub(super) struct Page {
    view: Retained<DropArea>,
    choose: Retained<NSButton>,
    file_label: Retained<NSTextField>,
    source: Retained<NSPopUpButton>,
    destination: Retained<NSTextField>,
    start: Retained<NSButton>,
    cancel: Retained<NSButton>,
    activity: Retained<NSTextField>,
    notice: Retained<NSTextField>,
    result: Retained<NSTextView>,
    result_caption: Retained<NSTextField>,
    save_text: Retained<NSButton>,
    save_markdown: Retained<NSButton>,
    panel: RefCell<Option<Retained<NSSavePanel>>>,
    selection: RefCell<Option<Selection>>,
    plan: RefCell<Option<Plan>>,
    language: RefCell<String>,
    operation: RefCell<Option<PageOperation>>,
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

pub(super) fn disambiguate_source_titles(rows: &mut [(String, String)]) {
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
    fn build(owner: &Delegate, bounds: NSRect) -> Self {
        let mtm = owner.mtm();
        let area = DropArea::new(owner, rect(0., 0., PAGE_WIDTH, PAGE_HEIGHT));
        let choose = owner.button(
            &area,
            "选择音频…",
            sel!(chooseFileTranscription:),
            18.,
            385.,
            120.,
        );
        let file_label = owner.label(&area, FILE_HINT, rect(150., 391., 398., 22.), 13.);
        file_label.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        file_label.setMaximumNumberOfLines(1);
        file_label.setAccessibilityIdentifier(Some(ns_string!("voiceport.file.selection")));
        let source_label = owner.label(&area, "来源", rect(24., 354., 74., 22.), 13.);
        let source = owner.popup(
            &area,
            &[],
            sel!(selectFileTranscriptionSource:),
            rect(95., 348., 451., 30.),
        );
        source_label.setAccessibilityIdentifier(Some(ns_string!("voiceport.file.source-label")));
        source.setAccessibilityLabel(Some(ns_string!("文件转写来源")));
        let destination = owner.label(&area, "", rect(24., 312., 524., 32.), 12.);
        destination
            .setAccessibilityIdentifier(Some(ns_string!("voiceport.file.audio-destination")));
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
        let notice = owner.label(&area, "", rect(24., 244., 524., 32.), 12.);
        notice.setAccessibilityIdentifier(Some(ns_string!("voiceport.file.notice")));
        let result_heading = owner.label(&area, "转写结果", rect(24., 219., 84., 22.), 14.);
        result_heading.setFont(Some(&NSFont::boldSystemFontOfSize(14.)));
        let result_caption = owner.caption(&area, RESULT_HINT, rect(112., 219., 436., 21.));
        result_caption.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        result_caption.setMaximumNumberOfLines(1);
        result_caption
            .setAccessibilityIdentifier(Some(ns_string!("voiceport.file.result-details")));
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(24., 52., 524., 162.));
        scroll.setHasVerticalScroller(true);
        scroll.setBorderType(NSBorderType::BezelBorder);
        let result = NSTextView::initWithFrame(NSTextView::alloc(mtm), rect(0., 0., 524., 162.));
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
            12.,
            132.,
        );
        let save_markdown = owner.button(
            &area,
            "导出 Markdown…",
            sel!(exportFileTranscriptionMarkdown:),
            158.,
            12.,
            166.,
        );
        let fixed_width: [&NSView; 5] = [&choose, &source_label, &start, &cancel, &result_heading];
        for view in fixed_width {
            view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        }
        let flexible_width: [&NSView; 6] = [
            &file_label,
            &source,
            &destination,
            &activity,
            &notice,
            &result_caption,
        ];
        for view in flexible_width {
            view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewMinYMargin
                    | NSAutoresizingMaskOptions::ViewWidthSizable,
            );
        }
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        start.setEnabled(false);
        cancel.setEnabled(false);
        source.setEnabled(false);
        save_text.setEnabled(false);
        save_markdown.setEnabled(false);
        area.setFrame(bounds);
        Self {
            view: area,
            choose,
            file_label,
            source,
            destination,
            start,
            cancel,
            activity,
            notice,
            result,
            result_caption,
            save_text,
            save_markdown,
            panel: RefCell::new(None),
            selection: RefCell::new(None),
            plan: RefCell::new(None),
            language: RefCell::new(String::new()),
            operation: RefCell::new(None),
            displayed_result: RefCell::new(None),
            displayed_error: Cell::new(None),
            #[cfg(debug_assertions)]
            fixture: RefCell::new(None),
        }
    }

    pub(super) fn content_view(&self) -> Retained<NSView> {
        self.view.clone().into_super()
    }

    pub(super) fn key_views(&self) -> [&NSView; 7] {
        [
            &self.choose,
            &self.source,
            &self.start,
            &self.cancel,
            &self.result,
            &self.save_text,
            &self.save_markdown,
        ]
    }

    fn accept_selection(
        &self,
        owner: &Delegate,
        selection: Result<PathBuf, service::Error>,
    ) -> bool {
        if owner.ivars().quitting.get() || !self.choose.isEnabled() {
            return false;
        }
        let (path, input) =
            match selection.and_then(|path| Input::open(&path).map(|input| (path, input))) {
                Ok(selected) => selected,
                Err(error) => {
                    self.selection.borrow_mut().take();
                    self.plan.borrow_mut().take();
                    set_label(&self.file_label, FILE_HINT);
                    self.file_label.setToolTip(None);
                    set_label(&self.destination, "");
                    self.destination.setToolTip(None);
                    owner.with_controller(|controller| self.refresh(controller));
                    set_label(&self.notice, error.message);
                    return false;
                }
            };
        let name: String = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_control() { '�' } else { c })
            .collect();
        set_label(&self.file_label, &name);
        self.file_label.setToolTip(Some(&NSString::from_str(&name)));
        self.selection.replace(Some(Selection {
            input,
            name,
            attempted: false,
        }));
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
        if self.selection.borrow().is_none() || !self.choose.isEnabled() {
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
                    let detail = format!("音频去向：{label} · {language}");
                    set_label(&self.destination, &detail);
                    self.destination
                        .setToolTip(Some(&NSString::from_str(&detail)));
                    *self.plan.borrow_mut() = Some(plan);
                }
                Err(message) => {
                    set_label(&self.destination, message);
                    self.destination
                        .setToolTip(Some(&NSString::from_str(message)));
                    self.plan.borrow_mut().take();
                }
            }
            self.refresh(controller);
        });
    }

    fn start(&self, owner: &Delegate) {
        if owner.ivars().quitting.get() || !self.start.isEnabled() {
            return;
        }
        let Some(plan) = self.plan.borrow().clone() else {
            return;
        };
        if picker::selected_id(&self.source).as_deref() != Some(plan.target.source.as_str()) {
            // A changed selection needs its own destination prompt and consent.
            self.select_source(owner);
            return;
        }
        owner.with_controller(|controller| {
            if controller.busy() {
                return;
            }
            let launch = {
                let mut selection = self.selection.borrow_mut();
                let Some(selection) = selection.as_mut() else {
                    return;
                };
                controller
                    .start_prepared_file(selection.input.clone(), plan, true)
                    .map(|status| {
                        selection.attempted = true;
                        controller.file_jobs.keep_for_page(&status.operation);
                        self.operation.replace(Some(PageOperation {
                            id: status.operation,
                            file_name: selection.name.clone(),
                        }));
                        self.displayed_error.set(None);
                    })
            };
            match launch {
                Ok(()) => set_label(&self.notice, ""),
                Err(error) => {
                    if error.code == "source_changed" {
                        self.selection.borrow_mut().take();
                        self.plan.borrow_mut().take();
                        set_label(&self.destination, "");
                        self.destination.setToolTip(None);
                    }
                    set_label(&self.notice, error.message);
                }
            }
            self.refresh(controller);
        });
    }

    fn refresh(&self, controller: &Controller) {
        let operation = self.operation.borrow();
        let current = operation
            .as_ref()
            .and_then(|job| controller.file_jobs.result(&job.id).ok());
        let status = current.map(|(status, _)| status);
        let state = status.map(|status| status.state);
        let error = status.and_then(|status| status.error.as_ref());
        let pending = matches!(state, Some(State::Working | State::Cancelling));
        let has_result = state == Some(State::Ready);
        let needs_new_file = error.is_some_and(|error| {
            matches!(
                error.code,
                "invalid_audio" | "file_changed" | "file_read_error"
            )
        });
        let attempted = self
            .selection
            .borrow()
            .as_ref()
            .is_some_and(|file| file.attempted);
        if attempted && (has_result || needs_new_file) {
            self.selection.borrow_mut().take();
            self.plan.borrow_mut().take();
        }
        let selected = self.selection.borrow().is_some();
        let new_selection = selected && !attempted;
        let can_select = !pending && self.panel.borrow().is_none();
        self.choose.setEnabled(can_select);
        self.source.setEnabled(can_select && selected);
        let plan = self.plan.borrow();
        self.start
            .setEnabled(can_select && !controller.busy() && selected && plan.is_some());
        let retry =
            selected && attempted && matches!(state, Some(State::Failed | State::Cancelled));
        let external = plan
            .as_ref()
            .is_some_and(|plan| plan.target.destination.requires_confirmation());
        let start_title = match (external, retry) {
            (true, true) => ns_string!("同意发送并重试"),
            (false, true) => ns_string!("重试转写"),
            (true, false) => ns_string!("同意发送并转写"),
            (false, false) => ns_string!("开始转写"),
        };
        if *self.start.title() != *start_title {
            self.start.setTitle(start_title);
        }
        let cleanup = error.is_some_and(|error| error.code == "cleanup_pending");
        let cancel_title = if cleanup {
            ns_string!("检查退出状态")
        } else {
            ns_string!("取消转写")
        };
        if *self.cancel.title() != *cancel_title {
            self.cancel.setTitle(cancel_title);
        }
        self.cancel
            .setEnabled(cleanup || state == Some(State::Working));
        self.save_text.setEnabled(can_select && has_result);
        self.save_markdown.setEnabled(can_select && has_result);
        let activity = match state {
            _ if cleanup => "等待外部程序退出",
            Some(State::Working) => "正在转写…",
            Some(State::Cancelling) => "正在取消…",
            _ if controller.busy() => "正在处理其他任务…",
            _ if new_selection => "待转写",
            Some(State::Ready) => "转写完成",
            Some(State::Failed) => "转写失败",
            Some(State::Cancelled) => "已取消",
            None => "",
        };
        set_label(&self.activity, activity);
        let error_code = error.map(|error| error.code);
        if self.displayed_error.replace(error_code) != error_code {
            let message = if state == Some(State::Cancelled)
                && status.is_some_and(|status| status.target.destination.requires_confirmation())
            {
                "文件转写已取消。已发送的音频无法撤回。"
            } else {
                error.map_or("", |error| error.message)
            };
            set_label(&self.notice, message);
        }
        let result_job = operation.as_ref().filter(|_| has_result);
        let desired = result_job.map(|job| job.id.as_str());
        let mut displayed = self.displayed_result.borrow_mut();
        if displayed.as_deref() != desired {
            let text = current.and_then(|(_, text)| text).unwrap_or_default();
            self.result.setString(&NSString::from_str(text));
            *displayed = desired.map(str::to_owned);
            let caption = result_job.map_or_else(
                || RESULT_HINT.into(),
                |job| format!("{} · 仅在此页显示", job.file_name),
            );
            set_label(&self.result_caption, &caption);
            self.result_caption
                .setToolTip(result_job.map(|_| NSString::from_str(&caption)).as_deref());
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
        self.ivars().file_page.get().expect("mounted file page")
    }

    pub(super) fn file_page_with_bounds(&self, bounds: NSRect) -> &Page {
        self.ivars()
            .file_page
            .get_or_init(|| Page::build(self, bounds))
    }

    fn present_file_page(&self) -> &Page {
        #[cfg(debug_assertions)]
        if let Some(page) = self.ivars().file_page.get()
            && page.fixture.borrow().is_some()
        {
            self.with_controller(|controller| page.refresh(controller));
            return page;
        }
        self.show_workbench_page(workbench::Page::Files);
        self.refresh_file_page();
        self.file_page()
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
        let Some(window) = page.view.window() else {
            return;
        };
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
        self.refresh_file_page();
        let owner = self.retain();
        let selected_panel = panel.clone();
        panel.beginSheetModalForWindow_completionHandler(
            &window,
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
            if let Some(operation) = page.operation.borrow().as_ref() {
                let _ = controller.file_jobs.cancel(&operation.id);
            }
            page.refresh(controller);
        });
    }

    pub(super) fn export_file_transcription(&self, format: ExportFormat) {
        let page = self.file_page();
        if self.ivars().quitting.get() || !page.save_text.isEnabled() {
            return;
        }
        let Some(operation) = page.operation.borrow().as_ref().map(|job| job.id.clone()) else {
            return;
        };
        #[cfg(debug_assertions)]
        if let Some(fixture) = page.fixture.borrow_mut().as_mut() {
            if let Some(choice) = fixture.save.take() {
                page.finish_export(self, &operation, Some(&choice), format);
            }
            return;
        }
        let Some(window) = page.view.window() else {
            return;
        };
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
        self.refresh_file_page();
        let owner = self.retain();
        let selected_panel = panel.clone();
        panel.beginSheetModalForWindow_completionHandler(
            &window,
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
