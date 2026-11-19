//! Tray icon (in-memory RGBA), tooltip, context menu, and flyout placement.

use std::sync::{OnceLock, mpsc::Sender};

use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
};

use crate::{
    constants::{APP_NAME, TOOLTIP_MAX_CHARS, TRAY_ICON_SIZE},
    poller::Connection,
};

// ---------------------------------------------------------------------------
// Status key + rendering (pure)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKey {
    Unconfigured,
    Unknown,
    Online,
    Offline,
    Error,
}

impl StatusKey {
    pub fn from_state(conn: Connection, has_error: bool) -> Self {
        match (conn, has_error) {
            (Connection::Unconfigured, _) => Self::Unconfigured,
            (Connection::Offline, _) => Self::Offline,
            (Connection::Online, true) => Self::Error,
            (Connection::Online, false) => Self::Online,
            (Connection::Unknown, _) => Self::Unknown,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Online => "Connected",
            Self::Offline => "Offline",
            Self::Error => "Last action failed",
            Self::Unconfigured => "Not configured",
            Self::Unknown => "Not checked yet",
        }
    }
}

pub fn tooltip_text(key: StatusKey, devices: usize) -> String {
    let base = format!("{APP_NAME} — {}", key.label());
    let full = if devices > 0 && key != StatusKey::Unconfigured {
        format!(
            "{base} · {devices} device{}",
            if devices == 1 { "" } else { "s" }
        )
    } else {
        base
    };
    clamp_tooltip(&full, TOOLTIP_MAX_CHARS)
}

/// Truncate on a char boundary, adding an ellipsis when shortened.
pub fn clamp_tooltip(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// 32x32 RGBA of the bundled PiDA mark, tinted by connection state:
/// full colour online, grayscale when unreachable/unconfigured, red after a
/// failed action.
pub fn render_icon_rgba(key: StatusKey) -> Vec<u8> {
    let mut px = base_icon_rgba().to_vec();
    match key {
        StatusKey::Online => {}
        StatusKey::Offline | StatusKey::Unconfigured | StatusKey::Unknown => {
            grayscale(&mut px);
        }
        StatusKey::Error => red_tint(&mut px),
    }
    px
}

/// Decode + downscale `assets/icon.png` once. The icon is compiled into the exe.
fn base_icon_rgba() -> &'static [u8] {
    static ICON: OnceLock<Vec<u8>> = OnceLock::new();
    ICON.get_or_init(|| {
        let img = image::load_from_memory(include_bytes!("../assets/icon.png"))
            .expect("bundled assets/icon.png must be a valid PNG")
            .to_rgba8();
        image::imageops::resize(
            &img,
            TRAY_ICON_SIZE,
            TRAY_ICON_SIZE,
            image::imageops::FilterType::Lanczos3,
        )
        .into_raw()
    })
}

fn luminance(r: u8, g: u8, b: u8) -> u8 {
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32).round() as u8
}

fn grayscale(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        let l = luminance(p[0], p[1], p[2]);
        p[0] = l;
        p[1] = l;
        p[2] = l;
    }
}

fn red_tint(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        let l = luminance(p[0], p[1], p[2]) as f32 / 255.0;
        p[0] = (48.0 + 207.0 * l).min(255.0) as u8;
        p[1] = (8.0 + 48.0 * l).min(255.0) as u8;
        p[2] = (12.0 + 52.0 * l).min(255.0) as u8;
    }
}

// ---------------------------------------------------------------------------
// Flyout placement (pure; works in physical pixels)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect4 {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// Anchor the flyout next to the tray icon, inside the monitor's work area.
/// Taskbar side is inferred from the gap between the monitor and work rects.
pub fn compute_flyout_pos(
    anchor: (f32, f32),
    size: (f32, f32),
    work: Rect4,
    monitor: Rect4,
    margin: f32,
) -> (f32, f32) {
    let (w, h) = size;
    let clamp = |v: f32, lo: f32, hi: f32| v.min(hi.max(lo)).max(lo);

    let bottom_bar = work.bottom < monitor.bottom;
    let top_bar = work.top > monitor.top;
    let left_bar = work.left > monitor.left;
    let right_bar = work.right < monitor.right;

    if left_bar || right_bar {
        let x = if left_bar {
            work.left + margin
        } else {
            work.right - w - margin
        };
        let y = clamp(
            anchor.1 - h / 2.0,
            work.top + margin,
            work.bottom - h - margin,
        );
        return (x, y);
    }

    let x = clamp(
        anchor.0 - w / 2.0,
        work.left + margin,
        work.right - w - margin,
    );
    let y = if top_bar && !bottom_bar {
        work.top + margin
    } else {
        work.bottom - h - margin
    };
    (x, y.max(work.top + margin))
}

