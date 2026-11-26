//! The input-device page presents observations from the shared audio lane.
//! Only an explicit level-test action can request audio capture here.
use super::*;
use objc2::rc::Weak;
use objc2_app_kit::NSAutoresizingMaskOptions as Mask;
use std::time::Instant;

mod presentation;
use presentation::{Activity, Catalog, Choice};

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

struct Controls {
    root: Retained<NSView>,
    selection: Retained<NSPopUpButton>,
    refresh: Retained<NSButton>,
    inventory: Retained<NSTextField>,
    system: Retained<NSTextField>,
    permission: Retained<NSTextField>,
    activity: Retained<NSTextField>,
    actual: Retained<NSTextField>,
    test: Retained<NSButton>,
    level: Retained<NSTextField>,
}

pub(super) struct State {
    owner: Weak<Delegate>,
    controls: OnceCell<Controls>,
    was_visible: Cell<bool>,
    catalog: RefCell<Catalog>,
    rows: RefCell<Vec<Choice>>,
    test_id: RefCell<Option<String>>,
    test_start_error: RefCell<Option<String>>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = State]
    pub(super) struct Panel;
    unsafe impl NSObjectProtocol for Panel {}
    impl Panel {
        #[unsafe(method(refreshMicrophones:))]
        fn refresh_action(&self, _sender: Option<&AnyObject>) {
            self.request_snapshot();
        }

        #[unsafe(method(chooseMicrophone:))]
        fn choose_action(&self, _sender: Option<&AnyObject>) {
            let index = self.controls().selection.indexOfSelectedItem() as usize;
            let choice = self.ivars().rows.borrow().get(index).cloned();
            let catalog = self.ivars().catalog.borrow();
            let Some(owner) = self.ivars().owner.load() else { return };
            let allowed = self.visible() && selection_allowed(&owner);
            owner.with_controller(|c| {
                if !allowed || c.busy() || c.audio_input_busy() {
                    return;
                }
                match choice {
                    Some(Choice::System) => set_preference(c, None),
                    Some(Choice::Device(device)) if catalog.can_choose(&device.uid, Instant::now()) => {
                        set_preference(c, Some(device.uid));
                    }
                    _ => {}
                }
            });
            drop(catalog);
            self.refresh();
        }

        #[unsafe(method(testMicrophoneLevel:))]
        fn test_action(&self, _sender: Option<&AnyObject>) {
            let Some(owner) = self.ivars().owner.load() else { return };
            let allowed = self.visible() && selection_allowed(&owner);
            owner.with_controller(|c| {
                if self.owns_running_test(c) {
                    c.stop_microphone_test();
                } else if allowed && !c.busy() && !c.audio_input_busy() {
                    c.start_microphone_test();
                    let id = c.microphone_test_id();
                    self.ivars().test_id.replace(id.map(str::to_owned));
                    self.ivars().test_start_error.replace(
                        id.is_none().then(|| c.microphone_test_status().to_owned()),
                    );
                }
            });
            self.refresh();
        }
    }
);

impl Panel {
    fn controls(&self) -> &Controls {
        self.ivars().controls.get().unwrap()
    }

    pub(super) fn view(&self) -> Retained<NSView> {
        self.controls().root.clone()
    }

    pub(super) fn tab_views(&self) -> [&NSView; 3] {
        let ui = self.controls();
        [&ui.selection, &ui.refresh, &ui.test]
    }

    fn visible(&self) -> bool {
        let root = &self.controls().root;
        !root.isHiddenOrHasHiddenAncestor()
            && root
                .window()
                .is_some_and(|w| w.isVisible() && !w.isMiniaturized())
    }

    fn stop_owned_test(&self, owner: &Delegate) {
        owner.with_controller(|c| {
            if self.owns_running_test(c) {
                c.stop_microphone_test();
            }
        });
    }

    fn owns_running_test(&self, c: &Controller) -> bool {
        c.microphone_test_id()
            .is_some_and(|running| Some(running) == self.ivars().test_id.borrow().as_deref())
    }

    fn request_snapshot(&self) {
        self.ivars().catalog.borrow_mut().request();
        self.refresh();
    }

