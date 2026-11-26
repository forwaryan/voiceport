use super::*;
use objc2::rc::Weak;

pub(super) struct Page {
    root: Retained<NSView>,
    pub(super) versions: Retained<NSTextField>,
    pub(super) status: Retained<NSTextField>,
    pub(super) metrics: Retained<NSTextField>,
    pub(super) progress: Retained<NSProgressIndicator>,
    pub(super) detail: Retained<NSTextField>,
    pub(super) check: Retained<NSButton>,
    pub(super) download: Retained<NSButton>,
    pub(super) install: Retained<NSButton>,
    _actions: Retained<PageActions>,
    rendered: RefCell<Option<UpdateViewSnapshot>>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Weak<Delegate>]
    struct PageActions;

    impl PageActions {
        #[unsafe(method(check:))]
        fn check(&self, _sender: &NSButton) {
            if let Some(owner) = self.ivars().load() {
                let _ = owner.request_update_check(true);
            }
        }

        #[unsafe(method(download:))]
        fn download(&self, _sender: &NSButton) {
            if let Some(owner) = self.ivars().load() {
                owner.offer_update();
            }
        }

        #[unsafe(method(install:))]
        fn install(&self, _sender: &NSButton) {
            if let Some(owner) = self.ivars().load() {
                owner.install_staged_update();
            }
        }
    }
);

