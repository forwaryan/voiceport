use crate::{
    audio::{
        devices::Device,
        worker::{InputSnapshot, Inventory},
    },
    controller::Controller,
    lifecycle::Phase,
};
use std::time::{Duration, Instant};

const SNAPSHOT_AGE: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Choice {
    System,
    Device(Device),
    Retained(Option<Device>),
}

impl Choice {
    pub(super) fn label(&self) -> String {
        match self {
            Self::System => "跟随系统输入（默认）".into(),
            Self::Device(device) => device.label(),
            Self::Retained(Some(device)) => format!("{} · 待重新确认", device.label()),
            Self::Retained(None) => "已指定设备 · 当前不可确认".into(),
        }
    }
}

#[derive(Default)]
enum ReadState {
    #[default]
    Idle,
    Waiting,
    Reading(Inventory<InputSnapshot>),
    Failed(String),
}

#[derive(Default)]
pub(super) struct Catalog {
    state: ReadState,
    snapshot: Option<InputSnapshot>,
}

impl Catalog {
    pub(super) fn request(&mut self) {
        if !self.reading() {
            self.state = ReadState::Waiting;
        }
    }

    pub(super) fn suspend(&mut self) {
        // A hidden page drops its receiver. The lane stays reserved until the
        // actual native query finishes, even if the page becomes visible again.
        self.state = ReadState::Waiting;
    }

    pub(super) fn requested(&self) -> bool {
        matches!(self.state, ReadState::Waiting | ReadState::Reading(_))
    }

    pub(super) fn reading(&self) -> bool {
        matches!(self.state, ReadState::Reading(_))
    }

    pub(super) fn poll(
        &mut self,
        can_read: bool,
        begin: impl FnOnce() -> Result<Inventory<InputSnapshot>, String>,
    ) {
        match &self.state {
            ReadState::Waiting if can_read => {
                self.state = match begin() {
                    Ok(query) => ReadState::Reading(query),
                    Err(error) => ReadState::Failed(error),
                };
            }
            ReadState::Reading(query) => match query.poll() {
                Some(Ok(snapshot)) => {
                    // Retain the previous snapshot for an overall failure. A
                    // completed read may still have independent row failures.
                    self.snapshot = Some(snapshot);
                    self.state = ReadState::Idle;
                }
                Some(Err(error)) => self.state = ReadState::Failed(error),
                None => {}
            },
            _ => {}
        }
    }

    fn current(&self, now: Instant) -> bool {
        matches!(self.state, ReadState::Idle)
            && self
                .snapshot
                .as_ref()
                .is_some_and(|s| now.saturating_duration_since(s.observed) < SNAPSHOT_AGE)
    }

