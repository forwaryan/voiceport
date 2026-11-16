//! Independent, sequential source probes. This model has no dictation or selection state.
use crate::{
    local_models::{Entry, SessionSource},
    plugin_process::{Custody, Failure},
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime},
};
use voiceport_core::CancellationToken;

// Store::validate allows 128 configured sources; Catalog::discover can add
// three built-in source entries and one unavailable saved-source entry.
pub const MAX_ENTRIES: usize = 128 + 3 + 1;
const BUDGET: Duration = Duration::from_secs(30);
const CLEANUP_DETAIL: &str = "来源测试清理尚未确认；新测试和听写已暂停。再次点取消可检查一次。";

type Executor =
    dyn Fn(SessionSource, &str, &CancellationToken, &Custody) -> Result<(), Failure> + Send + Sync;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    Success,
    Failed,
    TimedOut,
    Cancelled,
    CleanupPending,
}
impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "等待测试",
            Self::Running => "测试中",
            Self::Success => "测试通过",
            Self::Failed => "测试失败",
            Self::TimedOut => "测试超时",
            Self::Cancelled => "已取消",
            Self::CleanupPending => "等待清理确认",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProbeResult {
    pub status: Status,
    pub elapsed: Option<Duration>,
    pub completed_at: Option<SystemTime>,
    pub detail: String,
}
impl ProbeResult {
    /// Compact row text; detailed adapter errors belong in the separate tooltip.
    pub fn display(&self) -> String {
        let elapsed = self.elapsed.map_or_else(
            || "—".into(),
            |elapsed| format!("{} ms", elapsed.as_millis()),
        );
        let recent = self.completed_at.map(|completed| {
            let age = SystemTime::now()
                .duration_since(completed)
                .unwrap_or_default();
            if age < Duration::from_secs(60) {
                "刚刚完成".into()
            } else if age < Duration::from_secs(3600) {
                format!("{} 分钟前", age.as_secs() / 60)
            } else if age < Duration::from_secs(86400) {
                format!("{} 小时前", age.as_secs() / 3600)
            } else {
                format!("{} 天前", age.as_secs() / 86400)
            }
        });
        match recent {
            Some(recent) => format!("{} · {elapsed} · {recent}", self.status.label()),
            None => format!("{} · {elapsed}", self.status.label()),
        }
    }
    fn pending() -> Self {
        Self {
            status: Status::Pending,
            elapsed: None,
            completed_at: None,
            detail: "等待前一个来源测试结束。".into(),
        }
    }
}

// Never derive Debug for Entry snapshots: configured sources can contain secrets.
struct Row {
    entry: Entry,
    result: ProbeResult,
}
struct Queued {
    id: String,
    source: SessionSource,
}

/// Abandoning the model must not drop the last owner of an unknown child. There
/// is intentionally no signal, retry, observation or blocking join in Drop.
struct HeldCustody(Custody);
impl Drop for HeldCustody {
    fn drop(&mut self) {
        if self.0.unresolved() {
            std::mem::forget(self.0.clone());
        }
    }
}
struct Completion {
    started: Instant,
    finished: Instant,
    completed_at: SystemTime,
    result: Result<(), String>,
    cleanup: Option<HeldCustody>,
    _custody: HeldCustody,
}
enum Event {
    Started(Instant),
    Finished(Completion),
}
#[derive(Clone, Copy)]
enum Stop {
    Cancelled(Instant),
    TimedOut,
}
struct Observation {
    // A receipt is useful only for this exact owner, never for a later job.
    custody: Custody,
    receiver: Receiver<bool>,
}
struct CleanupOwner {
    custody: HeldCustody,
    confirmed: bool,
}
struct Cleanup {
    owners: Vec<CleanupOwner>,
    observation: Option<Observation>,
}
struct Active {
    id: String,
    cancel: CancellationToken,
    custody: HeldCustody,
    receiver: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
    exit: Receiver<()>,
    settled: bool,
    disconnected: bool,
    started: Option<Instant>,
    completion: Option<Completion>,
    stop: Option<Stop>,
    cleanup: Option<Cleanup>,
}

