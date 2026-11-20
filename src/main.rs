//! PiDA Tray — lightweight Windows tray controller for a PiDA server.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod api;
mod cache;
mod catalog;
mod config;
mod constants;
mod poller;
mod power;
mod sound;
mod theme;
mod tray;
mod ui;

use std::sync::Arc;

use eframe::egui;

/// `--demo N`, `--demo=N`, or `PIDA_TRAY_DEMO=N` starts an isolated instance
/// with N synthetic devices and no network. Bare `--demo` uses 8.
fn demo_count() -> Option<usize> {
    if let Ok(value) = std::env::var("PIDA_TRAY_DEMO") {
        return Some(value.trim().parse().unwrap_or(8));
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    for (i, arg) in args.iter().enumerate() {
        if arg == "--demo" {
            return Some(args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(8));
        }
        if let Some(rest) = arg.strip_prefix("--demo=") {
            return Some(rest.parse().unwrap_or(8));
        }
    }
    None
}

fn main() -> eframe::Result<()> {
    let demo = demo_count();
    let instance_id = match demo {
        Some(_) => format!("{}.demo", constants::APP_ID),
        None => constants::APP_ID.to_owned(),
    };
    let instance = single_instance::SingleInstance::new(&instance_id)
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

    let app_dir = match demo {
        Some(_) => {
            // Keep demo data away from the real profile and start fresh.
            let dir = std::env::temp_dir().join("pida-tray-demo");
            let _ = std::fs::remove_dir_all(&dir);
            dir
        }
        None => config::app_dir(),
    };
    let shared = poller::Shared::new(app_dir);
    let (poller_handle, commands) = poller::Poller::new();
    match demo {
        Some(count) => {
            poller::seed_demo(&shared, count);
            runtime.spawn(poller::run_demo(shared.clone(), commands));
        }
        None => {
            runtime.spawn(poller::run(shared.clone(), commands));
        }
    }

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
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_taskbar(false)
        .with_visible(false)
        .with_position(constants::PARKED_POS)
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

    drop(instance);
    runtime.shutdown_background();
    result
}

#[cfg(windows)]
fn hwnd_from_frame(frame: &eframe::Frame) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;

    let handle = frame.window_handle().ok()?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return None;
    };
    Some(HWND(win32.hwnd.get() as *mut core::ffi::c_void))
}

/// A region applied to the flyout window, cached so it is only re-applied when
/// it actually changes.
///
/// Windows keeps a region in *window* coordinates and does not adjust it when
/// the window is resized, so the size it was built for is part of the cache
/// key: a client-size change with an unchanged card top would otherwise leave
/// the card clipped to the old bounds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WindowRegion {
    /// Whole window; Windows tracks the bounds across resizes on its own.
    Full,
    /// Clipped to the card, for a window of the recorded size.
    Clipped { top: i32, width: i32, height: i32 },
}

/// Clip the window, including its hit-testing, to the card.
///
/// The session canvas leaves an empty strip above a short card. Left in the
/// window, that strip would swallow clicks meant for whatever is behind the
/// flyout, so the window region ends just above the card's top edge.
///
/// `top_px` is in window coordinates; at or above zero means "no clipping".
#[cfg(windows)]
pub(crate) fn set_window_region(frame: &eframe::Frame, top_px: i32) -> Option<WindowRegion> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, SetWindowRgn};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let hwnd = hwnd_from_frame(frame)?;
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return None;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let top = top_px.clamp(0, height);
    if top <= 0 {
        // Clearing the region hands bounds tracking back to Windows, which
        // follows resizes and DPI changes by itself.
        unsafe { SetWindowRgn(hwnd, None, true) };
        return Some(WindowRegion::Full);
    }
    let rgn = unsafe { CreateRectRgn(0, top, width, height) };
    if rgn.is_invalid() {
        return None;
    }
    if unsafe { SetWindowRgn(hwnd, Some(rgn), true) } == 0 {
        // On failure Windows does not take ownership of the region.
        let _ = unsafe { DeleteObject(rgn.into()) };
        return None;
    }
    Some(WindowRegion::Clipped { top, width, height })
}

#[cfg(not(windows))]
pub(crate) fn set_window_region(_frame: &eframe::Frame, _top_px: i32) -> Option<WindowRegion> {
    None
}

