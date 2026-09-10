//! Inline window selection; all painting and controls use the AppDock palette.
use crate::{
    appearance as theme,
    model::{WindowId, WindowInfo},
};
use objc2::{
    DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::AnyObject, sel,
};
use objc2_app_kit::*;
use objc2_foundation::{
    MainThreadMarker, NSAttributedString, NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString,
};
use std::cell::Cell;

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
#[derive(Default)]
struct RowIvars {
    selected: Cell<bool>,
}
define_class!(
    #[unsafe(super=NSButton)] #[thread_kind=MainThreadOnly] #[ivars=RowIvars]
    struct ChoiceRow;
    unsafe impl NSObjectProtocol for ChoiceRow {}
    impl ChoiceRow {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self,event:&NSEvent) {
            if event.clickCount()==2 && self.isEnabled() {
                if let Some(target)=self.target() {
                    unsafe { let _:()=msg_send![&*target,attachPickerWindow:self]; }
                }
            } else { unsafe { let _:()=msg_send![super(self),mouseDown:event]; } }
        }
        #[unsafe(method(drawRect:))]
        fn draw(&self,dirty:NSRect) {
            theme::color(if self.ivars().selected.get(){theme::BORDER}else if self.isHighlighted(){theme::HOVER}else{theme::WINDOW}).setFill();
            NSRectFill(self.bounds());
            if self.ivars().selected.get() { theme::color(theme::TEXT).setFill(); NSRectFill(rect(0.,0.,2.,self.bounds().size.height)); }
            unsafe { let _:()=msg_send![super(self),drawRect:dirty]; }
        }
    }
);
impl ChoiceRow {
    fn new(m: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        unsafe {
            msg_send![super(Self::alloc(m).set_ivars(RowIvars::default())),initWithFrame:frame]
        }
    }
    fn select(&self, selected: bool) {
        self.ivars().selected.set(selected);
        let view: &NSView = self;
        view.setNeedsDisplay(true);
    }
}
define_class!(
    #[unsafe(super=NSView)] #[thread_kind=MainThreadOnly]
    struct PickerSurface;
    unsafe impl NSObjectProtocol for PickerSurface {}
    impl PickerSurface {
        #[unsafe(method(drawRect:))]
        fn draw(&self,_:NSRect) { theme::color(theme::SURFACE).setFill(); NSRectFill(self.bounds()); }
        #[unsafe(method(isOpaque))] fn opaque(&self)->bool { true }
    }
);
fn attributed(text: &str, color: u32, size: f64) -> Retained<NSAttributedString> {
    let color = theme::color(color);
    let font = theme::font(size);
    let attrs = NSDictionary::from_slices(
        &[unsafe { NSForegroundColorAttributeName }, unsafe {
            NSFontAttributeName
        }],
        &[&*color as &AnyObject, &*font as &AnyObject],
    );
    // The attributes have their documented NSColor/NSFont value types.
    unsafe { NSAttributedString::new_with_attributes(&NSString::from_str(text), &attrs) }
}
fn label(m: MainThreadMarker, text: &str, size: f64, color: u32) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), m);
    label.setFont(Some(&theme::font(size)));
    label.setTextColor(Some(&theme::color(color)));
    label
}
fn button(
    m: MainThreadMarker,
    target: &AnyObject,
    text: &str,
    action: objc2::runtime::Sel,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(text),
            Some(target),
            Some(action),
            m,
        )
    };
    button.setBordered(false);
    button.setAttributedTitle(&attributed(text, theme::TEXT, 13.));
    button
}
pub struct InlinePicker {
    pub view: Retained<NSView>,
    pub search: Retained<NSTextField>,
    title: Retained<NSTextField>,
    subtitle: Retained<NSTextField>,
    count: Retained<NSTextField>,
    empty: Retained<NSTextField>,
    scroll: Retained<NSScrollView>,
    list: Retained<NSView>,
    refresh: Retained<NSButton>,
    cancel: Retained<NSButton>,
    attach: Retained<NSButton>,
    rows: Vec<Retained<ChoiceRow>>,
    pub choice_ids: Vec<WindowId>,
    selected: Option<WindowId>,
    signature: String,
}
impl InlinePicker {
    pub fn new(m: MainThreadMarker, target: &AnyObject, frame: NSRect) -> Self {
        let surface: Retained<PickerSurface> =
            unsafe { msg_send![super(PickerSurface::alloc(m).set_ivars(())),initWithFrame:frame] };
        let view = Retained::into_super(surface);
        view.setHidden(true);
        let title = label(m, "Add an app window", 19., theme::TEXT);
        let subtitle = label(
            m,
            "Choose an open window to add to this workspace.",
            12.,
            theme::SECONDARY,
        );
        let count = label(m, "", 12., theme::SECONDARY);
        let empty = label(
            m,
            "No matching windows. Try another search or Refresh.",
            13.,
            theme::SECONDARY,
        );
        let search = NSTextField::initWithFrame(NSTextField::alloc(m), rect(0., 0., 100., 30.));
        search.setBezeled(false);
        search.setBordered(false);
        search.setDrawsBackground(true);
        search.setBackgroundColor(Some(&theme::color(theme::WINDOW)));
        search.setTextColor(Some(&theme::color(theme::TEXT)));
        search.setFont(Some(&theme::font(13.)));
        search.setFocusRingType(NSFocusRingType::None);
        search.setPlaceholderAttributedString(Some(&attributed(
            "Search apps or window titles",
            theme::SECONDARY,
            13.,
        )));
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(m), rect(0., 0., 100., 100.));
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(NSBorderType::NoBorder);
        let list = NSView::initWithFrame(NSView::alloc(m), rect(0., 0., 100., 100.));
        scroll.setDocumentView(Some(&list));
        let refresh = button(m, target, "Refresh", sel!(refreshWindows:));
        let cancel = button(m, target, "Cancel", sel!(cancelPicker:));
        let attach = button(m, target, "Attach window", sel!(attachWindow:));
        attach.setBordered(true);
        attach.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        attach.setBezelColor(Some(&theme::color(theme::BORDER)));
        attach.setEnabled(false);
        for child in [
            &*title as &NSView,
            &*subtitle,
            &*count,
            &*empty,
            &*search,
            &*scroll,
            &*refresh,
            &*cancel,
            &*attach,
        ] {
            view.addSubview(child);
        }
        let mut picker = Self {
            view,
            search,
            title,
            subtitle,
            count,
            empty,
            scroll,
            list,
            refresh,
            cancel,
            attach,
            rows: vec![],
            choice_ids: vec![],
            selected: None,
            signature: String::new(),
        };
        picker.layout(frame);
        picker
    }
    pub fn layout(&mut self, frame: NSRect) {
        self.view.setFrame(frame);
        let w = frame.size.width;
        let h = frame.size.height;
        self.title.setFrame(rect(24., h - 48., w - 48., 26.));
        self.subtitle.setFrame(rect(24., h - 70., w - 48., 18.));
        self.search.setFrame(rect(24., h - 114., w - 48., 30.));
        self.count.setFrame(rect(24., h - 140., w - 48., 18.));
        self.scroll
            .setFrame(rect(24., 68., w - 48., (h - 218.).max(50.)));
        self.empty.setFrame(rect(24., h - 175., w - 48., 24.));
        self.refresh.setFrame(rect(24., 20., 80., 28.));
        self.cancel.setFrame(rect(w - 238., 20., 80., 28.));
        self.attach.setFrame(rect(w - 150., 20., 126., 28.));
        self.layout_rows();
    }
    fn layout_rows(&self) {
        let size = self.scroll.contentSize();
        let height = (self.rows.len() as f64 * 46.).max(size.height);
        self.list.setFrameSize(NSSize::new(size.width, height));
        for (i, row) in self.rows.iter().enumerate() {
            row.setFrame(rect(0., height - (i + 1) as f64 * 46., size.width, 42.));
        }
    }
    pub fn configure(&mut self, replace: bool) {
        self.title.setStringValue(&NSString::from_str(if replace {
            "Replace app window"
        } else {
            "Add an app window"
        }));
        self.signature.clear();
        self.selected = None;
    }
    pub fn query(&self) -> String {
        self.search.stringValue().to_string().to_lowercase()
    }
    pub fn selected(&self) -> Option<WindowId> {
        self.selected.filter(|id| self.choice_ids.contains(id))
    }
    pub fn select_window(&mut self, id: WindowId) -> bool {
        if !self.choice_ids.contains(&id) {
            return false;
        }
        self.selected = Some(id);
        self.update_selection();
        true
    }
    pub fn move_selection(&mut self, delta: isize) {
        if self.choice_ids.is_empty() {
            return;
        }
        let index = self
            .selected
            .and_then(|id| {
                self.choice_ids
                    .iter()
                    .position(|candidate| *candidate == id)
            })
            .map_or(0, |i| {
                (i as isize + delta).clamp(0, self.choice_ids.len() as isize - 1) as usize
            });
        self.select_window(self.choice_ids[index]);
        self.list.scrollRectToVisible(self.rows[index].frame());
    }
    fn update_selection(&self) {
        for (row, id) in self.rows.iter().zip(&self.choice_ids) {
            row.select(Some(*id) == self.selected);
        }
        self.attach.setEnabled(self.selected().is_some());
        self.attach.setAttributedTitle(&attributed(
            "Attach window",
            if self.selected().is_some() {
                theme::TEXT
            } else {
                theme::SECONDARY
            },
            13.,
        ));
    }
    pub fn render(&mut self, m: MainThreadMarker, target: &AnyObject, windows: Vec<&WindowInfo>) {
        let signature = format!("{windows:?}");
        if signature == self.signature {
            return;
        }
        self.signature = signature;
        let previous = self.selected;
        self.choice_ids = windows.iter().map(|w| w.id).collect();
        self.selected = previous
            .filter(|id| self.choice_ids.contains(id))
            .or_else(|| {
                if previous.is_none() {
                    self.choice_ids.first().copied()
                } else {
                    None
                }
            });
        for row in self.rows.drain(..) {
            row.removeFromSuperview();
        }
        for window in windows {
            let row = ChoiceRow::new(m, rect(0., 0., 100., 42.));
            row.setBordered(false);
            row.setAlignment(NSTextAlignment::Left);
            row.setAttributedTitle(&attributed(
                &format!("  {} — {}", window.app, window.title),
                theme::TEXT,
                13.,
            ));
            if let Some(cell) = row.cell() {
                cell.setWraps(false);
                cell.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            }
            row.setTag(window.id as isize);
            row.setToolTip(Some(&NSString::from_str(&format!(
                "{} — {} (process {}, window {})",
                window.app, window.title, window.pid, window.id
            ))));
            unsafe {
                row.setTarget(Some(target));
                row.setAction(Some(sel!(selectPickerWindow:)));
            }
            self.list.addSubview(&row);
            self.rows.push(row);
        }
        self.empty.setHidden(!self.rows.is_empty());
        self.count.setStringValue(&NSString::from_str(&format!(
            "{} available window{}",
            self.rows.len(),
            if self.rows.len() == 1 { "" } else { "s" }
        )));
        self.layout_rows();
        self.update_selection();
        if let Some(index) = self.selected.and_then(|id| {
            self.choice_ids
                .iter()
                .position(|candidate| *candidate == id)
        }) {
            self.list.scrollRectToVisible(self.rows[index].frame());
        }
    }
}