#[cfg(windows)]
pub fn monitor_rects_at(x: f32, y: f32) -> Option<(Rect4, Rect4)> {
    use windows::Win32::{
        Foundation::{POINT, RECT},
        Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint},
    };
    let conv = |r: RECT| Rect4 {
        left: r.left as f32,
        top: r.top as f32,
        right: r.right as f32,
        bottom: r.bottom as f32,
    };
    // SAFETY: `info.cbSize` is set as the API requires; the handle comes from MonitorFromPoint.
    unsafe {
        let hmon = MonitorFromPoint(
            POINT {
                x: x as i32,
                y: y as i32,
            },
            MONITOR_DEFAULTTONEAREST,
        );
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(hmon, &mut info)
            .as_bool()
            .then(|| (conv(info.rcWork), conv(info.rcMonitor)))
    }
}

#[cfg(not(windows))]
pub fn monitor_rects_at(_x: f32, _y: f32) -> Option<(Rect4, Rect4)> {
    None
}

/// Effective DPI scale (`dpi / 96.0`) of the monitor containing physical `(x, y)`.
#[cfg(windows)]
pub fn monitor_dpi_scale_at(x: f32, y: f32) -> Option<f32> {
    use windows::Win32::{
        Foundation::POINT,
        Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint},
        UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
    };
    // SAFETY: `hmon` comes from MonitorFromPoint and out pointers are valid stack integers.
    unsafe {
        let hmon = MonitorFromPoint(
            POINT {
                x: x as i32,
                y: y as i32,
            },
            MONITOR_DEFAULTTONEAREST,
        );
        let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
        GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)
            .ok()
            .filter(|()| dpi_x > 0)
            .map(|()| dpi_x as f32 / 96.0)
    }
}

#[cfg(not(windows))]
pub fn monitor_dpi_scale_at(_x: f32, _y: f32) -> Option<f32> {
    None
}

#[cfg(windows)]
pub fn cursor_pos() -> Option<(f32, f32)> {
    use windows::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};
    let mut p = POINT::default();
    // SAFETY: `p` is a valid out-pointer.
    unsafe { GetCursorPos(&mut p).ok().map(|_| (p.x as f32, p.y as f32)) }
}

#[cfg(not(windows))]
pub fn cursor_pos() -> Option<(f32, f32)> {
    None
}

// ---------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub enum TrayAction {
    /// Left click; carries the icon's centre in physical pixels.
    Toggle(Option<(f32, f32)>),
    /// Menu "Open / Hide Flyout".
    ToggleFromMenu,
    Refresh,
    Settings,
    Quit,
}

pub struct TrayController {
    icon: TrayIcon,
    last_key: Option<StatusKey>,
    last_tooltip: String,
}

impl TrayController {
    /// Must be created on the thread that runs the event loop.
    pub fn new(
        tx: Sender<TrayAction>,
        wake: impl Fn() + Send + Sync + Clone + 'static,
    ) -> Result<Self, String> {
        let toggle = MenuItem::new("Open / Hide Flyout", true, None);
        let refresh = MenuItem::new("Refresh", true, None);
        let settings = MenuItem::new("Settings...", true, None);
        let quit = MenuItem::new(format!("Quit {APP_NAME}"), true, None);
        let menu = Menu::new();
        let sep = PredefinedMenuItem::separator();
        let items: [&dyn IsMenuItem; 5] = [&toggle, &refresh, &settings, &sep, &quit];
        menu.append_items(&items).map_err(|e| e.to_string())?;

        let key = StatusKey::Unknown;
        let icon = TrayIconBuilder::new()
            .with_icon(rgba_icon(key)?)
            .with_tooltip(tooltip_text(key, 0))
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())?;

        let tx_icon = tx.clone();
        let wake_icon = wake.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = e
            {
                let cx = rect.position.x as f32 + rect.size.width as f32 / 2.0;
                let cy = rect.position.y as f32 + rect.size.height as f32 / 2.0;
                let _ = tx_icon.send(TrayAction::Toggle(Some((cx, cy))));
                wake_icon();
            }
        }));

        let ids: [(MenuId, TrayAction); 4] = [
            (toggle.id().clone(), TrayAction::ToggleFromMenu),
            (refresh.id().clone(), TrayAction::Refresh),
            (settings.id().clone(), TrayAction::Settings),
            (quit.id().clone(), TrayAction::Quit),
        ];
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if let Some((_, action)) = ids.iter().find(|(id, _)| *id == e.id) {
                let _ = tx.send(*action);
                wake();
            }
        }));

        Ok(Self {
            icon,
            last_key: Some(key),
            last_tooltip: tooltip_text(key, 0),
        })
    }

    /// Diff-cached: only touches the shell when the icon state or tooltip text changed.
    pub fn sync(&mut self, key: StatusKey, devices: usize) {
        if self.last_key != Some(key) {
            if let Ok(icon) = rgba_icon(key) {
                let _ = self.icon.set_icon(Some(icon));
            }
            self.last_key = Some(key);
        }
        let tip = tooltip_text(key, devices);
        if tip != self.last_tooltip {
            let _ = self.icon.set_tooltip(Some(&tip));
            self.last_tooltip = tip;
        }
    }
}

