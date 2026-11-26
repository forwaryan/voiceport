//! Shared native update state for the workbench, menu and control endpoint.
use super::*;
use crate::app_update::{self, Check, DownloadProgress, DownloadSnapshot, Staged, Update};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};
use voiceport_control::Response;

#[cfg(debug_assertions)]
mod fixture;
mod page;
mod presentation;

pub(super) use presentation::{UpdateViewPhase, UpdateViewSnapshot};

const AUTO_UPDATE_PREF: &str = "automaticallyCheckForUpdates";
const FIRST_CHECK: Duration = Duration::from_secs(15);
const CHECK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);
const DOWNLOAD_REFRESH: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(super) struct UpdateUi {
    version: OnceCell<String>,
    initialized: bool,
    automatic: bool,
    next: Option<Instant>,
    state: State,
    last_check: CheckSummary,
    page: Option<page::Page>,
    page_was_visible: bool,
    #[cfg(debug_assertions)]
    fixture: Option<CheckFixture>,
}

impl UpdateUi {
    pub(super) fn running_version(&self) -> String {
        #[cfg(debug_assertions)]
        if self.fixture.is_some() {
            return env!("CARGO_PKG_VERSION").into();
        }
        self.version.get_or_init(current_version).clone()
    }
}

#[derive(Default)]
struct CheckSummary {
    latest: Option<String>,
    state: Option<&'static str>,
    reason: Option<&'static str>,
}

impl CheckSummary {
    fn record(&mut self, result: &app_update::Result<Check>) {
        let (state, latest, reason) = match result {
            Ok(Check::Available(update)) => ("update_available", Some(&update.version), None),
            Ok(Check::Current {
                latest,
                local_newer,
            }) => (
                if *local_newer {
                    "local_newer"
                } else {
                    "up_to_date"
                },
                Some(latest),
                None,
            ),
            Ok(Check::Incompatible { latest, reason }) => {
                ("incompatible", Some(latest), Some(*reason))
            }
            Err(reason) => ("failed", None, Some(*reason)),
        };
        self.state = Some(state);
        self.reason = reason;
        if let Some(latest) = latest {
            self.latest = Some(latest.clone());
        }
    }
}

#[cfg(debug_assertions)]
pub(super) type FixtureRelease = app_update::Result<Vec<u8>>;