impl Page {
    fn new(owner: &Delegate, frame: NSRect) -> Self {
        let mtm = owner.mtm();
        let actions = PageActions::alloc(mtm).set_ivars(Weak::new(owner));
        let actions: Retained<PageActions> = unsafe { msg_send![super(actions), init] };
        let root = NSView::initWithFrame(NSView::alloc(mtm), frame);
        root.setAccessibilityIdentifier(Some(ns_string!("voiceport.update.page")));
        let width = frame.size.width - 32.;
        let top = frame.size.height;
        let anchor = NSAutoresizingMaskOptions::ViewMinYMargin;
        let stretch = anchor | NSAutoresizingMaskOptions::ViewWidthSizable;
        let label = |id: &str, offset, height, size| {
            let field = owner.label(&root, "", rect(16., top - offset, width, height), size);
            field.setAutoresizingMask(stretch);
            field.setAccessibilityIdentifier(Some(&NSString::from_str(id)));
            field
        };
        let heading = owner.label(&root, "软件更新", rect(16., top - 42., width, 30.), 22.);
        heading.setFont(Some(&NSFont::systemFontOfSize_weight(22., unsafe {
            NSFontWeightSemibold
        })));
        heading.setAutoresizingMask(stretch);
        let versions = label("voiceport.update.versions", 70., 22., 13.);
        versions.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let status = label("voiceport.update.status", 110., 24., 16.);
        let metrics = label("voiceport.update.metrics", 139., 23., 13.);
        let progress = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            rect(16., top - 156., width, 12.),
        );
        progress.setStyle(NSProgressIndicatorStyle::Bar);
        progress.setControlSize(NSControlSize::Small);
        progress.setIndeterminate(false);
        progress.setMinValue(0.);
        progress.setMaxValue(100.);
        progress.setAutoresizingMask(stretch);
        progress.setAccessibilityLabel(Some(ns_string!("软件下载进度")));
        root.addSubview(&progress);
        let detail = label("voiceport.update.detail", 220., 56., 13.);
        detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let button = |id: &str, title: &str, action, x, width| {
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title),
                    Some(&actions),
                    Some(action),
                    mtm,
                )
            };
            button.setFrame(rect(x, top - 261., width, 32.));
            button.setAutoresizingMask(anchor);
            button.setAccessibilityIdentifier(Some(&NSString::from_str(id)));
            root.addSubview(&button);
            button
        };
        let check = button(
            "voiceport.update.check",
            "检查更新",
            sel!(check:),
            16.,
            104.,
        );
        let download = button(
            "voiceport.update.download",
            "下载并校验",
            sel!(download:),
            126.,
            104.,
        );
        let install = button(
            "voiceport.update.install",
            "安装并重启",
            sel!(install:),
            236.,
            108.,
        );
        Self {
            root,
            versions,
            status,
            metrics,
            progress,
            detail,
            check,
            download,
            install,
            _actions: actions,
            rendered: RefCell::new(None),
        }
    }

    pub(super) fn is_visible(&self) -> bool {
        if self.root.isHiddenOrHasHiddenAncestor() {
            return false;
        }
        if let Some(window) = self.root.window() {
            return window.isVisible() && !window.isMiniaturized();
        }
        // The debug fixture mounts the page on an offscreen host with no window.
        cfg!(debug_assertions) && unsafe { self.root.superview().is_some() }
    }

    pub(super) fn render(&self, snapshot: &UpdateViewSnapshot) {
        use UpdateViewPhase::*;
        let mut rendered = self.rendered.borrow_mut();
        if rendered.as_ref() == Some(snapshot) {
            return;
        }
        let phase = snapshot.phase;
        let version_text = match snapshot.latest_version.as_deref() {
            Some(latest) if snapshot.latest_version_stale => {
                cached_versions(&snapshot.current_version, latest)
            }
            Some(latest) => versions(&snapshot.current_version, Some(latest)),
            None => format!("当前 {} · 最新版本未知", snapshot.current_version),
        };
        let status = match phase {
            Idle => "尚未检查更新",
            Checking => "正在检查更新…",
            Available => "发现新版本",
            Current => "已是最新版本",
            LocalNewer => "当前版本较新",
            Incompatible => "更新包不可用",
            CheckFailed => "无法检查更新",
            Connecting => "正在连接下载…",
            Downloading => "下载中",
            Verifying => "正在校验并暂存…",
            DownloadFailed => "无法准备更新",
            Ready => "更新已校验，可以安装",
            Installing => "正在处理更新安装…",
            Quitting => "VoicePort 正在退出…",
        };
        let mut metrics = String::new();
        let mut percent = None;
        let mut guidance = None;
        if let Some(transfer) = snapshot.transfer {
            percent = transfer.percent;
            metrics = format!("已下载 {}", download_bytes(transfer.received_bytes));
            if let Some(percent) = percent {
                metrics.push_str(&format!(" · {percent}%"));
            } else {
                metrics.push_str(" · 总量未知");
            }
            if let Some(rate) = transfer.bytes_per_second {
                metrics.push_str(&format!(" · {}", download_speed(rate)));
            }
            if transfer.stalled {
                guidance = Some("等待新的下载数据…");
            }
        }
        let mut detail = snapshot.reason.or(guidance).unwrap_or_default().to_owned();
        if let Some(reason) = snapshot.refresh_error {
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str(&format!("重新检查失败：{reason}"));
        }
        self.versions
            .setStringValue(&NSString::from_str(&version_text));
        self.status.setStringValue(&NSString::from_str(status));
        self.metrics.setStringValue(&NSString::from_str(&metrics));
        self.progress.setHidden(percent.is_none());
        if let Some(percent) = percent {
            self.progress.setDoubleValue(percent as f64);
        }
        self.detail.setStringValue(&NSString::from_str(&detail));
        self.check.setTitle(if snapshot.latest_version.is_some() {
            ns_string!("重新检查")
        } else if phase == CheckFailed {
            ns_string!("重试检查")
        } else {
            ns_string!("检查更新")
        });
        self.check.setEnabled(phase.can_check());
        self.download.setTitle(if phase == DownloadFailed {
            ns_string!("重试下载")
        } else {
            ns_string!("下载并校验")
        });
        self.download.setEnabled(phase.can_download());
        self.install.setEnabled(phase.can_install());
        *rendered = Some(snapshot.clone());
    }
}

impl Delegate {
    /// Build once and mount in the workbench with the returned Tab order.
    pub(in crate::app) fn build_update_page(
        &self,
        frame: NSRect,
    ) -> (Retained<NSView>, [Retained<NSView>; 3]) {
        let snapshot = self.update_view_snapshot();
        let mut update = self.ivars().update.borrow_mut();
        let page = update.page.get_or_insert_with(|| Page::new(self, frame));
        page.root.setFrame(frame);
        page.render(&snapshot);
        let key_views = [&page.check, &page.download, &page.install]
            .map(|button| button.clone().into_super().into_super());
        (page.root.clone(), key_views)
    }
}
