//! Native microphone settings. Inventory refresh only reads HAL; recording is
//! exclusively owned by the controller after an explicit test-button action.
use super::*;
use crate::audio::devices::{self, Device};
use objc2::Message;

pub(super) fn preference(defaults: &NSUserDefaults) -> Option<String> {
    defaults
        .stringForKey(ns_string!("microphoneDeviceUID"))
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn set_preference(c: &mut Controller, uid: Option<String>) {
    let defaults = NSUserDefaults::standardUserDefaults();
    if let Some(uid) = &uid {
        unsafe {
            defaults.setObject_forKey(
                Some(&NSString::from_str(uid)),
                ns_string!("microphoneDeviceUID"),
            );
        }
    } else {
        defaults.removeObjectForKey(ns_string!("microphoneDeviceUID"));
    }
    c.microphone_uid = uid;
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Choice {
    System,
    Device(Device),
    Retained(String),
}

fn choices(result: &Result<Vec<Device>, String>, saved: Option<&str>) -> (Vec<Choice>, usize) {
    let mut rows = vec![Choice::System];
    if let Ok(inputs) = result {
        rows.extend(inputs.iter().cloned().map(Choice::Device));
    }
    let position = saved.map_or(0, |uid| {
        rows.iter()
            .position(|choice| matches!(choice, Choice::Device(d) if d.uid == uid))
            .unwrap_or_else(|| {
                rows.push(Choice::Retained(uid.to_owned()));
                rows.len() - 1
            })
    });
    (rows, position)
}

pub(super) struct State {
    owner: Retained<Delegate>,
    window: OnceCell<Retained<NSWindow>>,
    selection: OnceCell<Retained<NSPopUpButton>>,
    inventory_note: OnceCell<Retained<NSTextField>>,
    actual: OnceCell<Retained<NSTextField>>,
    level: OnceCell<Retained<NSTextField>>,
    test: OnceCell<Retained<NSButton>>,
    rows: RefCell<Vec<Choice>>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = State]
    pub(super) struct Panel;
    unsafe impl NSObjectProtocol for Panel {}
    unsafe impl NSWindowDelegate for Panel {
        #[unsafe(method(windowWillClose:))]
        fn will_close(&self, _notice: &NSNotification) {
            let keep = self.retain();
            keep.window().setDelegate(None);
            let owner = &keep.ivars().owner;
            owner.with_controller(|c| c.stop_microphone_test());
            owner.ivars().microphone_panel.borrow_mut().take();
        }
    }
    impl Panel {
        #[unsafe(method(refreshMicrophones:))]
        fn refresh_action(&self, _sender: Option<&AnyObject>) { self.inventory(); }

        #[unsafe(method(chooseMicrophone:))]
        fn choose_action(&self, _sender: Option<&AnyObject>) {
            let owner = &self.ivars().owner;
            let index = self.ivars().selection.get().unwrap().indexOfSelectedItem() as usize;
            let rows = self.ivars().rows.borrow();
            let uid = match rows.get(index) {
                Some(Choice::System) => None,
                Some(Choice::Device(device)) => Some(device.uid.clone()),
                _ => return,
            };
            drop(rows);
            let allowed = self.selection_allowed();
            owner.with_controller(|c| {
                if !c.busy() && allowed {
                    set_preference(c, uid);
                }
            });
            self.inventory();
        }

        #[unsafe(method(testMicrophoneLevel:))]
        fn test_action(&self, _sender: Option<&AnyObject>) {
            let owner = &self.ivars().owner;
            let allowed = self.selection_allowed();
            owner.with_controller(|c| {
                if c.microphone_test_running() { c.stop_microphone_test(); }
                else if allowed { c.start_microphone_test(); }
            });
            self.refresh();
        }
    }
);

impl Panel {
    fn window(&self) -> &NSWindow {
        self.ivars().window.get().unwrap()
    }

    fn selection_allowed(&self) -> bool {
        let owner = &self.ivars().owner;
        !owner.settings_locked()
            && !owner.ivars().quitting.get()
            && owner.ivars().permission_rx.borrow().is_none()
            && owner.ivars().pending_hotkey.borrow().is_none()
            && !owner.ivars().control.borrow().preparing()
    }

    fn inventory(&self) {
        let owner = &self.ivars().owner;
        let uid = owner
            .ivars()
            .controller
            .borrow()
            .as_ref()
            .and_then(|c| c.microphone_uid.clone());
        let result = devices::inputs();
        let (rows, index) = choices(&result, uid.as_deref());
        let select = self.ivars().selection.get().unwrap();
        select.removeAllItems();
        for row in &rows {
            let title = match row {
                Choice::System => "跟随系统输入（默认）".into(),
                Choice::Device(device) => device.label(),
                Choice::Retained(_) => "已指定设备 · 当前不可确认（保留偏好）".into(),
            };
            select.addItemWithTitle(&NSString::from_str(&title));
        }
        select.selectItemAtIndex(index as isize);
        self.ivars().rows.replace(rows);
        self.ivars()
            .inventory_note
            .get()
            .unwrap()
            .setStringValue(&NSString::from_str(match &result {
                Ok(list) if list.is_empty() => "未发现可用输入。请检查连接后刷新设备。",
                Ok(_) => "同名设备末尾的标识用于区分；临时回退不会更改偏好。",
                Err(_) => "无法读取设备列表；未更改原偏好。请尝试刷新设备。",
            }));
        self.refresh();
    }

    pub(super) fn refresh(&self) {
        let owner = &self.ivars().owner;
        let slot = owner.ivars().controller.borrow();
        let Some(c) = slot.as_ref() else {
            return;
        };
        let running = c.microphone_test_running();
        let allowed = self.selection_allowed();
        self.ivars()
            .selection
            .get()
            .unwrap()
            .setEnabled(!c.busy() && allowed);
        let test = self.ivars().test.get().unwrap();
        test.setEnabled(running || (!c.busy() && allowed));
        test.setTitle(if running {
            ns_string!("停止测试")
        } else {
            ns_string!("测试电平 · 10 秒")
        });
        let actual = c.microphone_route().map_or_else(
            || "本轮实际输入：尚无已确认打开的麦克风".into(),
            |route| {
                if c.state.phase == Phase::Recording {
                    format!("本轮实际输入：{route}")
                } else {
                    format!("本轮录音使用：{route}")
                }
            },
        );
        let label = self.ivars().actual.get().unwrap();
        label.setStringValue(&NSString::from_str(&actual));
        label.setToolTip(Some(&NSString::from_str(&actual)));
        let level = self.ivars().level.get().unwrap();
        level.setStringValue(&NSString::from_str(c.microphone_test_status()));
        level.setToolTip(Some(&NSString::from_str(c.microphone_test_status())));
    }

    pub(super) fn close(&self) {
        self.window().close();
    }
}

pub(super) fn show(owner: &Delegate) {
    if let Some(panel) = owner.ivars().microphone_panel.borrow().as_ref() {
        panel.inventory();
        panel.window().makeKeyAndOrderFront(None);
        return;
    }
    let mtm = owner.mtm();
    let panel = Panel::alloc(mtm).set_ivars(State {
        owner: owner.retain(),
        window: OnceCell::new(),
        selection: OnceCell::new(),
        inventory_note: OnceCell::new(),
        actual: OnceCell::new(),
        level: OnceCell::new(),
        test: OnceCell::new(),
        rows: RefCell::default(),
    });
    let panel: Retained<Panel> = unsafe { msg_send![super(panel), init] };
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect(0., 0., 500., 241.),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(ns_string!("麦克风设置"));
    window.setDelegate(Some(ProtocolObject::from_ref(&*panel)));
    let view = window.contentView().unwrap();
    owner.label(&view, "听写使用的麦克风", rect(24., 202., 452., 24.), 15.);
    let selection = owner.popup(
        &view,
        &[],
        sel!(chooseMicrophone:),
        rect(22., 161., 336., 30.),
    );
    selection.setAccessibilityLabel(Some(ns_string!("听写麦克风偏好")));
    selection.setAccessibilityIdentifier(Some(ns_string!("voiceport.microphone.preference")));
    let refresh = owner.button(
        &view,
        "刷新设备",
        sel!(refreshMicrophones:),
        365.,
        161.,
        112.,
    );
    unsafe {
        selection.setTarget(Some(&*panel));
        refresh.setTarget(Some(&*panel));
    }
    let inventory_note = owner.caption(&view, "", rect(24., 135., 452., 22.));
    let actual = owner.label(&view, "", rect(24., 101., 452., 28.), 12.);
    actual.setAccessibilityIdentifier(Some(ns_string!("voiceport.microphone.actual-route")));
    let test = owner.button(
        &view,
        "测试电平 · 10 秒",
        sel!(testMicrophoneLevel:),
        20.,
        64.,
        152.,
    );
    test.setAccessibilityIdentifier(Some(ns_string!("voiceport.microphone.test")));
    unsafe {
        test.setTarget(Some(&*panel));
    }
    owner.caption(
        &view,
        "只显示峰值，不保存音频、不转写。",
        rect(179., 66., 290., 25.),
    );
    let level = owner.label(&view, "", rect(24., 20., 452., 35.), 12.);
    level.setAccessibilityIdentifier(Some(ns_string!("voiceport.microphone.level")));
    level.setMaximumNumberOfLines(2);
    panel.ivars().window.set(window).ok();
    panel.ivars().selection.set(selection).ok();
    panel.ivars().inventory_note.set(inventory_note).ok();
    panel.ivars().actual.set(actual).ok();
    panel.ivars().test.set(test).ok();
    panel.ivars().level.set(level).ok();
    panel.inventory();
    panel.window().center();
    panel.window().makeKeyAndOrderFront(None);
    owner.ivars().microphone_panel.replace(Some(panel));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_saved_microphone_remains_selected_not_system_default() {
        let (rows, selected) = choices(&Err("offline".into()), Some("previous"));
        assert_eq!(
            rows,
            vec![Choice::System, Choice::Retained("previous".into())]
        );
        assert_eq!(selected, 1);
        assert_eq!(choices(&Ok(vec![]), None), (vec![Choice::System], 0));
    }
}