#[cfg(debug_assertions)]
#[derive(Clone)]
struct CheckFixture {
    home: PathBuf,
    installed: PathBuf,
    requests: mpsc::Sender<mpsc::Sender<FixtureRelease>>,
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Checking {
        manual: bool,
        receiver: Receiver<app_update::Result<Check>>,
        previous: Option<Cached>,
    },
    Offered(Offer),
    Downloading {
        receiver: Receiver<app_update::Result<Staged>>,
        offer: Offer,
        progress: DownloadProgress,
        next_refresh: Instant,
    },
    Ready(Staged, Option<&'static str>, DownloadSnapshot),
    Installing,
    Notice(Notice),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetainedResult {
    None,
    Text,
    Audio,
    TextAndAudio,
}

impl RetainedResult {
    fn when_idle(c: &Controller) -> Result<Self, &'static str> {
        match c.state.phase {
            Phase::Permissions => return Err("听写正在准备。请等准备结束或取消听写。"),
            Phase::Recording => return Err("正在录音。请先停止并转写，或取消听写。"),
            Phase::Transcribing => return Err("正在转写。请等转写完成或取消听写。"),
            Phase::Delivering => return Err("正在投递文字。请等待投递完成。"),
            Phase::Stopping => {
                return Err("听写正在结束或清理。请等待。如面板提示清理待确认，可按提示处理。");
            }
            Phase::Idle | Phase::Ready | Phase::Failed => {}
        }
        if c.source_tests.busy() {
            return Err("来源测试正在运行或清理。请等待或在来源测试中停止测试。");
        }
        if c.microphone_test_running() {
            return Err("麦克风正在测试。请在设置中停止测试或等待结束。");
        }
        if c.file_jobs.busy() {
            return Err("音频文件转写正在运行或清理。请等待当前任务结束。");
        }
        if c.busy() {
            return Err("文字投递尚未完成。请等待当前任务结束。");
        }
        if c.discovering() {
            return Err("正在检查本地模型。请等检查完成。");
        }
        let text = !c.transcript.trim().is_empty()
            || c.raw_transcript
                .as_ref()
                .is_some_and(|text| !text.trim().is_empty());
        Ok(match (text, c.has_retained_recording()) {
            (false, false) => Self::None,
            (true, false) => Self::Text,
            (false, true) => Self::Audio,
            (true, true) => Self::TextAndAudio,
        })
    }

    fn guidance(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Text => {
                Some("本次退出会清除仍保留在 VoicePort 中的文字。请先查看并保存需要保留的文字。")
            }
            Self::Audio => {
                Some("本次退出会释放失败的录音，重启后无法再重试。可以先回到听写页面重试转写。")
            }
            Self::TextAndAudio => Some(
                "本次退出会清除仍保留的文字并释放失败的录音。请先保存需要保留的文字，或重试转写。",
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InstallSnapshot {
    generation: u64,
    retained: RetainedResult,
    cli_result: bool,
}

struct Offer {
    candidate: Arc<Update>,
    refresh_error: Option<&'static str>,
    download_error: Option<FailedDownload>,
}

impl Offer {
    fn new(candidate: Update) -> Self {
        Self {
            candidate: Arc::new(candidate),
            refresh_error: None,
            download_error: None,
        }
    }
}

struct FailedDownload {
    reason: &'static str,
    progress: DownloadSnapshot,
}

enum Cached {
    Offer(Offer),
    Download(Staged, DownloadSnapshot),
}

struct Notice {
    title: &'static str,
    reason: &'static str,
    latest: Option<String>,
    show_release: bool,
}

impl Notice {
    fn failure(title: &'static str, reason: &'static str) -> Self {
        Self {
            title,
            reason,
            latest: None,
            show_release: true,
        }
    }

    fn detail(&self, current: &str) -> String {
        format!(
            "{}\n\n{}",
            versions(current, self.latest.as_deref()),
            self.reason
        )
    }
}

impl State {
    fn busy(&self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Checking { .. } => Some(("checking", "An update check is already running.")),
            Self::Downloading { .. } => Some((
                "downloading",
                "The App is downloading and verifying an update.",
            )),
            Self::Installing => Some(("installing", "The App is handling update installation.")),
            _ => None,
        }
    }

    fn checked(manual: bool, result: app_update::Result<Check>, previous: Option<Cached>) -> Self {
        match result {
            Ok(Check::Available(candidate)) => Self::Offered(Offer::new(candidate)),
            Ok(Check::Current {
                latest,
                local_newer,
            }) if manual => Self::Notice(Notice {
                title: if local_newer {
                    "当前版本较新"
                } else {
                    "已是最新版本"
                },
                reason: if local_newer {
                    "当前版本比 GitHub 的最新发行版本更新。"
                } else {
                    "当前版本与 GitHub 的最新发行版本一致。"
                },
                latest: Some(latest),
                show_release: false,
            }),
            Ok(Check::Incompatible { latest, reason }) if manual => Self::Notice(Notice {
                title: "更新包不可用",
                reason,
                latest: Some(latest),
                show_release: true,
            }),
            Err(reason) => match previous {
                Some(Cached::Offer(mut offer)) => {
                    offer.refresh_error = Some(reason);
                    Self::Offered(offer)
                }
                Some(Cached::Download(staged, progress)) => {
                    Self::Ready(staged, Some(reason), progress)
                }
                None if manual => Self::Notice(Notice::failure("无法检查更新", reason)),
                None => Self::Idle,
            },
            Ok(_) => Self::Idle,
        }
    }

    fn can_refresh(&self) -> bool {
        matches!(self, Self::Offered(..) | Self::Ready(..))
    }

    fn menu_title(&self, current: &str) -> String {
        match self {
            Self::Idle => "软件更新…".into(),
            Self::Checking { .. } => format!("正在检查更新… · {}", versions(current, None)),
            Self::Offered(offer) => match (offer.download_error.as_ref(), offer.refresh_error) {
                (Some(_), _) => format!("无法准备更新… · {}", versions(current, None)),
                (None, None) => format!(
                    "发现新版本… · {}",
                    versions(current, Some(&offer.candidate.version))
                ),
                (None, Some(_)) => format!(
                    "刷新失败… · {}",
                    cached_versions(current, &offer.candidate.version)
                ),
            },
            Self::Downloading { progress, .. } => {
                let snapshot = progress.snapshot();
                let transfer = presentation::UpdateTransfer::read(
                    snapshot,
                    !snapshot.verifying,
                    Instant::now(),
                );
                download_title(transfer, snapshot.verifying)
            }
            Self::Installing => "正在处理更新安装…".into(),
            Self::Ready(candidate, failure, _) => match failure {
                None => format!("v{} 已校验，可安装…", candidate.version),
                Some(_) => format!(
                    "已校验 · 刷新失败… · {}",
                    cached_versions(current, &candidate.version)
                ),
            },
            Self::Notice(notice) => format!(
                "{}… · {}",
                notice.title,
                versions(current, notice.latest.as_deref())
            ),
        }
    }
}

fn download_title(transfer: presentation::UpdateTransfer, verifying: bool) -> String {
    if verifying {
        return match transfer.percent {
            Some(percent) => format!("下载完成 {percent}% · 正在校验并暂存…"),
            None => "下载完成 · 正在校验并暂存…".into(),
        };
    }
    let mut title = match transfer.percent {
        Some(percent) => format!("下载中 {percent}%"),
        None if transfer.received_bytes == 0 => "正在连接下载…".into(),
        None => format!(
            "下载中 · {} · 百分比不可用",
            download_bytes(transfer.received_bytes)
        ),
    };
    if let Some(rate) = transfer.bytes_per_second {
        title.push_str(&format!(" · {}", download_speed(rate)));
    }
    title
}

fn download_speed(rate: f64) -> String {
    if rate < 1.0 {
        "<1 B/s".into()
    } else {
        format!("{}/s", download_bytes(rate as u64))
    }
}

fn download_bytes(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.1} KB", bytes as f64 / 1_000.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_000_000.0),
    }
}

fn versions(current: &str, latest: Option<&str>) -> String {
    match latest {
        Some(latest) => format!("当前 {current} · 最新 {latest}"),
        None => format!("当前 {current}"),
    }
}

fn cached_versions(current: &str, known: &str) -> String {
    format!("当前 {current} · 上次已知 {known}")
}

fn current_version() -> String {
    NSBundle::mainBundle()
        .objectForInfoDictionaryKey(ns_string!("CFBundleShortVersionString"))
        .and_then(|value| value.downcast::<NSString>().ok())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "未知".into())
}

