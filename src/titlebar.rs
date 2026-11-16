//! Colour the native Windows title bar to match the active palette.
//!
//! Windows 11 (build 22000+) lets an app set the caption and caption-text colours.
//! On older Windows the colour calls fail harmlessly and only the dark/light
//! switch applies.

use eframe::egui::Color32;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};

use crate::theme::Palette;

/// Remembers what was applied last, so the window is only touched when colours change.
#[derive(Default)]
pub struct TitleBar {
    last: Option<(Color32, Color32, bool)>,
}

fn colorref(c: Color32) -> u32 {
    // COLORREF is 0x00BBGGRR.
    (c.b() as u32) << 16 | (c.g() as u32) << 8 | c.r() as u32
}

impl TitleBar {
    pub fn apply(&mut self, frame: &eframe::Frame, pal: &Palette) {
        let want = (pal.bg, pal.text, pal.dark);
        if self.last == Some(want) {
            return;
        }
        let Some(window) = frame.winit_window() else { return };
        let Ok(handle) = window.window_handle() else { return };
        let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
        let hwnd = h.hwnd.get() as *mut core::ffi::c_void;
        let dark: i32 = pal.dark as i32;
        let caption = colorref(pal.bg);
        let text = colorref(pal.text);
        unsafe {
            DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, &dark as *const i32 as *const _, 4);
            DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR as u32, &caption as *const u32 as *const _, 4);
            DwmSetWindowAttribute(hwnd, DWMWA_TEXT_COLOR as u32, &text as *const u32 as *const _, 4);
        }
        self.last = Some(want);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_is_bgr() {
        assert_eq!(colorref(Color32::from_rgb(0x11, 0x22, 0x33)), 0x0033_2211);
    }
}
