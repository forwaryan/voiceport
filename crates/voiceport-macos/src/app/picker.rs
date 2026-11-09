//! Native source/plugin rows use stable IDs; display titles may legitimately repeat.
use objc2::{MainThreadOnly, rc::Retained};
use objc2_app_kit::{NSMenu, NSMenuItem, NSPopUpButton};
use objc2_foundation::{NSString, ns_string};

pub(super) fn set_rows(control: &NSPopUpButton, rows: &[(String, String)]) {
    let menu = NSMenu::new(control.mtm());
    menu.setAutoenablesItems(false);
    for (id, title) in rows {
        // NSPopUpButton.addItemWithTitle removes an existing equal title.
        // Inserting distinct menu objects preserves every row, including duplicates.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(control.mtm()),
                &NSString::from_str(title),
                None,
                ns_string!(""),
            )
        };
        // This menu owns only NSString IDs; readers also check the object type.
        unsafe { item.setRepresentedObject(Some(&NSString::from_str(id))) };
        menu.addItem(&item);
    }
    control.setMenu(Some(&menu));
    control.selectItem(menu.itemArray().firstObject().as_deref());
}

fn item_id(item: &NSMenuItem) -> Option<String> {
    item.representedObject()?
        .downcast::<NSString>()
        .ok()
        .map(|id| id.to_string())
}

pub(super) fn selected_id(control: &NSPopUpButton) -> Option<String> {
    let item = control.selectedItem()?;
    item_id(&item)
}

pub(super) fn select_id(control: &NSPopUpButton, id: &str) -> bool {
    if selected_id(control).as_deref() == Some(id) {
        return true;
    }
    let item: Option<Retained<NSMenuItem>> = control
        .itemArray()
        .iter()
        .find(|item| item_id(item).as_deref() == Some(id));
    // Missing identities stay unselected instead of silently choosing another source.
    control.selectItem(item.as_deref());
    item.is_some()
}

// AppKit controls must be exercised on the OS main thread, which libtest's
// worker threads do not provide. A standalone cfg(test) main-thread harness
// calls this entry, without launching the production app or showing a window.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod regression {
    use super::*;
    use objc2::{DefinedClass, MainThreadMarker, define_class, msg_send, sel};
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize};
    use std::cell::RefCell;

    define_class!(
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = RefCell<Option<String>>]
        struct SelectionTarget;
        unsafe impl NSObjectProtocol for SelectionTarget {}
        impl SelectionTarget {
            #[unsafe(method(pickerChanged:))]
            fn changed(&self, sender: &NSPopUpButton) {
                *self.ivars().borrow_mut() = selected_id(sender);
            }
        }
    );

    fn rows(values: &[(&str, &str)]) -> Vec<(String, String)> {
        values
            .iter()
            .map(|(id, title)| ((*id).into(), (*title).into()))
            .collect()
    }

    fn assert_rows(control: &NSPopUpButton, target: &SelectionTarget, rows: &[(String, String)]) {
        assert_eq!(control.numberOfItems(), rows.len() as isize);
        for (i, (id, title)) in rows.iter().enumerate() {
            let item = control.itemAtIndex(i as isize).unwrap();
            assert_eq!(item.title().to_string(), *title);
            assert!(item.isEnabled());
            // Select the actual row object as AppKit does, then use the same
            // represented-ID accessor as production source/edit/plugin actions.
            control.selectItem(Some(&item));
            assert_eq!(selected_id(control).as_deref(), Some(id.as_str()));
            assert!(select_id(control, id));
            assert_eq!(control.indexOfSelectedItem(), i as isize);
            *target.ivars().borrow_mut() = None;
            control
                .menu()
                .unwrap()
                .performActionForItemAtIndex(i as isize);
            assert_eq!(target.ivars().borrow().as_deref(), Some(id.as_str()));
        }
    }

    pub(crate) fn run(mtm: MainThreadMarker) {
        assert!(MainThreadMarker::new().is_some());
        // Only the harness application singleton, with no delegate, run loop or
        // window; needed for native menu target/action dispatch.
        let _app = NSApplication::sharedApplication(mtm);
        let target = SelectionTarget::alloc(mtm).set_ivars(RefCell::new(None));
        let target: Retained<SelectionTarget> = unsafe { msg_send![super(target), init] };
        let control = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(NSPoint::new(0., 0.), NSSize::new(480., 30.)),
            false,
        );
        unsafe {
            control.setTarget(Some(&target));
            control.setAction(Some(sel!(pickerChanged:)));
        }
        for (case, values) in [
            (
                "duplicate-source",
                vec![
                    ("chatgpt", "ChatGPT"),
                    ("source-a", "Work"),
                    ("source-b", "Work"),
                    ("source-c", "Personal"),
                ],
            ),
            (
                "builtin-collision",
                vec![
                    ("chatgpt", "ChatGPT"),
                    ("source-a", "ChatGPT"),
                    ("source-c", "Personal"),
                ],
            ),
            (
                "management-labels",
                vec![
                    ("source-a", "Work · http"),
                    ("source-b", "Work · http"),
                    ("source-c", "Personal · http"),
                ],
            ),
            (
                "plugin-labels",
                vec![
                    ("plugin-a", "Adapter · 1 · 停止后转写"),
                    ("plugin-b", "Adapter · 1 · 停止后转写"),
                    ("plugin-c", "Other · 1 · 停止后转写"),
                ],
            ),
        ] {
            let initial = rows(&values);
            set_rows(&control, &initial);
            assert_rows(&control, &target, &initial);
            for (id, _) in &initial {
                assert!(select_id(&control, id));
                assert_eq!(selected_id(&control).as_deref(), Some(id.as_str()));
                // Catalog refresh/reorder and editing display labels must keep
                // every stable identity, even when all labels become identical.
                let refreshed: Vec<_> = initial
                    .iter()
                    .rev()
                    .map(|(id, _)| (id.clone(), "Renamed".into()))
                    .collect();
                set_rows(&control, &refreshed);
                assert!(select_id(&control, id));
                assert_eq!(selected_id(&control).as_deref(), Some(id.as_str()));
                assert_rows(&control, &target, &refreshed);
            }
            println!("native picker {case}: exact rows, selection, refresh and rename passed");
        }
        let unavailable = rows(&[
            ("chatgpt", "ChatGPT"),
            ("source-deleted", "Personal（不可用）"),
        ]);
        set_rows(&control, &unavailable);
        assert!(select_id(&control, "source-deleted"));
        assert_eq!(selected_id(&control).as_deref(), Some("source-deleted"));
        assert!(!select_id(&control, "missing"));
        assert_eq!(selected_id(&control), None);
        assert_eq!(control.indexOfSelectedItem(), -1);
        set_rows(&control, &[]);
        assert_eq!(control.numberOfItems(), 0);
        assert_eq!(selected_id(&control), None);
        assert!(!select_id(&control, "chatgpt"));
        // Malformed/non-ID native items fail closed instead of falling back to
        // their title or their position in a separately held row array.
        set_rows(&control, &unavailable);
        let item = control.selectedItem().unwrap();
        unsafe { item.setRepresentedObject(None) };
        assert_eq!(selected_id(&control), None);
        unsafe { item.setRepresentedObject(Some(&NSObject::new())) };
        assert_eq!(selected_id(&control), None);
        println!(
            "native picker unavailable/missing/empty/invalid identity: passed; os_main_thread=true"
        );
    }
}
