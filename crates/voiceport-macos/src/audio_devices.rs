//! CoreAudio input inventory and verified, per-engine input routing. Never sets
//! the Mac's default device. HAL object IDs are transient; only UIDs are saved.
use core_foundation::{
    base::TCFType,
    string::{CFString, CFStringRef},
};
use objc2::{
    rc::{Retained, autoreleasepool},
    runtime::ProtocolObject,
};
use objc2_audio_toolbox::{
    AudioUnitGetProperty, AudioUnitSetProperty, kAudioOutputUnitProperty_CurrentDevice,
    kAudioUnitScope_Global,
};
use objc2_avf_audio::{
    AVAudioEngine, AVAudioEngineConfigurationChangeNotification, AVAudioFormat, AVAudioInputNode,
    AVAudioPCMBuffer, AVAudioTime,
};
use objc2_core_audio::*;
#[cfg(debug_assertions)]
#[path = "audio_device_fixture.rs"]
pub(crate) mod fixture;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};
use std::{
    ptr::{self, NonNull},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Device {
    id: u32,
    pub uid: String,
    pub name: String,
}

impl Device {
    pub fn label(&self) -> String {
        let digest = ring::digest::digest(&ring::digest::SHA256, self.uid.as_bytes());
        let id = digest.as_ref();
        format!(
            "{} · {:02X}{:02X}{:02X}{:02X}",
            self.name, id[0], id[1], id[2], id[3]
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Fallback {
    Missing,
    InventoryFailed,
    OpenFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Route {
    device: Device,
    fallback: Option<Fallback>,
    recovered: bool,
    follows_default: bool,
}

impl Route {
    pub fn summary(&self) -> String {
        let note = match self.fallback {
            Some(Fallback::Missing) => " · 指定设备不可用，临时使用系统输入",
            Some(Fallback::InventoryFailed) => " · 设备列表读取失败，临时使用系统输入",
            Some(Fallback::OpenFailed) => " · 无法打开指定设备，临时使用系统输入",
            None => "",
        };
        format!(
            "{}{note}{}",
            self.device.label(),
            if self.recovered {
                " · 已恢复录音"
            } else {
                ""
            }
        )
    }
}

trait Inventory {
    fn inputs(&self) -> Result<Vec<Device>, String>;
    fn system_input(&self) -> Result<Device, String> {
        self.default_candidate()?
            .ok_or_else(|| "macOS 当前没有可用的系统输入。".into())
    }
    fn default_candidate(&self) -> Result<Option<Device>, String>;
}

#[derive(Debug)]
pub(super) enum OpenError {
    Unavailable(String),
    Unverified(String),
    AdmissionClosed,
}

impl From<OpenError> for String {
    fn from(error: OpenError) -> Self {
        error.message()
    }
}

impl From<&str> for OpenError {
    fn from(message: &str) -> Self {
        Self::Unverified(message.into())
    }
}

impl From<String> for OpenError {
    fn from(message: String) -> Self {
        Self::Unverified(message)
    }
}

impl OpenError {
    pub fn message(self) -> String {
        match self {
            Self::Unavailable(message) | Self::Unverified(message) => message,
            Self::AdmissionClosed => "音频操作已停止或取消。".into(),
        }
    }
}

// No speculative "actual" route: only a successful, verified open can publish it.
fn open_route<T>(
    inventory: &impl Inventory,
    preferred_uid: Option<&str>,
    recovered: bool,
    check: &impl Fn() -> Result<(), String>,
    mut open: impl FnMut(&Device) -> Result<T, OpenError>,
) -> Result<(Route, T), OpenError> {
    check()?;
    let mut rejected = None;
    let mut fallback = None;
    let mut uncertain = false;
    if let Some(uid) = preferred_uid {
        let devices = inventory.inputs();
        check()?;
        match devices {
            Ok(devices) => match devices.into_iter().find(|d| d.uid == uid) {
                Some(device) => match open(&device) {
                    Ok(input) => {
                        return Ok((
                            Route {
                                device,
                                fallback,
                                recovered,
                                follows_default: false,
                            },
                            input,
                        ));
                    }
                    Err(error) => {
                        uncertain = matches!(error, OpenError::Unverified(_));
                        rejected = Some((device.id, device.uid, error));
                        fallback = Some(Fallback::OpenFailed);
                    }
                },
                None => fallback = Some(Fallback::Missing),
            },
            Err(_) => {
                uncertain = true;
                fallback = Some(Fallback::InventoryFailed);
            }
        }
    }
    let failed = |error: OpenError| {
        if uncertain {
            OpenError::Unverified(error.message())
        } else {
            error
        }
    };
    check()?;
    let device = inventory.default_candidate()?.ok_or_else(|| {
        failed(OpenError::Unavailable(
            "macOS 当前没有可用的系统输入。".into(),
        ))
    })?;
    check()?;
    if let Some((id, uid, error)) = rejected
        && device.id == id
        && device.uid == uid
    {
        return Err(error);
    }
    let input = match open(&device).map_err(failed) {
        Err(error @ OpenError::Unavailable(_)) if recovered => {
            let still_default = inventory
                .default_candidate()?
                .is_some_and(|current| current.id == device.id && current.uid == device.uid);
            check()?;
            return Err(if still_default {
                error
            } else {
                OpenError::Unverified(INPUT_CHANGED.into())
            });
        }
        result => result?,
    };
    if preferred_uid == Some(device.uid.as_str()) {
        fallback = None;
    }
    Ok((
        Route {
            device,
            fallback,
            recovered,
            follows_default: true,
        },
        input,
    ))
}

struct Hal;
const SYSTEM: u32 = kAudioObjectSystemObject as u32;
const GLOBAL: u32 = kAudioObjectPropertyScopeGlobal;
const INPUT: u32 = kAudioObjectPropertyScopeInput;
const MAX_LIST_BYTES: u32 = 64 * 1024;

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn status(code: i32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("CoreAudio 错误（{code}）"))
    }
}

fn read_property<T>(id: u32, selector: u32, mut value: T) -> Result<T, String> {
    let mut property = address(selector, GLOBAL);
    let expected = size_of::<T>() as u32;
    let mut size = expected;
    // All callers supply the exact ABI type for this selector and an initialized
    // output; the successful native result must occupy precisely that size.
    status(unsafe {
        AudioObjectGetPropertyData(
            id,
            NonNull::from(&mut property),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    })?;
    if size != expected {
        return Err("系统音频设备属性格式不正确。".into());
    }
    Ok(value)
}

fn string_property(id: u32, selector: u32) -> Result<String, String> {
    let reference = read_property::<CFStringRef>(id, selector, ptr::null())?;
    if reference.is_null() {
        return Err("系统音频设备标识不可用。".into());
    }
    // CoreAudio returns retained CFStrings for the device UID and object name.
    Ok(unsafe { CFString::wrap_under_create_rule(reference) }.to_string())
}

fn list_size(id: u32, property: &mut AudioObjectPropertyAddress) -> Result<u32, String> {
    let mut size = 0_u32;
    status(unsafe {
        AudioObjectGetPropertyDataSize(
            id,
            NonNull::from(property),
            0,
            ptr::null(),
            NonNull::from(&mut size),
        )
    })?;
    Ok(size)
}

fn identity(id: u32) -> Result<String, String> {
    let uid = string_property(id, kAudioDevicePropertyDeviceUID)?;
    if uid.is_empty() {
        return Err("系统音频设备没有有效标识。".into());
    }
    Ok(uid)
}

fn alive(id: u32) -> Result<bool, String> {
    Ok(read_property(id, kAudioDevicePropertyDeviceIsAlive, 0_u32)? != 0)
}

fn input_device(id: u32) -> Result<Option<Device>, String> {
    if id == 0 {
        return Ok(None);
    }
    let mut streams = address(kAudioDevicePropertyStreams, INPUT);
    // Some output-only devices omit the input-scope property entirely.
    if !unsafe { AudioObjectHasProperty(id, NonNull::from(&mut streams)) }
        || list_size(id, &mut streams)? == 0
        || !alive(id)?
    {
        return Ok(None);
    }
    let uid = identity(id)?;
    let name = string_property(id, kAudioObjectPropertyName)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "未命名输入设备".into());
    Ok(Some(Device { id, uid, name }))
}

impl Inventory for Hal {
    fn inputs(&self) -> Result<Vec<Device>, String> {
        let mut property = address(kAudioHardwarePropertyDevices, GLOBAL);
        let mut size = list_size(SYSTEM, &mut property)?;
        let word = size_of::<u32>() as u32;
        if size > MAX_LIST_BYTES || !size.is_multiple_of(word) {
            return Err("系统音频设备列表格式不正确。".into());
        }
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut ids = vec![0_u32; (size / word) as usize];
        status(unsafe {
            AudioObjectGetPropertyData(
                SYSTEM,
                NonNull::from(&mut property),
                0,
                ptr::null(),
                NonNull::from(&mut size),
                NonNull::new(ids.as_mut_ptr()).unwrap().cast(),
            )
        })?;
        if size as usize > ids.len() * size_of::<u32>() || !size.is_multiple_of(word) {
            return Err("系统音频设备列表已更改，请刷新。".into());
        }
        ids.truncate((size / word) as usize);
        let mut devices = Vec::new();
        for id in ids {
            if let Some(device) = input_device(id)? {
                devices.push(device);
            }
        }
        devices.sort_by(|a, b| (&a.name, &a.uid).cmp(&(&b.name, &b.uid)));
        devices.dedup_by(|a, b| a.uid == b.uid);
        Ok(devices)
    }

    fn default_candidate(&self) -> Result<Option<Device>, String> {
        input_device(Hal::default_input_id()?)
    }
}

impl Hal {
    fn default_input_id() -> Result<u32, String> {
        read_property(SYSTEM, kAudioHardwarePropertyDefaultInputDevice, 0_u32)
    }
}

pub(crate) fn inputs() -> Result<Vec<Device>, String> {
    #[cfg(debug_assertions)]
    if let Some(fixture) = fixture::current() {
        return fixture.inputs();
    }
    Hal.inputs()
}

/// Menu metadata only. Capture still resolves and verifies its own input route.
pub(crate) fn menu_input_name(preferred: Option<&str>) -> Result<Option<String>, String> {
    if let Some(uid) = preferred {
        return inputs().map(|devices| {
            devices
                .into_iter()
                .find(|device| device.uid == uid)
                .map(|device| device.label())
        });
    }
    #[cfg(debug_assertions)]
    if let Some(fixture) = fixture::current() {
        return fixture.system_input().map(|device| Some(device.label()));
    }
    Hal.system_input().map(|device| Some(device.label()))
}

/// A read-only preview. Route admission still checks the actual opened input.
pub(super) fn system_input_preview() -> Result<Device, String> {
    fn confirm(device: Device, current: u32) -> Result<Device, String> {
        if device.id != current {
            return Err("读取时系统默认输入已变化，请刷新。".into());
        }
        Ok(device)
    }

    #[cfg(debug_assertions)]
    if let Some(fixture) = fixture::current() {
        return confirm(fixture.system_input()?, fixture.final_default_input_id());
    }
    confirm(Hal.system_input()?, Hal::default_input_id()?)
}

fn unit_device(node: &AVAudioInputNode) -> Result<u32, String> {
    let unit = unsafe { node.audioUnit() };
    if unit.is_null() {
        return Err("麦克风音频单元不可用。".into());
    }
    let mut id = 0_u32;
    let mut size = size_of::<u32>() as u32;
    status(unsafe {
        AudioUnitGetProperty(
            unit,
            kAudioOutputUnitProperty_CurrentDevice,
            kAudioUnitScope_Global,
            0,
            NonNull::from(&mut id).cast(),
            NonNull::from(&mut size),
        )
    })?;
    if id == 0 || size != size_of::<u32>() as u32 {
        return Err("无法确认实际打开的输入设备。".into());
    }
    Ok(id)
}

// Each tap has its own one-shot membership. Closing waits for an already
// admitted callback; callbacks arriving after close cannot enter the sink.
type Consumer = dyn Fn(&AVAudioPCMBuffer) + Send + Sync;
#[derive(Default)]
struct TapGate(Mutex<Option<Box<Consumer>>>, AtomicBool);
impl TapGate {
    fn consume(&self, buffer: &AVAudioPCMBuffer) {
        if self.1.load(Ordering::Acquire) {
            return;
        }
        if let Ok(slot) = self.0.lock()
            && !self.1.load(Ordering::Acquire)
            && let Some(consume) = slot.as_ref()
        {
            consume(buffer);
        }
    }
    fn invalidate(&self) {
        self.1.store(true, Ordering::Release);
    }
    fn close(&self) {
        self.invalidate();
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

// The HAL notification and the tap admission race on this one-shot state.
// A later notification does not revoke an already working, pinned input.
#[derive(Default)]
struct DefaultAdmission(AtomicU8);

impl DefaultAdmission {
    const PENDING: u8 = 0;
    const INVALIDATED: u8 = 1;
    const ADMITTED: u8 = 2;

    fn pending(&self) -> bool {
        self.0.load(Ordering::Acquire) == Self::PENDING
    }

    fn invalidate(&self) {
        let _ = self.0.compare_exchange(
            Self::PENDING,
            Self::INVALIDATED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn admit(&self) -> bool {
        self.0
            .compare_exchange(
                Self::PENDING,
                Self::ADMITTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

struct DefaultListener(block2::RcBlock<dyn Fn(u32, NonNull<AudioObjectPropertyAddress>)>);

impl DefaultListener {
    fn new(admission: &Arc<DefaultAdmission>) -> Result<Self, String> {
        let observed = Arc::downgrade(admission);
        // CoreAudio copies the block and calls it on delivery with a null queue.
        // Only touch the admission state here, even if delivery is reentrant.
        let listener =
            block2::RcBlock::new(move |_: u32, _: NonNull<AudioObjectPropertyAddress>| {
                if let Some(admission) = observed.upgrade() {
                    admission.invalidate();
                }
            });
        let mut property = address(kAudioHardwarePropertyDefaultInputDevice, GLOBAL);
        status(unsafe {
            AudioObjectAddPropertyListenerBlock(
                SYSTEM,
                NonNull::from(&mut property),
                None,
                block2::RcBlock::as_ptr(&listener),
            )
        })?;
        Ok(Self(listener))
    }
}

impl Drop for DefaultListener {
    fn drop(&mut self) {
        let mut property = address(kAudioHardwarePropertyDefaultInputDevice, GLOBAL);
        if let Err(error) = status(unsafe {
            AudioObjectRemovePropertyListenerBlock(
                SYSTEM,
                NonNull::from(&mut property),
                None,
                block2::RcBlock::as_ptr(&self.0),
            )
        }) {
            eprintln!("无法移除系统输入监听：{error}");
        }
    }
}

const INPUT_CHANGED: &str = "准备期间麦克风已变化，请重新开始。";

struct DefaultWatch {
    id: u32,
    admission: Arc<DefaultAdmission>,
    _listener: Option<DefaultListener>,
}

struct NativeInput {
    engine: Retained<AVAudioEngine>,
    node: Retained<AVAudioInputNode>,
    observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

pub(super) struct OpenInput {
    native: InputEngine,
    pub format: Retained<AVAudioFormat>,
    gate: Arc<TapGate>,
    default_watch: Option<DefaultWatch>,
    next_check: Instant,
}

enum InputEngine {
    Native(NativeInput),
    #[cfg(debug_assertions)]
    Synthetic(fixture::Input),
}

impl OpenInput {
    pub fn select(
        preferred: Option<&str>,
        check: &impl Fn() -> Result<(), String>,
        validate: impl Fn(&AVAudioFormat) -> Result<(), String>,
    ) -> Result<(Route, Self), String> {
        Self::open_prepared(preferred, false, check, validate)
            .map(|(route, (input, ()))| (route, input))
            .map_err(OpenError::message)
    }

    pub fn open_prepared<T>(
        preferred: Option<&str>,
        recovered: bool,
        check: &impl Fn() -> Result<(), String>,
        prepare: impl Fn(&AVAudioFormat) -> Result<T, String>,
    ) -> Result<(Route, (Self, T)), OpenError> {
        #[cfg(debug_assertions)]
        if let Some(fixture) = fixture::current() {
            return open_route(&fixture, preferred, recovered, check, |device| {
                fixture.open(device, check, &prepare)
            });
        }
        open_route(&Hal, preferred, recovered, check, |device| {
            Self::open(device, check, &prepare)
        })
    }

    fn open<T>(
        device: &Device,
        check: &impl Fn() -> Result<(), String>,
        prepare: &impl Fn(&AVAudioFormat) -> Result<T, String>,
    ) -> Result<(Self, T), OpenError> {
        check()?;
        let engine = unsafe { AVAudioEngine::new() };
        check()?;
        let node = unsafe { engine.inputNode() };
        check()?;
        let unit = unsafe { node.audioUnit() };
        if unit.is_null() {
            return Err(OpenError::Unavailable("麦克风音频单元不可用。".into()));
        }
        if unit_device(&node).ok() != Some(device.id) {
            check()?;
            status(unsafe {
                AudioUnitSetProperty(
                    unit,
                    kAudioOutputUnitProperty_CurrentDevice,
                    kAudioUnitScope_Global,
                    0,
                    ptr::from_ref(&device.id).cast(),
                    size_of::<u32>() as u32,
                )
            })?;
        }
        check()?;
        if unit_device(&node)? != device.id {
            return Err(OpenError::Unverified("系统没有采用所选麦克风。".into()));
        }
        check()?;
        let format = unsafe { node.outputFormatForBus(0) };
        let prepared = prepare(&format)?;
        check()?;
        let gate = Arc::new(TapGate::default());
        let sink = gate.clone();
        let tap = block2::RcBlock::new(
            move |buffer: NonNull<AVAudioPCMBuffer>, _time: NonNull<AVAudioTime>| {
                autoreleasepool(|_| sink.consume(unsafe { buffer.as_ref() }));
            },
        );
        unsafe {
            node.installTapOnBus_bufferSize_format_block(
                0,
                1024,
                None,
                block2::RcBlock::as_ptr(&tap),
            );
        }
        let observed = gate.clone();
        let notification =
            block2::RcBlock::new(move |_: NonNull<NSNotification>| observed.invalidate());
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(AVAudioEngineConfigurationChangeNotification),
                Some(&engine),
                None,
                &notification,
            )
        };
        let native = NativeInput {
            engine,
            node,
            observer,
        };
        // Construct the owner before prepare/start so failures also close the tap.
        let input = Self {
            native: InputEngine::Native(native),
            format,
            gate,
            default_watch: None,
            next_check: Instant::now(),
        };
        {
            let native = match &input.native {
                InputEngine::Native(native) => native,
                #[cfg(debug_assertions)]
                InputEngine::Synthetic(_) => unreachable!(),
            };
            check()?;
            unsafe { native.engine.prepare() };
            check()?;
            unsafe {
                native
                    .engine
                    .startAndReturnError()
                    .map_err(|error| OpenError::Unavailable(super::native_error(error)))?;
            }
        }
        check()?;
        if !input.matches(device) || input.gate.1.load(Ordering::Acquire) {
            return Err(OpenError::Unverified(
                "无法确认所选麦克风已使用预期格式打开。".into(),
            ));
        }
        check()?;
        Ok((input, prepared))
    }

    pub fn verify_start(
        &mut self,
        route: &Route,
        check: &impl Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        check()?;
        if route.follows_default {
            let admission = Arc::new(DefaultAdmission::default());
            let listener = match &self.native {
                InputEngine::Native(_) => Some(DefaultListener::new(&admission)?),
                #[cfg(debug_assertions)]
                InputEngine::Synthetic(input) => {
                    input.backend.watch_default(&admission);
                    None
                }
            };
            self.default_watch = Some(DefaultWatch {
                id: route.device.id,
                admission,
                _listener: listener,
            });
            check()?;
        }
        let same_input = self.matches(&route.device);
        check()?;
        let same_default = !route.follows_default || self.default_still_matches(&route.device);
        check()?;
        if !same_input
            || !same_default
            || self.gate.1.load(Ordering::Acquire)
            || self
                .default_watch
                .as_ref()
                .is_some_and(|watch| !watch.admission.pending())
        {
            return Err(INPUT_CHANGED.into());
        }
        Ok(())
    }

    pub fn attach(
        &self,
        check: &impl Fn() -> Result<(), String>,
        accepting: impl Fn() -> bool + Send + Sync + 'static,
        consume: impl Fn(&AVAudioPCMBuffer) + Send + Sync + 'static,
    ) -> Result<(), OpenError> {
        check()?;
        if let Some(watch) = &self.default_watch {
            // Details/UID and Speech setup can block. Read only the default ID
            // here, outside the tap lock, with no further route lookup.
            let current_id = match &self.native {
                InputEngine::Native(_) => Hal::default_input_id(),
                #[cfg(debug_assertions)]
                InputEngine::Synthetic(input) => Ok(input.backend.final_default_input_id()),
            };
            check()?;
            if current_id != Ok(watch.id) {
                return Err(INPUT_CHANGED.into());
            }
        }
        let mut slot = self.gate.0.lock().expect("newly opened tap gate");
        // Resolve default-route invalidation before distinguishing a closed
        // capture from an unverified route. The lock keeps callbacks out.
        if self
            .default_watch
            .as_ref()
            .is_some_and(|watch| !watch.admission.admit())
            || self.gate.1.load(Ordering::Acquire)
        {
            return Err(INPUT_CHANGED.into());
        }
        if !accepting() {
            return Err(OpenError::AdmissionClosed);
        }
        *slot = Some(Box::new(move |buffer| {
            if accepting() {
                consume(buffer);
            }
        }));
        #[cfg(debug_assertions)]
        if let InputEngine::Synthetic(input) = &self.native {
            input.attached();
        }
        Ok(())
    }

    fn default_still_matches(&self, device: &Device) -> bool {
        let selected = match &self.native {
            InputEngine::Native(_) => Hal.system_input(),
            #[cfg(debug_assertions)]
            InputEngine::Synthetic(input) => input.backend.system_input(),
        };
        selected.is_ok_and(|selected| selected.id == device.id && selected.uid == device.uid)
    }

    fn matches(&self, device: &Device) -> bool {
        match &self.native {
            InputEngine::Native(input) => {
                (unsafe { input.engine.isRunning() })
                    && unit_device(&input.node).is_ok_and(|id| {
                        id == device.id
                            && alive(id) == Ok(true)
                            && identity(id).as_ref() == Ok(&device.uid)
                    })
                    && unsafe { input.node.outputFormatForBus(0) } == self.format
            }
            #[cfg(debug_assertions)]
            InputEngine::Synthetic(input) => input.healthy(),
        }
    }

    /// A previously admitted tap can be salvaged only after positive loss
    /// evidence, not after an unexplained device/UID/format readback failure.
    pub fn revoke_and_confirm_loss(&self, route: &Route) -> bool {
        let invalidated = self.gate.1.load(Ordering::Acquire);
        self.gate.close();
        if invalidated {
            return true;
        }
        match &self.native {
            InputEngine::Native(_) => {
                alive(route.device.id) == Ok(false)
                    || Hal.inputs().is_ok_and(|devices| {
                        devices.iter().all(|device| device.uid != route.device.uid)
                    })
            }
            #[cfg(debug_assertions)]
            InputEngine::Synthetic(input) => !input.healthy(),
        }
    }

    pub fn healthy(&mut self, route: &Route, force: bool) -> bool {
        if self.gate.1.load(Ordering::Acquire) {
            return false;
        }
        #[cfg(debug_assertions)]
        if let InputEngine::Synthetic(input) = &self.native {
            input.pulse(&self.gate);
        }
        let now = Instant::now();
        if !force && now < self.next_check {
            return true;
        }
        self.next_check = now + Duration::from_millis(350);
        self.matches(&route.device)
    }
}

impl Drop for OpenInput {
    fn drop(&mut self) {
        self.gate.close();
        match &self.native {
            InputEngine::Native(input) => unsafe {
                NSNotificationCenter::defaultCenter().removeObserver(input.observer.as_ref());
                input.engine.stop();
                input.node.removeTapOnBus(0);
            },
            #[cfg(debug_assertions)]
            InputEngine::Synthetic(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_taps_cannot_feed_new_source_or_next_session() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (_, buffer) = super::super::tests::synthetic_buffer(48_000., 1, 32);
        let frames = Arc::new(AtomicUsize::new(0));
        let monitor = |gate: &TapGate| {
            let frames = frames.clone();
            *gate.0.lock().unwrap() = Some(Box::new(move |_| {
                frames.fetch_add(1, Ordering::Relaxed);
            }));
        };
        let old = TapGate::default();
        monitor(&old);
        old.consume(&buffer);
        old.close();
        let replacement = TapGate::default();
        old.consume(&buffer);
        replacement.consume(&buffer); // Unverified source is not attached yet.
        monitor(&replacement);
        old.consume(&buffer);
        replacement.consume(&buffer);
        assert_eq!(frames.load(Ordering::Relaxed), 2);
        replacement.invalidate(); // Engine configuration notification closes access immediately.
        replacement.consume(&buffer);
        replacement.close();
        replacement.consume(&buffer);
        assert_eq!(frames.load(Ordering::Relaxed), 2);
    }

    struct Fake {
        devices: Result<Vec<Device>, String>,
        default: Result<Device, String>,
    }
    impl Inventory for Fake {
        fn inputs(&self) -> Result<Vec<Device>, String> {
            self.devices.clone()
        }
        fn default_candidate(&self) -> Result<Option<Device>, String> {
            self.default.clone().map(Some)
        }
    }
    fn device(id: u32, uid: &str) -> Device {
        Device {
            id,
            uid: uid.into(),
            name: "同名输入".into(),
        }
    }

    #[test]
    fn same_name_and_replug_use_uid_not_name_or_cached_object_id() {
        let mut mock = Fake {
            devices: Ok(vec![device(1, "other"), device(2, "saved")]),
            default: Ok(device(1, "other")),
        };
        let (route, id) =
            open_route(&mock, Some("saved"), false, &|| Ok(()), |d| Ok(d.id)).unwrap();
        assert_eq!(id, 2);
        assert_eq!(route.fallback, None);
        mock.devices = Ok(vec![device(2, "other"), device(7, "saved")]);
        mock.default = Ok(device(2, "other"));
        let (route, id) = open_route(&mock, Some("saved"), true, &|| Ok(()), |d| Ok(d.id)).unwrap();
        assert_eq!(id, 7);
        assert!(route.recovered);
    }

    #[test]
    fn disconnected_or_closed_lid_falls_back_only_after_default_opens() {
        let mock = Fake {
            devices: Ok(vec![device(3, "external")]),
            default: Ok(device(3, "external")),
        };
        assert!(
            open_route(&mock, Some("built-in"), true, &|| Ok(()), |_| Err::<(), _>(
                "default would not open".into()
            ))
            .is_err()
        );
        let (route, ()) =
            open_route(&mock, Some("built-in"), true, &|| Ok(()), |_| Ok(())).unwrap();
        assert_eq!(route.device.uid, "external");
        assert_eq!(route.fallback, Some(Fallback::Missing));
        assert!(route.summary().contains("已恢复录音"));
    }

    #[test]
    fn enumeration_failure_is_not_absence_and_default_is_checked_independently() {
        let mut mock = Fake {
            devices: Err("enumeration failed".into()),
            default: Ok(device(6, "external")),
        };
        let (route, opened) =
            open_route(&mock, Some("built-in"), false, &|| Ok(()), |d| Ok(d.id)).unwrap();
        assert_eq!(opened, 6);
        assert_eq!(route.fallback, Some(Fallback::InventoryFailed));
        let (route, _) = open_route(&mock, None, false, &|| Ok(()), |d| Ok(d.id)).unwrap();
        assert_eq!(route.fallback, None);
        mock.default = Err("none".into());
        assert!(open_route(&mock, Some("built-in"), false, &|| Ok(()), |_| Ok(())).is_err());
    }

    #[test]
    fn only_retries_a_distinct_verified_candidate_after_open_failure() {
        let mut mock = Fake {
            devices: Ok(vec![device(1, "saved")]),
            default: Ok(device(2, "default")),
        };
        let mut attempted = Vec::new();
        let (route, ()) = open_route(&mock, Some("saved"), true, &|| Ok(()), |d| {
            attempted.push(d.id);
            if d.id == 1 {
                Err("gone".into())
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(attempted, [1, 2]);
        assert_eq!(route.fallback, Some(Fallback::OpenFailed));
        mock.default = Ok(device(1, "saved"));
        let mut calls = 0;
        assert!(
            open_route(&mock, Some("saved"), false, &|| Ok(()), |_| {
                calls += 1;
                Err::<(), _>("gone".into())
            })
            .is_err()
        );
        assert_eq!(calls, 1);
    }
}
