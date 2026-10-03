//! The two places this widget has to talk to the operating system directly.

use std::mem::size_of_val;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, SW_SHOWNORMAL, WS_EX_APPWINDOW,
    WS_EX_TOOLWINDOW,
};

/// The `HWND` behind an eframe window or creation context, as a plain integer.
///
/// An integer rather than an `HWND` because the other side of it is
/// [`crate::resident`], on another thread: a raw pointer is neither `Send` nor
/// meaningful to an atomic, and a window handle is a number that happens to be
/// spelled as a pointer.
pub fn window_handle(window: &impl HasWindowHandle) -> Option<isize> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return None;
    };
    Some(win32.hwnd.get())
}

/// Give the window's outline to the compositor, in the theme's colour.
///
/// # Why not paint it ourselves
///
/// The panel used to draw its own `radius-xlarge` corner and `stroke-default` edge.
/// That produced a double border, worst in the top corners, and the reason is that on
/// Windows 11 the window already *has* an outline: DWM rounds every top-level window
/// and strokes it, whatever the window said about decorations. Two rounded borders at
/// two different radii, with the gap between the arcs showing through — that is what
/// the doubling was.
///
/// Removing DWM's is not available to us. `DWMWA_BORDER_COLOR = DWMWA_COLOR_NONE`
/// clears the sides and the bottom but leaves a 1px line across the top, because the
/// window still carries `WS_CAPTION` for DWM to draw a caption edge from — and
/// clearing that style bit does not stick, since winit puts it back. The line is
/// composited over the client area, so nothing egui paints can cover it.
///
/// So the other way round: let DWM own the shape, and tell it what colour to use.
/// One border, one radius, antialiased by the compositor, and the corners clipped by
/// the same code path that clips every other window on the desktop. The panel then
/// paints a square, unstroked rect and lets the platform cut it out — which is also
/// why the window is transparent.
///
/// The cost is the radius: it is the platform's, not Azur's `radius-xlarge`. That is
/// the right way round. A window's shape is platform chrome — it should match the
/// window next to it — where `radius-xlarge` is for the cards and dialogs *inside* an
/// application.
///
/// Best-effort: `DWMWA_BORDER_COLOR` arrived in Windows 11, and on 10 the call fails
/// harmlessly, where DWM neither rounds nor strokes.
pub fn adopt_window_outline(window: &impl HasWindowHandle, border: egui::Color32) {
    let Some(raw) = window_handle(window) else {
        return;
    };
    let hwnd = HWND(raw as *mut core::ffi::c_void);

    unsafe {
        // Explicit rather than relying on the default, so the intent is on the page:
        // this window is rounded, and by DWM.
        let corners = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const corners).cast(),
            size_of_val(&corners) as u32,
        );
        // A `COLORREF` is `0x00bbggrr` — blue first, which is the one place in this
        // program where a colour is not in the order it is written.
        let colour: u32 =
            ((border.b() as u32) << 16) | ((border.g() as u32) << 8) | (border.r() as u32);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            (&raw const colour).cast(),
            size_of_val(&colour) as u32,
        );
    }
}

/// Take the window out of Alt-Tab, as well as out of the taskbar.
///
/// egui's `with_taskbar(false)` is not enough on its own: winit implements it with
/// `ITaskbarList::DeleteTab`, which removes the button and leaves the window in the
/// Alt-Tab list — measured as `WS_EX_APPWINDOW` still set and `WS_EX_TOOLWINDOW`
/// absent. That is wrong twice over for this program. A widget you summon with a
/// keystroke and dismiss by looking away is not a window anybody wants to Alt-Tab
/// *to*; and between startup and the first summon the window is parked off the
/// desktop but not hidden — see [`crate::PARKED`] — so without this it would sit in
/// the Alt-Tab list as an entry that switches to nothing at all.
///
/// `WS_EX_TOOLWINDOW` covers both, and it has to be re-asserted rather than set.
///
/// # Why every frame
///
/// Setting it once works and then stops being true. Measured: the call from the panel's
/// constructor reads back correctly and is *reverted afterwards* — winit rewrites the
/// extended style after the window is built, restoring `WS_EX_APPWINDOW`, which is the
/// same thing that defeated clearing `WS_CAPTION` earlier in this program's life. A
/// version of this that stopped at the first success left the widget in the switcher,
/// and looked like it had worked.
///
/// So `App`'s `logic` — the one hook that runs while the window is invisible — calls this on every frame instead. The steady state is
/// one `GetWindowLongPtrW` — the style is read, found to be right, and nothing is
/// written — against a toolkit that asserts its own idea of the style whenever it feels
/// like it. Frames only run while the window is visible or on its way there, which is
/// exactly when being in Alt-Tab would matter.
pub fn keep_out_of_alt_tab(window: &impl HasWindowHandle) {
    let Some(raw) = window_handle(window) else {
        return;
    };
    let hwnd = HWND(raw as *mut core::ffi::c_void);
    let tool = WS_EX_TOOLWINDOW.0 as isize;
    let app = WS_EX_APPWINDOW.0 as isize;
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if current & tool != 0 && current & app == 0 {
            return;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (current & !app) | tool);
    }
}

/// Open the Windows sound page.
///
/// The widget covers the controls people reach for a dozen times a day; spatial
/// audio, sample rates, exclusive mode and per-device properties are the long tail,
/// and the right place for those is the page that already has them. `ms-settings:`
/// is a registered protocol, so this is a shell open rather than a process launch —
/// no path to guess, and it works the same on a machine where the Settings app has
/// been updated out from under us.
pub fn open_sound_settings() {
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            w!("ms-settings:sound"),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}
