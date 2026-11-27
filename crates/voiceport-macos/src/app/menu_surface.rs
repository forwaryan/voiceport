//! In-memory status for both menu-bar surfaces. Device discovery has its own
//! read-only worker so an idle menu query does not reserve the recording lane.
use super::*;
use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub appearance: StatusAppearance,
    pub state: &'static str,
    pub shortcut: String,
    shortcut_instruction: &'static str,
    hotkey_failed: bool,
    source: String,
    pub toggle_title: &'static str,
    pub can_toggle: bool,
    pub can_cancel: bool,
    can_copy: bool,
    repaste_registered: bool,
    output_notice: Option<&'static str>,
    pub idle: bool,
}

impl Snapshot {
    pub(super) fn read(d: &Delegate, c: &Controller, trusted: bool) -> Self {
        let iv = d.ivars();
        let preparing = iv.permission_rx.borrow().is_some();
        let returning = iv.pending_hotkey.borrow().is_some();
        let locked = d.settings_locked() || iv.quitting.get();
        let hotkey_failed = iv.hotkey_error.borrow().is_some();
        let opening = c.opening_microphone();
        let recording = c.state.phase == Phase::Recording;
        let busy = c.busy();
        let release_notice = c.audio_release_notice();
        let capture_stage = c.capture_stage();
        use StatusAppearance::{Attention, Ready, Recording, Working};
        let (appearance, state) = if let Some(notice) = release_notice {
            (Working, notice)
        } else if let Some(stage) = capture_stage {
            let appearance = if recording { Recording } else { Working };
            (appearance, stage.title())
        } else {
            match c.state.phase {
                Phase::Recording => (Recording, "正在听写"),
                Phase::Permissions if opening => (Working, "正在打开麦克风"),
                Phase::Permissions => (Working, "正在准备权限"),
                Phase::Transcribing if c.finalizing_capture() => (Working, "正在停止麦克风"),
                Phase::Transcribing => (Working, "正在转写"),
                Phase::Delivering => (Working, "正在输入文字"),
                Phase::Stopping => (Working, "正在停止"),
                _ if c.file_jobs.busy() => (Working, "正在处理音频文件"),
                _ if c.source_tests.busy() => (Working, "正在测试来源"),
                _ if preparing => (Working, "正在准备权限"),
                _ if returning => (Working, "正在返回输入位置"),
                Phase::Ready
                    if completed_delivery_failure(
                        c.state.phase,
                        !c.transcript.is_empty(),
                        c.delivery_failure_stage(),
                    )
                    .is_some() =>
                {
                    (Attention, "转写完成 · 自动输入未完成")
                }
                _ if hotkey_failed => (Attention, "快捷键需要设置"),
                _ if c.automatic && !trusted => (Attention, "需要辅助功能授权"),
                Phase::Failed => (Attention, "需要处理 · 打开查看"),
                Phase::Ready => (Ready, "转写完成"),
                Phase::Idle => (Ready, "就绪"),
            }
        };
        let controls_locked = preparing || returning || locked;
        Self {
            appearance,
            state,
            shortcut: iv.shortcut_binding.get().label(),
            shortcut_instruction: iv.shortcut_mode.get().instruction(),
            hotkey_failed,
            source: quick_panel::selected_title(c).into(),
            toggle_title: if opening {
                "取消麦克风准备"
            } else if recording {
                "停止并转写"
            } else {
                "开始听写"
            },
            can_toggle: !controls_locked
                && (opening || recording || (!busy && release_notice.is_none())),
            can_cancel: !iv.quitting.get() && ((busy && !c.file_jobs.busy()) || returning),
            can_copy: !controls_locked && !busy && c.has_latest_result(),
            repaste_registered: iv.repaste_hotkey.borrow().is_some(),
            output_notice: c.latest_output_notice(),
            idle: !controls_locked && !busy,
        }
    }
}

impl Delegate {
    pub(super) fn refresh_menu_snapshot(&self, c: &Controller, trusted: bool) -> Snapshot {
        let next = Snapshot::read(self, c, trusted);
        let mut cached = self.ivars().menu_snapshot.borrow_mut();
        if cached.as_ref() != Some(&next) {
            self.publish_menu_snapshot(cached.as_ref(), &next);
            *cached = Some(next.clone());
        }
        next
    }

    pub(super) fn prepare_context_menu(&self) {
        if let Some(snapshot) = self.ivars().menu_snapshot.borrow().as_ref() {
            self.apply_context_menu(snapshot);
        }
        self.apply_overlay_menu();
        self.prepare_update_menu();
    }

    pub(super) fn apply_overlay_menu(&self) {
        if let Some(item) = self.ivars().overlay_menu.get() {
            let state = status_overlay::checked(status_overlay::enabled());
            if item.state() != state {
                item.setState(state);
            }
        }
    }