fn rgba_icon(key: StatusKey) -> Result<Icon, String> {
    Icon::from_rgba(render_icon_rgba(key), TRAY_ICON_SIZE, TRAY_ICON_SIZE)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect4 = Rect4 {
        left: 0.0,
        top: 0.0,
        right: 1920.0,
        bottom: 1080.0,
    };

    #[test]
    fn tooltip_is_clamped_on_char_boundaries() {
        let long = "é".repeat(300);
        let out = clamp_tooltip(&long, TOOLTIP_MAX_CHARS);
        assert_eq!(out.chars().count(), TOOLTIP_MAX_CHARS);
        assert!(out.ends_with('…'));
        assert_eq!(clamp_tooltip("short", 127), "short");
        assert!(tooltip_text(StatusKey::Online, 3).chars().count() <= TOOLTIP_MAX_CHARS);
    }

    #[test]
    fn tooltip_mentions_state_and_device_count() {
        assert!(tooltip_text(StatusKey::Online, 1).ends_with("1 device"));
        assert!(tooltip_text(StatusKey::Online, 2).ends_with("2 devices"));
        assert!(!tooltip_text(StatusKey::Unconfigured, 2).contains("device"));
    }

    #[test]
    fn status_key_mapping() {
        use Connection::*;
        assert_eq!(
            StatusKey::from_state(Unconfigured, true),
            StatusKey::Unconfigured
        );
        assert_eq!(StatusKey::from_state(Offline, false), StatusKey::Offline);
        assert_eq!(StatusKey::from_state(Online, true), StatusKey::Error);
        assert_eq!(StatusKey::from_state(Online, false), StatusKey::Online);
        assert_eq!(StatusKey::from_state(Unknown, false), StatusKey::Unknown);
    }

    #[test]
    fn icon_uses_brand_asset_and_state_tints() {
        let online = render_icon_rgba(StatusKey::Online);
        assert_eq!(online.len(), 32 * 32 * 4);
        assert_eq!(online[3], 0, "corner pixel should be transparent");
        assert!(
            online
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 200 && (p[0] != p[1] || p[1] != p[2])),
            "the online icon should keep the brand colours"
        );

        let offline = render_icon_rgba(StatusKey::Offline);
        assert!(
            offline
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[3] > 200)
                .all(|p| p[0] == p[1] && p[1] == p[2]),
            "offline icon should be grayscale"
        );

        let failed = render_icon_rgba(StatusKey::Error);
        assert!(
            failed
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[3] > 200)
                .all(|p| p[0] >= p[1] && p[0] >= p[2]),
            "error icon should be red-tinted"
        );

        assert_ne!(online, offline);
        assert_ne!(online, failed);
    }

    #[test]
    fn bottom_taskbar_places_above_and_centres_on_icon() {
        let work = Rect4 {
            bottom: 1040.0,
            ..MON
        };
        let (x, y) = compute_flyout_pos((1700.0, 1060.0), (360.0, 400.0), work, MON, 8.0);
        assert_eq!((x, y), (1520.0, 1040.0 - 400.0 - 8.0));
    }

    #[test]
    fn clamps_to_screen_margin_near_edges() {
        let work = Rect4 {
            bottom: 1040.0,
            ..MON
        };
        let (x, _) = compute_flyout_pos((1915.0, 1060.0), (360.0, 400.0), work, MON, 8.0);
        assert_eq!(x, 1920.0 - 360.0 - 8.0);
        let (x, _) = compute_flyout_pos((5.0, 1060.0), (360.0, 400.0), work, MON, 8.0);
        assert_eq!(x, 8.0);
    }

    #[test]
    fn top_taskbar_places_below() {
        let work = Rect4 { top: 40.0, ..MON };
        let (_, y) = compute_flyout_pos((900.0, 10.0), (360.0, 300.0), work, MON, 8.0);
        assert_eq!(y, 48.0);
    }

    #[test]
    fn side_taskbar_hugs_the_bar() {
        let work = Rect4 {
            right: 1860.0,
            ..MON
        };
        let (x, y) = compute_flyout_pos((1890.0, 500.0), (360.0, 300.0), work, MON, 8.0);
        assert_eq!((x, y), (1860.0 - 360.0 - 8.0, 350.0));
    }
}