/// DWM draws a rectangular drop shadow plus a 1px border around every window
/// that has frame styles, even when `decorations(false)` hides the title bar.
/// Furthermore, `egui-winit` passes `with_undecorated_shadow(true)` on Windows
/// (ignoring `with_has_shadow(false)`), which triggers winit's `WM_NCCALCSIZE`
/// 1px top/bottom inset hack (`MARKER_UNDECORATED_SHADOW`). With DWM NC
/// rendering disabled, that 1px strip renders as a dark line across the top and
/// shifts the client surface down by 1px.
///
/// Subclassing the HWND once to return `0` for `WM_NCCALCSIZE` (leaving the
/// full window rect as client area) and `1` for `WM_ERASEBKGND` (preventing
/// white GDI background flashes on move/resize) eliminates both defects cleanly.
#[cfg(windows)]
fn ensure_borderless_window_policy(frame: &eframe::Frame) {
    use std::sync::atomic::{AtomicIsize, Ordering};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMWA_CLOAK, DWMWA_NCRENDERING_POLICY, DWMWA_TRANSITIONS_FORCEDISABLED,
        DwmSetWindowAttribute,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, DefWindowProcW, GWL_EXSTYLE, GWL_STYLE, GWLP_WNDPROC, HTBOTTOM,
        HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW,
        SetWindowPos, WM_ERASEBKGND, WM_NCACTIVATE, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCPAINT,
        WNDPROC, WS_BORDER, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, WS_MAXIMIZEBOX,
        WS_MINIMIZEBOX, WS_SYSMENU,
    };

    static HOOKED_HWND: AtomicIsize = AtomicIsize::new(0);
    static ORIG_WNDPROC: AtomicIsize = AtomicIsize::new(0);

    // Forward to the window procedure that winit installed.
    unsafe fn call_orig(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let orig = ORIG_WNDPROC.load(Ordering::Acquire);
        if orig != 0 {
            let proc: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(orig) };
            unsafe { CallWindowProcW(proc, hwnd, msg, wparam, lparam) }
        } else {
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }

    unsafe extern "system" fn borderless_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_NCCALCSIZE && wparam.0 != 0 {
            return LRESULT(0);
        }
        // Claim every non-client paint so Windows never draws a caption, even
        // if a style write from the windowing layer briefly restores one.
        if msg == WM_NCACTIVATE {
            return LRESULT(1);
        }
        if msg == WM_NCPAINT {
            return LRESULT(0);
        }
        if msg == WM_ERASEBKGND {
            return LRESULT(1);
        }
        if msg == WM_NCHITTEST {
            // The flyout carries its own height grip on the card's top edge; a
            // native resize border there would swallow that drag (and a click
            // near the card's bottom edge). Keep every edge a client area.
            let hit = unsafe { call_orig(hwnd, msg, wparam, lparam) };
            let code = hit.0 as u32;
            let resizing = matches!(
                code,
                HTLEFT
                    | HTRIGHT
                    | HTTOP
                    | HTTOPLEFT
                    | HTTOPRIGHT
                    | HTBOTTOM
                    | HTBOTTOMLEFT
                    | HTBOTTOMRIGHT
            );
            return if resizing {
                LRESULT(HTCLIENT as isize)
            } else {
                hit
            };
        }
        unsafe { call_orig(hwnd, msg, wparam, lparam) }
    }

    let Some(hwnd) = hwnd_from_frame(frame) else {
        return;
    };
    let hwnd_val = hwnd.0 as isize;

    if HOOKED_HWND
        .compare_exchange(0, hwnd_val, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        unsafe {
            // Cache the previous procedure before installing ours: a message
            // dispatched during the install must still reach winit.
            let prev =
                windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
            ORIG_WNDPROC.store(prev, Ordering::Release);
            let prev = SetWindowLongPtrW(
                hwnd,
                GWLP_WNDPROC,
                borderless_wnd_proc as *const () as usize as isize,
            );
            if prev != 0 && prev != ORIG_WNDPROC.load(Ordering::Acquire) {
                ORIG_WNDPROC.store(prev, Ordering::Release);
            }

            let disable_transitions: i32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED,
                &disable_transitions as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            );

            let cloaked: i32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_CLOAK,
                &cloaked as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            );

            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    // Strip the system frame and the "app window" registration. winit starts
    // from `WS_CAPTION | WS_BORDER | WS_SYSMENU | WS_SIZEBOX` and sets
    // `WS_EX_APPWINDOW`, and eframe never forwards `with_taskbar(false)`. The
    // leftover caption styles let DWM flash a title bar with close/minimise
    // buttons for a frame or two whenever it repaints the non-client area, and
    // `WS_EX_APPWINDOW` is why the tray-only process shows up in the taskbar
    // and Task Manager's app list. winit rewrites styles when its own flags
    // change, so this is re-applied every frame and only touches the window
    // when something actually drifted.
    //
    // `WS_THICKFRAME` is deliberately kept so the window is a normal resizable
    // window, but its resize borders are neutralized in `WM_NCHITTEST`: the
    // flyout's height is set by the card's own top-edge grip.
    //
    // The window deliberately stays a plain overlapped window (no `WS_POPUP`):
    // an owner-less popup changes activation behaviour enough that the flyout
    // lost foreground on open and its focus-loss auto-hide fired immediately.
    unsafe {
        let style =
            windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let wanted = style
            & !(WS_CAPTION.0 | WS_BORDER.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0);
        let ex =
            windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let wanted_ex = (ex & !WS_EX_APPWINDOW.0) | WS_EX_TOOLWINDOW.0;

        let mut changed = false;
        if style != wanted {
            SetWindowLongPtrW(hwnd, GWL_STYLE, wanted as isize);
            changed = true;
        }
        if ex != wanted_ex {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted_ex as isize);
            changed = true;
        }
        if changed {
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    // Re-applied on every call instead of once: DWM drops the non-client
    // policy when the window is moved or resized, which is exactly what the
    // flyout does on each show. The call is cheap.
    let policy: i32 = DWMNCRP_DISABLED.0;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY,
            &policy as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

#[cfg(not(windows))]
fn ensure_borderless_window_policy(_frame: &eframe::Frame) {}

/// Forces the flyout to the foreground without relying on the windowing
/// layer's own activation path.
///
/// A tray click lands in `explorer.exe`, so this process owns neither the
/// foreground window nor the last input event, and a plain `SetForegroundWindow`
/// is refused by the foreground lock. Attaching to the current foreground
/// thread first makes the call legal, which is what keeps the flyout from being
/// auto-hidden by its own focus-loss guard a frame after it opens.
#[cfg(windows)]
pub(crate) fn focus_flyout_window(frame: &eframe::Frame) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
    };

    let Some(hwnd) = hwnd_from_frame(frame) else {
        return;
    };
    unsafe {
        let foreground = GetForegroundWindow();
        let foreground_thread = if foreground.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground, None)
        };
        let our_thread = GetCurrentThreadId();
        let attached = foreground_thread != 0
            && foreground_thread != our_thread
            && AttachThreadInput(our_thread, foreground_thread, true).as_bool();

        let _ = SetForegroundWindow(hwnd);

        if attached {
            let _ = AttachThreadInput(our_thread, foreground_thread, false);
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn focus_flyout_window(_frame: &eframe::Frame) {}

/// Whether this window actually owns the Win32 foreground.
///
/// egui's `viewport().focused` is only as good as the windowing layer's own
/// bookkeeping, and eframe creates the window with `with_visible(false)`, so
/// that flag can stay `false` while Windows has long since handed us focus.
/// The foreground window is the source of truth for the click-away guard.
#[cfg(windows)]
pub(crate) fn flyout_has_foreground(frame: &eframe::Frame) -> Option<bool> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let hwnd = hwnd_from_frame(frame)?;
    let foreground = unsafe { GetForegroundWindow() };
    Some(foreground == hwnd)
}

