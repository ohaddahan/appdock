//! Native fixture state and stage execution. AppKit callbacks may reenter:
//! release RefMut before focus, picker, rename, or window-order operations.
use super::*;
use crate::worker::Snapshot;
use std::cell::RefMut;
#[derive(Default)]
pub(super) struct FixtureState {
    pub(super) dock_test_stage: usize,
    pub(super) dock_test_tick: u64,
    pub(super) smoke_failed: bool,
    pub(super) fixture_child: Option<std::process::Child>,
    pub(super) fixture_rename_started: bool,
    pub(super) fixture_rename_tested: bool,
    pub(super) fixture_attach_pending: Option<WindowId>,
    pub(super) fixture_attach_after: u64,
    pub(super) fixture_picker_cancel_tested: bool,
    pub(super) fixture_reveal_started: bool,
    pub(super) fixture_reveal_tested: bool,
    pub(super) fixture_reveal_ordered: bool,
    pub(super) fixture_keyboard_ready: bool,
    pub(super) startup_menu_stage: u8,
}
impl Drop for FixtureState {
    fn drop(&mut self) {
        if let Some(mut child) = self.fixture_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Delegate {
    pub(super) fn fixture_picker_step<'a>(
        &self,
        count: u64,
        mut b: RefMut<'a, Option<Ui>>,
        _s: &Snapshot,
    ) -> Option<RefMut<'a, Option<Ui>>> {
        let u = b.as_mut()?;
        if u.picker_open
            && !u.pending_picker
            && count >= u.fixture.fixture_attach_after
            && let Some(id) = u.fixture.fixture_attach_pending.take()
        {
            if u.fixture.fixture_child.is_some() && !u.fixture.fixture_keyboard_ready {
                u.fixture.fixture_keyboard_ready = true;
                u.fixture.fixture_attach_pending = Some(id);
                u.fixture.fixture_attach_after = count + 2;
                drop(b);
                self.present_picker();
                return None;
            }
            u.fixture.fixture_keyboard_ready = false;
            let search = u.picker.search.clone();
            let test = u.fixture.fixture_child.is_some()
                && !std::env::args().any(|a| a == "--pointer-smoke");
            let cancel = test && !u.fixture.fixture_picker_cancel_tested;
            u.fixture.fixture_picker_cancel_tested |= test;
            if cancel {
                u.fixture.fixture_attach_pending = Some(id);
            }
            drop(b);
            if test {
                self.verify_rename_keyboard(&search);
                search.setStringValue(&NSString::from_str("__no_matching_window__"));
                self.filter();
                assert!(
                    self.ivars()
                        .ui
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .picker
                        .choice_ids
                        .is_empty(),
                    "Inline picker did not filter results"
                );
                search.setStringValue(&NSString::from_str(""));
                self.filter();
            }
            if cancel {
                self.dismiss_picker(true);
                assert!(
                    self.ivars()
                        .ui
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .picker
                        .view
                        .isHidden(),
                    "Cancel did not dismiss inline picker"
                );
                self.show_picker(false);
                return None;
            }
            assert!(
                self.ivars()
                    .ui
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .picker
                    .select_window(id),
                "Fixture target missing from inline picker"
            );
            self.attach_selected_window();
            println!("Integrated inline picker attachment submitted");
            return None;
        }
        Some(b)
    }
    pub(super) fn fixture_simple_step<'a>(
        &self,
        count: u64,
        mut b: RefMut<'a, Option<Ui>>,
        s: &Snapshot,
    ) -> Option<RefMut<'a, Option<Ui>>> {
        let u = b.as_mut()?;
        if std::env::args().any(|a| a == "--cleanup-smoke") && count == 12 {
            assert!(
                s.workspace.tabs.is_empty(),
                "Stale startup tabs were not removed"
            );
            assert!(
                s.live.is_empty() && s.selected.is_none(),
                "Fresh launch unexpectedly attached a window"
            );
            assert!(
                persistence::load(&persistence::path())
                    .unwrap()
                    .tabs
                    .is_empty(),
                "Startup cleanup was not persisted"
            );
            println!("Native startup cleanup passed: stale saved tabs removed and persisted");
            u.client.send(Command::Quit);
        }
        if std::env::args().any(|a| a == "--disconnected-smoke") {
            if count == 4 {
                drop(b);
                self.release_current();
                return None;
            }
            if count == 10 {
                assert!(
                    s.workspace.tabs.is_empty(),
                    "Disconnected tab was not removed"
                );
                println!("Disconnected tab release regression passed");
                u.client.send(Command::Quit);
            }
        }
        if std::env::args().any(|a| a == "--rename-smoke") {
            if count == 4 {
                let button = u
                    .tabs
                    .subviews()
                    .iter()
                    .find_map(|v| v.downcast::<TabButton>().ok())
                    .expect("Tab button missing");
                let number = u.window.windowNumber();
                drop(b);
                let event=NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(NSEventType::LeftMouseDown,NSPoint::new(20.,15.),NSEventModifierFlags::empty(),0.,number,None,1,2,1.).unwrap();
                unsafe {
                    let _: () = msg_send![&*button,mouseDown:&*event];
                }
                return None;
            }
            if count == 6 {
                let field = u
                    .rename_editor
                    .as_ref()
                    .expect("Double-click did not open inline rename")
                    .field
                    .clone();
                drop(b);
                self.verify_rename_keyboard(&field);
                field.setStringValue(&NSString::from_str("Discord - account 1"));
                self.finish_rename(true);
                return None;
            }
            if count == 10 {
                assert_eq!(s.workspace.tabs[0].name, "Discord - account 1");
                assert!(u.rename_editor.is_none());
                drop(b);
                self.begin_rename(7);
                return None;
            }
        }
        if std::env::args().any(|a| a == "--rename-smoke") {
            if count == 12 {
                let field = u.rename_editor.as_ref().unwrap().field.clone();
                drop(b);
                field.setStringValue(&NSString::from_str("Unsaved label"));
                self.finish_rename(false);
                return None;
            }
            if count == 16 {
                assert_eq!(s.workspace.tabs[0].name, "Discord - account 1");
                assert!(u.rename_editor.is_none());
                println!(
                    "Native rename regression passed: AppDock owns keyboard focus, Ctrl+A/Cmd+A select all, typing, commit and cancel"
                );
                u.client.send(Command::Quit);
            }
        }
        Some(b)
    }
    pub(super) fn fixture_docking_step<'a>(
        &self,
        count: u64,
        mut b: RefMut<'a, Option<Ui>>,
        s: &Snapshot,
    ) -> Option<RefMut<'a, Option<Ui>>> {
        let u = b.as_mut()?;
        if count == 8
            && std::env::args().any(|a| {
                matches!(
                    a.as_str(),
                    "--ui-smoke" | "--surface-smoke" | "--design-smoke"
                )
            })
        {
            let view = u.window.contentView().unwrap();
            let bounds = view.bounds();
            let bitmap = view
                .bitmapImageRepForCachingDisplayInRect(bounds)
                .expect("Native bitmap unavailable");
            view.cacheDisplayInRect_toBitmapImageRep(bounds, &bitmap);
            if std::env::args().any(|a| a == "--surface-smoke") {
                let w = bitmap.pixelsWide();
                let h = bitmap.pixelsHigh();
                let alpha = |y| bitmap.colorAtX_y(w / 2, y).unwrap().alphaComponent();
                assert!(
                    alpha(h / 2) < 0.01,
                    "App area must not obscure the external window"
                );
                assert!(
                    alpha(20) > 0.99 || alpha(h - 20) > 0.99,
                    "Tab strip must stay opaque"
                );
                assert!(
                    bitmap.colorAtX_y(2, h / 2).unwrap().alphaComponent() > 0.99
                        && bitmap.colorAtX_y(w - 3, h / 2).unwrap().alphaComponent() > 0.99,
                    "AppDock side frame is missing"
                );
                assert!(
                    alpha(2) > 0.99 && alpha(h - 3) > 0.99,
                    "AppDock top/bottom frame is missing"
                );
                println!(
                    "Native surface regression passed: transparent app area, opaque controls and surrounding frame"
                );
            }
            let data = unsafe {
                bitmap.representationUsingType_properties(
                    NSBitmapImageFileType::PNG,
                    &objc2_foundation::NSDictionary::new(),
                )
            }
            .expect("PNG encoding failed");
            let output =
                std::env::var("APPDOCK_SMOKE_SCREENSHOT").expect("Screenshot destination required");
            assert!(
                data.writeToFile_atomically(&NSString::from_str(&output), true),
                "Screenshot write failed"
            );
            println!("Native AppKit screenshot saved to {output}");
        }
        if std::env::args().any(|a| {
            matches!(
                a.as_str(),
                "--ui-smoke" | "--surface-smoke" | "--design-smoke"
            )
        }) && count
            == std::env::var("APPDOCK_SMOKE_TICKS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10)
        {
            u.client.send(Command::Quit);
        }
        if std::env::args().any(|a| {
            matches!(
                a.as_str(),
                "--dock-smoke" | "--picker-smoke" | "--frame-smoke" | "--pointer-smoke"
            )
        }) && u.fixture.dock_test_stage < 20
        {
            if s.status.starts_with("Paused:") || count > 400 {
                eprintln!(
                    "Integrated docking smoke failed at stage {}: {}",
                    u.fixture.dock_test_stage, s.status
                );
                u.fixture.smoke_failed = true;
                u.fixture.dock_test_stage = 20;
                u.client.send(Command::Quit);
            } else if count
                > u.fixture.dock_test_tick
                    + if u.fixture.fixture_child.is_some() {
                        2
                    } else {
                        8
                    }
            {
                let stage = u.fixture.dock_test_stage;
                if stage == 0 || stage == 1 {
                    let bundle = if stage == 0 {
                        "com.hnc.Discord"
                    } else {
                        "org.telegram.desktop"
                    };
                    let candidates: Vec<_> = s
                        .windows
                        .iter()
                        .filter(|w| {
                            w.eligible
                                && if let Some(child) = &u.fixture.fixture_child {
                                    w.pid == child.id() as i32 && !s.occupied.contains(&w.id)
                                } else {
                                    w.identity.bundle == bundle
                                }
                        })
                        .collect();
                    if (candidates.len() == 1
                        || (u.fixture.fixture_child.is_some() && !candidates.is_empty()))
                        && s.live.len() == stage
                        && (stage == 0 || s.selected == s.workspace.tabs.first().map(|t| t.id))
                    {
                        let window_id = candidates[0].id;
                        u.fixture.fixture_attach_pending = Some(window_id);
                        u.fixture.fixture_attach_after =
                            if stage == 0 && std::env::var_os("APPDOCK_PICKER_PREVIEW").is_some() {
                                count + 80
                            } else {
                                count
                            };
                        u.fixture.dock_test_stage += 1;
                        u.fixture.dock_test_tick = count;
                        let window = u.window.clone();
                        drop(b);
                        window.makeKeyAndOrderFront(None);
                        self.show_picker(false);
                        return None;
                    }
                } else if stage == 2
                    && std::env::args().any(|a| a == "--picker-smoke")
                    && s.live.len() == 2
                    && s.selected == s.workspace.tabs.last().map(|t| t.id)
                {
                    println!("Native picker regression complete; restoring windows");
                    u.client.send(Command::Quit);
                    u.fixture.dock_test_stage = 20;
                } else if (2..14).contains(&stage)
                    && s.live.len() == 2
                    && (stage == 2 || s.selected == Some(s.workspace.tabs[(stage - 3) % 2].id))
                {
                    if u.fixture.fixture_child.is_some() {
                        if !s.selected_frame.is_some_and(|frame| frame.near(s.area)) {
                            return None;
                        }
                        if std::env::args().any(|a| a == "--pointer-smoke") {
                            u.fixture.dock_test_stage = 14;
                            u.fixture.dock_test_tick = count;
                            return None;
                        }
                        if u.fixture.startup_menu_stage < 2 {
                            let mut menu_snapshot = s.clone();
                            let identity = &s.workspace.tabs[0].identity.bundle;
                            let bundle = if identity.is_empty() {
                                "dev.appdock.fixture"
                            } else {
                                identity.as_str()
                            };
                            // CLI fixture children have no app bundle. Supply their
                            // disposable identity to exercise the real menu action.
                            for window in &mut menu_snapshot.windows {
                                if u.fixture
                                    .fixture_child
                                    .as_ref()
                                    .is_some_and(|child| child.id() as i32 == window.pid)
                                {
                                    window.identity.bundle = bundle.to_owned();
                                }
                            }
                            self.update_startup_menu(&menu_snapshot);
                            let enabled = s
                                .workspace
                                .startup_apps
                                .iter()
                                .any(|app| app.bundle == bundle);
                            let stage = u.fixture.startup_menu_stage;
                            if (stage == 0 && !enabled) || (stage == 1 && enabled) {
                                let item = self
                                    .ivars()
                                    .startup_menu
                                    .get()
                                    .unwrap()
                                    .itemArray()
                                    .iter()
                                    .find(|item| {
                                        item.representedObject()
                                            .and_then(|o| o.downcast::<NSString>().ok())
                                            .is_some_and(|s| s.to_string() == bundle)
                                    })
                                    .expect("Startup app menu item missing");
                                u.fixture.startup_menu_stage += 1;
                                drop(b);
                                unsafe {
                                    let _: () = msg_send![self,toggleStartupApp:&*item];
                                }
                                return None;
                            }
                            return None;
                        }
                        if !s.workspace.startup_apps.is_empty() {
                            return None;
                        }
                        if !u.fixture.fixture_rename_tested {
                            if !u.fixture.fixture_rename_started {
                                u.fixture.fixture_rename_started = true;
                                u.client.send(Command::Raise);
                                let id = s.selected.unwrap();
                                drop(b);
                                self.begin_rename(id);
                                self.ivars()
                                    .ui
                                    .borrow()
                                    .as_ref()
                                    .unwrap()
                                    .client
                                    .send(Command::Raise);
                                return None;
                            }
                            if u.editing_focus_pending.is_some() {
                                return None;
                            }
                            // Exercise the production snapshot synchronization while
                            // a real native rename editor is open on its original tab.
                            let original = u.rename_editor.as_ref().unwrap().id;
                            let mut changed = s.clone();
                            changed.selected = s
                                .workspace
                                .tabs
                                .iter()
                                .map(|t| t.id)
                                .find(|id| *id != original);
                            sync_selection(u, &changed);
                            assert_eq!(u.rename_editor.as_ref().unwrap().id, original);
                            for view in u.tabs.subviews() {
                                if let Some(button) = view.downcast_ref::<TabButton>() {
                                    assert_eq!(
                                        button.ivars().active.get(),
                                        Some(button.tag() as TabId) == changed.selected
                                    );
                                }
                            }
                            sync_selection(u, s);
                            println!(
                                "Startup app menu toggle persisted and cleared the selected app rule"
                            );
                            println!(
                                "A2 snapshot update changed rendered selection and retained open rename target"
                            );
                            let field = u
                                .rename_editor
                                .as_ref()
                                .expect("Renaming ended while app was attached")
                                .field
                                .clone();
                            u.fixture.fixture_rename_tested = true;
                            u.fixture.dock_test_tick = count;
                            let manager = u.window.clone();
                            let number = s.backdrop.as_ref().unwrap().0;
                            let frame = s.selected_frame.unwrap();
                            drop(b);
                            self.verify_rename_keyboard(&field);
                            self.verify_app_pointer_routes(&manager, number, frame);
                            self.finish_rename(false);
                            println!(
                                "Attached-app rename retained keyboard focus despite queued Raise requests"
                            );
                            return None;
                        }
                        assert!(
                            s.workspace.geometry.height > 400.,
                            "Manager did not grow around native minimum size"
                        );
                    }
                    let wanted = s.workspace.tabs[(stage - 2) % 2].id;
                    u.client.switch(wanted);
                    println!("Integrated switch {} requested", stage - 1);
                    u.fixture.dock_test_stage += 1;
                    u.fixture.dock_test_tick = count;
                } else if stage == 14 && s.selected == Some(s.workspace.tabs[1].id) {
                    if u.fixture.fixture_child.is_some() && !u.fixture.fixture_reveal_tested {
                        if !u.fixture.fixture_reveal_started {
                            u.fixture.fixture_reveal_started = true;
                            u.fixture.dock_test_tick = count;
                            let window = u.window.clone();
                            let client = u.client.clone();
                            drop(b);
                            window.orderBack(None);
                            client.send(Command::Raise);
                            return None;
                        }
                        // The reveal itself verifies keyboard ownership immediately;
                        // desktop focus can legitimately change before this later stage.
                        u.fixture.dock_test_tick = count;
                        u.client.send(Command::Raise);
                        return None;
                    }
                    u.fixture.dock_test_stage = 15;
                    u.fixture.dock_test_tick = count;
                    let window = u.window.clone();
                    let mut f = window.frame();
                    f.origin.x += 35.;
                    f.size.width += 60.;
                    drop(b);
                    window.setFrame_display(f, true);
                    return None;
                } else if stage == 15 {
                    if std::env::args().any(|a| a == "--pointer-smoke") {
                        if !s.selected_frame.is_some_and(|frame| frame.near(s.area)) {
                            return None;
                        }
                        self.verify_app_pointer_routes(
                            &u.window,
                            s.backdrop.as_ref().unwrap().0,
                            s.selected_frame.unwrap(),
                        );
                    }
                    println!(
                        "Integrated docking and manager resize completed; releasing first tab"
                    );
                    if let Some(t) = s.workspace.tabs.first() {
                        u.client.send(Command::Release(t.id));
                    }
                    u.fixture.dock_test_stage = 16;
                    u.fixture.dock_test_tick = count;
                } else if stage == 16 && s.live.len() == 1 {
                    println!("Integrated release completed; restoring remaining window on exit");
                    u.client.send(Command::Quit);
                    u.fixture.dock_test_stage = 20;
                }
            }
        }
        Some(b)
    }
    pub(super) fn verify_app_pointer_routes(&self, window: &NSWindow, number: u32, frame: Rect) {
        let primary = NSScreen::screens(self.mtm())
            .firstObject()
            .unwrap()
            .frame()
            .size
            .height;
        for (x, y) in [(0.2, 0.3), (0.5, 0.5), (0.8, 0.8)] {
            let point = NSPoint::new(
                frame.x + frame.width * x,
                primary - frame.y - frame.height * y,
            );
            assert_eq!(
                NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(point, 0, self.mtm()),
                number as isize,
                "A window above the docked app is intercepting pointer input"
            );
        }
        let bounds = window.contentView().unwrap().bounds();
        let point = window
            .convertRectToScreen(rect(90., bounds.size.height - 16., 1., 1.))
            .origin;
        assert_eq!(
            NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(point, 0, self.mtm()),
            window.windowNumber(),
            "AppDock controls are not clickable"
        );
        println!(
            "Native pointer routing passed: app content targets the external app; tab controls target AppDock"
        );
    }
    pub(super) fn verify_rename_keyboard(&self, field: &NSTextField) {
        let editor = field
            .currentEditor()
            .expect("Rename field has no keyboard editor")
            .downcast::<RenameFieldEditor>()
            .expect("Wrong rename field editor");
        let app = NSApplication::sharedApplication(self.mtm());
        let foreground = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|a| a.processIdentifier());
        let child = self.ivars().ui.borrow().as_ref().and_then(|u| {
            u.fixture
                .fixture_child
                .as_ref()
                .map(|child| child.id() as i32)
        });
        println!(
            "Fixture keyboard context: active={}, key_window={}, foreground_manager={}, foreground_child={}",
            app.isActive(),
            field.window().is_some_and(|w| w.isKeyWindow()),
            foreground == Some(std::process::id() as i32),
            foreground == child
        );
        assert!(
            app.isActive(),
            "AppDock did not take keyboard focus for renaming"
        );
        let number = field.window().unwrap().windowNumber();
        let key = |modifiers, characters: &str| {
            NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                    NSEventType::KeyDown,NSPoint::new(0.,0.),modifiers,0.,number,None,
                    &NSString::from_str(characters),&NSString::from_str(characters),false,0,
                ).unwrap()
        };
        for modifiers in [NSEventModifierFlags::Control, NSEventModifierFlags::Command] {
            editor.setSelectedRange(objc2_foundation::NSRange::new(1, 0));
            app.sendEvent(&key(modifiers, "a"));
            assert_eq!(
                NSTextInputClient::selectedRange(&**editor),
                objc2_foundation::NSRange::new(0, editor.string().length()),
                "Select-all shortcut did not reach rename field"
            );
        }
        app.sendEvent(&key(NSEventModifierFlags::empty(), "x"));
        assert_eq!(
            editor.string().to_string(),
            "x",
            "Typed key did not replace rename selection"
        );
    }
}

#[path = "fixtures_review.rs"]
mod review;
pub(crate) use review::{run as review, targets as review_targets};
