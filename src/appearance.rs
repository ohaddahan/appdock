//! Matches Terminator's default AppearanceConfig and Inter UI typography.
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSFont};
use objc2_foundation::NSString;
pub const SURFACE: u32 = 0x191A1C;
pub const WINDOW: u32 = 0x26282C;
pub const HOVER: u32 = 0x212326;
pub const BORDER: u32 = 0x33353B;
pub const TAB_IDLE: u32 = 0x292D34;
pub const TAB_ACTIVE: u32 = 0x3A4556;
pub const TAB_BORDER: u32 = 0x4B5260;
pub const TAB_ACCENT: u32 = 0xAAC9FF;
pub const TEXT: u32 = 0xD1D3D9;
pub const SECONDARY: u32 = 0x9FA2A8;
pub fn color(rgb: u32) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        ((rgb >> 16) & 255) as f64 / 255.,
        ((rgb >> 8) & 255) as f64 / 255.,
        (rgb & 255) as f64 / 255.,
        1.,
    )
}
pub fn font(size: f64) -> Retained<NSFont> {
    NSFont::fontWithName_size(&NSString::from_str("Inter"), size)
        .unwrap_or_else(|| NSFont::systemFontOfSize(size))
}
pub fn install_font() {
    let data = objc2_core_foundation::CFData::from_bytes(include_bytes!(
        "../assets/fonts/Inter-Regular.ttf"
    ));
    // Registration is process-local; do not install fonts into the user's account.
    unsafe {
        let descriptors = objc2_core_text::CTFontManagerCreateFontDescriptorsFromData(&data);
        objc2_core_text::CTFontManagerRegisterFontDescriptors(
            &descriptors,
            objc2_core_text::CTFontManagerScope::Process,
            true,
            None,
        );
    }
}