pub struct SourceTests {
    rows: BTreeMap<String, Row>,
    queue: VecDeque<Queued>,
    active: Option<Active>,
    language: String,
    generation: u64,
    budget: Duration,
    executor: Arc<Executor>,
}
impl Default for SourceTests {
    fn default() -> Self {
        Self::new()
    }
}
impl SourceTests {
    pub fn new() -> Self {
        Self {
            rows: BTreeMap::new(),
            queue: VecDeque::new(),
            active: None,
            language: String::new(),
            generation: 0,
            budget: BUDGET,
            executor: Arc::new(crate::source_probe::execute),
        }
    }
    #[cfg(test)]
    pub(crate) fn with_executor(
        budget: Duration,
        executor: impl Fn(SessionSource, &str, &CancellationToken, &Custody) -> Result<(), Failure>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        let mut tests = Self::new();
        tests.budget = budget;
        tests.executor = Arc::new(executor);
        tests
    }
    pub fn busy(&self) -> bool {
        self.active.is_some() || !self.queue.is_empty()
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn cleanup_pending(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.cleanup.is_some())
    }
    /// Equality includes the entire catalog/configuration snapshot, not just ID.
    pub fn row(&self, entry: &Entry) -> Option<&ProbeResult> {
        self.rows
            .get(&entry.id)
            .filter(|row| row.entry == *entry)
            .map(|row| &row.result)
    }
    pub(crate) fn retain_catalog(&mut self, entries: &[Entry]) {
        if !self.busy() {
            self.rows.retain(|_, row| entries.contains(&row.entry));
        }
    }
    pub fn start(&mut self, entries: Vec<Entry>, language: String) -> Result<(), String> {
        if self.busy() {
            return Err("来源测试或清理尚未结束。".into());
        }
        if entries.is_empty() || entries.len() > MAX_ENTRIES {
            return Err(format!("请选择 1 至 {MAX_ENTRIES} 个来源。"));
        }
        let mut ids = BTreeSet::new();
        if entries.iter().any(|entry| !ids.insert(&entry.id)) {
            return Err("同一来源不能在一次测试中重复。".into());
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("Source test generation exhausted.")?;
        self.language = language;
        for entry in entries {
            let mut result = ProbeResult::pending();
            match &entry.source {
                Ok(source) => self.queue.push_back(Queued {
                    id: entry.id.clone(),
                    source: source.clone(),
                }),
                Err(error) => {
                    result.status = Status::Failed;
                    result.detail = error.clone();
                    result.completed_at = Some(SystemTime::now());
                }
            }
            self.rows.insert(entry.id.clone(), Row { entry, result });
        }
        self.dispatch_next();
        Ok(())
    }
    pub fn poll(&mut self) {
        self.update_active();
        self.dispatch_next();
    }
    pub fn cancel(&mut self) {
        // Consume already finished work without accidentally dispatching a queued
        // source just before cancellation.
        self.update_active();
        self.cancel_queue();
        let Some(active) = &mut self.active else {
            return;
        };
        if active.cleanup.is_some() {
            Self::request_observation(active);
            return;
        }
        if active.stop.is_none() {
            active.stop = Some(Stop::Cancelled(Instant::now()));
        }
        active.cancel.cancel();
        if let Some(row) = self.rows.get_mut(&active.id) {
            row.result.detail = "正在取消；等待测试线程释放资源。".into();
        }
    }
    pub fn summary(&self) -> String {
        if self.active.as_ref().is_some_and(|a| a.cleanup.is_some()) {
            return CLEANUP_DETAIL.into();
        }
        if self.busy() {
            return format!("来源测试中；还有 {} 个来源等待。", self.queue.len());
        }
        let count = |status| {
            self.rows
                .values()
                .filter(|r| r.result.status == status)
                .count()
        };
        if self.rows.is_empty() {
            "尚未测试来源。".into()
        } else {
            format!(
                "来源测试已结束：通过 {}，失败 {}，超时 {}，取消 {}。",
                count(Status::Success),
                count(Status::Failed),
                count(Status::TimedOut),
                count(Status::Cancelled)
            )
        }
    }
    fn cancel_queue(&mut self) {
        for queued in self.queue.drain(..) {
            if let Some(row) = self.rows.get_mut(&queued.id) {
                row.result.status = Status::Cancelled;
                row.result.detail = "已取消，未启动此来源测试。".into();
                row.result.completed_at = Some(SystemTime::now());
            }
        }
    }
    fn dispatch_next(&mut self) {
        while self.active.is_none() {
            let Some(queued) = self.queue.pop_front() else {
                return;
            };
            let custody = Custody::default();
            let cancel = CancellationToken::new();
            let (sender, receiver) = mpsc::channel();
            let (handoff, join_handle) = mpsc::channel::<JoinHandle<()>>();
            let (exited, exit) = mpsc::channel();
            // Establish the reaper before starting any source work. Joining a
            // finished-looking thread can still wait for its TLS destructors.
            let reaper = std::thread::Builder::new()
                .name("voiceport-source-test-release".into())
                .spawn(move || {
                    if let Ok(thread) = join_handle.recv() {
                        let _ = thread.join();
                        let _ = exited.send(());
                    }
                });
            let reaper = match reaper {
                Ok(reaper) => reaper,
                Err(_) => {
                    let row = &mut self.rows.get_mut(&queued.id).expect("queued row").result;
                    row.status = Status::Failed;
                    row.detail = "无法启动来源测试资源回收线程。".into();
                    row.completed_at = Some(SystemTime::now());
                    continue;
                }
            };
            // Parent ownership is established before the worker can pin or run.
            self.active = Some(Active {
                id: queued.id.clone(),
                cancel: cancel.clone(),
                custody: HeldCustody(custody.clone()),
                receiver,
                thread: Some(reaper),
                exit,
                settled: false,
                disconnected: false,
                started: None,
                completion: None,
                stop: None,
                cleanup: None,
            });
            let row = &mut self.rows.get_mut(&queued.id).expect("queued row").result;
            row.status = Status::Running;
            row.detail = "正在测试此来源。".into();
            let language = self.language.clone();
            let executor = self.executor.clone();
            let worker_custody = HeldCustody(custody);
            let thread = std::thread::Builder::new()
                .name("voiceport-source-test".into())
                .spawn(move || {
                    let started = Instant::now();
                    let _ = sender.send(Event::Started(started));
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if cancel.is_cancelled() {
                            return Err(Failure::from("来源测试已取消。"));
                        }
                        let source = queued.source.pin().map_err(Failure::from)?;
                        if cancel.is_cancelled() {
                            return Err(Failure::from("来源测试已取消。"));
                        }
                        executor(source, &language, &cancel, &worker_custody.0)
                    }))
                    .unwrap_or_else(|_| Err("来源测试线程异常结束。".into()));
                    // Source/native resources have left the unwind scope. No
                    // snapshot, transcript or panic payload enters the result.
                    drop(executor);
                    drop(language);
                    drop(cancel);
                    let (result, cleanup) = match result {
                        Ok(()) => (Ok(()), None),
                        Err(failure) => (Err(failure.message), failure.cleanup.map(HeldCustody)),
                    };
                    let finished = Instant::now();
                    let completion = Completion {
                        started,
                        finished,
                        completed_at: SystemTime::now(),
                        result,
                        cleanup,
                        _custody: worker_custody,
                    };
                    // A failed send still retains unknown custody through Drop.
                    let _ = sender.send(Event::Finished(completion));
                });
            match thread {
                Ok(thread) => {
                    // A lost reaper channel leaves exit unconfirmed and keeps
                    // admission closed; dropping a handle is not stop evidence.
                    let _ = handoff.send(thread);
                }
                Err(_) => {
                    self.active = None;
                    row.status = Status::Failed;
                    row.detail = "无法启动来源测试线程。".into();
                    row.completed_at = Some(SystemTime::now());
                }
            }
        }
    }
    fn update_active(&mut self) {
        let Some(mut active) = self.active.take() else {
            return;
        };
        Self::receive_events(&mut active);
        if !active.settled && active.exit.try_recv().is_ok() {
            // The background reaper joined the actual source worker, including
            // TLS teardown. The UI never joins either thread.
            active.thread.take();
            active.settled = true;
            Self::receive_events(&mut active);
        }
        // An installed child is normal during execution. It becomes a cleanup
        // block only on a result, lost channel, or confirmed worker exit.
        if active.cleanup.is_none()
            && (active.completion.is_some() || active.disconnected || active.settled)
        {
            let mut owners = vec![];
            if active.custody.0.unresolved() {
                owners.push(CleanupOwner {
                    custody: HeldCustody(active.custody.0.clone()),
                    confirmed: false,
                });
            }
            if let Some(completion) = &mut active.completion
                && completion
                    .cleanup
                    .as_ref()
                    .is_some_and(|owner| !owners.iter().any(|o| o.custody.0.same_owner(&owner.0)))
            {
                owners.push(CleanupOwner {
                    custody: completion.cleanup.take().expect("distinct cleanup owner"),
                    confirmed: false,
                });
            }
            if !owners.is_empty() {
                active.cleanup = Some(Cleanup {
                    owners,
                    observation: None,
                });
            }
        }
        if active.cleanup.is_some() {
            self.cancel_queue();
            let row = &mut self.rows.get_mut(&active.id).expect("active row").result;
            row.status = Status::CleanupPending;
            row.completed_at = None;
            row.elapsed = active
                .completion
                .as_ref()
                .map(|c| c.finished.duration_since(c.started));
            row.detail = CLEANUP_DETAIL.into();
            Self::poll_observation(&mut active);
            if active.settled
                && active.cleanup.as_ref().is_some_and(|c| {
                    c.owners
                        .iter()
                        .all(|o| o.confirmed && !o.custody.0.unresolved())
                })
                && !active.custody.0.unresolved()
            {
                self.finish(active, true);
            } else {
                self.active = Some(active);
            }
            return;
        }
        if active.settled {
            self.finish(active, false);
            return;
        }
        if let Some(started) = active.started {
            let elapsed = active.completion.as_ref().map_or_else(
                || started.elapsed(),
                |c| c.finished.duration_since(c.started),
            );
            let row = &mut self.rows.get_mut(&active.id).expect("active row").result;
            row.elapsed = Some(elapsed);
            if elapsed >= self.budget && active.stop.is_none() && active.completion.is_none() {
                active.stop = Some(Stop::TimedOut);
                active.cancel.cancel();
                row.status = Status::TimedOut;
                row.detail = "测试超过时限；等待测试线程释放资源。".into();
            }
        }
        self.active = Some(active);
    }
    fn receive_events(active: &mut Active) {
        if active.disconnected {
            return;
        }
        loop {
            match active.receiver.try_recv() {
                Ok(Event::Started(started)) => active.started = Some(started),
                Ok(Event::Finished(completion)) => {
                    active.started = Some(completion.started);
                    active.completion = Some(completion);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    active.disconnected = true;
                    if active.completion.is_none() {
                        active.cancel.cancel();
                    }
                    break;
                }
            }
        }
    }
    fn finish(&mut self, active: Active, cleanup_confirmed: bool) {
        let row = &mut self.rows.get_mut(&active.id).expect("active row").result;
        let elapsed = active
            .completion
            .as_ref()
            .map(|c| c.finished.duration_since(c.started));
        let cancelled = matches!(active.stop, Some(Stop::Cancelled(at)) if active.completion.as_ref().is_none_or(|c| at < c.finished));
        // A deadline poll can race a completed worker that has not sent its
        // event yet. Its measured duration is authoritative once available.
        let timed_out = elapsed.map_or(matches!(active.stop, Some(Stop::TimedOut)), |elapsed| {
            elapsed >= self.budget
        });
        let (status, detail) = if cancelled {
            (Status::Cancelled, "来源测试已取消。".into())
        } else if timed_out {
            (Status::TimedOut, "来源测试超过时限。".into())
        } else {
            match active.completion.as_ref().map(|c| &c.result) {
                Some(Ok(())) => (Status::Success, "来源测试已完成。".into()),
                Some(Err(message)) => (Status::Failed, message.clone()),
                None => (Status::Failed, "来源测试工作线程未返回结果。".into()),
            }
        };
        row.status = status;
        row.elapsed = elapsed;
        row.completed_at = Some(if cleanup_confirmed {
            SystemTime::now()
        } else {
            active
                .completion
                .as_ref()
                .map_or_else(SystemTime::now, |c| c.completed_at)
        });
        row.detail = if cleanup_confirmed {
            format!("{detail}清理已确认。")
        } else {
            detail
        };
    }
    fn request_observation(active: &mut Active) {
        let Some(cleanup) = &mut active.cleanup else {
            return;
        };
        if cleanup.observation.is_some() {
            return;
        }
        let Some(owner) = cleanup.owners.iter().find(|owner| !owner.confirmed) else {
            return;
        };
        let custody = owner.custody.0.clone();
        // Active retains this same owner throughout observation (and retains
        // unresolved custody on model Drop), so a negative check can drop its clone.
        let worker_owner = custody.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        if std::thread::Builder::new()
            .name("voiceport-source-test-cleanup".into())
            .spawn(move || {
                let _ = sender.send(worker_owner.observe_once());
            })
            .is_ok()
        {
            cleanup.observation = Some(Observation { custody, receiver });
        }
    }
    fn poll_observation(active: &mut Active) {
        // Channel loss is not evidence that the worker released its resources.
        if !active.settled {
            return;
        }
        let Some(cleanup) = &mut active.cleanup else {
            return;
        };
        let Some(observation) = &cleanup.observation else {
            return;
        };
        match observation.receiver.try_recv() {
            Ok(true) if !observation.custody.unresolved() => {
                if let Some(owner) = cleanup
                    .owners
                    .iter_mut()
                    .find(|owner| owner.custody.0.same_owner(&observation.custody))
                {
                    owner.confirmed = true;
                }
                cleanup.observation = None;
            }
            Ok(_) | Err(TryRecvError::Disconnected) => cleanup.observation = None,
            Err(TryRecvError::Empty) => {}
        }
    }
}
impl Drop for SourceTests {
    fn drop(&mut self) {
        self.queue.clear();
        if let Some(active) = &self.active {
            active.cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests;