#[cfg(not(windows))]
pub(crate) fn flyout_has_foreground(_frame: &eframe::Frame) -> Option<bool> {
    None
}

/// Moves the parked flyout to its final physical position and uncloaks it in a
/// single tick, reusing the frame it already painted while hidden.
///
/// egui's `OuterPosition`/`Visible` commands are applied after the frame, so
/// uncloaking alongside them would show the window at its parked coordinates
/// first. Driving the HWND directly is what removes the extra vsyncs from the
/// open path; the engine catches up with the new geometry on its next frame.
#[cfg(windows)]
pub(crate) fn reveal_flyout_at(frame: &eframe::Frame, x: f32, y: f32, width: f32, height: f32) {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos,
    };

    let Some(hwnd) = hwnd_from_frame(frame) else {
        return;
    };
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x.round() as i32,
            y.round() as i32,
            width.round() as i32,
            height.round() as i32,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
    set_window_cloaked(frame, false);
    focus_flyout_window(frame);
}

#[cfg(not(windows))]
pub(crate) fn reveal_flyout_at(
    _frame: &eframe::Frame,
    _x: f32,
    _y: f32,
    _width: f32,
    _height: f32,
) {
}

/// Controls DWM cloaking (`DWMWA_CLOAK`) so a mapped window can settle its
/// size and swap buffers offscreen before DWM composites it onto the desktop.
#[cfg(windows)]
pub(crate) fn set_window_cloaked(frame: &eframe::Frame, cloaked: bool) {
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};

    let Some(hwnd) = hwnd_from_frame(frame) else {
        return;
    };
    let val: i32 = i32::from(cloaked);
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            &val as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

#[cfg(not(windows))]
pub(crate) fn set_window_cloaked(_frame: &eframe::Frame, _cloaked: bool) {}
