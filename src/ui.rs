//! AppKit stays on the main thread. The worker exchanges only owned Rust data.
pub(crate) mod fixtures;
use crate::{
    appearance as theme,
    macos::App,
    model::*,
    persistence,
    worker::{self, Client, Command},
};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use objc2::{
    AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
    sel,
};
use objc2_app_kit::*;
use objc2_foundation::{
    MainThreadMarker, NSAttributedString, NSData, NSDictionary, NSNotification, NSObject,
    NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer,
};
use std::{
    cell::{Cell, OnceCell, RefCell},
    str::FromStr,
};
const CHROME: f64 = 36.;
const STATUS_CHROME: f64 = 64.;
const FRAME: f64 = 8.;
const TAB_WIDTH: f64 = 160.;
fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
// Keep the app area transparent while attached: activating the tab strip must
// never paint an opaque rectangle over the external application's window.
struct SurfaceIvars {
    attached: Cell<bool>,
    chrome: Cell<f64>,
}
impl Default for SurfaceIvars {
    fn default() -> Self {
        Self {
            attached: Cell::new(false),
            chrome: Cell::new(CHROME),
        }
    }
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
            let chrome=self.ivars().chrome.get();
            let painted=if self.ivars().attached.get() {
                rect(0., (bounds.size.height-chrome).max(0.), bounds.size.width, chrome.min(bounds.size.height))
            } else { bounds };
            NSRectFill(painted);
            if self.ivars().attached.get() {
                theme::color(theme::WINDOW).setFill();
                let body=(bounds.size.height-chrome).max(0.);
                for edge in [rect(0.,0.,FRAME,body),rect(bounds.size.width-FRAME,0.,FRAME,body),rect(0.,0.,bounds.size.width,FRAME),rect(0.,body-FRAME,bounds.size.width,FRAME)] { NSRectFill(edge); }
            }
            theme::color(theme::BORDER).setFill();
            NSRectFill(rect(0.,(bounds.size.height-chrome).max(0.),bounds.size.width,1.));
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
    fn chrome(&self) -> f64 {
        self.ivars().chrome.get()
    }
    fn set_chrome(&self, chrome: f64) -> bool {
        let changed = self.ivars().chrome.replace(chrome) != chrome;
        if changed {
            self.setNeedsDisplay(true);
        }
        changed
    }
}
#[derive(Default)]
struct TabIvars {
    active: Cell<bool>,
    action: Cell<bool>,
    badge: Option<String>,
}
define_class!(
    #[unsafe(super=NSButton)]
    #[thread_kind=MainThreadOnly]
    #[ivars=TabIvars]
    struct TabButton;
    unsafe impl NSObjectProtocol for TabButton {}
    impl TabButton {
        #[unsafe(method(drawRect:))]
        fn draw(&self,_:NSRect){
            let bounds=self.bounds();
            theme::color(theme::SURFACE).setFill();NSRectFill(bounds);
            let active=self.ivars().active.get();
            let shape=NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect(0.5,2.5,bounds.size.width-1.,bounds.size.height-5.),5.,5.);
            theme::color(if active {theme::TAB_ACTIVE} else if self.isHighlighted() || self.ivars().action.get() {theme::BORDER} else {theme::TAB_IDLE}).setFill();shape.fill();
            theme::color(if active {theme::TAB_ACCENT} else {theme::TAB_BORDER}).setStroke();shape.setLineWidth(1.);shape.stroke();
            let label_width=bounds.size.width-28.; // Keep the close button inside the same tab surface.
            let badge_width=self.ivars().badge.as_ref().map_or(0.,|badge|if badge=="•"{14.}else{(badge.len() as f64*7.+16.).max(26.)});
            if let Some(cell)=self.cell(){cell.drawWithFrame_inView(rect(4.,0.,(label_width-badge_width-4.).max(20.),bounds.size.height),self);}
            if let Some(badge)=&self.ivars().badge {
                let dot=badge=="•";
                let width=badge_width-6.;
                let height=if dot{8.}else{18.};
                let badge_rect=rect(label_width-badge_width,(bounds.size.height-height)/2.,width,height);
                theme::color(0xC34A55).setFill();NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(badge_rect,height/2.,height/2.).fill();
                if !dot {
                    let color=NSColor::whiteColor();let font=theme::font(11.);
                    let attrs=NSDictionary::from_slices(&[unsafe{NSForegroundColorAttributeName},unsafe{NSFontAttributeName}],&[&*color as &AnyObject,&*font as &AnyObject]);
                    let label=unsafe{NSAttributedString::new_with_attributes(&NSString::from_str(badge),&attrs)};
                    let size=label.size();label.drawAtPoint(NSPoint::new(badge_rect.origin.x+(width-size.width)/2.,(bounds.size.height-size.height)/2.));
                }
            }
            if active {theme::color(theme::TAB_ACCENT).setFill();NSRectFill(rect(5.,3.,bounds.size.width-10.,3.));}
            if self.ivars().action.get(){theme::color(theme::TAB_ACCENT).setFill();NSRectFill(rect(2.,7.,3.,bounds.size.height-14.));}
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
    fn set_indication(&self, indication: TabIndication) {
        let active = indication == TabIndication::Active;
        let action = indication == TabIndication::Action;
        if self.ivars().active.get() == active && self.ivars().action.get() == action {
            return;
        }
        self.ivars().active.set(active);
        self.ivars().action.set(action);
        self.setContentTintColor(Some(&theme::color(
            if indication == TabIndication::Inactive {
                theme::SECONDARY
            } else {
                theme::TEXT
            },
        )));
        NSView::setNeedsDisplay(self, true);
    }
    fn new(
        m: MainThreadMarker,
        frame: NSRect,
        indication: TabIndication,
        badge: Option<String>,
    ) -> Retained<Self> {
        unsafe {
            msg_send![super(Self::alloc(m).set_ivars(TabIvars{active:Cell::new(indication == TabIndication::Active),action:Cell::new(indication == TabIndication::Action),badge})),initWithFrame:frame]
        }
    }
}
fn select_all_key(event: &NSEvent) -> bool {
    let modifiers = event.modifierFlags()
        & (NSEventModifierFlags::Control
            | NSEventModifierFlags::Command
            | NSEventModifierFlags::Option);
    (modifiers == NSEventModifierFlags::Control || modifiers == NSEventModifierFlags::Command)
        && event
            .charactersIgnoringModifiers()
            .is_some_and(|s| s.to_string().eq_ignore_ascii_case("a"))
}
define_class!(
    #[unsafe(super=NSTextView)]
    #[thread_kind=MainThreadOnly]
    struct RenameFieldEditor;
    unsafe impl NSObjectProtocol for RenameFieldEditor {}
    impl RenameFieldEditor {
        #[unsafe(method(keyDown:))]
        fn key_down(&self,event:&NSEvent) {
            if select_all_key(event) { unsafe { let _:()=msg_send![self,selectAll:Option::<&AnyObject>::None]; } }
            else { unsafe { let _:()=msg_send![super(self),keyDown:event]; } }
        }
        #[unsafe(method(performKeyEquivalent:))]
        fn key_equivalent(&self,event:&NSEvent)->bool {
            if select_all_key(event) { unsafe { let _:()=msg_send![self,selectAll:Option::<&AnyObject>::None]; } true }
            else { unsafe { msg_send![super(self),performKeyEquivalent:event] } }
        }
    }
);
impl RenameFieldEditor {
    fn new(m: MainThreadMarker) -> Retained<Self> {
        let editor: Retained<Self> = unsafe {
            msg_send![super(Self::alloc(m).set_ivars(())),initWithFrame:rect(0.,0.,100.,26.)]
        };
        editor.setFieldEditor(true);
        editor
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
    backdrop: OnceCell<crate::backdrop::Backdrop>,
    rename_field: Cell<Option<std::ptr::NonNull<AnyObject>>>,
    rename_text_view: OnceCell<Retained<RenameFieldEditor>>,
    rename_blur_requested: Cell<bool>,
    startup_menu: OnceCell<Retained<NSMenu>>,
    startup_menu_signature: RefCell<String>,
    settings_tracking: Cell<bool>,
}
struct Ui {
    fixture: fixtures::FixtureState,
    client: Client,
    refresh_started: std::time::Instant,
    refresh: crate::schedule::RefreshSchedule,
    window: Retained<NSWindow>,
    backdrop: crate::backdrop::Backdrop,
    tabs: Retained<NSView>,
    surface: Retained<WorkspaceSurface>,
    hint: Retained<NSTextField>,
    status: Retained<NSTextField>,
    permission_button: Retained<NSButton>,
    resume_button: Retained<NSButton>,
    replace_button: Retained<NSButton>,
    rename_editor: Option<RenameEditor>,
    picker: crate::picker::InlinePicker,
    picker_open: bool,
    pending_picker: bool,
    pending_settings: bool,
    settings_button: Retained<NSButton>,
    replacement: Option<TabId>,
    editing: Option<TabId>,
    tab_signature: String,
    manager: Option<GlobalHotKeyManager>,
    keys: Vec<HotKey>,
    registered: bool,
    shortcut_error: Option<String>,
    last_close_attempt: u64,
    last_frame: Option<Rect>,
    last_area: Option<Rect>,
    editing_focus_pending: Option<u64>,
    pending_rename: Option<TabId>,
    raise_after: Option<std::time::Instant>,
    last_direct_window: Option<u32>,
}
fn sync_selection(u: &mut Ui, s: &worker::Snapshot) {
    u.editing = u
        .rename_editor
        .as_ref()
        .map(|editor| editor.id)
        .or(u.pending_rename)
        .or_else(|| selection_for_actions(u.editing, s.selected, &s.workspace.tabs, &s.live));
    // An open rename editor keeps its original ID and view. Selection may
    // still change underneath it, so update rendered indicators in place.
    for view in u.tabs.subviews() {
        if let Some(button) = view.downcast_ref::<TabButton>() {
            let id = button.tag() as TabId;
            button.set_indication(tab_indication(
                id,
                s.live.iter().any(|(tab, _)| *tab == id),
                s.selected,
                u.editing,
            ));
        }
    }
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
    unsafe impl NSMenuDelegate for Delegate {
        #[unsafe(method(menuWillOpen:))] fn settings_opened(&self,_:&NSMenu){self.ivars().settings_tracking.set(true);}
        #[unsafe(method(menuDidClose:))] fn settings_closed(&self,_:&NSMenu){self.ivars().settings_tracking.set(false);}
    }
    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method_id(windowWillReturnFieldEditor:toObject:))]
        fn field_editor(&self,_:&NSWindow,client:Option<&AnyObject>)->Option<Retained<AnyObject>> {
            // AppKit asks for field editors during calls made with the UI borrowed.
            // Route only this control through independent state, never the RefCell.
            self.ivars().rename_text_view.get()
                .filter(|_|client.is_some_and(|c|Some(std::ptr::NonNull::from(c))==self.ivars().rename_field.get()))
                .map(|editor|unsafe { Retained::cast_unchecked(editor.clone()) })
        }
        #[unsafe(method(windowShouldClose:))] fn close(&self,sender:&NSWindow)->bool {if self.ivars().ui.borrow().as_ref().is_some_and(|u|std::ptr::eq(&*u.window,sender)){self.close_request();false}else{true}}
        #[unsafe(method(windowWillMove:))] fn will_move(&self,_:&NSNotification){self.start_tracking();}
        #[unsafe(method(windowDidMove:))] fn moved(&self,_:&NSNotification){self.geometry();}
        #[unsafe(method(windowDidResize:))] fn resized(&self,_:&NSNotification){self.geometry();}
        #[unsafe(method(windowDidBecomeKey:))] fn key(&self,_:&NSNotification){self.ivars().raise_requested.set(true);if let Some(backdrop)=self.ivars().backdrop.get(){backdrop.keep_below_selected();}}
        #[unsafe(method(windowDidResignKey:))] fn resigned_key(&self,_:&NSNotification){self.ivars().rename_blur_requested.set(true);}
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
            let picker=self.ivars().ui.borrow().as_ref().is_some_and(|u|u.picker_open);
            if picker {
                if command==sel!(cancelOperation:){self.dismiss_picker(true);true}
                else if command==sel!(insertNewline:){self.attach_selected_window();true}
                else if command==sel!(moveDown:) || command==sel!(moveUp:){
                    if let Some(u)=self.ivars().ui.borrow_mut().as_mut(){u.picker.move_selection(if command==sel!(moveDown:){1}else{-1});}true
                } else {false}
            } else if !self.ivars().ui.borrow().as_ref().is_some_and(|u|u.rename_editor.is_some()){false}
            else if command==sel!(cancelOperation:){self.finish_rename(false);true}else if command==sel!(insertNewline:){self.finish_rename(true);true}else{false}
        }
        #[unsafe(method(controlTextDidChange:))] fn text_changed(&self,_:&NSNotification){self.filter();}
    }
    impl Delegate {
        #[unsafe(method(trackDrag:))] fn drag_timer(&self,_:&NSTimer){self.track_drag();}
        #[unsafe(method(tick:))] fn timer(&self,_:&NSTimer){self.tick();}
        #[unsafe(method(selectTab:))] fn select_tab(&self,sender:&NSButton){self.dismiss_picker(false);self.finish_rename(true);let mut b=self.ivars().ui.borrow_mut();if let Some(u)=b.as_mut(){let id=sender.tag() as u64;u.editing=Some(id);u.client.switch(id);}}
        #[unsafe(method(addWindow:))] fn add(&self,_:&AnyObject){self.show_picker(false);}
        #[unsafe(method(replaceWindow:))] fn replace(&self,_:&AnyObject){self.show_picker(true);}
        #[unsafe(method(refreshWindows:))] fn refresh(&self,_:&AnyObject){if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::Discover);}}
        #[unsafe(method(attachWindow:))] fn attach(&self,_:&AnyObject){self.attach_selected_window();}
        #[unsafe(method(cancelPicker:))] fn cancel_picker(&self,_:&AnyObject){self.dismiss_picker(true);}
        #[unsafe(method(selectPickerWindow:))] fn choose_window(&self,sender:&NSButton){if let Some(u)=self.ivars().ui.borrow_mut().as_mut(){u.picker.select_window(sender.tag() as u64);}}
        #[unsafe(method(attachPickerWindow:))] fn attach_row(&self,sender:&NSButton){
            let selected={let mut b=self.ivars().ui.borrow_mut();b.as_mut().is_some_and(|u|u.picker_open && u.picker.select_window(sender.tag() as u64))};
            // Release the UI borrow before attachment dismisses the picker.
            if selected{self.attach_selected_window();}
        }
        #[unsafe(method(renameTabButton:))] fn rename_button(&self,sender:&NSButton){self.begin_rename(sender.tag() as u64);}
        #[unsafe(method(releaseTab:))] fn release(&self,_:&AnyObject){self.release_current();}
        #[unsafe(method(releaseThisTab:))] fn release_this(&self,sender:&NSButton){if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::Release(sender.tag() as u64));}}
        #[unsafe(method(resumeDocking:))] fn resume(&self,_:&AnyObject){if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::Resume);u.client.send(Command::Discover);}}
        #[unsafe(method(permission:))] fn permission(&self,_:&AnyObject){if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::RequestPermission);}let url=objc2_foundation::NSURL::URLWithString(&NSString::from_str("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")).unwrap();NSWorkspace::sharedWorkspace().openURL(&url);}
        #[unsafe(method(spaceChanged:))] fn space(&self,_:&NSNotification){if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::Pause);}}
        #[unsafe(method(dragTab:))] fn drag(&self,sender:&NSPanGestureRecognizer){if sender.state()==NSGestureRecognizerState::Ended && let Some(view)=sender.view(){let b=self.ivars().ui.borrow();if let Some(u)=b.as_ref(){let id=view.tag() as u64;let point=sender.locationInView(Some(&u.tabs));let index=(point.x/TAB_WIDTH).max(0.) as usize;u.client.send(Command::Reorder(id,index));}}}
        #[unsafe(method(closeSettingsFixture:))] fn close_settings_fixture(&self,_:&NSTimer){self.finish_settings_fixture();}
        #[unsafe(method(showSettings:))] fn settings(&self,_:&AnyObject) {self.show_settings();}
        #[unsafe(method(saveStartupApps:))] fn save_apps(&self,_:&AnyObject) {if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::SaveStartupApps);}}
        #[unsafe(method(resetStartupApps:))] fn reset_apps(&self,_:&AnyObject) {if let Some(u)=self.ivars().ui.borrow().as_ref(){u.client.send(Command::ResetStartupApps);}}
        #[unsafe(method(toggleStartupApp:))] fn toggle_startup(&self,sender:&NSMenuItem) {
            let Some(bundle)=sender.representedObject().and_then(|o|o.downcast::<NSString>().ok()).map(|s|s.to_string()) else {return};
            if let Some(u)=self.ivars().ui.borrow().as_ref() {
                u.client.send(Command::SetStartupApp(StartupApp {bundle,name:sender.title().to_string()},sender.state()!=NSControlStateValueOn));
            }
        }
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
        let mut workspace = match persistence::load_for_launch(&persistence::path()) {
            Ok(w) => w,
            Err(e) => {
                let a = NSAlert::new(m);
                a.setMessageText(&NSString::from_str("AppDock could not open this workspace"));
                a.setInformativeText(&NSString::from_str(&e.to_string()));
                a.runModal();
                app.terminate(None);
                return;
            }
        };
        let fixture_child = if std::env::args()
            .any(|a| matches!(a.as_str(), "--frame-smoke" | "--pointer-smoke"))
        {
            workspace.geometry.width = 700.;
            workspace.geometry.height = 400.;
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--overlay-targets")
                .spawn()
                .expect("Cannot start disposable frame targets");
            println!("Disposable frame target process: {}", child.id());
            Some(child)
        } else {
            None
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
        let titlebar = NSTitlebarAccessoryViewController::new(m);
        titlebar.setLayoutAttribute(NSLayoutAttribute::Left);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        let title = NSTextField::labelWithString(&window.title(), m);
        title.setFont(Some(&theme::font(13.)));
        title.setTextColor(Some(&theme::color(theme::TEXT)));
        title.sizeToFit();
        let title_size = title.frame().size;
        title.setFrameOrigin(NSPoint::new(8., (24. - title_size.height) / 2.));
        let add_x = 8. + title_size.width + 12.;
        let titlebar_view =
            NSView::initWithFrame(NSView::alloc(m), rect(0., 0., add_x + 206., 24.));
        titlebar_view.addSubview(&title);
        let add = self.button("+ Add App", sel!(addWindow:), rect(add_x, 0., 100., 24.));
        add.setFont(Some(&theme::font(13.)));
        add.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        add.setBordered(true);
        add.setBezelColor(Some(&theme::color(theme::BORDER)));
        // Native bezel drawing can choose black text when the window becomes
        // active. Pin the attributed title color in both button states.
        let text_color = NSColor::whiteColor();
        let font = theme::font(13.);
        let attributes = NSDictionary::from_slices(
            &[unsafe { NSForegroundColorAttributeName }, unsafe {
                NSFontAttributeName
            }],
            &[&*text_color as &AnyObject, &*font as &AnyObject],
        );
        // The attribute values have the AppKit-required NSColor and NSFont types.
        let add_title = unsafe {
            NSAttributedString::new_with_attributes(&NSString::from_str("+ Add App"), &attributes)
        };
        add.setAttributedTitle(&add_title);
        add.setAttributedAlternateTitle(&add_title);
        add.setToolTip(Some(&NSString::from_str("Add an existing app window")));
        titlebar_view.addSubview(&add);
        let settings = self.button(
            "Settings",
            sel!(showSettings:),
            rect(add_x + 108., 0., 90., 24.),
        );
        settings.setBordered(true);
        settings.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        settings.setBezelColor(Some(&theme::color(theme::BORDER)));
        let settings_title = unsafe {
            NSAttributedString::new_with_attributes(&NSString::from_str("Settings"), &attributes)
        };
        settings.setAttributedTitle(&settings_title);
        settings.setAttributedAlternateTitle(&settings_title);
        settings.setToolTip(Some(&NSString::from_str(
            "Save or reset startup app choices",
        )));
        titlebar_view.addSubview(&settings);
        titlebar.setView(&titlebar_view);
        window.addTitlebarAccessoryViewController(&titlebar);
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
        surface.setClipsToBounds(true);
        window.setContentView(Some(&surface));
        let content = window.contentView().unwrap();
        let h = content.bounds().size.height;
        let w = content.bounds().size.width;
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(m),
            rect(FRAME, h - 36., w - 2. * FRAME, 34.),
        );
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        scroll.setHasHorizontalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setClipsToBounds(true);
        scroll.contentView().setClipsToBounds(true);
        let tabs = NSView::initWithFrame(NSView::alloc(m), rect(0., 0., w - 2. * FRAME, 32.));
        tabs.setClipsToBounds(true);
        scroll.setDocumentView(Some(&tabs));
        content.addSubview(&scroll);
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
                "Your apps, together.\n\nUse Add App to add an open app window.\nDouble-click a tab to rename it. Drag tabs to reorder.\nClose a tab to release its window; the app keeps running.",
            ),
            m,
        );
        hint.setFont(Some(&theme::font(13.)));
        hint.setTextColor(Some(&theme::color(theme::SECONDARY)));
        hint.sizeToFit();
        let hint_height = hint.frame().size.height;
        hint.setFrame(rect(
            24.,
            h - CHROME - 24. - hint_height,
            w - 48.,
            hint_height,
        ));
        hint.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&hint);
        let picker = crate::picker::InlinePicker::new(
            m,
            self,
            rect(FRAME, FRAME, w - 2. * FRAME, h - CHROME - 2. * FRAME),
        );
        unsafe {
            picker
                .search
                .setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        content.addSubview(&picker.view);
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
        let backdrop = crate::backdrop::Backdrop::new(m);
        let _ = self.ivars().backdrop.set(backdrop.clone());
        *self.ivars().ui.borrow_mut() = Some(Ui {
            client,
            window: window.clone(),
            backdrop,
            tabs,
            surface,
            hint,
            status,
            permission_button,
            resume_button,
            replace_button,
            refresh_started: std::time::Instant::now(),
            refresh: crate::schedule::RefreshSchedule::default(),
            rename_editor: None,
            picker,
            picker_open: false,
            pending_picker: false,
            pending_settings: false,
            settings_button: settings.clone(),
            replacement: None,
            editing: None,
            tab_signature: String::new(),
            manager: manager.ok(),
            keys,
            registered: false,
            shortcut_error,
            last_close_attempt: 0,
            last_frame: None,
            last_area: None,
            editing_focus_pending: None,
            pending_rename: None,
            raise_after: None,
            fixture: fixtures::FixtureState {
                fixture_child,
                ..Default::default()
            },
            last_direct_window: None,
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
        let startup = NSMenu::new(m);
        startup.setAutoenablesItems(false);
        startup.setDelegate(Some(ProtocolObject::from_ref(self)));
        let startup_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(m),
                &NSString::from_str("Auto-add at Startup"),
                None,
                &NSString::from_str(""),
            )
        };
        startup_item.setSubmenu(Some(&startup));
        self.ivars().startup_menu.set(startup).unwrap();
        let settings_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(m),
                &NSString::from_str("Settings…"),
                Some(sel!(showSettings:)),
                &NSString::from_str(","),
            )
        };
        unsafe {
            settings_item.setTarget(Some(self));
        }
        submenu.addItem(&settings_item);
        submenu.addItem(&startup_item);
        submenu.addItem(&NSMenuItem::separatorItem(m));
        submenu.addItem(&retry);
        submenu.addItem(&quit);
        root.setSubmenu(Some(&submenu));
        menu.addItem(&root);
        app.setMainMenu(Some(&menu));
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        // Set the Dock icon after launch and activation-policy setup: AppKit can
        // replace an icon assigned before app.run() with the executable default.
        let icon_data = NSData::with_bytes(include_bytes!("../assets/branding/appdock.png"));
        let icon = NSImage::initWithData(NSImage::alloc(), &icon_data)
            .expect("embedded AppDock logo must be a valid image");
        // SAFETY: A valid image is supplied; this never passes None.
        unsafe { app.setApplicationIconImage(Some(&icon)) };
        app.dockTile().display();
        window.makeKeyAndOrderFront(None);
        if std::env::args().any(|a| {
            matches!(
                a.as_str(),
                "--design-smoke" | "--ui-smoke" | "--frame-smoke" | "--pointer-smoke"
            )
        }) {
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
    fn show_settings(&self) {
        self.dismiss_picker(false);
        self.finish_rename(true);
        let mut b = self.ivars().ui.borrow_mut();
        let Some(u) = b.as_mut() else { return };
        u.pending_settings = true;
        u.editing_focus_pending = Some(u.client.set_text_editing(true));
    }
    fn open_settings(&self, button: &NSButton, client: &Client) {
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
        if let Some(window) = button.window() {
            window.makeKeyAndOrderFront(None);
        }
        self.arm_settings_fixture();
        if let Some(menu) = self.ivars().startup_menu.get() {
            menu.popUpMenuPositioningItem_atLocation_inView(
                None,
                NSPoint::new(0., button.bounds().size.height),
                Some(button),
            );
        }
        client.set_text_editing(false);
        client.send(Command::Raise);
    }
    fn update_startup_menu(&self, snapshot: &worker::Snapshot) {
        if self.ivars().settings_tracking.get() {
            return;
        }
        let Some(menu) = self.ivars().startup_menu.get() else {
            return;
        };
        let mut apps = std::collections::BTreeMap::new();
        for window in &snapshot.windows {
            if !window.identity.bundle.is_empty() {
                apps.insert(window.identity.bundle.clone(), window.app.clone());
            }
        }
        for app in &snapshot.workspace.startup_apps {
            apps.entry(app.bundle.clone())
                .or_insert_with(|| app.name.clone());
        }
        let can_save = snapshot.workspace.tabs.iter().any(|tab| {
            !tab.identity.bundle.is_empty() && snapshot.live.iter().any(|(id, _)| *id == tab.id)
        });
        let signature = format!("{apps:?}{:?}{can_save}", snapshot.workspace.startup_apps);
        if *self.ivars().startup_menu_signature.borrow() == signature {
            return;
        }
        *self.ivars().startup_menu_signature.borrow_mut() = signature;
        menu.removeAllItems();
        for (title, action, enabled) in [
            (
                "Save Current Apps for Startup",
                sel!(saveStartupApps:),
                can_save,
            ),
            (
                "Reset Saved App Choices",
                sel!(resetStartupApps:),
                !snapshot.workspace.startup_apps.is_empty(),
            ),
        ] {
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm()),
                    &NSString::from_str(title),
                    Some(action),
                    &NSString::from_str(""),
                )
            };
            unsafe {
                item.setTarget(Some(self));
            }
            item.setEnabled(enabled);
            menu.addItem(&item);
        }
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        let hint = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str("Checkmarks save automatically for the next startup."),
                None,
                &NSString::from_str(""),
            )
        };
        hint.setEnabled(false);
        menu.addItem(&hint);
        let hint = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str("Apps with several windows need your choice."),
                None,
                &NSString::from_str(""),
            )
        };
        hint.setEnabled(false);
        menu.addItem(&hint);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        if apps.is_empty() {
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm()),
                    &NSString::from_str("No app windows detected yet"),
                    None,
                    &NSString::from_str(""),
                )
            };
            item.setEnabled(false);
            menu.addItem(&item);
        }
        let mut apps: Vec<_> = apps.into_iter().collect();
        apps.sort_by(|a, b| a.1.cmp(&b.1));
        for (bundle, name) in apps {
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm()),
                    &NSString::from_str(&name),
                    Some(sel!(toggleStartupApp:)),
                    &NSString::from_str(""),
                )
            };
            unsafe {
                item.setRepresentedObject(Some(&NSString::from_str(&bundle)));
            }
            item.setState(
                if snapshot
                    .workspace
                    .startup_apps
                    .iter()
                    .any(|app| app.bundle == bundle)
                {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                },
            );
            unsafe {
                item.setTarget(Some(self));
            }
            item.setEnabled(true);
            menu.addItem(&item);
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
                    x: frame.x + FRAME,
                    y: frame.y + title_height + u.surface.chrome() + FRAME,
                    width: content.size.width - 2. * FRAME,
                    height: (content.size.height - u.surface.chrome() - 2. * FRAME).max(100.),
                };
                u.last_frame = Some(frame);
                u.last_area = Some(area);
                let primary = NSScreen::screens(self.mtm())
                    .firstObject()
                    .map(|s| s.frame().size.height)
                    .unwrap_or(900.);
                u.backdrop.follow(area, primary);
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
        u.picker.layout(rect(
            FRAME,
            FRAME,
            bounds.size.width - 2. * FRAME,
            (bounds.size.height - u.surface.chrome() - 2. * FRAME).max(100.),
        ));
        let body = u.window.convertRectToScreen(rect(
            FRAME,
            FRAME,
            bounds.size.width - 2. * FRAME,
            (bounds.size.height - u.surface.chrome() - 2. * FRAME).max(100.),
        ));
        let area = Rect::from_cocoa(
            body.origin.x,
            body.origin.y,
            body.size.width,
            body.size.height,
            primary,
        );
        if u.last_frame != Some(geometry) || u.last_area != Some(area) {
            u.last_frame = Some(geometry);
            u.last_area = Some(area);
            u.backdrop.follow(area, primary);
            u.raise_after = Some(std::time::Instant::now());
            u.client.send(Command::Resize(area, geometry));
        }
    }
    fn tick(&self) {
        let count = self.ivars().ticks.get();
        self.ivars().ticks.set(count + 1);
        let mut b = self.ivars().ui.borrow_mut();
        let Some(u) = b.as_mut() else { return };
        if self.ivars().rename_blur_requested.replace(false)
            && u.rename_editor.is_some()
            && !u.window.isKeyWindow()
        {
            drop(b);
            self.finish_rename(true);
            return;
        }
        let (publish, discover) = u.refresh.due(
            u.refresh_started.elapsed(),
            NSEvent::pressedMouseButtons() != 0,
        );
        if publish {
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
        }
        if discover {
            u.client.send(Command::Discover);
        }
        u.client
            .set_pointer_down(NSEvent::pressedMouseButtons() != 0);
        let mut s = u.client.snapshot.lock().unwrap().clone();
        self.update_startup_menu(&s);
        if publish {
            let targets = s
                .live
                .iter()
                .filter_map(|(tab, window)| {
                    let pid = s.windows.iter().find(|w| w.id == *window)?.pid;
                    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
                    let path = app
                        .bundleURL()
                        .or_else(|| app.executableURL())?
                        .path()?
                        .to_string();
                    Some((*tab, *window, path))
                })
                .collect::<Vec<_>>();
            let dock = NSRunningApplication::runningApplicationsWithBundleIdentifier(
                &NSString::from_str("com.apple.dock"),
            )
            .firstObject();
            if let Some(dock) = dock {
                u.client
                    .send(Command::ReadBadges(dock.processIdentifier(), targets));
            } else {
                u.client.send(Command::ReadBadges(0, vec![]));
            }
        }
        if std::env::args().any(|a| a == "--design-smoke") {
            s.badges.insert(7, "7".into());
            s.badges.insert(8, "•".into());
        }
        let Some(mut b) = self.fixture_picker_step(count, b, &s) else {
            return;
        };
        let u = b.as_mut().unwrap();
        if u.editing_focus_pending == Some(s.editing_ack) {
            u.editing_focus_pending = None;
            if let Some(id) = u.pending_rename.take() {
                drop(b);
                self.open_rename(id);
                return;
            }
            if u.pending_settings {
                u.pending_settings = false;
                let button = u.settings_button.clone();
                let client = u.client.clone();
                drop(b);
                self.open_settings(&button, &client);
                return;
            }
            if u.pending_picker {
                u.pending_picker = false;
                drop(b);
                self.present_picker();
                return;
            }
        }
        if !s.paused
            && u.rename_editor.is_none()
            && u.pending_rename.is_none()
            && !u.picker_open
            && !u.pending_picker
            && !u.pending_settings
            && !u.window.inLiveResize()
            && NSEvent::pressedMouseButtons() == 0
            && !self.ivars().dragging.get()
            && u.last_frame
                .is_some_and(|frame| frame.near(s.workspace.geometry))
            && u.last_area.is_some_and(|area| area.near(s.area))
            && let Some(selected) = s.selected_frame
        {
            let content = u.surface.bounds().size;
            let width = content.width.max(selected.width + 2. * FRAME);
            let height = content
                .height
                .max(selected.height + u.surface.chrome() + 2. * FRAME);
            if width - content.width > 3. || height - content.height > 3. {
                let window = u.window.clone();
                let frame = window.frame();
                let new_height = frame.size.height + height - content.height;
                drop(b);
                window.setFrame_display(
                    rect(
                        frame.origin.x,
                        frame.origin.y + frame.size.height - new_height,
                        width,
                        new_height,
                    ),
                    true,
                );
                return;
            }
        }
        if !s.paused
            && !s.quitting
            && !s.stopped
            && u.window.isVisible()
            && !u.window.isMiniaturized()
            && !NSApplication::sharedApplication(self.mtm()).isHidden()
            && let Some((number, frames)) = &s.backdrop
        {
            let primary = NSScreen::screens(self.mtm())
                .firstObject()
                .map(|s| s.frame().size.height)
                .unwrap_or(900.);
            u.backdrop.place(
                *number,
                Some(u.window.windowNumber() as u32),
                frames,
                primary,
            );
        } else {
            u.backdrop.hide();
        }
        let attached = std::env::args().any(|a| a == "--surface-smoke")
            || s.selected
                .is_some_and(|id| s.live.iter().any(|(tab, _)| *tab == id));
        // A non-opaque controls window can cast a second shadow along its
        // rectangular cutout, visible around the target's rounded corners.
        if u.window.hasShadow() == attached {
            u.window.setHasShadow(!attached);
        }
        u.surface.set_attached(attached && !u.picker_open);
        u.hint.setHidden(attached || u.picker_open);
        if self.ivars().raise_requested.replace(false) {
            u.raise_after = Some(std::time::Instant::now() - std::time::Duration::from_millis(251));
        }
        sync_selection(u, &s);
        let Some(mut b) = self.fixture_simple_step(count, b, &s) else {
            return;
        };
        let u = b.as_mut().unwrap();
        if u.raise_after.is_some_and(|t| t.elapsed().as_millis() > 250)
            && !u.window.inLiveResize()
            && !u.picker_open
            && !u.pending_picker
            && !u.pending_settings
            && u.rename_editor.is_none()
            && u.pending_rename.is_none()
        {
            u.raise_after = None;
            u.client.send(Command::Raise);
        }
        let Some(mut b) = self.fixture_docking_step(count, b, &s) else {
            return;
        };
        let u = b.as_mut().unwrap();
        if s.stopped {
            if let Some(mut child) = u.fixture.fixture_child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            if u.fixture.smoke_failed {
                std::process::exit(1);
            }
            if std::env::args().any(|a| {
                matches!(
                    a.as_str(),
                    "--dock-smoke" | "--picker-smoke" | "--frame-smoke" | "--pointer-smoke"
                )
            }) {
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
                    String::new()
                } else {
                    s.status.clone()
                }
            })
        };
        u.status.setStringValue(&NSString::from_str(&text));
        u.status.setHidden(text.is_empty());
        let chrome = if !text.is_empty() || !s.trusted || s.paused || disconnected {
            STATUS_CHROME
        } else {
            CHROME
        };
        if u.surface.set_chrome(chrome) {
            let bounds = u.surface.bounds();
            let mut frame = u.hint.frame();
            frame.origin.y = bounds.size.height - chrome - 24. - frame.size.height;
            u.hint.setFrame(frame);
            drop(b);
            self.geometry();
            return;
        }
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
        // Keep the external app above its surrounding frame so it receives mouse
        // input. Transparent drawing is not a window-level click-through contract.
        // Match the cached exact window number, not
        // merely its app: unrelated windows from the same process stay independent.
        if !s.paused
            && !s.quitting
            && !u.picker_open
            && !u.pending_picker
            && !u.pending_settings
            && u.rename_editor.is_none()
            && u.pending_rename.is_none()
            && !u.window.isMiniaturized()
        {
            let focused = foreground
                .filter(|pid| s.stacked_windows.iter().any(|(_, _, owner)| owner == pid))
                .and_then(|pid| {
                    let stack = crate::window_tracking::stack()?;
                    let front = stack.iter().find(|w| w.pid == pid)?;
                    let (tab, number, _) = s
                        .stacked_windows
                        .iter()
                        .find(|(_, number, owner)| *number == front.number && *owner == pid)?;
                    let index = stack.iter().position(|w| w.number == *number)?;
                    let already_behind = stack
                        .get(index + 1)
                        .is_some_and(|w| w.number as isize == u.window.windowNumber());
                    Some((*tab, *number, already_behind))
                });
            if let Some((tab, number, already_behind)) = focused {
                if u.last_direct_window != Some(number)
                    && s.selected != Some(tab)
                    && !u.client.is_switching_to(tab)
                {
                    u.editing = Some(tab);
                    u.client.switch(tab);
                }
                u.last_direct_window = Some(number);
                if already_behind
                    && u.fixture.fixture_reveal_ordered
                    && !u.fixture.fixture_reveal_tested
                {
                    self.verify_app_pointer_routes(
                        &u.window,
                        number,
                        s.selected_frame.expect("Missing selected frame"),
                    );
                    u.fixture.fixture_reveal_tested = true;
                    println!(
                        "Direct app focus brought AppDock forward without taking keyboard focus"
                    );
                }
                if !already_behind || !u.window.isVisible() {
                    let verify = u.fixture.fixture_reveal_started
                        && !u.fixture.fixture_reveal_tested
                        && u.fixture.fixture_child.is_some();
                    if verify {
                        u.fixture.fixture_reveal_ordered = true;
                    }
                    let window = u.window.clone();
                    drop(b);
                    let app = NSApplication::sharedApplication(self.mtm());
                    if app.isHidden() {
                        app.unhideWithoutActivation();
                    }
                    window.orderWindow_relativeTo(NSWindowOrderingMode::Below, number as isize);
                    if verify {
                        assert_eq!(
                            NSWorkspace::sharedWorkspace()
                                .frontmostApplication()
                                .map(|app| app.processIdentifier()),
                            foreground,
                            "Revealing AppDock stole app keyboard focus"
                        );
                    }
                    return;
                }
            } else {
                u.last_direct_window = None;
            }
        }
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
            if eligible
                && u.rename_editor.is_none()
                && u.pending_rename.is_none()
                && !u.picker_open
                && !u.pending_picker
                && !u.pending_settings
                && e.state == HotKeyState::Pressed
                && !s.workspace.tabs.is_empty()
            {
                let previous = u.keys.first().is_some_and(|k| k.id() == e.id);
                if let Some(id) = u.client.navigate(&s.workspace.tabs, s.selected, previous) {
                    u.editing = Some(id);
                }
            }
        }
        sync_selection(u, &s);
        let signature = format!(
            "{:?}{:?}{:?}{:?}{:?}",
            s.workspace.tabs, s.live, s.selected, u.editing, s.badges
        );
        if signature != u.tab_signature
            && u.rename_editor.is_none()
            && NSEvent::pressedMouseButtons() == 0
        {
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
                    rect(i as f64 * TAB_WIDTH + 4., 0., TAB_WIDTH - 8., 32.),
                    tab_indication(t.id, connected.is_some(), s.selected, u.editing),
                    s.badges.get(&t.id).and_then(|label| badge_text(label)),
                );
                button.setTitle(&NSString::from_str(&label));
                if let Some(cell) = button.cell() {
                    cell.setWraps(false);
                    cell.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                }
                button.setBordered(false);
                button.setAlignment(NSTextAlignment::Left);
                button.setFont(Some(&theme::font(13.)));
                button.setContentTintColor(Some(&theme::color(
                    if s.selected == Some(t.id) || (connected.is_none() && u.editing == Some(t.id))
                    {
                        theme::TEXT
                    } else {
                        theme::SECONDARY
                    },
                )));
                unsafe {
                    button.setTarget(Some(self));
                    button.setAction(Some(sel!(selectTab:)));
                }
                button.setTag(t.id as isize);
                let badge_hint = s
                    .badges
                    .get(&t.id)
                    .map(|label| format!(" — App-wide Dock badge: {label}"))
                    .unwrap_or_default();
                button.setToolTip(Some(&NSString::from_str(&format!(
                    "{}{} — double-click to rename, drag to reorder",
                    t.name, badge_hint
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
        let Some(u) = b.as_mut() else {
            return;
        };
        if !u.picker_open && !u.pending_picker {
            return;
        }
        let s = u.client.snapshot.lock().unwrap().clone();
        let query = u.picker.query();
        let mut windows: Vec<_> = s
            .windows
            .iter()
            .filter(|w| {
                w.eligible
                    && !s.occupied.contains(&w.id)
                    && u.fixture
                        .fixture_child
                        .as_ref()
                        .is_none_or(|child| w.pid == child.id() as i32)
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
        windows.sort_by_key(|w| (w.app.to_lowercase(), w.title.to_lowercase(), w.pid, w.id));
        u.picker.render(self.mtm(), self, windows);
    }
    fn show_picker(&self, replace: bool) {
        self.finish_rename(true);
        let (window, search) = {
            let mut b = self.ivars().ui.borrow_mut();
            let Some(u) = b.as_mut() else {
                return;
            };
            u.pending_settings = false;
            u.replacement = if replace { u.editing } else { None };
            if replace && u.replacement.is_none() {
                return;
            }
            u.picker.configure(replace);
            u.pending_picker = true;
            u.editing_focus_pending = Some(u.client.set_text_editing(true));
            u.client.send(Command::Discover);
            (u.window.clone(), u.picker.search.clone())
        };
        search.setStringValue(&NSString::from_str(""));
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
        window.makeKeyAndOrderFront(None);
        self.filter();
    }
    fn present_picker(&self) {
        let (window, view, search, surface, hint) = {
            let mut b = self.ivars().ui.borrow_mut();
            let Some(u) = b.as_mut() else {
                return;
            };
            u.picker_open = true;
            (
                u.window.clone(),
                u.picker.view.clone(),
                u.picker.search.clone(),
                u.surface.clone(),
                u.hint.clone(),
            )
        };
        self.ivars()
            .rename_text_view
            .get_or_init(|| RenameFieldEditor::new(self.mtm()));
        self.ivars()
            .rename_field
            .set(Some(std::ptr::NonNull::from(&*search as &AnyObject)));
        surface.set_attached(false);
        hint.setHidden(true);
        view.setHidden(false);
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
        window.makeKeyAndOrderFront(None);
        unsafe {
            search.selectText(None);
        }
        if self
            .ivars()
            .ui
            .borrow()
            .as_ref()
            .is_some_and(|u| u.fixture.fixture_child.is_some())
        {
            println!("Inline picker visible: {}", window.windowNumber());
        }
        self.filter();
    }
    fn dismiss_picker(&self, raise: bool) {
        let pending = {
            let mut b = self.ivars().ui.borrow_mut();
            let Some(u) = b.as_mut() else {
                return;
            };
            if !u.picker_open && !u.pending_picker {
                return;
            }
            u.picker_open = false;
            u.pending_picker = false;
            u.editing_focus_pending = None;
            let s = u.client.snapshot.lock().unwrap();
            let attached = s
                .selected
                .is_some_and(|id| s.live.iter().any(|(tab, _)| *tab == id));
            (
                u.window.clone(),
                u.picker.view.clone(),
                u.surface.clone(),
                u.hint.clone(),
                u.client.clone(),
                attached,
            )
        };
        let (window, view, surface, hint, client, attached) = pending;
        self.ivars().rename_field.set(None);
        window.makeFirstResponder(None);
        view.setHidden(true);
        surface.set_attached(attached);
        hint.setHidden(attached);
        client.set_text_editing(false);
        if raise {
            client.send(Command::Raise);
        }
    }
    fn attach_selected_window(&self) {
        let pending = {
            let b = self.ivars().ui.borrow();
            let Some(u) = b.as_ref() else {
                return;
            };
            if !u.picker_open {
                return;
            }
            u.picker
                .selected()
                .map(|id| (u.client.clone(), u.replacement, id))
        };
        if let Some((client, replacement, id)) = pending {
            self.dismiss_picker(false);
            client.send(Command::Attach(replacement, id));
        }
    }
    fn begin_rename(&self, id: TabId) {
        self.dismiss_picker(false);
        self.finish_rename(true);
        let window = {
            let mut b = self.ivars().ui.borrow_mut();
            let Some(u) = b.as_mut() else {
                return;
            };
            if !u
                .client
                .snapshot
                .lock()
                .unwrap()
                .workspace
                .tabs
                .iter()
                .any(|t| t.id == id)
            {
                return;
            }
            u.pending_settings = false;
            u.editing = Some(id);
            u.pending_rename = Some(id);
            u.editing_focus_pending = Some(u.client.set_text_editing(true));
            u.window.clone()
        };
        // Show the field only after the worker confirms all earlier focus IPC
        // has finished. Once it is editable, no old app-raise can steal its keys.
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
        window.makeKeyAndOrderFront(None);
    }
    fn open_rename(&self, id: TabId) {
        let (tabs, window, index, name) = {
            let mut b = self.ivars().ui.borrow_mut();
            let Some(u) = b.as_mut() else { return };
            let s = u.client.snapshot.lock().unwrap();
            let Some(index) = s.workspace.tabs.iter().position(|t| t.id == id) else {
                u.client.set_text_editing(false);
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
        self.ivars()
            .rename_text_view
            .get_or_init(|| RenameFieldEditor::new(self.mtm()));
        self.ivars()
            .rename_field
            .set(Some(std::ptr::NonNull::from(&*field as &AnyObject)));
        if let Some(u) = self.ivars().ui.borrow_mut().as_mut() {
            u.rename_editor = Some(RenameEditor {
                id,
                field: field.clone(),
            });
        }
        tabs.addSubview(&field);
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
        window.makeKeyAndOrderFront(None);
        unsafe {
            field.selectText(None);
        }
        // Keep the app body clickable while the header's rename field owns keys.
        let selected = {
            let b = self.ivars().ui.borrow();
            b.as_ref().and_then(|u| {
                u.client
                    .snapshot
                    .lock()
                    .unwrap()
                    .backdrop
                    .as_ref()
                    .map(|(number, _)| *number)
            })
        };
        if let Some(number) = selected {
            window.orderWindow_relativeTo(NSWindowOrderingMode::Below, number as isize);
        }
    }
    fn finish_rename(&self, save: bool) {
        let pending = {
            let mut b = self.ivars().ui.borrow_mut();
            let Some(u) = b.as_mut() else { return };
            let was_pending = u.pending_rename.take().is_some();
            let editor = u.rename_editor.take();
            if editor.is_some() || was_pending {
                u.editing_focus_pending = None;
                u.tab_signature.clear();
                Some((editor, u.client.clone()))
            } else {
                None
            }
        };
        if let Some((editor, client)) = pending {
            self.ivars().rename_field.set(None);
            if let Some(editor) = editor {
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
            client.set_text_editing(false);
        }
    }
    fn close_request(&self) {
        if let Some(u) = self.ivars().ui.borrow_mut().as_mut() {
            if u.pending_settings {
                u.pending_settings = false;
                u.editing_focus_pending = None;
                u.client.set_text_editing(false);
            }
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