    fn publish_menu_snapshot(&self, previous: Option<&Snapshot>, next: &Snapshot) {
        let iv = self.ivars();
        if let Some(button) = iv.status.get().and_then(|status| status.button(self.mtm())) {
            self.update_status_icon(&button, next.appearance);
            if previous.is_none_or(|old| old.state != next.state || old.source != next.source) {
                button.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                    "VoicePort · {} · {}",
                    next.source, next.state
                ))));
            }
            if previous.is_none_or(|old| {
                old.state != next.state
                    || old.source != next.source
                    || old.shortcut != next.shortcut
                    || old.shortcut_instruction != next.shortcut_instruction
            }) {
                button.setToolTip(Some(&NSString::from_str(&format!(
                    "VoicePort · {}\n当前转写：{}\n{} {}\n点击快速切换 · 右键更多操作",
                    next.state, next.source, next.shortcut, next.shortcut_instruction
                ))));
            }
        }
        if iv.context_menu_visible.get() {
            self.apply_context_menu(next);
        }
    }

    pub(super) fn apply_context_menu(&self, snapshot: &Snapshot) {
        let iv = self.ivars();
        if let Some(item) = iv.menu_state.get() {
            set_menu_title(item, &format!("VoicePort · {}", snapshot.state));
        }
        if let Some(item) = iv.menu_shortcut.get() {
            let help = if snapshot.hotkey_failed {
                "快捷键未注册，请在设置中更换"
            } else {
                snapshot.shortcut_instruction
            };
            set_menu_title(item, &format!("{} · {help}", snapshot.shortcut));
        }
        if let Some(item) = iv.menu_toggle.get() {
            set_menu_title(item, snapshot.toggle_title);
            set_menu_enabled(item, snapshot.can_toggle);
        }
        if let Some(item) = iv.menu_cancel.get() {
            set_menu_enabled(item, snapshot.can_cancel);
        }
        if let Some(item) = iv.menu_copy.get() {
            set_menu_enabled(item, snapshot.can_copy);
        }
        if let Some(item) = iv.menu_repaste.get() {
            let title = if snapshot.repaste_registered {
                format!(
                    "{} · 重贴最近听写",
                    crate::shortcut::Shortcut::REPASTE.label()
                )
            } else {
                "重贴快捷键不可用 · 可复制最近听写".into()
            };
            set_menu_title(item, &title);
        }
        if let Some(item) = iv.menu_output_notice.get() {
            set_menu_title(item, snapshot.output_notice.unwrap_or_default());
            item.setHidden(snapshot.output_notice.is_none());
        }
    }

    pub(super) fn poll_menu_device(&self) {
        let iv = self.ivars();
        if iv.quitting.get() {
            return;
        }
        let slot = iv.controller.borrow();
        if let Some(c) = slot.as_ref() {
            let visible = iv
                .quick_panel
                .get()
                .is_some_and(quick_panel::QuickPanel::is_shown);
            iv.menu_device.borrow_mut().poll(
                c.microphone_uid.as_deref(),
                visible,
                Instant::now(),
                query_device,
            );
        }
    }
}

fn set_menu_title(item: &NSMenuItem, text: &str) {
    let title = NSString::from_str(text);
    if !item.title().isEqualToString(&title) {
        item.setTitle(&title);
    }
}

fn set_menu_enabled(item: &NSMenuItem, enabled: bool) {
    if item.isEnabled() != enabled {
        item.setEnabled(enabled);
    }
}

const DEVICE_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
pub(super) type DeviceResult = Result<Option<String>, String>;

#[derive(Default)]
pub(super) struct DeviceRefresh {
    preferred: Option<String>,
    generation: u64,
    in_flight: Option<(u64, Receiver<DeviceResult>)>,
    next: Option<Instant>,
    cached: Option<DeviceResult>,
}

impl DeviceRefresh {
    pub(super) fn poll(
        &mut self,
        preferred: Option<&str>,
        visible: bool,
        now: Instant,
        launch: impl FnOnce(Option<String>) -> std::io::Result<Receiver<DeviceResult>>,
    ) {
        if self.preferred.as_deref() != preferred {
            self.preferred = preferred.map(str::to_owned);
            self.generation += 1;
            self.cached = None;
            self.next = None;
        }
        if let Some((generation, receiver)) = &self.in_flight {
            let reply = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Disconnected) => Some(Err("读取中断".into())),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(reply) = reply {
                if *generation == self.generation {
                    self.cached = Some(reply);
                    self.next = Some(now + DEVICE_REFRESH_INTERVAL);
                }
                self.in_flight = None;
            }
        }
        if visible && self.in_flight.is_none() && self.next.is_none_or(|next| now >= next) {
            match launch(self.preferred.clone()) {
                Ok(receiver) => self.in_flight = Some((self.generation, receiver)),
                Err(_) => {
                    self.cached = Some(Err("无法启动设备读取".into()));
                    self.next = Some(now + DEVICE_REFRESH_INTERVAL);
                }
            }
        }
    }

    pub(super) fn label(&self, preferred: Option<&str>) -> String {
        let choice = if preferred.is_some() {
            "指定输入"
        } else {
            "跟随系统输入"
        };
        match self
            .cached
            .as_ref()
            .filter(|_| self.preferred.as_deref() == preferred)
        {
            Some(Ok(Some(name))) => format!("{choice} · {name}"),
            Some(Ok(None)) if preferred.is_some() => "指定输入暂不可用 · 保留设置".into(),
            Some(Ok(None) | Err(_)) => format!("{choice} · 设备待确认"),
            None => choice.into(),
        }
    }
}

fn query_device(preferred: Option<String>) -> std::io::Result<Receiver<DeviceResult>> {
    let (reply, receive) = mpsc::channel();
    std::thread::Builder::new()
        .name("voiceport-menu-device".into())
        .spawn(move || {
            let result = objc2::rc::autoreleasepool(|_| {
                crate::audio::devices::menu_input_name(preferred.as_deref())
            });
            let _ = reply.send(result);
        })?;
    Ok(receive)
}