fn location() -> (PathBuf, PathBuf) {
    (
        PathBuf::from(NSBundle::mainBundle().bundlePath().to_string()),
        PathBuf::from(objc2_foundation::NSHomeDirectory().to_string()),
    )
}

fn alert(mtm: MainThreadMarker, title: &str, detail: &str, buttons: &[&str]) -> NSModalResponse {
    NSApplication::sharedApplication(mtm).activate();
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(detail));
    for button in buttons {
        alert.addButtonWithTitle(&NSString::from_str(button));
    }
    alert.runModal()
}

fn release_page() {
    if let Some(url) =
        objc2_foundation::NSURL::URLWithString(&NSString::from_str(app_update::RELEASES_PAGE))
    {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

impl Delegate {
    fn update_message(&self, title: &str, detail: &str, show_release: bool) {
        let buttons: &[&str] = if show_release {
            &["好", "查看发布页"]
        } else {
            &["好"]
        };
        if alert(self.mtm(), title, detail, buttons) == NSAlertSecondButtonReturn && show_release {
            release_page();
        }
    }

    fn refresh_update_views(&self) {
        self.update_menu();
        let update = self.ivars().update.borrow();
        if let Some(page) = update.page.as_ref().filter(|page| page.is_visible()) {
            page.render(&update.view_snapshot(self.ivars().quitting.get(), Instant::now()));
        }
    }

    pub(super) fn refresh_opened_update_page(&self) {
        let mut update = self.ivars().update.borrow_mut();
        let visible = update.page.as_ref().is_some_and(page::Page::is_visible);
        let was_visible = std::mem::replace(&mut update.page_was_visible, visible);
        if visible
            && !was_visible
            && let Some(page) = &update.page
        {
            page.render(&update.view_snapshot(self.ivars().quitting.get(), Instant::now()));
        }
    }

    fn update_menu(&self) {
        if self.ivars().context_menu_visible.get() {
            self.render_update_menu(false);
        }
    }

    pub(super) fn prepare_update_menu(&self) {
        self.render_update_menu(true);
    }

    /// Read-only compact caption shared with the native update menu.
    pub(super) fn compact_update_title(&self) -> String {
        let snapshot = self.update_view_snapshot();
        if let Some(transfer) = snapshot.transfer
            && matches!(
                snapshot.phase,
                UpdateViewPhase::Connecting
                    | UpdateViewPhase::Downloading
                    | UpdateViewPhase::Verifying
            )
        {
            return download_title(transfer, snapshot.phase == UpdateViewPhase::Verifying);
        }
        let update = self.ivars().update.borrow();
        match &update.state {
            State::Idle => format!("当前版本 {}", snapshot.current_version),
            state => state.menu_title(&snapshot.current_version),
        }
    }

    fn render_update_menu(&self, opening: bool) {
        let update = self.ivars().update.borrow();
        if let Some(item) = self.ivars().update_item.get() {
            let current = update.running_version();
            let title = NSString::from_str(&update.state.menu_title(&current));
            if !item.title().isEqualToString(&title) {
                item.setTitle(&title);
            }
            let detail = match &update.state {
                State::Notice(notice) => Some(NSString::from_str(&notice.detail(&current))),
                _ => None,
            };
            if item.toolTip() != detail {
                item.setToolTip(detail.as_deref());
            }
            let enabled = !self.ivars().quitting.get();
            if item.isEnabled() != enabled {
                item.setEnabled(enabled);
            }
        }
        if let Some(item) = self.ivars().update_refresh_item.get() {
            let enabled = update.state.can_refresh();
            // Keep the row positions stable while AppKit tracks a selection.
            if opening && item.isHidden() == enabled {
                item.setHidden(!enabled);
            }
            if item.isEnabled() != enabled {
                item.setEnabled(enabled);
            }
        }
        if let Some(item) = self.ivars().update_auto_item.get() {
            let state = status_overlay::checked(update.automatic);
            if item.state() != state {
                item.setState(state);
            }
        }
    }

    pub(super) fn toggle_auto_update(&self) {
        let (installed, home) = location();
        if !app_update::checkable_installation(&installed, &home) {
            self.update_message(
                "当前副本不能自动更新",
                "请使用应用程序目录中的 VoicePort.app。",
                true,
            );
            return;
        }
        let mut update = self.ivars().update.borrow_mut();
        update.automatic = !update.automatic;
        NSUserDefaults::standardUserDefaults()
            .setBool_forKey(update.automatic, &NSString::from_str(AUTO_UPDATE_PREF));
        update.next = update.automatic.then(Instant::now);
        drop(update);
        self.update_menu();
    }

    pub(super) fn start_update_check(&self, manual: bool) {
        if let Err(("unsupported", reason)) = self.request_update_check(manual)
            && manual
        {
            let notice = Notice::failure("无法检查更新", reason);
            self.update_message(
                notice.title,
                &notice.detail(&current_version()),
                notice.show_release,
            );
        }
    }

    pub(super) fn control_update_check(&self) -> Response {
        match self.request_update_check(true) {
            Ok(()) => Response::ok(self.update_snapshot()),
            Err((code, reason)) => {
                let mut response = Response::error(code, reason);
                response.data = self.update_snapshot();
                response
            }
        }
    }

    pub(super) fn update_snapshot(&self) -> serde_json::Value {
        let update = self.ivars().update.borrow();
        let quitting = self.ivars().quitting.get();
        let active = update.state.busy().map(|(state, _)| state);
        let last = &update.last_check;
        let state = if quitting {
            "quitting"
        } else if let Some(active) = active {
            active
        } else if matches!(update.state, State::Ready(..)) {
            "ready_to_install"
        } else {
            last.state.unwrap_or("idle")
        };
        serde_json::json!({
            "state": state,
            "busy": quitting || active.is_some(),
            "current_version": update.running_version(),
            "latest_version": last.latest,
            "latest_version_stale": update.latest_version_stale(),
            "last_check_state": last.state,
            "reason": last.reason,
        })
    }

    fn request_update_check(&self, manual: bool) -> Result<(), (&'static str, &'static str)> {
        let mut update = self.ivars().update.borrow_mut();
        if self.ivars().quitting.get() {
            return Err(("busy", "The App is quitting."));
        }
        if let Some((_, reason)) = update.state.busy() {
            return Err(("busy", reason));
        }
        update.next = Some(Instant::now() + CHECK_INTERVAL);
        #[cfg(debug_assertions)]
        let fixture = update.fixture.clone();
        #[cfg(debug_assertions)]
        let (installed, home) = fixture.as_ref().map_or_else(location, |fixture| {
            (fixture.installed.clone(), fixture.home.clone())
        });
        #[cfg(not(debug_assertions))]
        let (installed, home) = location();
        if !app_update::checkable_installation(&installed, &home) {
            let reason = "请使用应用程序目录中的 VoicePort.app。";
            update.last_check.record(&Err(reason));
            drop(update);
            self.refresh_update_views();
            return Err(("unsupported", reason));
        }
        let running = update.running_version();
        #[cfg(debug_assertions)]
        let fixture_seconds = if super::menu_accept_fixture::is_active() { 45 } else { 8 };
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("voiceport-update-check".into())
            .spawn(move || {
                #[cfg(debug_assertions)]
                if let Some(fixture) = fixture {
                    let (reply, release) = mpsc::channel();
                    let result = fixture
                        .requests
                        .send(reply)
                        .ok()
                        .and_then(|()| release.recv_timeout(Duration::from_secs(fixture_seconds)).ok());
                    let _ = sender.send(app_update::check_fixture_release(
                        &running,
                        result.unwrap_or(Err("检查更新已中断。")),
                    ));
                    return;
                }
                let _ = sender.send(app_update::check(&home, &running));
            });
        let previous = match std::mem::take(&mut update.state) {
            State::Offered(offer) => Some(Cached::Offer(offer)),
            State::Ready(staged, _, progress) => Some(Cached::Download(staged, progress)),
            _ => None,
        };
        let result = if worker.is_ok() {
            update.state = State::Checking {
                manual,
                receiver,
                previous,
            };
            Ok(())
        } else {
            let reason = "无法启动后台检查，请稍后重试。";
            update.last_check.record(&Err(reason));
            update.state = State::checked(manual, Err(reason), previous);
            Err(("update_check_failed", reason))
        };
        drop(update);
        self.refresh_update_views();
        result
    }

    #[cfg(debug_assertions)]
    pub(super) fn configure_update_fixture(
        &self,
        home: PathBuf,
        installed: PathBuf,
        requests: mpsc::Sender<mpsc::Sender<FixtureRelease>>,
    ) {
        let mut update = self.ivars().update.borrow_mut();
        update.initialized = true;
        update.fixture = Some(CheckFixture {
            home,
            installed,
            requests,
        });
    }

    fn offer_update(&self) {
        let (candidate, mut detail, retry, refresh_failed) = {
            let update = self.ivars().update.borrow();
            let State::Offered(offer) = &update.state else {
                return;
            };
            let current = update.running_version();
            let version = &offer.candidate.version;
            let mut detail = versions(&current, Some(version));
            if let Some(reason) = offer.refresh_error {
                detail = format!("{}\n{reason}", cached_versions(&current, version));
            }
            (
                Arc::clone(&offer.candidate),
                detail,
                offer.download_error.as_ref().map(|failure| failure.reason),
                offer.refresh_error.is_some(),
            )
        };
        let (title, action) = if let Some(reason) = retry {
            detail.push_str("\n\n");
            detail.push_str(reason);
            ("无法准备更新", "重试下载")
        } else if refresh_failed {
            ("无法刷新 VoicePort 版本", "下载并校验")
        } else {
            ("发现 VoicePort 新版本", "下载并校验")
        };
        if !app_update::can_install_in_app(&location().0) {
            self.update_message(
                title,
                &format!("{detail}\n\n{}", app_update::MANUAL_ONLY),
                true,
            );
            return;
        }
        match alert(
            self.mtm(),
            title,
            &format!(
                "{detail}\n\n下载和校验时可以继续使用 VoicePort。校验完成后，应用会等你确认安装并重启。"
            ),
            &[action, "查看发布页", "稍后"],
        ) {
            choice if choice == NSAlertFirstButtonReturn => self.download_update(&candidate),
            choice if choice == NSAlertSecondButtonReturn => release_page(),
            _ => {}
        }
    }

    fn download_update(&self, confirmed: &Arc<Update>) {
        let (installed, home) = location();
        if !self.begin_update_download(confirmed, move |candidate, progress| {
            app_update::stage(&home, &installed, candidate, progress)
        }) {
            self.update_message("更新信息已变化", "请查看当前的软件更新状态。", false);
        }
    }

    fn begin_update_download(
        &self,
        confirmed: &Arc<Update>,
        stage: impl FnOnce(&Update, &DownloadProgress) -> app_update::Result<Staged> + Send + 'static,
    ) -> bool {
        if self.ivars().quitting.get() {
            return false;
        }
        let mut update = self.ivars().update.borrow_mut();
        let mut offer = match std::mem::take(&mut update.state) {
            State::Offered(offer) if Arc::ptr_eq(&offer.candidate, confirmed) => offer,
            state => {
                update.state = state;
                return false;
            }
        };
        let (sender, receiver) = mpsc::channel();
        let progress = DownloadProgress::default();
        let worker_progress = progress.clone();
        let candidate = Arc::clone(&offer.candidate);
        let started = std::thread::Builder::new()
            .name("voiceport-update-download".into())
            .spawn(move || {
                let _ = sender.send(stage(&candidate, &worker_progress));
            })
            .is_ok();
        offer.download_error = None;
        update.state = if started {
            State::Downloading {
                receiver,
                offer,
                progress,
                next_refresh: Instant::now(),
            }
        } else {
            offer.download_error = Some(FailedDownload {
                reason: "无法启动后台下载，请稍后重试。",
                progress: progress.snapshot(),
            });
            State::Offered(offer)
        };
        drop(update);
        self.refresh_update_views();
        true
    }

    fn update_install_snapshot(&self) -> Result<InstallSnapshot, &'static str> {
        let ivars = self.ivars();
        if ivars.quitting.get() {
            return Err("VoicePort 正在退出。请重新打开应用后再安装。");
        }
        if self.settings_locked() {
            return Err("请先完成或关闭来源、模型、快捷键或听写设置中的操作。");
        }
        if !ivars
            .permission_rx
            .try_borrow()
            .is_ok_and(|rx| rx.is_none())
        {
            return Err("请先完成权限检查。");
        }
        if !ivars
            .pending_hotkey
            .try_borrow()
            .is_ok_and(|key| key.is_none())
        {
            return Err("快捷键听写正在准备。请松开快捷键，等待准备结束或取消听写。");
        }
        let slot = ivars
            .controller
            .try_borrow()
            .map_err(|_| "听写状态正在变化，请稍候。")?;
        let c = slot.as_ref().ok_or("VoicePort 尚未就绪，请稍候。")?;
        Ok(InstallSnapshot {
            generation: c.state.generation(),
            retained: RetainedResult::when_idle(c)?,
            cli_result: self.retained_cli_result_for_update()?,
        })
    }

    fn update_install_blocked(&self, reason: &str) {
        self.update_message(
            "目前不能重启",
            &format!("{reason}\n\n更新包仍已准备好。请处理后再次选择安装。"),
            false,
        );
    }

    fn confirm_update_install(&self, version: &str, failure: Option<&str>) -> bool {
        let before = match self.update_install_snapshot() {
            Ok(snapshot) => snapshot,
            Err(reason) => {
                self.update_install_blocked(reason);
                return false;
            }
        };
        let mut detail = "VoicePort 将退出并重新打开。会保留模型、设置和旧应用包。".to_owned();
        if let Some(reason) = failure {
            detail = format!(
                "{}\n刷新失败：{reason}\n\n{detail}",
                cached_versions(&current_version(), version)
            );
        }
        let guidance = before.retained.guidance();
        if let Some(guidance) = guidance {
            detail.push_str("\n\n");
            detail.push_str(guidance);
        }
        if before.cli_result {
            detail.push_str("\n\n退出会清除命令行转写结果。请先用原操作编号运行 voiceport dictation result 保存文字。");
        }
        let (buttons, confirm): (&[&str], _) = if guidance.is_some() {
            (
                &["查看听写", "稍后", "放弃并安装"],
                NSAlertThirdButtonReturn,
            )
        } else if before.cli_result {
            (&["稍后", "放弃并安装"], NSAlertSecondButtonReturn)
        } else {
            (&["安装并重启", "稍后"], NSAlertFirstButtonReturn)
        };
        let choice = alert(
            self.mtm(),
            &format!("安装 VoicePort v{version}？"),
            &detail,
            buttons,
        );
        if guidance.is_some() && choice == NSAlertFirstButtonReturn {
            self.show_workbench_page(workbench::Page::Result);
        }
        if choice != confirm {
            return false;
        }
        // A native modal loop can run a hotkey, CLI or settings action.
        // Never apply an earlier confirmation to a different dictation result.
        match self.update_install_snapshot() {
            Ok(after) if after == before => true,
            Ok(_) => {
                self.update_install_blocked("听写或保留内容已变化。请重新查看并确认。");
                false
            }
            Err(reason) => {
                self.update_install_blocked(reason);
                false
            }
        }
    }

    fn take_staged_for_install(&self) -> Option<(Staged, Option<&'static str>, DownloadSnapshot)> {
        let mut update = self.ivars().update.borrow_mut();
        if self.ivars().quitting.get() || !matches!(update.state, State::Ready(..)) {
            return None;
        }
        let State::Ready(staged, failure, progress) =
            std::mem::replace(&mut update.state, State::Installing)
        else {
            unreachable!()
        };
        Some((staged, failure, progress))
    }

    fn install_staged_update(&self) {
        let Some((mut staged, failure, progress)) = self.take_staged_for_install() else {
            return;
        };
        self.refresh_update_views();
        if self.confirm_update_install(&staged.version, failure) {
            let (installed, _) = location();
            match staged.handoff(Path::new(&installed)) {
                Ok(commit) => {
                    *self.ivars().update_commit.borrow_mut() = Some(commit);
                    NSApplication::sharedApplication(self.mtm()).terminate(None);
                    return;
                }
                Err(error) => self.update_message(
                    "无法安装更新",
                    &format!("{error}\n\n可以重试安装或重新检查更新。"),
                    true,
                ),
            }
        }
        self.ivars().update.borrow_mut().state = State::Ready(staged, failure, progress);
        self.refresh_update_views();
    }

    pub(super) fn update_tick(&self) {
        if self.ivars().quitting.get() {
            return;
        }
        enum Completion {
            Check(bool, app_update::Result<Check>),
            Stage(app_update::Result<Staged>),
        }
        let mut update = self.ivars().update.borrow_mut();
        if !update.initialized {
            update.initialized = true;
            update.automatic = NSUserDefaults::standardUserDefaults()
                .boolForKey(&NSString::from_str(AUTO_UPDATE_PREF));
            update
                .next
                .get_or_insert_with(|| Instant::now() + FIRST_CHECK);
            drop(update);
            self.refresh_update_views();
            return;
        }
        let automatic_due =
            update.automatic && update.next.is_some_and(|next| next <= Instant::now());
        let mut refresh_download = false;
        let finished = match &mut update.state {
            State::Checking {
                manual, receiver, ..
            } => match receiver.try_recv() {
                Ok(result) => Some(Completion::Check(*manual, result)),
                Err(TryRecvError::Disconnected) => {
                    Some(Completion::Check(*manual, Err("检查更新已中断。")))
                }
                Err(TryRecvError::Empty) => None,
            },
            State::Downloading {
                receiver,
                next_refresh,
                ..
            } => match receiver.try_recv() {
                Ok(result) => Some(Completion::Stage(result)),
                Err(TryRecvError::Disconnected) => Some(Completion::Stage(Err("下载更新已中断。"))),
                Err(TryRecvError::Empty) => {
                    let now = Instant::now();
                    if now >= *next_refresh {
                        *next_refresh = now + DOWNLOAD_REFRESH;
                        refresh_download = true;
                    }
                    None
                }
            },
            State::Idle | State::Offered(..) | State::Notice(..) if automatic_due => {
                drop(update);
                self.start_update_check(false);
                return;
            }
            _ => None,
        };
        let Some(finished) = finished else {
            drop(update);
            if refresh_download {
                self.refresh_update_views();
            }
            return;
        };
        // The timer drives dictation and IPC as well. Publish state here and
        // keep dialogs on explicit user actions.
        let previous = std::mem::take(&mut update.state);
        update.state = match (finished, previous) {
            (Completion::Check(manual, result), State::Checking { previous, .. }) => {
                update.last_check.record(&result);
                State::checked(manual, result, previous)
            }
            (
                Completion::Stage(result),
                State::Downloading {
                    mut offer,
                    progress,
                    ..
                },
            ) => {
                let progress = progress.snapshot();
                match result {
                    Ok(staged) => State::Ready(staged, offer.refresh_error, progress),
                    Err(reason) => {
                        offer.download_error = Some(FailedDownload { reason, progress });
                        State::Offered(offer)
                    }
                }
            }
            _ => unreachable!("update completion must match its active worker"),
        };
        drop(update);
        self.refresh_update_views();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_checks_live_tasks_and_actual_retained_text_instead_of_terminal_phase() {
        let mut c = Controller::new();
        for phase in [Phase::Idle, Phase::Ready, Phase::Failed] {
            c.state.phase = phase;
            c.transcript.clear();
            c.raw_transcript = None;
            assert_eq!(RetainedResult::when_idle(&c), Ok(RetainedResult::None));
            c.transcript = "synthetic result".into();
            assert_eq!(RetainedResult::when_idle(&c), Ok(RetainedResult::Text));
            c.transcript.clear();
            c.raw_transcript = Some("synthetic original".into());
            assert_eq!(RetainedResult::when_idle(&c), Ok(RetainedResult::Text));
        }
        for phase in [
            Phase::Permissions,
            Phase::Recording,
            Phase::Transcribing,
            Phase::Delivering,
            Phase::Stopping,
        ] {
            c.state.phase = phase;
            assert!(RetainedResult::when_idle(&c).is_err(), "{phase:?}");
        }
    }

    #[test]
    fn menu_reports_the_current_version_while_checking_and_on_failure() {
        let (_, receiver) = mpsc::channel();
        let checking = State::Checking {
            manual: true,
            previous: None,
            receiver,
        };
        assert_eq!(checking.menu_title("0.3.13"), "正在检查更新… · 当前 0.3.13");
        let failed = State::checked(true, Err("无法连接 GitHub。"), None);
        assert_eq!(failed.menu_title("0.3.13"), "无法检查更新… · 当前 0.3.13");
        let State::Notice(notice) = failed else {
            panic!("expected a query failure notice")
        };
        assert_eq!(notice.detail("0.3.13"), "当前 0.3.13\n\n无法连接 GitHub。");
        assert!(matches!(
            State::checked(false, Err("无法连接 GitHub。"), None),
            State::Idle
        ));
    }

    #[test]
    fn ready_download_remains_refreshable_but_is_not_latest_after_failure() {
        let ready = State::Ready(Staged::fixture("0.3.14"), None, DownloadSnapshot::default());
        assert!(ready.can_refresh());
        assert_eq!(ready.busy(), None);
        assert_eq!(ready.menu_title("0.3.13"), "v0.3.14 已校验，可安装…");
        let State::Ready(ready, _, progress) = ready else {
            unreachable!()
        };
        let state = State::checked(
            true,
            Err("无法连接 GitHub。"),
            Some(Cached::Download(ready, progress)),
        );
        assert!(state.can_refresh());
        assert_eq!(
            state.menu_title("0.3.13"),
            "已校验 · 刷新失败… · 当前 0.3.13 · 上次已知 0.3.14"
        );
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn offered_update_remains_refreshable_but_is_not_latest_after_failure() {
        for manual in [true, false] {
            let candidate = app_update::fixture_release_check("0.3.13");
            let State::Offered(offer) = State::checked(manual, candidate, None) else {
                panic!("expected offer")
            };
            let state =
                State::checked(manual, Err("无法连接 GitHub。"), Some(Cached::Offer(offer)));
            assert!(state.can_refresh());
            assert_eq!(
                state.menu_title("0.3.13"),
                "刷新失败… · 当前 0.3.13 · 上次已知 0.3.14"
            );
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn menu_uses_the_queried_release_in_manual_and_automatic_results() {
        for manual in [true, false] {
            let offer = State::checked(manual, app_update::fixture_release_check("0.3.13"), None);
            assert_eq!(
                offer.menu_title("0.3.13"),
                "发现新版本… · 当前 0.3.13 · 最新 0.3.14"
            );
        }
        let same = State::checked(true, app_update::fixture_release_check("0.3.14"), None);
        assert_eq!(
            same.menu_title("0.3.14"),
            "已是最新版本… · 当前 0.3.14 · 最新 0.3.14"
        );
        let newer = State::checked(true, app_update::fixture_release_check("0.3.15"), None);
        assert_eq!(
            newer.menu_title("0.3.15"),
            "当前版本较新… · 当前 0.3.15 · 最新 0.3.14"
        );
        let unavailable = State::checked(
            true,
            Ok(Check::Incompatible {
                latest: "0.3.14".into(),
                reason: "此版本缺少 macOS arm64 更新包。",
            }),
            None,
        );
        assert_eq!(
            unavailable.menu_title("0.3.13"),
            "更新包不可用… · 当前 0.3.13 · 最新 0.3.14"
        );
    }
}
