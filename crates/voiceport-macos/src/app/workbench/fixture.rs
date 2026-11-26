//! Offscreen AppKit checks. The fixture installs no app delegate and creates no controller or device.
use super::*;

pub(in crate::app) fn isolated_regression(mtm: MainThreadMarker) {
    if std::env::var("VOICEPORT_SHELL_ACCEPTANCE").as_deref() == Ok("isolated-gui")
        && objc2_foundation::NSBundle::mainBundle().bundleIdentifier().as_deref()
            == Some(objc2_foundation::ns_string!("org.voiceport.workbench-focus-fixture"))
    {
        accept_shell_gui::run(mtm);
        return;
    }
    let _application = NSApplication::sharedApplication(mtm);
    for language in [Language::Chinese, Language::English] {
        let (window, workbench) = Workbench::build(mtm, language);
        let sidebar = workbench.sidebar();
        for page in ROWS.iter().filter_map(|row| row.page()) {
            let view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
            let field =
                NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(8., 8., 200., 24.));
            view.addSubview(&field);
            workbench.mount(page, &view, &[&field]);
        }
        window.contentView().unwrap().layoutSubtreeIfNeeded();
        assert!(!window.isVisible());
        assert!(!window.isKeyWindow());
        assert_eq!(sidebar.numberOfRows(), 10);
        assert_eq!(
            NSAccessibility::accessibilityLabel(sidebar)
                .unwrap()
                .to_string(),
            language.text("工作台导航", "Workbench Navigation")
        );
        let delegate: &ProtocolObject<dyn NSTableViewDelegate> =
            ProtocolObject::from_ref(&*workbench);
        let sidebar_view: &NSView = sidebar;
        for (index, row) in ROWS.into_iter().enumerate() {
            let cell = sidebar
                .viewAtColumn_row_makeIfNecessary(0, index as isize, true)
                .unwrap()
                .downcast::<NSTableCellView>()
                .unwrap();
            let title = unsafe { cell.textField() }
                .unwrap()
                .stringValue()
                .to_string();
            match row {
                Row::Group(zh, en) => {
                    assert_eq!(title, language.text(zh, en));
                    assert!(!delegate.tableView_shouldSelectRow(sidebar, index as isize));
                }
                Row::Page(page) => {
                    assert_eq!(title, page.title(language));
                    assert!(delegate.tableView_shouldSelectRow(sidebar, index as isize));
                    assert_eq!(
                        NSAccessibility::accessibilityIdentifier(&*cell)
                            .unwrap()
                            .to_string(),
                        format!("voiceport.workbench.nav.{}", page.identifier())
                    );
                }
            }
        }
        assert!(std::ptr::eq(
            &*window.initialFirstResponder().unwrap(),
            sidebar_view
        ));
        workbench.select(Page::Result);
        let pages = workbench.ivars().pages.borrow();
        let result = &pages[&Page::Result];
        assert!(!result.view.isHidden());
        assert_eq!(sidebar.selectedRow(), 1);
        assert_eq!(
            NSAccessibility::accessibilityRole(&*result.view).as_deref(),
            Some(unsafe { NSAccessibilityGroupRole })
        );
        assert_eq!(
            unsafe { sidebar.nextValidKeyView() }.as_deref(),
            Some(&*result.key_views[0])
        );
        assert_eq!(
            unsafe { result.key_views[0].nextValidKeyView() }.as_deref(),
            Some(sidebar_view)
        );
        assert!(window.makeFirstResponder(Some(sidebar)));
        window.selectNextKeyView(None);
        let editor = result.key_views[0]
            .downcast_ref::<NSTextField>()
            .unwrap()
            .currentEditor()
            .expect("Tab focuses the result field editor");
        let editor_responder: &NSResponder = &editor;
        workbench.select(Page::Result);
        assert_eq!(window.firstResponder().as_deref(), Some(editor_responder));
        workbench.select(Page::Files);
        let sidebar_responder: &NSResponder = sidebar;
        assert_eq!(window.firstResponder().as_deref(), Some(sidebar_responder));
        workbench.select(Page::Result);

        // Send the events directly to the table, without posting to the desktop.
        for (character, code, expected) in [
            ("\u{f701}", 125, Page::History),
            ("\u{f701}", 125, Page::Files),
            ("\u{f701}", 125, Page::Sources),
            ("\u{f700}", 126, Page::Files),
        ] {
            let character = NSString::from_str(character);
            let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::KeyDown,
                objc2_foundation::NSPoint::ZERO,
                NSEventModifierFlags::Function | NSEventModifierFlags::NumericPad,
                0.,
                window.windowNumber(),
                None,
                &character,
                &character,
                false,
                code,
            ).unwrap();
            sidebar.keyDown(&event);
            assert_eq!(workbench.ivars().selected.get(), Some(expected));
        }
        assert!(result.view.isHidden());
        assert_eq!(
            unsafe { sidebar.previousValidKeyView() }.as_deref(),
            Some(&*pages[&Page::Files].key_views[0])
        );

        let themes = unsafe { [NSAppearanceNameAqua, NSAppearanceNameDarkAqua] };
        let theme_names = objc2_foundation::NSArray::from_slice(&themes);
        for theme in themes {
            let appearance = NSAppearance::appearanceNamed(theme).unwrap();
            window.setAppearance(Some(&appearance));
            // Native source lists may inherit a vibrant variant of the window's theme.
            for view in [sidebar_view, &*pages[&Page::Files].view] {
                assert_eq!(
                    view.effectiveAppearance()
                        .bestMatchFromAppearancesWithNames(&theme_names)
                        .as_deref(),
                    Some(theme)
                );
            }
        }
        window.close();
        assert!(!window.isVisible());
        assert!(!window.isKeyWindow());
        assert_eq!(workbench.ivars().selected.get(), Some(Page::Files));
        workbench.select(Page::Files);
        assert!(!pages[&Page::Files].view.isHidden());
        assert_eq!(sidebar.selectedRow(), 3);
    }
    let owner = crate::app::Delegate::new(mtm);
    let window = owner.prepare_workbench(None);
    let shell = owner.ivars().workbench.get().unwrap();
    assert_eq!(shell.ivars().selected.get(), Some(Page::Result));
    {
        let pages = shell.ivars().pages.borrow();
        assert_eq!(pages.len(), 7);
        for page in ROWS.iter().filter_map(|row| row.page()) {
            assert!(std::ptr::eq(window, owner.prepare_workbench(Some(page))));
            let mounted = &pages[&page];
            assert_eq!(shell.ivars().selected.get(), Some(page));
            assert!(!mounted.view.isHidden());
            assert!(
                mounted
                    .view
                    .window()
                    .is_some_and(|host| std::ptr::eq(&*host, window))
            );
            assert_eq!(
                unsafe { shell.sidebar().nextKeyView() }.as_deref(),
                mounted.key_views.first().map(|key| &**key)
            );
            for (&other, hidden) in pages.iter().filter(|&(&other, _)| other != page) {
                assert!(hidden.view.isHidden(), "{other:?} visible on {page:?}");
            }
        }
        let device = owner.ivars().microphone_panel.get().unwrap();
        assert!(pages[&Page::Device].view.bounds().size.height >= 358.);
        assert!(std::ptr::eq(&*pages[&Page::Device].view, &*device.view()));
        let files = owner.file_page_with_bounds(shell.page_bounds());
        assert!(std::ptr::eq(
            &*pages[&Page::Files].view,
            &*files.content_view()
        ));
    }
    owner.select_page(true);
    assert_eq!(shell.ivars().selected.get(), Some(Page::Settings));
    owner.select_page(false);
    assert_eq!(shell.ivars().selected.get(), Some(Page::Result));
    assert!(std::ptr::eq(
        window,
        owner.prepare_workbench(Some(Page::Device))
    ));
    window.close();
    assert!(std::ptr::eq(window, owner.prepare_workbench(None)));
    assert_eq!(shell.ivars().selected.get(), Some(Page::Device));
    assert!(!window.isVisible() && !window.isKeyWindow());
    println!(
        "workbench_native: native groups, bilingual names, selection, key loop, appearance and production window reuse passed offscreen"
    );
}

#[path = "accept_shell_gui.rs"]
mod accept_shell_gui;