    pub(super) fn refresh(&self) {
        let Some(owner) = self.ivars().owner.load() else {
            return;
        };
        let visible = self.visible();
        let was_visible = self.ivars().was_visible.replace(visible);
        if !visible {
            if was_visible {
                self.ivars().catalog.borrow_mut().suspend();
                self.stop_owned_test(&owner);
            }
            return;
        }
        let ui = self.controls();
        let slot = owner.ivars().controller.borrow();
        let Some(c) = slot.as_ref() else {
            return;
        };
        let allowed = selection_allowed(&owner);
        let mut catalog = self.ivars().catalog.borrow_mut();
        if !was_visible {
            catalog.request();
        }
        let can_read = allowed && c.state.phase != Phase::Permissions && !c.audio_input_busy();
        catalog.poll(can_read, || c.input_device_snapshot());
        let now = Instant::now();
        let (choices, index) = catalog.choices(c.microphone_uid.as_deref(), now);
        if *self.ivars().rows.borrow() != choices {
            ui.selection.removeAllItems();
            for row in &choices {
                ui.selection
                    .addItemWithTitle(&NSString::from_str(&row.label()));
            }
            self.ivars().rows.replace(choices);
        }
        if ui.selection.indexOfSelectedItem() != index as isize {
            ui.selection.selectItemAtIndex(index as isize);
        }

        let available = allowed && !c.busy() && !c.audio_input_busy();
        if ui.selection.isEnabled() != available {
            ui.selection.setEnabled(available);
        }
        let can_request = !catalog.requested();
        if ui.refresh.isEnabled() != can_request {
            ui.refresh.setEnabled(can_request);
        }
        let owns_test = self.owns_running_test(c);
        if ui.test.isEnabled() != (owns_test || available) {
            ui.test.setEnabled(owns_test || available);
        }
        let title = if owns_test {
            ns_string!("停止测试")
        } else {
            ns_string!("测试电平 · 10 秒")
        };
        if &*ui.test.title() != title {
            ui.test.setTitle(title);
        }

        let route = c.microphone_route();
        let activity = Activity::read(c, catalog.reading(), route.is_some());
        set_label(&ui.inventory, &catalog.note(now));
        set_label(&ui.system, &catalog.system(now));
        set_label(&ui.permission, &catalog.permission(now));
        set_label(&ui.activity, activity.label());
        set_label(&ui.actual, &activity.actual(route.as_deref()));
        let test_id = self.ivars().test_id.borrow();
        let start_error = self.ivars().test_start_error.borrow();
        let level = if owns_test && c.microphone_test_display_is_stale(now) {
            "电平读数已过期，等待输入设备响应…"
        } else {
            c.microphone_test_detail(test_id.as_deref())
                .or(start_error.as_deref())
                .unwrap_or("只测量峰值，不保存音频或转写。")
        };
        set_label(&ui.level, level);
    }

    pub(super) fn close(&self) {
        if let Some(owner) = self.ivars().owner.load() {
            self.stop_owned_test(&owner);
        }
    }
}

fn selection_allowed(owner: &Delegate) -> bool {
    !owner.settings_locked()
        && !owner.ivars().quitting.get()
        && owner.ivars().permission_rx.borrow().is_none()
        && owner.ivars().pending_hotkey.borrow().is_none()
        && !owner.ivars().control.borrow().preparing()
}

fn set_label(control: &NSTextField, value: &str) {
    if control.stringValue().to_string() != value {
        let value = NSString::from_str(value);
        control.setStringValue(&value);
        control.setToolTip(Some(&value));
    }
}

pub(super) fn build(owner: &Delegate, frame: NSRect) -> Retained<Panel> {
    let mtm = owner.mtm();
    let panel = Panel::alloc(mtm).set_ivars(State {
        owner: Weak::new(owner),
        controls: OnceCell::new(),
        was_visible: Cell::new(false),
        catalog: RefCell::default(),
        rows: RefCell::default(),
        test_id: RefCell::default(),
        test_start_error: RefCell::default(),
    });
    let panel: Retained<Panel> = unsafe { msg_send![super(panel), init] };
    let root = NSView::initWithFrame(NSView::alloc(mtm), frame);
    root.setAutoresizingMask(Mask::ViewWidthSizable | Mask::ViewHeightSizable);
    let top = frame.size.height - 358.;
    let width = frame.size.width;
    let wide = Mask::ViewMinYMargin | Mask::ViewWidthSizable;
    let heading = owner.label(
        &root,
        "听写输入偏好",
        rect(24., top + 316., width - 48., 24.),
        15.,
    );
    heading.setAutoresizingMask(wide);
    let selection = owner.popup(
        &root,
        &[],
        sel!(chooseMicrophone:),
        rect(22., top + 278., width - 166., 30.),
    );
    selection.setAutoresizingMask(wide);
    selection.setAccessibilityLabel(Some(ns_string!("听写麦克风偏好")));
    selection.setAccessibilityIdentifier(Some(ns_string!("voiceport.microphone.preference")));
    let refresh = owner.button(
        &root,
        "刷新设备",
        sel!(refreshMicrophones:),
        width - 136.,
        top + 278.,
        112.,
    );
    refresh.setAutoresizingMask(Mask::ViewMinYMargin | Mask::ViewMinXMargin);
    let inventory = owner.caption(&root, "", rect(24., top + 242., width - 48., 32.));
    inventory.setAutoresizingMask(wide);
    let status = |y, id: &NSString| {
        let label = owner.label(&root, "", rect(24., top + y, width - 48., 31.), 12.);
        label.setAutoresizingMask(wide);
        label.setAccessibilityIdentifier(Some(id));
        label.setMaximumNumberOfLines(2);
        label
    };
    let system = status(208., ns_string!("voiceport.microphone.system-input"));
    let permission = status(175., ns_string!("voiceport.microphone.permission"));
    let activity = status(142., ns_string!("voiceport.microphone.activity"));
    let actual = status(109., ns_string!("voiceport.microphone.actual-route"));
    let test = owner.button(
        &root,
        "测试电平 · 10 秒",
        sel!(testMicrophoneLevel:),
        20.,
        top + 67.,
        152.,
    );
    test.setAutoresizingMask(Mask::ViewMinYMargin);
    test.setAccessibilityIdentifier(Some(ns_string!("voiceport.microphone.test")));
    let level = status(27., ns_string!("voiceport.microphone.level"));
    unsafe {
        selection.setTarget(Some(&*panel));
        refresh.setTarget(Some(&*panel));
        test.setTarget(Some(&*panel));
    }
    panel
        .ivars()
        .controls
        .set(Controls {
            root,
            selection,
            refresh,
            inventory,
            system,
            permission,
            activity,
            actual,
            test,
            level,
        })
        .ok();
    panel
}
