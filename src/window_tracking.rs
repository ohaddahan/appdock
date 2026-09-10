//! Public WindowServer geometry and transient stacking anchors. AX owns identity.
use crate::model::Rect;
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGRect,
};
use objc2_core_graphics::{
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
    kCGWindowBounds, kCGWindowLayer, kCGWindowNumber, kCGWindowOwnerPID,
};
pub fn frame(window_number: isize) -> Option<Rect> {
    let id = u32::try_from(window_number).ok()?;
    let windows = CGWindowListCopyWindowInfo(CGWindowListOption::OptionIncludingWindow, id)?;
    // CGWindowListCopyWindowInfo documents an array of CF dictionaries.
    let windows = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(windows) };
    let info = windows.get(0)?.downcast::<CFDictionary>().ok()?;
    let info = unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(info) };
    let bounds = info
        .get(unsafe { kCGWindowBounds })?
        .downcast::<CFDictionary>()
        .ok()?;
    let mut r = CGRect::default();
    if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut r) } {
        return None;
    }
    Some(Rect {
        x: r.origin.x,
        y: r.origin.y,
        width: r.size.width,
        height: r.size.height,
    })
}

#[derive(Clone, Copy, Debug)]
pub struct StackWindow {
    pub number: u32,
    pub pid: i32,
    pub frame: Rect,
}

/// Normal onscreen windows, in front-to-back order. No titles or image capture.
pub fn stack() -> Option<Vec<StackWindow>> {
    let list = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    )?;
    let list = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(list) };
    Some(
        list.iter()
            .filter_map(|entry| {
                let dict = entry.downcast::<CFDictionary>().ok()?;
                let dict =
                    unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(dict) };
                let number = |key: &CFString| dict.get(key)?.downcast::<CFNumber>().ok()?.as_i64();
                if number(unsafe { kCGWindowLayer })? != 0 {
                    return None;
                }
                let bounds = dict
                    .get(unsafe { kCGWindowBounds })?
                    .downcast::<CFDictionary>()
                    .ok()?;
                let mut r = CGRect::default();
                if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut r) } {
                    return None;
                }
                Some(StackWindow {
                    number: u32::try_from(number(unsafe { kCGWindowNumber })?).ok()?,
                    pid: i32::try_from(number(unsafe { kCGWindowOwnerPID })?).ok()?,
                    frame: Rect {
                        x: r.origin.x,
                        y: r.origin.y,
                        width: r.size.width,
                        height: r.size.height,
                    },
                })
            })
            .collect(),
    )
}

/// Called only after AXRaise and exact AXFocusedWindow readback. The frontmost
/// normal window of that process must also match its AX frame; never guess by title.
pub fn focused_number(pid: i32, frame: Rect) -> Option<u32> {
    let window = stack()?.into_iter().find(|w| w.pid == pid)?;
    window.frame.near(frame).then_some(window.number)
}
