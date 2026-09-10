//! Public WindowServer bounds for AppDock's own window, not AX identity matching.
use crate::model::Rect;
use objc2_core_foundation::{CFArray, CFDictionary, CFRetained, CFString, CFType, CGRect};
use objc2_core_graphics::{
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
    kCGWindowBounds,
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
