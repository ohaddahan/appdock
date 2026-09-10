//! Opaque normal-level cover below the selected external window, separate from
//! the transparent controls window so editing a tab never covers the app.
use crate::{appearance as theme, model::Rect, window_tracking};
use objc2::{MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSBackingStoreType, NSWindow, NSWindowAnimationBehavior, NSWindowOrderingMode,
    NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use std::{cell::Cell, rc::Rc};

#[derive(Clone)]
pub struct Backdrop {
    window: Retained<NSWindow>,
    selected: Rc<Cell<Option<u32>>>,
}
impl Backdrop {
    pub fn new(m: MainThreadMarker) -> Self {
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(m),
                NSRect::new(NSPoint::new(0., 0.), NSSize::new(100., 100.)),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setOpaque(true);
        window.setBackgroundColor(Some(&theme::color(theme::SURFACE)));
        window.setHasShadow(false);
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
        Self {
            window,
            selected: Rc::new(Cell::new(None)),
        }
    }
    pub fn hide(&self) {
        self.selected.set(None);
        if self.window.isVisible() {
            self.window.orderOut(None);
        }
    }
    /// Safe inside a key-window callback: no UI borrow, focus change or AX IPC.
    pub fn keep_below_selected(&self) {
        if let Some(number) = self.selected.get() {
            self.window
                .orderWindow_relativeTo(NSWindowOrderingMode::Below, number as isize);
        }
    }
    pub fn follow(&self, area: Rect, primary_height: f64) {
        if self.window.isVisible() {
            let size = self.window.frame().size;
            self.window.setFrame_display(
                NSRect::new(
                    NSPoint::new(area.x, primary_height - area.y - size.height),
                    size,
                ),
                true,
            );
        }
    }
    pub fn place(
        &self,
        selected: u32,
        controls: Option<u32>,
        frames: &[Rect],
        primary_height: f64,
    ) {
        let Some(stack) = window_tracking::stack() else {
            self.hide();
            return;
        };
        let Some(index) = stack.iter().position(|w| w.number == selected) else {
            self.hide();
            return;
        };
        self.selected.set(Some(selected));
        // Keep the cover behind both the app and its manager. The manager stays
        // behind the app during normal interaction and above it for inline UI.
        let anchor_index = controls
            .and_then(|number| stack.iter().position(|w| w.number == number))
            .map_or(index, |i| i.max(index));
        let anchor = stack[anchor_index].number;
        // Include actual bounds, including app-enforced minimum sizes.
        let Some(bounds) = coverage(frames.iter().copied()) else {
            self.hide();
            return;
        };
        let frame = NSRect::new(
            NSPoint::new(bounds.x, primary_height - bounds.y - bounds.height),
            NSSize::new(bounds.width, bounds.height),
        );
        if self.window.frame() != frame {
            self.window.setFrame_display(frame, true);
        }
        if stack
            .get(anchor_index + 1)
            .is_none_or(|w| w.number as isize != self.window.windowNumber())
        {
            self.window
                .orderWindow_relativeTo(NSWindowOrderingMode::Below, anchor as isize);
        }
    }
    pub fn number(&self) -> u32 {
        self.window.windowNumber() as u32
    }
}

fn coverage(frames: impl Iterator<Item = Rect>) -> Option<Rect> {
    frames.filter(|r| r.valid()).reduce(|a, b| {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Rect {
            x,
            y,
            width: (a.x + a.width).max(b.x + b.width) - x,
            height: (a.y + a.height).max(b.y + b.height) - y,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cover_includes_larger_inactive_windows_and_negative_desktop_coordinates() {
        let a = Rect {
            x: -800.,
            y: -200.,
            width: 1000.,
            height: 800.,
        };
        let b = Rect {
            x: -700.,
            y: -100.,
            width: 600.,
            height: 400.,
        };
        assert_eq!(coverage([a, b].into_iter()), Some(a));
        assert_eq!(coverage([].into_iter()), None);
    }
}
