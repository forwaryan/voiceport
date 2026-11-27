//! Native workbench navigation and page hosting. Controllers belong to the mounted pages.
use super::{rect, symbol_image};
use objc2::{
    DefinedClass, MainThreadOnly, Message, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject,
};
use objc2_app_kit::*;
use objc2_foundation::{
    MainThreadMarker, NSIndexSet, NSLocale, NSNotification, NSObject, NSObjectProtocol, NSRect,
    NSString, ns_string,
};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::BTreeMap,
};

const SIDEBAR_WIDTH: f64 = 196.;
const WIDTH: f64 = 816.;
const HEIGHT: f64 = 520.;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Page {
    #[default]
    Result,
    History,
    Files,
    Sources,
    Device,
    Updates,
    Settings,
}

#[derive(Clone, Copy)]
pub(super) enum Language {
    Chinese,
    English,
}

impl Language {
    pub(super) fn preferred() -> Self {
        if NSLocale::preferredLanguages()
            .firstObject()
            .is_some_and(|language| language.to_string().starts_with("zh"))
        {
            Self::Chinese
        } else {
            Self::English
        }
    }

    pub(super) fn text(self, chinese: &'static str, english: &'static str) -> &'static str {
        match self {
            Self::Chinese => chinese,
            Self::English => english,
        }
    }
}

impl Page {
    pub(super) fn title(self, language: Language) -> &'static str {
        match self {
            Self::Result => language.text("听写结果", "Dictation Result"),
            Self::History => language.text("最近听写", "Recent Dictations"),
            Self::Files => language.text("转写文件", "Transcribe Files"),
            Self::Sources => language.text("转写来源", "Transcription Sources"),
            Self::Device => language.text("输入设备", "Input Device"),
            Self::Updates => language.text("软件更新", "Software Update"),
            Self::Settings => language.text("偏好设置", "Preferences"),
        }
    }

    fn identifier(self) -> &'static str {
        match self {
            Self::Result => "result",
            Self::History => "history",
            Self::Files => "files",
            Self::Sources => "sources",
            Self::Device => "device",
            Self::Updates => "updates",
            Self::Settings => "settings",
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Self::Result => "waveform",
            Self::History => "clock.arrow.circlepath",
            Self::Files => "doc.text",
            Self::Sources => "square.stack",
            Self::Device => "mic",
            Self::Updates => "arrow.triangle.2.circlepath",
            Self::Settings => "gearshape",
        }
    }
}

#[derive(Clone, Copy)]
enum Row {
    Group(&'static str, &'static str),
    Page(Page),
}

impl Row {
    fn page(self) -> Option<Page> {
        match self {
            Self::Page(page) => Some(page),
            Self::Group(..) => None,
        }
    }
}

const ROWS: [Row; 10] = [
    Row::Group("工作", "Work"),
    Row::Page(Page::Result),
    Row::Page(Page::History),
    Row::Page(Page::Files),
    Row::Group("配置", "Configuration"),
    Row::Page(Page::Sources),
    Row::Page(Page::Device),
    Row::Page(Page::Settings),
    Row::Group("系统", "System"),
    Row::Page(Page::Updates),
];

fn row_at(index: isize) -> Option<Row> {
    ROWS.get(usize::try_from(index).ok()?).copied()
}

struct MountedPage {
    view: Retained<NSView>,
    key_views: Vec<Retained<NSView>>,
}

pub(super) struct Ivars {
    language: Language,
    sidebar: OnceCell<Retained<NSTableView>>,
    host: OnceCell<Retained<NSView>>,
    pages: RefCell<BTreeMap<Page, MountedPage>>,
    selected: Cell<Option<Page>>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub(super) struct Workbench;

    unsafe impl NSObjectProtocol for Workbench {}
    unsafe impl NSControlTextEditingDelegate for Workbench {}

