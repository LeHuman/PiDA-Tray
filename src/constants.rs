//! App identity, timings, geometry, and dark palette.

use std::time::Duration;

use eframe::egui::{Color32, Pos2};

pub const APP_ID: &str = "PiDA-Tray";
pub const APP_NAME: &str = "PiDA Tray";
pub const APP_DIR_NAME: &str = "PiDA-Tray";
pub const AUTOSTART_VALUE_NAME: &str = "PiDATray";

// Geometry (egui points)
pub const FLYOUT_WIDTH: f32 = 360.0;
/// Stroke of the card frame. egui grows a `Frame` by its stroke size on every
/// side, so the window must include that growth or the right/bottom border is
/// clipped by the swap-chain edge.
pub const CARD_STROKE_WIDTH: f32 = 1.0;
/// Transparent bleed around the card so its anti-aliased edge and border never
/// land exactly on the swap-chain edge.
pub const CARD_BLEED: f32 = 1.0;
pub const FLYOUT_OUTER_WIDTH: f32 = FLYOUT_WIDTH + 2.0 * CARD_STROKE_WIDTH + 2.0 * CARD_BLEED;
pub const FLYOUT_MIN_HEIGHT: f32 = 260.0;
/// Initial panel height; users can drag the flyout edge to change it.
pub const FLYOUT_DEFAULT_HEIGHT: f32 = 720.0;
/// Upper bound; the actual cap is also clamped to the monitor work area.
pub const FLYOUT_MAX_HEIGHT: f32 = 900.0;
/// Where a hidden flyout is parked off every monitor so it stays mapped and warm.
pub const PARKED_POS: Pos2 = Pos2::new(-32000.0, -32000.0);
pub const FLYOUT_SCREEN_MARGIN: f32 = 8.0;
pub const FLYOUT_PADDING: f32 = 12.0;
pub const FLYOUT_RADIUS: u8 = 12;
pub const CARD_RADIUS: u8 = 10;

// Timings
pub const FOCUS_LOSS_GRACE: Duration = Duration::from_millis(220);
pub const SLIDER_DEBOUNCE: Duration = Duration::from_millis(250);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Parameterized POSTs include a server-side 4s settle loop and may queue up to 30s.
pub const DISPATCH_TIMEOUT: Duration = Duration::from_secs(40);

// Polling
pub const DEFAULT_IDLE_POLL_SECONDS: u64 = 60;
/// `0` means "Off" (refresh on flyout open only).
pub const IDLE_POLL_CHOICES: [u64; 4] = [0, 30, 60, 120];

// Tray
/// `NOTIFYICONDATAW::szTip` holds 128 UTF-16 units including the terminator.
pub const TOOLTIP_MAX_CHARS: usize = 127;
pub const TRAY_ICON_SIZE: u32 = 32;

/// Dark slate palette matching the sibling tray aesthetic.
pub struct Palette {
    pub surface: Color32,
    pub surface_header: Color32,
    pub surface_container: Color32,
    pub surface_container_high: Color32,
    pub outline: Color32,
    pub track: Color32,
    pub on_surface: Color32,
    pub on_surface_variant: Color32,
    pub muted: Color32,
    pub primary: Color32,
    pub focus: Color32,
    pub on_primary: Color32,
    pub error: Color32,
    pub error_surface: Color32,
    pub ok: Color32,
    pub warn: Color32,
}

pub const PALETTE: Palette = Palette {
    surface: Color32::from_rgb(0x11, 0x13, 0x18), // #111318
    surface_header: Color32::from_rgb(0x14, 0x17, 0x1D), // #14171D
    surface_container: Color32::from_rgb(0x18, 0x1B, 0x22), // #181B22
    surface_container_high: Color32::from_rgb(0x21, 0x26, 0x30), // #212630
    outline: Color32::from_rgb(0x64, 0x74, 0x8B), // #64748B
    track: Color32::from_rgb(0x33, 0x3A, 0x48),   // #333A48
    on_surface: Color32::from_rgb(0xE6, 0xE8, 0xEE), // #E6E8EE
    on_surface_variant: Color32::from_rgb(0x9C, 0xA3, 0xAF), // #9CA3AF
    muted: Color32::from_rgb(0x8D, 0x95, 0xA5),   // #8D95A5
    primary: Color32::from_rgb(0x25, 0x63, 0xEB), // #2563EB
    focus: Color32::from_rgb(0x60, 0xA5, 0xFA),   // #60A5FA
    on_primary: Color32::WHITE,
    error: Color32::from_rgb(0xF8, 0x71, 0x71), // #F87171
    error_surface: Color32::from_rgb(0x2B, 0x16, 0x1B),
    ok: Color32::from_rgb(0x10, 0xB9, 0x81),   // #10B981
    warn: Color32::from_rgb(0xF5, 0x9E, 0x0B), // #F59E0B
};
