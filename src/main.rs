//! PiDA Tray — lightweight Windows tray controller for a PiDA server.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod api;
mod cache;
mod catalog;
mod config;
mod constants;
mod poller;
mod power;
mod theme;
mod tray;
mod ui;

use std::sync::Arc;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let instance = single_instance::SingleInstance::new(constants::APP_ID)
        .expect("failed to create single-instance lock");
    if !instance.is_single() {
        return Ok(());
    }

    // Two worker threads are plenty for a handful of HTTP calls.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("pida-io")
        .enable_all()
        .build()
        .expect("failed to start async runtime");

    let shared = poller::Shared::new(config::app_dir());
    let (poller_handle, commands) = poller::Poller::new();
    runtime.spawn(poller::run(shared.clone(), commands));

    let app_icon = egui::IconData {
        rgba: tray::render_icon_rgba(tray::StatusKey::Online),
        width: constants::TRAY_ICON_SIZE,
        height: constants::TRAY_ICON_SIZE,
    };

    let viewport = egui::ViewportBuilder::default()
        .with_title(constants::APP_NAME)
        .with_app_id(constants::APP_NAME)
        .with_inner_size([
            constants::FLYOUT_OUTER_WIDTH,
            constants::FLYOUT_DEFAULT_HEIGHT,
        ])
        .with_decorations(false)
        .with_transparent(true)
        .with_has_shadow(false)
        .with_resizable(true)
        .with_min_inner_size([constants::FLYOUT_OUTER_WIDTH, constants::FLYOUT_MIN_HEIGHT])
        .with_max_inner_size([constants::FLYOUT_OUTER_WIDTH, constants::FLYOUT_MAX_HEIGHT])
        .with_active(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_taskbar(false)
        // Keep the window mapped but parked off-screen: hiding it with
        // `set_visible` leaves a blank swap chain that eframe does not repaint
        // until an unrelated input event arrives.
        .with_position([-32000.0, -32000.0])
        .with_icon(Arc::new(app_icon));

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        centered: false,
        persist_window: false,
        system_font_fallback: false,
        ..Default::default()
    };

    let result = eframe::run_native(
        constants::APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(ui::PiDaTrayApp::new(cc, shared, poller_handle)))),
    );

    drop(runtime);
    drop(instance);
    result
}

#[cfg(windows)]
static WINDOW_HWND: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

/// Disable DWM non-client rendering so Windows does not draw a 1px grey frame or
/// rectangular drop shadow around the transparent rounded flyout card.
#[cfg(windows)]
pub(crate) fn ensure_borderless_window_policy(frame: &eframe::Frame) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::sync::Once;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMWA_NCRENDERING_POLICY, DwmSetWindowAttribute,
    };

    static APPLIED: Once = Once::new();
    APPLIED.call_once(|| {
        let Ok(handle) = frame.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return;
        };
        WINDOW_HWND.set(win32.hwnd.get()).ok();
        let hwnd = HWND(win32.hwnd.get() as *mut core::ffi::c_void);
        let policy: i32 = DWMNCRP_DISABLED.0;
        // SAFETY: `hwnd` is a valid top-level window for the current frame and `policy` matches `i32` attribute size.
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_NCRENDERING_POLICY,
                (&raw const policy).cast(),
                size_of::<i32>() as u32,
            );
        }
    });
}

/// Cloak the mapped window so it cannot take focus or appear in Alt+Tab while parked.
#[cfg(windows)]
pub(crate) fn set_window_cloaked(cloaked: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};

    let Some(raw) = WINDOW_HWND.get().copied() else {
        return;
    };
    let value: i32 = i32::from(cloaked);
    let hwnd = HWND(raw as *mut core::ffi::c_void);
    // SAFETY: the HWND was captured from the live window and `value` lives for the call.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            (&raw const value).cast(),
            size_of::<i32>() as u32,
        );
    }
}

#[cfg(not(windows))]
pub(crate) fn ensure_borderless_window_policy(_frame: &eframe::Frame) {}

#[cfg(not(windows))]
pub(crate) fn set_window_cloaked(_cloaked: bool) {}
