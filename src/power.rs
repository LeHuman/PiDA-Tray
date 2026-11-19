//! Hidden message-only window that wakes the poller after system resume.

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;

    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
            RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
        },
    };
    use windows::core::w;

    const WM_POWERBROADCAST: u32 = 0x0218;
    const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;

    type WakeFn = Box<dyn Fn() + Send + Sync>;
    static ON_RESUME: OnceLock<WakeFn> = OnceLock::new();

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_POWERBROADCAST && wparam.0 == PBT_APMRESUMEAUTOMATIC {
            if let Some(f) = ON_RESUME.get() {
                f();
            }
            return LRESULT(1);
        }
        // SAFETY: forwarding unmodified arguments from the system.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    pub fn spawn(on_resume: impl Fn() + Send + Sync + 'static) {
        if ON_RESUME.set(Box::new(on_resume)).is_err() {
            return;
        }
        std::thread::Builder::new()
            .name("power-listener".into())
            .spawn(|| {
                // SAFETY: standard message-only window setup on a dedicated thread.
                unsafe {
                    let Ok(hinst) = GetModuleHandleW(None) else {
                        return;
                    };
                    let class = w!("PiDATrayPowerListener");
                    let wc = WNDCLASSW {
                        lpfnWndProc: Some(wndproc),
                        hInstance: hinst.into(),
                        lpszClassName: class,
                        ..Default::default()
                    };
                    if RegisterClassW(&wc) == 0 {
                        return;
                    }
                    let Ok(_hwnd) = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        class,
                        w!(""),
                        WINDOW_STYLE(0),
                        0,
                        0,
                        0,
                        0,
                        Some(HWND_MESSAGE),
                        None,
                        Some(hinst.into()),
                        None,
                    ) else {
                        return;
                    };
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            })
            .ok();
    }
}

#[cfg(windows)]
pub use imp::spawn;

#[cfg(not(windows))]
pub fn spawn(_on_resume: impl Fn() + Send + Sync + 'static) {}
