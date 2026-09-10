//! AppKit stays on the main thread. The worker exchanges only owned Rust data.
use crate::{
    appearance as theme,
    macos::App,
    model::*,
    persistence,
    worker::{self, Client, Command},
};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use objc2::{
    DefinedClass, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
    sel,
};
use objc2_app_kit::*;
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, NSTimer,
};
use std::{
    cell::{Cell, OnceCell, RefCell},
    str::FromStr,
};
const CHROME: f64 = 64.;
const TAB_WIDTH: f64 = 220.;
fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
// Keep the app area transparent while attached: activating the tab strip must
// never paint an opaque rectangle over the external application's window.
#[derive(Default)]
struct SurfaceIvars {
    attached: Cell<bool>,
}
define_class!(
    #[unsafe(super=NSView)]
    #[thread_kind=MainThreadOnly]
    #[ivars=SurfaceIvars]
    struct WorkspaceSurface;
    unsafe impl NSObjectProtocol for WorkspaceSurface {}
    impl WorkspaceSurface {
        #[unsafe(method(drawRect:))]
        fn draw(&self, _: NSRect) {
            let bounds=self.bounds();
            NSColor::clearColor().setFill();
            NSRectFillUsingOperation(bounds, NSCompositingOperation::Copy);
            theme::color(theme::SURFACE).setFill();
            let painted=if self.ivars().attached.get() {
                rect(0., (bounds.size.height-CHROME).max(0.), bounds.size.width, CHROME.min(bounds.size.height))
            } else { bounds };
            NSRectFill(painted);
            theme::color(theme::BORDER).setFill();
            NSRectFill(rect(0.,(bounds.size.height-CHROME).max(0.),bounds.size.width,1.));
        }
    }
);
impl WorkspaceSurface {
    fn new(m: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        unsafe {
            msg_send![super(Self::alloc(m).set_ivars(SurfaceIvars::default())), initWithFrame: frame]
        }
    }
    fn set_attached(&self, attached: bool) {
        if self.ivars().attached.replace(attached) != attached {
            self.setNeedsDisplay(true);
        }
    }
}
#[derive(Default)]
struct TabIvars {
    active: Cell<bool>,
}
define_class!(
    #[unsafe(super=NSButton)]
    #[thread_kind=MainThreadOnly]
    #[ivars=TabIvars]
    struct TabButton;
    unsafe impl NSObjectProtocol for TabButton {}
    impl TabButton {
        #[unsafe(method(drawRect:))]
        fn draw(&self,dirty:NSRect){
            theme::color(if self.ivars().active.get(){theme::WINDOW}else if self.isHighlighted(){theme::HOVER}else{theme::SURFACE}).setFill();
            NSRectFill(self.bounds());
            unsafe {let _:()=msg_send![super(self),drawRect:dirty];}
            if self.ivars().active.get(){theme::color(theme::SECONDARY).setFill();NSRectFill(rect(0.,if self.isFlipped(){self.bounds().size.height-2.}else{0.},self.bounds().size.width,2.));}
        }
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self,event:&NSEvent){
            if event.clickCount()==2 {
                if let Some(target)=self.target(){unsafe{let _:()=msg_send![&*target,renameTabButton:self];}}
            } else {unsafe {let _:()=msg_send![super(self),mouseDown:event];}}
        }
    }
);
impl TabButton {
    fn new(m: MainThreadMarker, frame: NSRect, active: bool) -> Retained<Self> {
        unsafe {
            msg_send![super(Self::alloc(m).set_ivars(TabIvars{active:Cell::new(active)})),initWithFrame:frame]
        }
    }
}
struct RenameEditor {
    id: TabId,
    field: Retained<NSTextField>,
}
#[derive(Default)]
struct Ivars {
    ui: RefCell<Option<Ui>>,
    ticks: Cell<u64>,
    // Window activation can synchronously reenter from an AppKit call.
    raise_requested: Cell<bool>,
    dragging: Cell<bool>,
    tracking_samples: Cell<u64>,
    drag_timer: OnceCell<Retained<NSTimer>>,
    tracking_test: Cell<bool>,
}
struct Ui {
    client: Client,
    window: Retained<NSWindow>,
    tabs: Retained<NSView>,
    surface: Retained<WorkspaceSurface>,
    hint: Retained<NSTextField>,
    status: Retained<NSTextField>,
    permission_button: Retained<NSButton>,
    resume_button: Retained<NSButton>,
    replace_button: Retained<NSButton>,
    rename_editor: Option<RenameEditor>,
    picker: Retained<NSPanel>,
    search: Retained<NSSearchField>,
    choices: Retained<NSPopUpButton>,
    choice_ids: Vec<WindowId>,
    replacement: Option<TabId>,
    editing: Option<TabId>,
    tab_signature: String,
    manager: Option<GlobalHotKeyManager>,
    keys: Vec<HotKey>,
    registered: bool,
    shortcut_error: Option<String>,
    last_close_attempt: u64,
    last_frame: Option<Rect>,
    raise_after: Option<std::time::Instant>,
    dock_test_stage: usize,
    dock_test_tick: u64,
    smoke_failed: bool,
}
define_class!(
    #[unsafe(super=NSObject)] #[thread_kind=MainThreadOnly] #[ivars=Ivars] struct Delegate;
    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))] fn launched(&self,_:&NSNotification){self.setup();}
        #[unsafe(method(applicationShouldTerminate:))] fn should_terminate(&self,_:&NSApplication)->NSApplicationTerminateReply {
            if self.ivars().ui.borrow().as_ref().is_none_or(|u|u.client.snapshot.lock().unwrap().stopped) { NSApplicationTerminateReply::TerminateNow } else { self.close_request();NSApplicationTerminateReply::TerminateCancel }
        }
    }
    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowShouldClose:))] fn close(&self,sender:&NSWindow)->bool {if self.ivars().ui.borrow().as_ref().is_some_and(|u|std::ptr::eq(&*u.window,sender)){self.close_request();false}else{true}}
        #[unsafe(method(windowWillMove:))] fn will_move(&self,_:&NSNotification){self.start_tracking();}
        #[unsafe(method(windowDidMove:))] fn moved(&self,_:&NSNotification){self.geometry();}
        #[unsafe(method(windowDidResize:))] fn resized(&self,_:&NSNotification){self.geometry();}
        #[unsafe(method(windowDidBecomeKey:))] fn key(&self,_:&NSNotification){self.ivars().raise_requested.set(true);}
    }
    unsafe impl NSTextFieldDelegate for Delegate {}
    unsafe impl NSSearchFieldDelegate for Delegate {}
    unsafe impl NSControlTextEditingDelegate for Delegate {
        #[unsafe(method(controlTextDidEndEditing:))] fn editing_ended(&self,note:&NSNotification){
            let ended={let b=self.ivars().ui.borrow();b.as_ref().and_then(|u|u.rename_editor.as_ref()).is_some_and(|editor|note.object().is_some_and(|object|Retained::as_ptr(&object).cast::<std::ffi::c_void>()==Retained::as_ptr(&editor.field).cast::<std::ffi::c_void>()))};
            if ended{self.finish_rename(true);}
        }
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn editing_command(&self,_:&NSControl,_:&NSTextView,command:objc2::runtime::Sel)->bool {
            if !self.ivars().ui.borrow().as_ref().is_some_and(|u|u.rename_editor.is_some()){false}
            else if command==sel!(cancelOperation:){self.finish_rename(false);true}else if command==sel!(insertNewline:){self.finish_rename(true);true}else{false}
        }
        #[unsafe(method(controlTextDidChange:))] fn text_changed(&self,_:&NSNotification){self.filter();}
    }
    impl Delegate {
        #[unsafe(method(trackDrag:))] fn drag_timer(&self,_:&NSTimer){self.track_drag();}
        #[unsafe(method(tick:))] fn timer(&self,_:&NSTimer){self.tick();}
        #[unsafe(method(selectTab:))] fn select_tab(&self,sender:&NSButton){let mut b=self.ivars().ui.borrow_mut();if let Some(u)=b.as_mut(){let id=sender.tag() as u64;u.editing=Some(id);u.client.switch(id);}}
        #[unsafe(method(addWindow:))] fn add(&self,_:&AnyObject){self.show_picker(false);}
        #[unsafe(method(replaceWindow:))] fn replace(&self,_:&AnyObject){self.show_picker(true);}
        #[unsafe(method(refreshWindows:))] fn refresh(&self,_:&AnyObject){if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::Discover);}}
        #[unsafe(method(attachWindow:))] fn attach(&self,_:&AnyObject){self.attach_selected_window();}
        #[unsafe(method(renameTabButton:))] fn rename_button(&self,sender:&NSButton){self.begin_rename(sender.tag() as u64);}
        #[unsafe(method(releaseTab:))] fn release(&self,_:&AnyObject){self.release_current();}
        #[unsafe(method(releaseThisTab:))] fn release_this(&self,sender:&NSButton){if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::Release(sender.tag() as u64));}}
        #[unsafe(method(resumeDocking:))] fn resume(&self,_:&AnyObject){if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::Resume);u.client.send(Command::Discover);}}
        #[unsafe(method(permission:))] fn permission(&self,_:&AnyObject){if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::RequestPermission);}let url=objc2_foundation::NSURL::URLWithString(&NSString::from_str("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")).unwrap();NSWorkspace::sharedWorkspace().openURL(&url);}
        #[unsafe(method(spaceChanged:))] fn space(&self,_:&NSNotification){if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::Pause);}}
        #[unsafe(method(dragTab:))] fn drag(&self,sender:&NSPanGestureRecognizer){if sender.state()==NSGestureRecognizerState::Ended && let Some(view)=sender.view(){let b=self.ivars().ui.borrow();if let Some(u)=b.as_ref(){let id=view.tag() as u64;let point=sender.locationInView(Some(&u.tabs));let index=(point.x/TAB_WIDTH).max(0.) as usize;u.client.send(Command::Reorder(id,index));}}}
        #[unsafe(method(retryRestoration:))] fn retry_restoration(&self,_:&AnyObject){if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::RetryRestoration);}}
        #[unsafe(method(quitApp:))] fn quit(&self,_:&AnyObject){self.close_request();}
    }
);
impl Delegate {
    fn new(m: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![super(Self::alloc(m).set_ivars(Ivars::default())), init] }
    }
    fn button(
        &self,
        title: &str,
        action: objc2::runtime::Sel,
        frame: NSRect,
    ) -> Retained<NSButton> {
        unsafe {
            let b = NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(self),
                Some(action),
                self.mtm(),
            );
            b.setFrame(frame);
            b.setBezelStyle(NSBezelStyle::Push);
            b.setBordered(false);
            b.setFont(Some(&theme::font(13.)));
            b.setContentTintColor(Some(&theme::color(theme::TEXT)));
            b
        }
    }
    fn setup(&self) {
        let m = self.mtm();
        theme::install_font();
        let app = NSApplication::sharedApplication(m);
        let mut workspace = match persistence::load(&persistence::path()) {
            Ok(w) => w,
            Err(e) => {
                let a = NSAlert::new(m);
                a.setMessageText(&NSString::from_str("AppDock could not open this workspace"));
                a.setInformativeText(&NSString::from_str(&e));
                a.runModal();
                app.terminate(None);
                return;
            }
        };
        if std::env::args().any(|a| {
            matches!(
                a.as_str(),
                "--disconnected-smoke" | "--rename-smoke" | "--design-smoke"
            )
        }) {
            assert!(
                workspace.tabs.is_empty(),
                "Fixture requires empty workspace"
            );
            workspace.tabs.push(SavedTab {
                id: 7,
                name: "Disconnected fixture".into(),
                identity: Identity {
                    bundle: "dev.appdock.fixture.disconnected".into(),
                    identifier: None,
                },
            });
        }
        if std::env::args().any(|a| a == "--design-smoke") {
            workspace.tabs[0].name = "Discord · personal".into();
            workspace.tabs.push(SavedTab {
                id: 8,
                name: "Telegram · work".into(),
                identity: Identity {
                    bundle: "dev.appdock.fixture.second".into(),
                    identifier: None,
                },
            });
        }
        let client = worker::start(workspace.clone());
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(m),
                rect(0., 0., workspace.geometry.width, workspace.geometry.height),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
            window.setContentMinSize(NSSize::new(700., 400.));
        }
        window.setTitle(&NSString::from_str(
            if std::env::args().any(|a| a == "--design-smoke") {
                "AppDock — preview"
            } else {
                "AppDock"
            },
        ));
        window.setAppearance(
            NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }).as_deref(),
        );
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        let height = NSScreen::screens(m)
            .firstObject()
            .map(|s| s.frame().size.height)
            .unwrap_or(900.);
        window.setFrame_display(
            rect(
                workspace.geometry.x,
                height - workspace.geometry.y - workspace.geometry.height,
                workspace.geometry.width,
                workspace.geometry.height,
            ),
            true,
        );
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        let surface = WorkspaceSurface::new(m, window.contentView().unwrap().bounds());
        window.setContentView(Some(&surface));
        let content = window.contentView().unwrap();
        let h = content.bounds().size.height;
        let w = content.bounds().size.width;
        let scroll = {
            NSScrollView::initWithFrame(NSScrollView::alloc(m), rect(0., h - 36., w - 44., 34.))
        };
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        scroll.setHasHorizontalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        let tabs = { NSView::initWithFrame(NSView::alloc(m), rect(0., 0., w - 44., 32.)) };
        scroll.setDocumentView(Some(&tabs));
        content.addSubview(&scroll);
        let add = self.button("+", sel!(addWindow:), rect(w - 38., h - 34., 30., 30.));
        add.setFont(Some(&theme::font(20.)));
        add.setToolTip(Some(&NSString::from_str("Add an existing app window")));
        add.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&add);
        let permission_button = self.button(
            "Allow window control…",
            sel!(permission:),
            rect(w - 196., h - 62., 188., 26.),
        );
        let resume_button = self.button(
            "Resume",
            sel!(resumeDocking:),
            rect(w - 90., h - 62., 82., 26.),
        );
        let replace_button = self.button(
            "Replace window…",
            sel!(replaceWindow:),
            rect(w - 242., h - 62., 144., 26.),
        );
        for button in [&permission_button, &resume_button, &replace_button] {
            button.setHidden(true);
            button.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewMinXMargin
                    | NSAutoresizingMaskOptions::ViewMinYMargin,
            );
            content.addSubview(button);
        }
        let status = NSTextField::labelWithString(
            &NSString::from_str("Checking Accessibility permission…"),
            m,
        );
        status.setFrame(rect(12., h - 59., w - 266., 20.));
        status.setFont(Some(&theme::font(12.)));
        status.setTextColor(Some(&theme::color(theme::SECONDARY)));
        status.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&status);
        let hint = NSTextField::labelWithString(
            &NSString::from_str(
                "Your apps, together.\n\nUse + to add an open app window.\nDouble-click a tab to rename it. Drag tabs to reorder.\nClose a tab to release its window; the app keeps running.",
            ),
            m,
        );
        hint.setFrame(rect(40., 80., 620., 150.));
        hint.setFont(Some(&theme::font(13.)));
        hint.setTextColor(Some(&theme::color(theme::SECONDARY)));
        content.addSubview(&hint);
        let picker = {
            NSPanel::initWithContentRect_styleMask_backing_defer(
                NSPanel::alloc(m),
                rect(0., 0., 680., 175.),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            picker.setReleasedWhenClosed(false);
        }
        picker.setTitle(&NSString::from_str("Choose an existing window"));
        let pv = picker.contentView().unwrap();
        let search =
            { NSSearchField::initWithFrame(NSSearchField::alloc(m), rect(16., 128., 648., 28.)) };
        search.setPlaceholderString(Some(&NSString::from_str("Search apps or window titles")));
        unsafe {
            search.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        pv.addSubview(&search);
        let choices = {
            NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(m),
                rect(16., 78., 648., 32.),
                false,
            )
        };
        pv.addSubview(&choices);
        pv.addSubview(&self.button("Refresh", sel!(refreshWindows:), rect(16., 20., 100., 32.)));
        pv.addSubview(&self.button(
            "Attach window",
            sel!(attachWindow:),
            rect(504., 20., 160., 32.),
        ));
        let parsed = [&workspace.previous_shortcut, &workspace.next_shortcut]
            .iter()
            .map(|s| HotKey::from_str(s))
            .collect::<std::result::Result<Vec<_>, _>>();
        let (keys, shortcut_error) = match parsed {
            Ok(k) => (k, None),
            Err(e) => (vec![], Some(format!("Invalid shortcut configuration: {e}"))),
        };
        let manager = GlobalHotKeyManager::new();
        let shortcut_error =
            shortcut_error.or_else(|| manager.as_ref().err().map(|e| e.to_string()));
        *self.ivars().ui.borrow_mut() = Some(Ui {
            client,
            window: window.clone(),
            tabs,
            surface,
            hint,
            status,
            permission_button,
            resume_button,
            replace_button,
            rename_editor: None,
            picker,
            search,
            choices,
            choice_ids: vec![],
            replacement: None,
            editing: None,
            tab_signature: String::new(),
            manager: manager.ok(),
            keys,
            registered: false,
            shortcut_error,
            last_close_attempt: 0,
            last_frame: None,
            raise_after: None,
            dock_test_stage: 0,
            dock_test_tick: 0,
            smoke_failed: false,
        });
        unsafe {
            NSWorkspace::sharedWorkspace()
                .notificationCenter()
                .addObserver_selector_name_object(
                    self,
                    sel!(spaceChanged:),
                    Some(NSWorkspaceActiveSpaceDidChangeNotification),
                    None,
                );
            let _ = NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.15,
                self,
                sel!(tick:),
                None,
                true,
            );
        }
        // A separate sampler must keep firing during the native title-bar tracking loop.
        unsafe {
            let timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                1.0 / 120.0,
                self,
                sel!(trackDrag:),
                None,
                true,
            );
            timer.setFireDate(&objc2_foundation::NSDate::distantFuture());
            let run_loop = objc2_foundation::NSRunLoop::mainRunLoop();
            run_loop.addTimer_forMode(&timer, NSEventTrackingRunLoopMode);
            run_loop.addTimer_forMode(&timer, objc2_foundation::NSDefaultRunLoopMode);
            self.ivars().drag_timer.set(timer).unwrap();
        }
        let menu = NSMenu::new(m);
        let root = NSMenuItem::new(m);
        let submenu = NSMenu::new(m);
        let quit = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(m),
                &NSString::from_str("Quit AppDock"),
                Some(sel!(quitApp:)),
                &NSString::from_str("q"),
            )
        };
        unsafe {
            quit.setTarget(Some(self));
        }
        let retry = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(m),
                &NSString::from_str("Retry restoration"),
                Some(sel!(retryRestoration:)),
                &NSString::from_str(""),
            )
        };
        unsafe {
            retry.setTarget(Some(self));
        }
        submenu.addItem(&retry);
        submenu.addItem(&quit);
        root.setSubmenu(Some(&submenu));
        menu.addItem(&root);
        app.setMainMenu(Some(&menu));
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        window.makeKeyAndOrderFront(None);
        if std::env::args().any(|a| a == "--design-smoke") {
            println!("Preview window ID: {}", window.windowNumber());
        }
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        self.geometry();
        self.tick();
        if std::env::args().any(|a| a == "--tracking-smoke") {
            self.ivars().tracking_test.set(true);
            self.start_tracking();
            let end = objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.15);
            let run_loop = objc2_foundation::NSRunLoop::mainRunLoop();
            while end.timeIntervalSinceNow() > 0. {
                run_loop.runMode_beforeDate(unsafe { NSEventTrackingRunLoopMode }, &end);
            }
            self.ivars().dragging.set(false);
            let samples = self.ivars().tracking_samples.get();
            assert!(samples >= 3, "Tracking-mode sampler did not run");
            println!(
                "Native tracking-mode regression passed: {samples} WindowServer samples during 150 ms tracking loop"
            );
            self.close_request();
        }
    }
    fn release_current(&self) {
        let b = self.ivars().ui.borrow();
        let Some(u) = b.as_ref() else { return };
        let snapshot = u.client.snapshot.lock().unwrap();
        if let Some(id) = action_tab(u.editing, snapshot.selected, &snapshot.workspace.tabs) {
            u.client.send(Command::Release(id));
        }
    }
    fn start_tracking(&self) {
        self.ivars().dragging.set(true);
        if let Some(timer) = self.ivars().drag_timer.get() {
            timer.setFireDate(&objc2_foundation::NSDate::date());
        }
    }
    fn track_drag(&self) {
        if !self.ivars().dragging.get() {
            return;
        }
        let testing = self.ivars().tracking_test.get();
        if NSEvent::pressedMouseButtons() == 0 && !testing {
            self.ivars().dragging.set(false);
            if let Some(timer) = self.ivars().drag_timer.get() {
                timer.setFireDate(&objc2_foundation::NSDate::distantFuture());
            }
            self.geometry();
            return;
        }
        // No UI controls, filesystem, AX calls or application discovery in this sampler.
        let mut b = self.ivars().ui.borrow_mut();
        let Some(u) = b.as_mut() else { return };
        u.client.set_pointer_down(true);
        if let Some(frame) = crate::window_tracking::frame(u.window.windowNumber()) {
            self.ivars()
                .tracking_samples
                .set(self.ivars().tracking_samples.get() + 1);
            if u.last_frame != Some(frame) {
                let cocoa = u.window.frame();
                let content = u.window.contentView().unwrap().bounds();
                let title_height = cocoa.size.height - content.size.height;
                let area = Rect {
                    x: frame.x,
                    y: frame.y + title_height + CHROME,
                    width: content.size.width,
                    height: (content.size.height - CHROME).max(100.),
                };
                u.last_frame = Some(frame);
                u.raise_after = Some(std::time::Instant::now());
                u.client.send(Command::Resize(area, frame));
            }
        }
    }
    fn geometry(&self) {
        if self.ivars().dragging.get() && NSEvent::pressedMouseButtons() != 0 {
            return;
        }
        let mut b = self.ivars().ui.borrow_mut();
        let Some(u) = b.as_mut() else { return };
        u.client
            .set_pointer_down(NSEvent::pressedMouseButtons() != 0);
        let f = u.window.frame();
        let primary = NSScreen::screens(self.mtm())
            .firstObject()
            .map(|s| s.frame().size.height)
            .unwrap_or(900.);
        let geometry =
            Rect::from_cocoa(f.origin.x, f.origin.y, f.size.width, f.size.height, primary);
        let content = u.window.contentView().unwrap();
        let bounds = content.bounds();
        let body = u.window.convertRectToScreen(rect(
            0.,
            0.,
            bounds.size.width,
            (bounds.size.height - CHROME).max(100.),
        ));
        let area = Rect::from_cocoa(
            body.origin.x,
            body.origin.y,
            body.size.width,
            body.size.height,
            primary,
        );
        if u.last_frame != Some(geometry) {
            u.last_frame = Some(geometry);
            u.raise_after = Some(std::time::Instant::now());
            u.client.send(Command::Resize(area, geometry));
        }
    }
    fn tick(&self) {
        let count = self.ivars().ticks.get();
        self.ivars().ticks.set(count + 1);
        let mut b = self.ivars().ui.borrow_mut();
        let Some(u) = b.as_mut() else { return };
        if count.is_multiple_of(14) {
            let apps = NSWorkspace::sharedWorkspace()
                .runningApplications()
                .iter()
                .filter(|a| {
                    a.processIdentifier() != std::process::id() as i32
                        && a.activationPolicy() == NSApplicationActivationPolicy::Regular
                })
                .map(|a| App {
                    pid: a.processIdentifier(),
                    name: a.localizedName().map(|s| s.to_string()).unwrap_or_default(),
                    bundle: a
                        .bundleIdentifier()
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                })
                .collect();
            u.client.send(Command::Apps(apps));
            if count == 0 {
                u.client.send(Command::Discover);
            }
        }
        u.client
            .set_pointer_down(NSEvent::pressedMouseButtons() != 0);
        let s = u.client.snapshot.lock().unwrap().clone();
        let attached = std::env::args().any(|a| a == "--surface-smoke")
            || s.selected
                .is_some_and(|id| s.live.iter().any(|(tab, _)| *tab == id));
        u.surface.set_attached(attached);
        u.hint.setHidden(attached);
        if self.ivars().raise_requested.replace(false) {
            u.raise_after = Some(std::time::Instant::now() - std::time::Duration::from_millis(251));
        }
        u.editing = action_tab(u.editing, s.selected, &s.workspace.tabs);
        if std::env::args().any(|a| a == "--disconnected-smoke") {
            if count == 4 {
                drop(b);
                self.release_current();
                return;
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
                return;
            }
            if count == 6 {
                let field = u
                    .rename_editor
                    .as_ref()
                    .expect("Double-click did not open inline rename")
                    .field
                    .clone();
                drop(b);
                field.setStringValue(&NSString::from_str("Discord - account 1"));
                self.finish_rename(true);
                return;
            }
            if count == 10 {
                assert_eq!(s.workspace.tabs[0].name, "Discord - account 1");
                assert!(u.rename_editor.is_none());
                drop(b);
                self.begin_rename(7);
                return;
            }
        }
        if std::env::args().any(|a| a == "--rename-smoke") {
            if count == 12 {
                let field = u.rename_editor.as_ref().unwrap().field.clone();
                drop(b);
                field.setStringValue(&NSString::from_str("Unsaved label"));
                self.finish_rename(false);
                return;
            }
            if count == 16 {
                assert_eq!(s.workspace.tabs[0].name, "Discord - account 1");
                assert!(u.rename_editor.is_none());
                println!("Native double-click inline rename regression passed: commit and cancel");
                u.client.send(Command::Quit);
            }
        }
        if u.raise_after.is_some_and(|t| t.elapsed().as_millis() > 250)
            && !u.window.inLiveResize()
            && !u.picker.isVisible()
            && u.rename_editor.is_none()
        {
            u.raise_after = None;
            u.client.send(Command::Raise);
        }
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
                println!("Native surface regression passed: transparent app area, opaque controls");
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
        if std::env::args().any(|a| matches!(a.as_str(), "--dock-smoke" | "--picker-smoke"))
            && u.dock_test_stage < 20
        {
            if s.status.starts_with("Paused:") || count > 400 {
                eprintln!(
                    "Integrated docking smoke failed at stage {}: {}",
                    u.dock_test_stage, s.status
                );
                u.smoke_failed = true;
                u.dock_test_stage = 20;
                u.client.send(Command::Quit);
            } else if count > u.dock_test_tick + 8 {
                let stage = u.dock_test_stage;
                if stage == 0 || stage == 1 {
                    let bundle = if stage == 0 {
                        "com.hnc.Discord"
                    } else {
                        "org.telegram.desktop"
                    };
                    let candidates: Vec<_> = s
                        .windows
                        .iter()
                        .filter(|w| w.eligible && w.identity.bundle == bundle)
                        .collect();
                    if candidates.len() == 1
                        && s.live.len() == stage
                        && (stage == 0 || s.selected == s.workspace.tabs.first().map(|t| t.id))
                    {
                        let window_id = candidates[0].id;
                        u.dock_test_stage += 1;
                        u.dock_test_tick = count;
                        let window = u.window.clone();
                        drop(b);
                        window.makeKeyAndOrderFront(None);
                        self.show_picker(false);
                        let choice = {
                            let b = self.ivars().ui.borrow();
                            let u = b.as_ref().unwrap();
                            u.choice_ids
                                .iter()
                                .position(|id| *id == window_id)
                                .map(|index| (u.choices.clone(), index))
                        };
                        if let Some((choices, index)) = choice {
                            choices.selectItemAtIndex(index as isize);
                            self.attach_selected_window();
                            println!("Integrated picker attachment {} submitted", stage + 1);
                        }
                        return;
                    }
                } else if stage == 2
                    && std::env::args().any(|a| a == "--picker-smoke")
                    && s.live.len() == 2
                    && s.selected == s.workspace.tabs.last().map(|t| t.id)
                {
                    println!("Native picker regression complete; restoring windows");
                    u.client.send(Command::Quit);
                    u.dock_test_stage = 20;
                } else if (2..14).contains(&stage)
                    && s.live.len() == 2
                    && (stage == 2 || s.selected == Some(s.workspace.tabs[(stage - 3) % 2].id))
                {
                    let wanted = s.workspace.tabs[(stage - 2) % 2].id;
                    u.client.switch(wanted);
                    println!("Integrated switch {} requested", stage - 1);
                    u.dock_test_stage += 1;
                    u.dock_test_tick = count;
                } else if stage == 14 && s.selected == Some(s.workspace.tabs[1].id) {
                    u.dock_test_stage = 15;
                    u.dock_test_tick = count;
                    let window = u.window.clone();
                    let mut f = window.frame();
                    f.origin.x += 35.;
                    f.size.width += 60.;
                    drop(b);
                    window.setFrame_display(f, true);
                    return;
                } else if stage == 15 {
                    println!(
                        "Integrated docking and manager resize completed; releasing first tab"
                    );
                    if let Some(t) = s.workspace.tabs.first() {
                        u.client.send(Command::Release(t.id));
                    }
                    u.dock_test_stage = 16;
                    u.dock_test_tick = count;
                } else if stage == 16 && s.live.len() == 1 {
                    println!("Integrated release completed; restoring remaining window on exit");
                    u.client.send(Command::Quit);
                    u.dock_test_stage = 20;
                }
            }
        }
        if s.stopped {
            if u.smoke_failed {
                std::process::exit(1);
            }
            if std::env::args().any(|a| matches!(a.as_str(), "--dock-smoke" | "--picker-smoke")) {
                println!("Integrated restoration complete");
            }
            let window = u.window.clone();
            drop(b);
            window.orderOut(None);
            NSApplication::sharedApplication(self.mtm()).terminate(None);
            return;
        }
        u.permission_button.setHidden(s.trusted);
        u.resume_button.setHidden(!s.trusted || !s.paused);
        let disconnected = u
            .editing
            .is_some_and(|id| !s.live.iter().any(|(t, _)| *t == id));
        u.replace_button.setHidden(!s.trusted || !disconnected);
        let text = if !s.trusted {
            "Allow window control to arrange your app windows.".to_string()
        } else {
            u.shortcut_error.clone().unwrap_or_else(|| {
                if s.status == "Ready" || s.status.starts_with("Add an existing") {
                    "Double-click to rename · Drag to reorder".into()
                } else {
                    s.status.clone()
                }
            })
        };
        u.status.setStringValue(&NSString::from_str(&text));
        if s.quitting && u.last_close_attempt != s.close_attempt {
            u.last_close_attempt = s.close_attempt;
            drop(b);
            let a = NSAlert::new(self.mtm());
            a.setMessageText(&NSString::from_str("Some windows could not be restored"));
            a.setInformativeText(&NSString::from_str(&s.status));
            a.addButtonWithTitle(&NSString::from_str("Retry close"));
            a.addButtonWithTitle(&NSString::from_str("Keep AppDock open"));
            let retry = a.runModal() == 1000;
            let mut b = self.ivars().ui.borrow_mut();
            let u = b.as_mut().unwrap();
            u.client.send(if retry {
                Command::Quit
            } else {
                Command::CancelQuit
            });
            return;
        }
        let foreground = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|a| a.processIdentifier());
        let eligible = foreground == Some(std::process::id() as i32)
            || s.live.iter().any(|(_, id)| {
                s.windows
                    .iter()
                    .any(|w| w.id == *id && Some(w.pid) == foreground)
            });
        if eligible != u.registered
            && let Some(manager) = &u.manager
        {
            let r = if eligible {
                manager.register_all(&u.keys)
            } else {
                manager.unregister_all(&u.keys)
            };
            match r {
                Ok(()) => u.registered = eligible,
                Err(e) => {
                    let _ = manager.unregister_all(&u.keys);
                    u.registered = false;
                    u.shortcut_error = Some(format!("Shortcuts unavailable: {e}"));
                }
            }
        }
        while let Ok(e) = GlobalHotKeyEvent::receiver().try_recv() {
            if eligible && e.state == HotKeyState::Pressed && !s.workspace.tabs.is_empty() {
                let index = s
                    .workspace
                    .tabs
                    .iter()
                    .position(|t| Some(t.id) == s.selected)
                    .unwrap_or(0);
                let next = if u.keys.first().is_some_and(|k| k.id() == e.id) {
                    (index + s.workspace.tabs.len() - 1) % s.workspace.tabs.len()
                } else {
                    (index + 1) % s.workspace.tabs.len()
                };
                u.editing = Some(s.workspace.tabs[next].id);
                u.client.switch(s.workspace.tabs[next].id);
            }
        }
        let signature = format!(
            "{:?}{:?}{:?}{:?}",
            s.workspace.tabs, s.live, s.selected, u.editing
        );
        if signature != u.tab_signature && u.rename_editor.is_none() {
            u.tab_signature = signature;
            for view in u.tabs.subviews() {
                view.removeFromSuperview();
            }
            u.tabs.setFrameSize(NSSize::new(
                (s.workspace.tabs.len() as f64 * TAB_WIDTH).max(600.),
                32.,
            ));
            for (i, t) in s.workspace.tabs.iter().enumerate() {
                let connected = s.live.iter().find(|(id, _)| *id == t.id);
                let label = if connected.is_none() {
                    format!("○ {}", t.name)
                } else {
                    t.name.clone()
                };
                let button = TabButton::new(
                    self.mtm(),
                    rect(i as f64 * TAB_WIDTH, 0., TAB_WIDTH - 28., 32.),
                    u.editing == Some(t.id),
                );
                button.setTitle(&NSString::from_str(&label));
                button.setBordered(false);
                button.setAlignment(NSTextAlignment::Left);
                button.setFont(Some(&theme::font(13.)));
                button.setContentTintColor(Some(&theme::color(if u.editing == Some(t.id) {
                    theme::TEXT
                } else {
                    theme::SECONDARY
                })));
                unsafe {
                    button.setTarget(Some(self));
                    button.setAction(Some(sel!(selectTab:)));
                }
                button.setTag(t.id as isize);
                button.setToolTip(Some(&NSString::from_str(&format!(
                    "{} — double-click to rename, drag to reorder",
                    t.name
                ))));
                if let Some((_, wid)) = connected
                    && let Some(w) = s.windows.iter().find(|w| w.id == *wid)
                    && let Some(app) =
                        NSRunningApplication::runningApplicationWithProcessIdentifier(w.pid)
                    && let Some(icon) = app.icon()
                {
                    icon.setSize(NSSize::new(16., 16.));
                    button.setImage(Some(&icon));
                    button.setImagePosition(NSCellImagePosition::ImageLeft);
                }
                let drag = unsafe {
                    NSPanGestureRecognizer::initWithTarget_action(
                        NSPanGestureRecognizer::alloc(self.mtm()),
                        Some(self),
                        Some(sel!(dragTab:)),
                    )
                };
                button.addGestureRecognizer(&drag);
                u.tabs.addSubview(&button);
                let close = self.button(
                    "×",
                    sel!(releaseThisTab:),
                    rect(i as f64 * TAB_WIDTH + TAB_WIDTH - 28., 0., 28., 32.),
                );
                close.setTag(t.id as isize);
                close.setToolTip(Some(&NSString::from_str(&format!("Release {}", t.name))));
                u.tabs.addSubview(&close);
            }
        }
        drop(b);
        self.filter();
    }
    fn filter(&self) {
        let mut b = self.ivars().ui.borrow_mut();
        let Some(u) = b.as_mut() else { return };
        if !u.picker.isVisible() {
            return;
        }
        let s = u.client.snapshot.lock().unwrap().clone();
        let query = u.search.stringValue().to_string().to_lowercase();
        let windows: Vec<_> = s
            .windows
            .iter()
            .filter(|w| {
                w.eligible
                    && !s.occupied.contains(&w.id)
                    && (u.replacement.is_some()
                        || !s.workspace.tabs.iter().any(|t| {
                            t.identity.bundle == w.identity.bundle
                                && !s.live.iter().any(|(id, _)| *id == t.id)
                        }))
                    && format!("{} {}", w.app, w.title)
                        .to_lowercase()
                        .contains(&query)
            })
            .collect();
        let ids: Vec<_> = windows.iter().map(|w| w.id).collect();
        if ids == u.choice_ids {
            return;
        }
        u.choice_ids = ids;
        u.choices.removeAllItems();
        for w in windows {
            u.choices.addItemWithTitle(&NSString::from_str(&format!(
                "{} — {} (process {}, window {})",
                w.app, w.title, w.pid, w.id
            )));
        }
    }
    fn show_picker(&self, replace: bool) {
        let mut b = self.ivars().ui.borrow_mut();
        let Some(u) = b.as_mut() else { return };
        u.replacement = if replace { u.editing } else { None };
        if replace && u.replacement.is_none() {
            return;
        }
        u.client.send(Command::Discover);
        let picker = u.picker.clone();
        let unresolved = {
            let s = u.client.snapshot.lock().unwrap();
            !replace
                && s.workspace
                    .tabs
                    .iter()
                    .any(|t| !s.live.iter().any(|(id, _)| *id == t.id))
        };
        drop(b);
        picker.setTitle(&NSString::from_str(if unresolved {
            "Add window — use Replace window for disconnected apps"
        } else {
            "Choose an existing window"
        }));
        // AppKit can synchronously send window-delegate notifications here.
        picker.center();
        picker.makeKeyAndOrderFront(None);
        self.filter();
    }
    fn attach_selected_window(&self) {
        let pending = {
            let b = self.ivars().ui.borrow();
            let Some(u) = b.as_ref() else { return };
            let index = u.choices.indexOfSelectedItem();
            usize::try_from(index).ok().and_then(|index| {
                u.choice_ids
                    .get(index)
                    .map(|id| (u.client.clone(), u.replacement, *id, u.picker.clone()))
            })
        };
        if let Some((client, replacement, id, picker)) = pending {
            // Closing the key panel activates the manager synchronously. No UI
            // RefCell guard may survive this call into AppKit.
            picker.orderOut(None);
            client.send(Command::Attach(replacement, id));
        }
    }
    fn begin_rename(&self, id: TabId) {
        self.finish_rename(true);
        let (tabs, window, index, name) = {
            let mut b = self.ivars().ui.borrow_mut();
            let Some(u) = b.as_mut() else { return };
            let s = u.client.snapshot.lock().unwrap();
            let Some(index) = s.workspace.tabs.iter().position(|t| t.id == id) else {
                return;
            };
            u.editing = Some(id);
            (
                u.tabs.clone(),
                u.window.clone(),
                index,
                s.workspace.tabs[index].name.clone(),
            )
        };
        let field = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm()),
            rect(index as f64 * TAB_WIDTH + 6., 3., TAB_WIDTH - 38., 26.),
        );
        field.setFont(Some(&theme::font(13.)));
        field.setStringValue(&NSString::from_str(&name));
        field.setBackgroundColor(Some(&theme::color(theme::WINDOW)));
        field.setTextColor(Some(&theme::color(theme::TEXT)));
        unsafe {
            field.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        if let Some(u) = self.ivars().ui.borrow_mut().as_mut() {
            u.rename_editor = Some(RenameEditor {
                id,
                field: field.clone(),
            });
        }
        tabs.addSubview(&field);
        window.makeKeyAndOrderFront(None);
        unsafe {
            field.selectText(None);
        }
    }
    fn finish_rename(&self, save: bool) {
        let pending = {
            let mut b = self.ivars().ui.borrow_mut();
            let Some(u) = b.as_mut() else { return };
            u.rename_editor.take().map(|editor| {
                u.tab_signature.clear();
                (editor, u.client.clone())
            })
        };
        if let Some((editor, client)) = pending {
            let name = editor
                .field
                .currentEditor()
                .map(|text| text.string().to_string())
                .unwrap_or_else(|| editor.field.stringValue().to_string());
            if save && !name.trim().is_empty() {
                client.send(Command::Rename(editor.id, name));
            }
            editor.field.removeFromSuperview();
        }
    }
    fn close_request(&self) {
        if let Some(u) = self.ivars().ui.borrow().as_ref() {
            u.client.send(Command::Quit);
        }
    }
}
pub fn run() {
    let m = MainThreadMarker::new().expect("AppDock must start on the main thread");
    let app = NSApplication::sharedApplication(m);
    let delegate = Delegate::new(m);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
