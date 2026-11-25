//! Native menu states for checking, downloading and restarting, without modal background checks.
use super::*;
use crate::app_update::{self, Check, Staged, Update};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

const AUTO_UPDATE_PREF: &str = "automaticallyCheckForUpdates";
const FIRST_CHECK: Duration = Duration::from_secs(15);
const CHECK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);

#[derive(Default)]
pub(super) struct UpdateUi {
    initialized: bool,
    automatic: bool,
    next: Option<Instant>,
    state: State,
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Checking {
        manual: bool,
        receiver: Receiver<app_update::Result<Check>>,
        previous: Option<Update>,
    },
    Offered(Update),
    Downloading(Receiver<app_update::Result<Staged>>),
    Ready(Staged),
    Notice(Notice),
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
    fn checked(manual: bool, result: app_update::Result<Check>, previous: Option<Update>) -> Self {
        match result {
            Ok(Check::Available(candidate)) => Self::Offered(candidate),
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
            Err(reason) if manual => Self::Notice(Notice::failure("无法检查更新", reason)),
            Err(_) => previous.map_or(Self::Idle, Self::Offered),
            Ok(_) => Self::Idle,
        }
    }

    fn menu_title(&self, current: &str) -> String {
        match self {
            Self::Idle => "检查更新…".into(),
            Self::Checking { .. } => format!("正在检查更新… · {}", versions(current, None)),
            Self::Offered(candidate) => format!(
                "发现新版本… · {}",
                versions(current, Some(&candidate.version))
            ),
            Self::Downloading(_) => "正在下载并校验更新…".into(),
            Self::Ready(candidate) => format!("安装 v{} 并重启…", candidate.version),
            Self::Notice(notice) => format!(
                "{}… · {}",
                notice.title,
                versions(current, notice.latest.as_deref())
            ),
        }
    }
}