    fn observed_devices(&self) -> &[Device] {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.devices.as_deref().ok())
            .unwrap_or_default()
    }

    pub(super) fn can_choose(&self, uid: &str, now: Instant) -> bool {
        self.current(now) && self.observed_devices().iter().any(|d| d.uid == uid)
    }

    pub(super) fn choices(&self, saved: Option<&str>, now: Instant) -> (Vec<Choice>, usize) {
        let mut rows = vec![Choice::System];
        if self.current(now) {
            rows.extend(self.observed_devices().iter().cloned().map(Choice::Device));
        }
        let Some(uid) = saved else { return (rows, 0) };
        if let Some(index) = rows
            .iter()
            .position(|row| matches!(row, Choice::Device(d) if d.uid == uid))
        {
            return (rows, index);
        }
        let previous = self
            .observed_devices()
            .iter()
            .find(|d| d.uid == uid)
            .cloned();
        let index = rows.len();
        rows.push(Choice::Retained(previous));
        (rows, index)
    }

    pub(super) fn note(&self, now: Instant) -> String {
        match &self.state {
            ReadState::Waiting => "音频空闲后读取设备与授权。".into(),
            ReadState::Reading(_) => "正在读取设备与授权…".into(),
            ReadState::Failed(error) => format!("刷新失败。{error}"),
            ReadState::Idle => match &self.snapshot {
                None => "尚未检查设备。".into(),
                Some(_) if !self.current(now) => "设备列表已过期。切换指定设备前请刷新。".into(),
                Some(snapshot) => match &snapshot.devices {
                    Ok(devices) if devices.is_empty() => {
                        "未发现可选输入设备。已保存的偏好会保留。".into()
                    }
                    Ok(_) => "设备列表来自最近一次检查。".into(),
                    Err(error) => format!("无法读取设备列表。{error}"),
                },
            },
        }
    }

    pub(super) fn system(&self, now: Instant) -> String {
        self.row("系统默认输入", now, |snapshot| {
            match &snapshot.system {
                Ok(device) => device.label(),
                Err(error) => format!("无法确认。{error}"),
            }
        })
    }

    pub(super) fn permission(&self, now: Instant) -> String {
        self.row("麦克风授权", now, |snapshot| {
            match snapshot.permission {
                "authorized" => "已允许",
                "notDetermined" => "尚未授权",
                "denied" => "已拒绝，可在系统设置中更改",
                "restricted" => "受系统限制",
                _ => "无法确认",
            }
            .into()
        })
    }

    fn row(
        &self,
        label: &str,
        now: Instant,
        value: impl FnOnce(&InputSnapshot) -> String,
    ) -> String {
        match &self.snapshot {
            Some(snapshot) if self.current(now) => format!("{label}：{}", value(snapshot)),
            Some(snapshot) => format!("{label}（旧快照）：{}", value(snapshot)),
            None => format!("{label}：尚未确认"),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Activity {
    Idle,
    Reading,
    Permission,
    Opening,
    Recording,
    Recovering,
    Stopping,
    TestOpening,
    Testing,
    Releasing,
    OtherWork,
}

impl Activity {
    pub(super) fn read(c: &Controller, reading: bool, has_route: bool) -> Self {
        if c.finalizing_capture() {
            return Self::Stopping;
        }
        match c.state.phase {
            Phase::Permissions if c.opening_microphone() => Self::Opening,
            Phase::Permissions => Self::Permission,
            Phase::Recording if has_route => Self::Recording,
            // The controller first enters Recording with a verified route. It
            // clears that route when the worker starts device recovery.
            Phase::Recording => Self::Recovering,
            _ if c.microphone_test_measuring() => Self::Testing,
            _ if c.microphone_test_running() => Self::TestOpening,
            _ if reading => Self::Reading,
            _ if c.audio_input_busy() => Self::Releasing,
            _ if c.busy() => Self::OtherWork,
            _ => Self::Idle,
        }
    }

    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Idle => "音频状态：空闲",
            Self::Reading => "音频状态：正在检查设备",
            Self::Permission => "音频状态：等待麦克风权限",
            Self::Opening => "音频状态：正在打开麦克风",
            Self::Recording => "音频状态：正在录音",
            Self::Recovering => "音频状态：输入已断开，正在尝试恢复",
            Self::Stopping => "音频状态：正在停止录音并释放输入",
            Self::TestOpening => "音频状态：正在打开电平测试",
            Self::Testing => "音频状态：正在测试电平",
            Self::Releasing => "音频状态：后台音频槽仍被占用",
            Self::OtherWork => "音频状态：VoicePort 正在处理其他任务",
        }
    }

    pub(super) fn actual(&self, route: Option<&str>) -> String {
        match (self, route) {
            (Self::Recording, Some(route)) => format!("实际录音输入：{route}"),
            (Self::Stopping, Some(route)) => format!("停止前的录音输入：{route}"),
            (Self::Recovering, _) => "实际录音输入：等待替代设备通过检查".into(),
            _ => "实际录音输入：当前无录音输入".into(),
        }
    }
}

#[cfg(all(test, debug_assertions))]
mod tests;