    unsafe impl NSTableViewDataSource for Workbench {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _sidebar: &NSTableView) -> isize {
            ROWS.len() as isize
        }
    }

    unsafe impl NSTableViewDelegate for Workbench {
        #[unsafe(method(tableView:isGroupRow:))]
        fn is_group_row(&self, _sidebar: &NSTableView, index: isize) -> bool {
            matches!(row_at(index), Some(Row::Group(..)))
        }

        #[unsafe(method(tableView:shouldSelectRow:))]
        fn should_select_row(&self, _sidebar: &NSTableView, index: isize) -> bool {
            row_at(index)
                .and_then(Row::page)
                .is_some_and(|page| self.ivars().pages.borrow().contains_key(&page))
        }

        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for_row(
            &self,
            _sidebar: &NSTableView,
            _column: Option<&NSTableColumn>,
            index: isize,
        ) -> Option<Retained<NSView>> {
            row_at(index).map(|row| self.row_view(row))
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, _notification: &NSNotification) {
            if let Some(page) = row_at(self.sidebar().selectedRow()).and_then(Row::page) {
                self.show_selected(page);
            }
        }
    }
);

impl Workbench {
    /// Creates the existing full-window surface in an unordered, inactive state.
    pub(super) fn build(
        mtm: MainThreadMarker,
        language: Language,
    ) -> (Retained<NSWindow>, Retained<Self>) {
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0., 0., WIDTH, HEIGHT),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                true,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(&format!(
            "{} {}",
            voiceport_core::test_build::application_name(),
            language.text("工作台", "Workbench"),
        )));
        window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
        window.setAutorecalculatesKeyViewLoop(false);
        let root = window.contentView().expect("native workbench content view");

        let host = NSView::initWithFrame(NSView::alloc(mtm), rect(220., 56., 572., 408.));
        root.addSubview(&host);

        let sidebar = NSTableView::initWithFrame(
            NSTableView::alloc(mtm),
            rect(0., 0., SIDEBAR_WIDTH, HEIGHT - 32.),
        );
        sidebar.setStyle(NSTableViewStyle::SourceList);
        sidebar.setHeaderView(None);
        sidebar.setRowHeight(32.);
        sidebar.setFloatsGroupRows(false);
        sidebar.setAllowsMultipleSelection(false);
        sidebar.setAllowsEmptySelection(false);
        sidebar.setAccessibilityIdentifier(Some(ns_string!("voiceport.workbench.navigation")));
        sidebar.setAccessibilityLabel(Some(&NSString::from_str(
            language.text("工作台导航", "Workbench Navigation"),
        )));
        let column =
            NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("page"));
        column.setWidth(SIDEBAR_WIDTH);
        sidebar.addTableColumn(&column);
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(0., 16., SIDEBAR_WIDTH, HEIGHT - 32.),
        );
        scroll.setDrawsBackground(false);
        scroll.setDocumentView(Some(&sidebar));
        root.addSubview(&scroll);
        let separator =
            NSBox::initWithFrame(NSBox::alloc(mtm), rect(SIDEBAR_WIDTH, 0., 1., HEIGHT));
        separator.setBoxType(NSBoxType::Separator);
        root.addSubview(&separator);

        let workbench = Self::alloc(mtm).set_ivars(Ivars {
            language,
            sidebar: OnceCell::from(sidebar),
            host: OnceCell::from(host),
            pages: RefCell::default(),
            selected: Cell::default(),
        });
        let workbench: Retained<Self> = unsafe { msg_send![super(workbench), init] };
        let sidebar = workbench.sidebar();
        unsafe {
            sidebar.setDataSource(Some(ProtocolObject::from_ref(&*workbench)));
            sidebar.setDelegate(Some(ProtocolObject::from_ref(&*workbench)));
        }
        window.setInitialFirstResponder(Some(sidebar));
        (window, workbench)
    }

    pub(super) fn page_bounds(&self) -> NSRect {
        self.host().bounds()
    }

    /// Mount one native view in page coordinates. Supply focusable descendants
    /// in Tab order, including text document views rather than scroll wrappers.
    /// AppKit skips controls that are hidden or disabled. Register each page once.
    pub(super) fn mount(&self, page: Page, view: &NSView, key_views: &[&NSView]) {
        let pages = &self.ivars().pages;
        assert!(
            !pages.borrow().contains_key(&page),
            "duplicate workbench page: {page:?}"
        );
        assert!(
            unsafe { view.superview() }.is_none(),
            "page already has a parent"
        );
        assert!(
            key_views
                .iter()
                .all(|key| !std::ptr::eq(*key, view) && key.isDescendantOf(view)),
            "key views must belong to the page"
        );
        view.setFrame(self.page_bounds());
        view.setHidden(true);
        view.setAccessibilityElement(true);
        view.setAccessibilityRole(Some(unsafe { NSAccessibilityGroupRole }));
        view.setAccessibilityLabel(Some(&NSString::from_str(page.title(self.ivars().language))));
        view.setAccessibilityIdentifier(Some(&NSString::from_str(&format!(
            "voiceport.workbench.page.{}",
            page.identifier()
        ))));
        self.host().addSubview(view);
        pages.borrow_mut().insert(
            page,
            MountedPage {
                view: view.retain(),
                key_views: key_views.iter().map(|key| key.retain()).collect(),
            },
        );
    }

    /// Select a mounted page without ordering the window or activating the app.
    pub(super) fn select(&self, page: Page) {
        if !self.ivars().pages.borrow().contains_key(&page) {
            return;
        }
        let index = ROWS
            .iter()
            .position(|row| row.page() == Some(page))
            .expect("workbench page row");
        let sidebar = self.sidebar();
        if sidebar.selectedRow() != index as isize {
            sidebar.selectRowIndexes_byExtendingSelection(
                &NSIndexSet::indexSetWithIndex(index),
                false,
            );
        }
        self.show_selected(page);
    }

    fn show_selected(&self, page: Page) {
        let iv = self.ivars();
        let pages = iv.pages.borrow();
        let Some(next) = pages.get(&page) else { return };
        let previous = iv.selected.replace(Some(page));
        if previous == Some(page) {
            return;
        }
        let sidebar = self.sidebar();
        if let Some(window) = sidebar.window() {
            window.makeFirstResponder(Some(sidebar));
        }
        if let Some(previous) = previous.and_then(|page| pages.get(&page)) {
            previous.view.setHidden(true);
        }
        next.view.setHidden(false);
        let sidebar: &NSView = sidebar;
        let keys = next.key_views.iter().map(|view| &**view);
        for (from, to) in std::iter::once(sidebar)
            .chain(keys.clone())
            .zip(keys.chain(std::iter::once(sidebar)))
        {
            unsafe { from.setNextKeyView(Some(to)) };
        }
    }

    fn row_view(&self, row: Row) -> Retained<NSView> {
        let mtm = self.mtm();
        let language = self.ivars().language;
        let cell = NSTableCellView::initWithFrame(
            NSTableCellView::alloc(mtm),
            rect(0., 0., SIDEBAR_WIDTH, 32.),
        );
        let (title, x) = match row {
            Row::Group(chinese, english) => (language.text(chinese, english), 7.),
            Row::Page(page) => {
                let icon =
                    NSImageView::initWithFrame(NSImageView::alloc(mtm), rect(6., 8., 16., 16.));
                icon.setImage(symbol_image(page.symbol(), 16.).as_deref());
                icon.setAccessibilityElement(false);
                cell.addSubview(&icon);
                unsafe { cell.setImageView(Some(&icon)) };
                cell.setAccessibilityIdentifier(Some(&NSString::from_str(&format!(
                    "voiceport.workbench.nav.{}",
                    page.identifier()
                ))));
                (page.title(language), 29.)
            }
        };
        let label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
        label.setFrame(rect(x, 6., SIDEBAR_WIDTH - x - 12., 20.));
        if matches!(row, Row::Group(..)) {
            label.setFont(Some(&NSFont::boldSystemFontOfSize(12.)));
            label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        }
        cell.addSubview(&label);
        unsafe { cell.setTextField(Some(&label)) };
        cell.into_super()
    }

    fn host(&self) -> &NSView {
        self.ivars().host.get().expect("workbench host")
    }

    fn sidebar(&self) -> &NSTableView {
        self.ivars().sidebar.get().expect("workbench navigation")
    }
}

#[cfg(debug_assertions)]
mod fixture;
#[cfg(debug_assertions)]
pub(super) use fixture::isolated_regression;