fn versions(current: &str, latest: Option<&str>) -> String {
    match latest {
        Some(latest) => format!("当前 {current} · 最新 {latest}"),
        None => format!("当前 {current}"),
    }
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

    pub(super) fn update_menu(&self) {
        let update = self.ivars().update.borrow();
        if let Some(item) = self.ivars().update_item.get() {
            let current = current_version();
            item.setTitle(&NSString::from_str(&update.state.menu_title(&current)));
            let detail = match &update.state {
                State::Notice(notice) => Some(NSString::from_str(&notice.detail(&current))),
                _ => None,
            };
            item.setToolTip(detail.as_deref());
            item.setEnabled(!matches!(
                update.state,
                State::Checking { .. } | State::Downloading(_)
            ));
        }
        if let Some(item) = self.ivars().update_auto_item.get() {
            item.setState(if update.automatic {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
    }

    pub(super) fn toggle_auto_update(&self) {
        let (installed, home) = location();
        if !app_update::installed_path(&installed, &home) {
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

    fn start_update_check(&self, manual: bool) {
        let (installed, home) = location();
        let mut update = self.ivars().update.borrow_mut();
        update.next = Some(Instant::now() + CHECK_INTERVAL);
        if !app_update::installed_path(&installed, &home) {
            drop(update);
            if manual {
                let notice =
                    Notice::failure("无法检查更新", "请使用应用程序目录中的 VoicePort.app。");
                self.update_message(
                    notice.title,
                    &notice.detail(&current_version()),
                    notice.show_release,
                );
            }
            return;
        }
        let running = current_version();
        let (sender, receiver) = mpsc::channel();
        if std::thread::Builder::new()
            .name("voiceport-update-check".into())
            .spawn(move || {
                let _ = sender.send(app_update::check(&home, &running));
            })
            .is_ok()
        {
            let previous = match std::mem::take(&mut update.state) {
                State::Offered(candidate) => Some(candidate),
                _ => None,
            };
            update.state = State::Checking {
                manual,
                receiver,
                previous,
            };
        } else if manual {
            update.state = State::Notice(Notice::failure(
                "无法检查更新",
                "无法启动后台检查，请稍后重试。",
            ));
        }
        drop(update);
        self.update_menu();
    }

    fn offer_update(&self) {
        let version = match &self.ivars().update.borrow().state {
            State::Offered(candidate) => candidate.version.clone(),
            _ => return,
        };
        let versions = versions(&current_version(), Some(&version));
        match alert(
            self.mtm(),
            "发现 VoicePort 新版本",
            &format!(
                "{versions}\n\n下载并校验时可以继续使用 VoicePort。下载完成后，应用会等你确认安装并重启。"
            ),
            &["下载并校验", "查看发布页", "稍后"],
        ) {
            choice if choice == NSAlertFirstButtonReturn => self.download_update(&version),
            choice if choice == NSAlertSecondButtonReturn => release_page(),
            _ => {}
        }
    }

    fn download_update(&self, version: &str) {
        let (installed, home) = location();
        let mut update = self.ivars().update.borrow_mut();
        let candidate = match std::mem::take(&mut update.state) {
            State::Offered(candidate) if candidate.version == version => candidate,
            state => {
                update.state = state;
                drop(update);
                self.update_message("更新信息已变化", "请从菜单查看最新的更新状态。", false);
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("voiceport-update-download".into())
            .spawn(move || {
                let _ = sender.send(app_update::stage(&home, &installed, candidate));
            })
            .is_ok();
        update.state = if started {
            State::Downloading(receiver)
        } else {
            State::Notice(Notice::failure(
                "无法下载更新",
                "无法启动后台下载，请稍后重试。",
            ))
        };
        drop(update);
        self.update_menu();
    }

    fn may_install_update(&self) -> bool {
        let ivars = self.ivars();
        !ivars.quitting.get()
            && !self.settings_locked()
            && ivars.permission_rx.borrow().is_none()
            && ivars.pending_hotkey.borrow().is_none()
            && self.control_quiet_for_update()
            && ivars.controller.try_borrow().is_ok_and(|c| {
                c.as_ref()
                    .is_some_and(|c| c.state.phase == Phase::Idle && !c.busy() && !c.discovering())
            })
    }

    fn install_update(&self) {
        let version = match &self.ivars().update.borrow().state {
            State::Ready(candidate) => candidate.version.clone(),
            _ => return,
        };
        if !self.may_install_update() {
            self.update_message("目前不能重启", "请等听写和其他任务结束；先保存需要保留的转写文字，再清除结果或处理失败的录音。更新包仍已准备好。", false);
            return;
        }
        if alert(
            self.mtm(),
            &format!("安装 VoicePort v{version}？"),
            "VoicePort 将正常退出并重新打开。安装不修改模型、设置或转写数据；旧应用包会保留。",
            &["安装并重启", "稍后"],
        ) != NSAlertFirstButtonReturn
        {
            return;
        }
        // AppKit's modal loop runs ticks. A hotkey, CLI request or settings action
        // may have started since opening the confirmation. Never terminate it.
        if !self.may_install_update() {
            self.update_message(
                "VoicePort 已有未完成任务",
                "本次未安装。请在空闲后重新选择「安装并重启」。",
                false,
            );
            return;
        }
        let (installed, _) = location();
        let State::Ready(staged) = std::mem::take(&mut self.ivars().update.borrow_mut().state)
        else {
            return;
        };
        self.update_menu();
        match staged.handoff(Path::new(&installed)) {
            Ok(commit) => *self.ivars().update_commit.borrow_mut() = Some(commit),
            Err(error) => {
                self.update_message("无法安装更新", error, true);
                return;
            }
        }
        NSApplication::sharedApplication(self.mtm()).terminate(None);
    }

    pub(super) fn update_action(&self) {
        let mut update = self.ivars().update.borrow_mut();
        match update.state {
            State::Idle => {
                drop(update);
                self.start_update_check(true);
            }
            State::Offered(_) => {
                drop(update);
                self.offer_update();
            }
            State::Ready(_) => {
                drop(update);
                self.install_update();
            }
            State::Notice(_) => {
                let State::Notice(notice) = std::mem::take(&mut update.state) else {
                    return;
                };
                drop(update);
                self.update_menu();
                self.update_message(
                    notice.title,
                    &notice.detail(&current_version()),
                    notice.show_release,
                );
            }
            _ => {}
        }
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
            update.next = Some(Instant::now() + FIRST_CHECK);
            drop(update);
            self.update_menu();
            return;
        }
        let finished = match &update.state {
            State::Checking {
                manual, receiver, ..
            } => match receiver.try_recv() {
                Ok(result) => Some(Completion::Check(*manual, result)),
                Err(TryRecvError::Disconnected) => {
                    Some(Completion::Check(*manual, Err("检查更新已中断。")))
                }
                Err(TryRecvError::Empty) => None,
            },
            State::Downloading(receiver) => match receiver.try_recv() {
                Ok(result) => Some(Completion::Stage(result)),
                Err(TryRecvError::Disconnected) => Some(Completion::Stage(Err("下载更新已中断。"))),
                Err(TryRecvError::Empty) => None,
            },
            State::Idle | State::Offered(_) | State::Notice(..)
                if update.automatic && update.next.is_some_and(|next| next <= Instant::now()) =>
            {
                drop(update);
                self.start_update_check(false);
                return;
            }
            _ => None,
        };
        let Some(finished) = finished else {
            return;
        };
        let previous = match std::mem::take(&mut update.state) {
            State::Checking { previous, .. } => previous,
            _ => None,
        };
        // The same timer drives dictation and IPC. Publish results in the menu
        // and open dialogs only from explicit menu actions, outside this callback.
        update.state = match finished {
            Completion::Check(manual, result) => State::checked(manual, result, previous),
            Completion::Stage(Ok(staged)) => State::Ready(staged),
            Completion::Stage(Err(error)) => State::Notice(Notice::failure("无法准备更新", error)),
        };
        drop(update);
        self.update_menu();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
