//! `PiDaTrayApp`: reveal/park lifecycle, focus-loss grace, and the card shell.

mod button;
mod devices;
mod header;
mod icons;
mod settings;

use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use eframe::egui::{
    self, CornerRadius, Margin, Rect, RichText, Stroke, StrokeKind, Ui, ViewportCommand, vec2,
};

use crate::{
    constants::{
        CARD_BLEED, CARD_STROKE_WIDTH, FLYOUT_DEFAULT_HEIGHT, FLYOUT_MAX_HEIGHT, FLYOUT_MIN_HEIGHT,
        FLYOUT_OUTER_WIDTH, FLYOUT_PADDING, FLYOUT_RADIUS, FLYOUT_SCREEN_MARGIN, FLYOUT_WIDTH,
        FOCUS_LOSS_GRACE, PALETTE, PARKED_POS,
    },
    poller::{Command, Connection, Poller, Shared},
    power,
    theme::{self, border_card},
    tray::{self, StatusKey, TrayAction, TrayController},
};

use button::{ButtonKind, pill_button};

const VIEW_ANIMATION: f32 = 0.18;
/// Corner radius of the card's inner fill (the outer radius minus its stroke).
const SHEET_RADIUS: u8 = FLYOUT_RADIUS - CARD_STROKE_WIDTH as u8;

/// Round a logical height up to a whole physical pixel. Fractional inner sizes
/// truncate when DPI scaling converts them, which clips the card's bottom edge.
fn quantize_height(ctx: &egui::Context, height: f32) -> f32 {
    let ppp = ctx.pixels_per_point().max(0.5);
    (height * ppp).ceil() / ppp
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Devices,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RevealPhase {
    Hidden,
    /// Position and uncloak in one tick, reusing the frame the parked window
    /// already painted.
    RevealNow,
    Settling,
    ReadyToUncloak,
    Shown,
}

pub struct PiDaTrayApp {
    shared: Arc<Shared>,
    poller: Poller,
    actions: mpsc::Receiver<TrayAction>,
    tray: Option<TrayController>,

    visible: bool,
    reveal_phase: RevealPhase,
    /// Logical position to reveal at, consumed on the next tick.
    pending_reveal: Option<egui::Pos2>,
    view: View,
    /// Tray-icon centre in physical pixels, used to (re)place the flyout.
    anchor: Option<(f32, f32)>,
    last_viewport_height: f32,
    /// Last client height requested from the OS. Held for the duration of an
    /// animation so the window is resized once per transition rather than
    /// every frame (per-frame resizes make Windows composite stale frames).
    last_window_height: f32,
    /// Set when the user drags the flyout to a custom height.
    manual_height: Option<f32>,
    /// Content-fit cap for this monitor's work area.
    max_height: f32,
    /// Frames to wait for our own size command before treating a different
    /// window height as a user resize.
    size_settle: u8,
    dirty_height: bool,
    shown_at: Option<Instant>,
    seen_focused_while_shown: bool,
    hidden_by_blur_at: Option<Instant>,
    suppress_next_tray_up: bool,
    quitting: bool,
    parked: bool,
    /// Set once the flyout has actually painted a frame, so the parked swap
    /// chain holds real content that can be revealed immediately.
    painted_once: bool,

    devices: devices::DevicesView,
    settings: settings::SettingsView,
    /// Eased position between the devices grid (0.0) and settings (1.0).
    view_mix: f32,
    /// Card height when the current view transition started.
    view_from_height: Option<f32>,
    /// Sign of the last transition step, to restart the ease from the rendered
    /// card height when the direction reverses.
    view_dir: f32,
}

impl PiDaTrayApp {
    pub fn new(cc: &eframe::CreationContext<'_>, shared: Arc<Shared>, poller: Poller) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::install_fonts(&ctx);
        theme::apply_visuals(&ctx);

        // Any state change (even while hidden) wakes `logic()` once so the tray refreshes.
        {
            let ctx = ctx.clone();
            shared.set_repaint(move || ctx.request_repaint());
        }

        let (tx, actions) = mpsc::channel();
        let wake_ctx = ctx.clone();
        let tray = match TrayController::new(tx, move || wake_ctx.request_repaint()) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("failed to create tray icon: {e}");
                None
            }
        };

        {
            let (poller, ctx) = (poller.clone(), ctx.clone());
            power::spawn(move || {
                poller.send(Command::Wake);
                ctx.request_repaint();
            });
        }

        let manual_height = shared
            .settings()
            .flyout_height
            .filter(|h| h.is_finite())
            .map(|h| h.max(FLYOUT_MIN_HEIGHT));
        let last_viewport_height = manual_height.unwrap_or(FLYOUT_DEFAULT_HEIGHT);

        Self {
            shared,
            poller,
            actions,
            tray,
            visible: false,
            reveal_phase: RevealPhase::Hidden,
            pending_reveal: None,
            view: View::Devices,
            anchor: None,
            last_viewport_height,
            last_window_height: last_viewport_height,
            manual_height,
            max_height: FLYOUT_MAX_HEIGHT,
            size_settle: 0,
            dirty_height: false,
            shown_at: None,
            seen_focused_while_shown: false,
            hidden_by_blur_at: None,
            suppress_next_tray_up: false,
            quitting: false,
            parked: false,
            painted_once: false,
            devices: Default::default(),
            settings: Default::default(),
            view_mix: 0.0,
            view_from_height: None,
            view_dir: 0.0,
        }
    }

    // -- window management ---------------------------------------------------

    /// Logical `OuterPosition` for the current anchor and height. Also refreshes
    /// `max_height` from the monitor work area.
    fn compute_placement(&mut self, ctx: &egui::Context, height: f32) -> Option<egui::Pos2> {
        let anchor = self.anchor?;
        let (work, monitor) = tray::monitor_rects_at(anchor.0, anchor.1)?;
        let current_ppp = ctx.pixels_per_point().max(0.5);
        let target_ppp = tray::monitor_dpi_scale_at(anchor.0, anchor.1)
            .unwrap_or(current_ppp)
            .max(0.5);
        self.max_height = ((work.bottom - work.top) / target_ppp - 2.0 * FLYOUT_SCREEN_MARGIN)
            .clamp(FLYOUT_MIN_HEIGHT, FLYOUT_MAX_HEIGHT);
        let height = height.clamp(FLYOUT_MIN_HEIGHT, self.max_height);
        let size_px = (FLYOUT_OUTER_WIDTH * target_ppp, height * target_ppp);
        let (x, y) = tray::compute_flyout_pos(
            anchor,
            size_px,
            work,
            monitor,
            FLYOUT_SCREEN_MARGIN * target_ppp,
        );
        // `egui-winit` multiplies `OuterPosition` by the window's *current* `pixels_per_point`.
        Some(egui::pos2(x / current_ppp, y / current_ppp))
    }

    fn place(&mut self, ctx: &egui::Context, height: f32) {
        if let Some(pos) = self.compute_placement(ctx, height) {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        }
    }

    fn park_window(&mut self, ctx: &egui::Context) {
        if self.parked {
            return;
        }
        self.parked = true;
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(PARKED_POS));
    }

    fn show(&mut self, ctx: &egui::Context, anchor: Option<(f32, f32)>, settle: bool) {
        if let Some(anchor) = anchor {
            self.anchor = Some(anchor);
        } else if self.anchor.is_none() {
            self.anchor = tray::cursor_pos();
        }

        if self.reveal_phase == RevealPhase::Shown {
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.shown_at = Some(Instant::now());
            self.seen_focused_while_shown = false;
            return;
        }

        // The parked window's size, not the card's animated height: they only
        // differ if the flyout was hidden mid-transition.
        let previous_height = self.last_window_height;
        let desired = self.manual_height.unwrap_or(previous_height);
        let target = quantize_height(ctx, desired.clamp(FLYOUT_MIN_HEIGHT, self.max_height));
        let pos = self.compute_placement(ctx, target);

        self.visible = true;
        self.view = View::Devices;
        self.view_mix = 0.0;
        self.shown_at = None;
        self.seen_focused_while_shown = false;
        self.shared.set_visible(true);
        self.last_viewport_height = target;
        self.last_window_height = target;
        self.size_settle = 3;

        // Fast path: the parked swap chain already holds a frame at exactly
        // this size, so `logic` can move and uncloak the window in one tick
        // instead of waiting for viewport commands to round-trip.
        let fast = cfg!(windows)
            && !settle
            && self.painted_once
            && pos.is_some()
            && (target - previous_height).abs() <= f32::EPSILON;
        if fast {
            self.pending_reveal = pos;
            self.reveal_phase = RevealPhase::RevealNow;
        } else {
            self.pending_reveal = None;
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(FLYOUT_OUTER_WIDTH, target)));
            self.reveal_phase = RevealPhase::Settling;
        }

        self.poller.send(Command::Sync);
        ctx.request_repaint();
    }

    fn hide(&mut self, ctx: &egui::Context) {
        if self.dirty_height {
            let flyout_height = self.manual_height;
            self.shared
                .update(|s| s.settings.flyout_height = flyout_height);
            self.shared.save_settings();
            self.dirty_height = false;
        }
        self.devices.flush_all(&self.shared, &self.poller);
        self.devices.collapse_expanded();
        self.visible = false;
        self.reveal_phase = RevealPhase::Hidden;
        self.view = View::Devices;
        self.view_mix = 0.0;
        self.view_from_height = None;
        self.view_dir = 0.0;
        self.settings.reset();
        self.shown_at = None;
        self.seen_focused_while_shown = false;
        self.shared.set_visible(false);
        self.park_window(ctx);
        ctx.request_repaint();
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.hide(ctx);
        }
    }

    fn drain_tray_actions(&mut self, ctx: &egui::Context) {
        while let Ok(action) = self.actions.try_recv() {
            match action {
                TrayAction::PointerDown(anchor) => {
                    if let Some(anchor) = anchor {
                        self.anchor = Some(anchor);
                    }
                    let just_blurred = self
                        .hidden_by_blur_at
                        .take()
                        .is_some_and(|t| t.elapsed() < FOCUS_LOSS_GRACE);
                    if self.visible || just_blurred {
                        // Clicking the tray icon while open triggers taskbar focus
                        // loss on mouse-down; consume the matching mouse-up so the
                        // flyout closes cleanly instead of bouncing back open.
                        self.suppress_next_tray_up = true;
                        if self.visible {
                            self.hide(ctx);
                        }
                    }
                }
                TrayAction::Toggle(anchor) => {
                    if anchor.is_some() && self.suppress_next_tray_up {
                        self.suppress_next_tray_up = false;
                    } else {
                        self.suppress_next_tray_up = false;
                        self.toggle(ctx, anchor);
                    }
                }
                TrayAction::ToggleFromMenu => {
                    self.suppress_next_tray_up = false;
                    self.toggle(ctx, tray::cursor_pos());
                }
                TrayAction::Refresh => self.poller.send(Command::Sync),
                TrayAction::Settings => {
                    self.devices.collapse_expanded();
                    self.show(ctx, tray::cursor_pos(), true);
                    self.view = View::Settings;
                    self.settings.reset();
                }
                TrayAction::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }

    fn toggle(&mut self, ctx: &egui::Context, anchor: Option<(f32, f32)>) {
        if self.visible {
            self.hide(ctx)
        } else {
            self.show(ctx, anchor, false)
        }
    }

    /// Diff-cached inside `TrayController::sync`, so calling it every frame is cheap.
    fn sync_tray(&mut self) {
        let Some(tray) = self.tray.as_mut() else {
            return;
        };
        let (conn, has_error, devices) = self
            .shared
            .read(|s| (s.connection, s.banner.is_some(), s.catalog.len()));
        tray.sync(StatusKey::from_state(conn, has_error), devices);
    }

    fn apply_focus_policy(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let pinned = self.shared.read(|s| s.settings.pin_flyout);
        if pinned {
            self.seen_focused_while_shown = false;
            return;
        }

        let focused =
            crate::flyout_has_foreground(frame).or_else(|| ctx.input(|i| i.viewport().focused));
        if focused == Some(true) {
            self.seen_focused_while_shown = true;
        } else if focused == Some(false) {
            let past_grace = self
                .shown_at
                .map(|t| t.elapsed() >= FOCUS_LOSS_GRACE)
                .unwrap_or(false);
            // Only auto-hide if the flyout genuinely held focus and then lost
            // it. Windows can refuse activation for a tray-owned process
            // entirely; treating "never focused" as "user left" closed the
            // flyout a frame after it opened.
            if self.seen_focused_while_shown && past_grace {
                self.hidden_by_blur_at = Some(Instant::now());
                self.hide(ctx);
                return;
            }
        }

        // egui idles between events, and a click into another application
        // delivers no event to this window, so a blur would otherwise only be
        // noticed on the next poll tick. Pulse while the flyout is shown.
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    // -- drawing ---------------------------------------------------------------

    /// Advances the devices↔settings page transition.
    fn advance_view_mix(&mut self, ctx: &egui::Context) {
        let target = if self.view == View::Settings {
            1.0
        } else {
            0.0
        };
        if (self.view_mix - target).abs() <= f32::EPSILON {
            self.view_from_height = None;
            self.view_dir = 0.0;
            return;
        }
        let dir = (target - self.view_mix).signum();
        if self.view_dir != 0.0 && dir != self.view_dir {
            // Reversed mid-flight: ease from the card height actually on
            // screen, not from where the previous transition started.
            self.view_from_height = Some(self.last_viewport_height);
        }
        self.view_dir = dir;
        if self.view_from_height.is_none() {
            self.view_from_height = Some(self.last_viewport_height);
        }
        let dt = ctx.input(|i| i.stable_dt).min(0.05);
        let step = dt / VIEW_ANIMATION;
        self.view_mix = if self.view_mix < target {
            (self.view_mix + step).min(target)
        } else {
            (self.view_mix - step).max(target)
        };
        ctx.request_repaint();
    }

    /// Device grid body; returns the body height its content needs.
    fn devices_body(&mut self, ui: &mut Ui, snap: &crate::poller::Snapshot) -> f32 {
        let available = ui.available_height();
        let mut scroll_delta = 0.0;
        let frame = egui::Frame::new()
            .inner_margin(Margin::same(FLYOUT_PADDING as i8))
            .show(ui, |ui| {
                ui.set_width(FLYOUT_WIDTH - 2.0 * FLYOUT_PADDING);
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);

                if let Some(msg) = &snap.banner {
                    let clicked = egui::Frame::new()
                        .fill(PALETTE.error_surface)
                        .corner_radius(CornerRadius::same(8))
                        .stroke(Stroke::new(1.0, PALETTE.error))
                        .inner_margin(Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.add(
                                egui::Label::new(
                                    RichText::new(msg).size(11.5).color(PALETTE.error),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Click to dismiss")
                            .clicked()
                        })
                        .inner;
                    if clicked {
                        self.shared.update(|s| s.banner = None);
                    }
                }

                if snap.connection == Connection::Unconfigured {
                    ui.label(
                        RichText::new("No server configured").color(PALETTE.on_surface_variant),
                    );
                    if pill_button(
                        ui,
                        RichText::new("Configure Server URL").strong(),
                        vec2(0.0, 32.0),
                        6,
                        ButtonKind::Primary,
                    )
                    .clicked()
                    {
                        self.view = View::Settings;
                    }
                } else {
                    if snap.connection == Connection::Offline && !snap.catalog.is_empty() {
                        ui.label(
                            RichText::new("Offline — showing last known state")
                                .color(PALETTE.warn)
                                .size(11.0),
                        );
                    }
                    let scroll_max = ui.available_height().max(120.0);
                    let mut scroll = egui::ScrollArea::vertical()
                        .max_height(scroll_max)
                        // egui's default 64 pt minimum makes the card taller
                        // than a short flyout (empty/one-row states), which
                        // clipped the card's bottom edge.
                        .min_scrolled_height(0.0)
                        .auto_shrink([false, false]);
                    // The flyout height trails the animated reveal by a frame,
                    // and the view crossfade changes the tile width while the
                    // window is still shrinking. Hide the scrollbar during
                    // both so it never flashes or changes the grid geometry.
                    let view_transitioning = self.view_mix > 0.001 && self.view_mix < 0.999;
                    if self.devices.is_animating() || view_transitioning {
                        scroll = scroll.scroll_bar_visibility(
                            egui::containers::scroll_area::ScrollBarVisibility::AlwaysHidden,
                        );
                    }
                    let scroll = scroll.show(ui, |ui| {
                        self.devices.show(ui, snap, &self.shared, &self.poller)
                    });
                    scroll_delta = scroll.content_size.y - scroll.inner_rect.height();
                }
            });
        if snap.connection == Connection::Unconfigured {
            // No scroll area in this branch: the frame is content-sized.
            frame.response.rect.height()
        } else {
            // The scroll area fills the body, so add its under/overflow back.
            available + scroll_delta
        }
    }

    fn settings_body(&mut self, ui: &mut Ui) -> settings::SettingsOutcome {
        self.settings.show(ui, &self.shared, &self.poller)
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.advance_view_mix(&ctx);
        let snap = self.shared.snapshot();

        let card_response = egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                ui.set_width(FLYOUT_OUTER_WIDTH);

                // Card shell. The *card* height is animated and anchored to the
                // window bottom, so its rounded bottom edge never moves; the
                // window is only resized once per transition. Resizing the
                // window every frame makes Windows composite stale frames and
                // tears the bottom corners.
                let viewport = ui.max_rect();
                let card_h = self
                    .last_viewport_height
                    .clamp(FLYOUT_MIN_HEIGHT, viewport.height().max(FLYOUT_MIN_HEIGHT));
                let card_rect = Rect::from_min_max(
                    egui::pos2(viewport.left(), viewport.bottom() - card_h),
                    viewport.max,
                );
                let outer = card_rect.shrink(CARD_BLEED);
                let inner = outer.shrink(CARD_STROKE_WIDTH);
                ui.painter()
                    .rect_filled(outer, CornerRadius::same(FLYOUT_RADIUS), PALETTE.surface);

                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(inner));
                content.set_clip_rect(inner);
                content.set_width(FLYOUT_WIDTH);
                content.spacing_mut().item_spacing = vec2(0.0, 0.0);

                let head = header::show(&mut content, &snap, self.view == View::Settings);
                // The header's own rect, not the cursor delta: during a card
                // animation the card top is fractional and the delta picks up
                // subpixel noise, which would make the settled target drift.
                let header_h = head.height;
                let mut close_settings = false;
                let mut reset_size = false;
                // Body height the content needs *now* (drives the card) and the
                // height it will need once settled (drives the one-shot window
                // resize). They differ while content animates its own height.
                let body_required;
                let body_target;

                // The settings sheet slides up over the device grid; closing
                // slides it back down. The sheet is laid out at an offset rect
                // but masked by a *fixed* screen-space clip: visual transforms
                // also move the clip, letting a square backdrop paint outside
                // the card's rounded bottom.
                let settings_target = self.view == View::Settings;
                let fade = egui::emath::easing::cubic_out(self.view_mix);
                let scene = content.available_rect_before_wrap();

                if settings_target {
                    if fade < 0.999 {
                        let mut base = content.new_child(egui::UiBuilder::new().max_rect(scene));
                        base.set_clip_rect(scene);
                        self.devices_body(&mut base, &snap);
                    }
                    let slide = (1.0 - fade) * scene.height();
                    let sheet_rect =
                        Rect::from_min_size(scene.min + vec2(0.0, slide), scene.size());
                    let mut sheet = content.new_child(egui::UiBuilder::new().max_rect(sheet_rect));
                    sheet.set_clip_rect(scene);
                    // Backdrop: sheet colour from the reveal line down to the
                    // card's inner bottom, rounded there. Painting the whole
                    // translated rect would leave its square bottom corners
                    // covering the card's arcs while the sheet slides.
                    let backdrop = Rect::from_min_max(sheet_rect.min, scene.max);
                    sheet.painter().rect_filled(
                        backdrop,
                        CornerRadius {
                            nw: 0,
                            ne: 0,
                            sw: SHEET_RADIUS,
                            se: SHEET_RADIUS,
                        },
                        PALETTE.surface,
                    );
                    let outcome = self.settings_body(&mut sheet);
                    close_settings = outcome.close;
                    reset_size = outcome.reset_size;
                    body_required = sheet.min_rect().height().max(scene.height());
                    // The form pads its footer to fill the card, so its laid-out
                    // height tracks the current window. Use the measured form
                    // height as the settled target instead.
                    body_target = if outcome.content_height > 0.0 {
                        outcome.content_height
                    } else {
                        body_required
                    };
                } else {
                    body_required = self.devices_body(&mut content, &snap);
                    body_target = body_required + self.devices.expansion_extra();
                    if fade > 0.001 {
                        let slide = (1.0 - fade) * scene.height();
                        let sheet_rect =
                            Rect::from_min_size(scene.min + vec2(0.0, slide), scene.size());
                        let mut sheet =
                            content.new_child(egui::UiBuilder::new().max_rect(sheet_rect));
                        sheet.set_clip_rect(scene);
                        let backdrop = Rect::from_min_max(sheet_rect.min, scene.max);
                        sheet.painter().rect_filled(
                            backdrop,
                            CornerRadius {
                                nw: 0,
                                ne: 0,
                                sw: SHEET_RADIUS,
                                se: SHEET_RADIUS,
                            },
                            PALETTE.surface,
                        );
                        self.settings_body(&mut sheet);
                    }
                }
                (
                    head,
                    close_settings,
                    reset_size,
                    body_required,
                    body_target,
                    header_h,
                    outer,
                )
            });

        let (head, close_settings, reset_size, body_required, body_target, header_h, outer) =
            card_response.inner;

        // One border, painted after the content so it stays crisp over the
        // header/footer fills; the shell fill was painted before the content.
        ui.painter().rect_stroke(
            outer,
            CornerRadius::same(FLYOUT_RADIUS),
            Stroke::new(CARD_STROKE_WIDTH, border_card()),
            StrokeKind::Inside,
        );
        if reset_size {
            self.manual_height = None;
            self.last_viewport_height = FLYOUT_DEFAULT_HEIGHT;
            self.shared.update(|s| s.settings.flyout_height = None);
            self.shared.save_settings();
            self.size_settle = 3;
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(
                FLYOUT_OUTER_WIDTH,
                self.last_viewport_height,
            )));
            self.place(&ctx, self.last_viewport_height);
            ctx.request_repaint();
        }
        if close_settings {
            self.view = View::Devices;
        }
        if head.refresh {
            self.poller.send(Command::Sync);
        }
        if head.toggle_pin {
            let pin = !snap.settings.pin_flyout;
            self.shared.update(|s| s.settings.pin_flyout = pin);
            self.shared.save_settings();
        }
        if head.settings {
            self.view = if self.view == View::Settings {
                View::Devices
            } else {
                self.devices.collapse_expanded();
                View::Settings
            };
            if self.view == View::Devices {
                self.settings.reset();
            }
        }
        if head.back {
            self.settings.reset();
            self.view = View::Devices;
        }
        if head.hide {
            self.hide(&ctx);
            return;
        }

        self.painted_once = true;

        // Detect a user-dragged size before measuring content, otherwise the
        // auto-fit branch would immediately snap the window back.
        if self.size_settle > 0 {
            self.size_settle -= 1;
        } else if let Some(actual) =
            ctx.input(|i| i.viewport().inner_rect.map(|rect| rect.height()))
            && (actual - self.last_window_height).abs() > 2.0
        {
            let height = actual.clamp(FLYOUT_MIN_HEIGHT, self.max_height);
            self.manual_height = Some(height);
            self.last_viewport_height = height;
            self.last_window_height = height;
            self.dirty_height = true;
            self.place(&ctx, height);
        }

        // Natural height = the fixed shell's chrome + header + the body height
        // the content actually needs. The shell itself never grows with content,
        // so measure the body independently of the current window height.
        let chrome = 2.0 * (CARD_BLEED + CARD_STROKE_WIDTH);
        let fit = |body: f32| (chrome + header_h + body).ceil().max(FLYOUT_MIN_HEIGHT);
        let natural_h = fit(body_required);
        let measured_h = self
            .manual_height
            .unwrap_or(natural_h)
            .clamp(FLYOUT_MIN_HEIGHT, self.max_height);
        // Settled height. Independent of the current window size, so the
        // window can be pre-sized once instead of tracking the animation.
        let target_h = self
            .manual_height
            .unwrap_or(fit(body_target))
            .clamp(FLYOUT_MIN_HEIGHT, self.max_height);

        // Ease the card toward the height its content needs right now. Only
        // the view transition animates; content changes (card expand/collapse)
        // supply their own animated height and snap here.
        let card_next = if (measured_h - self.last_viewport_height).abs() > 0.5 {
            // Views have different natural heights (settings is taller than a
            // few rows of tiles). Ease the card between them so switching
            // doesn't snap; the incoming content is revealed as it grows.
            let next = match self.view_from_height {
                Some(from) if self.view_mix > 0.0 && self.view_mix < 1.0 => {
                    let progress = if self.view == View::Settings {
                        self.view_mix
                    } else {
                        1.0 - self.view_mix
                    };
                    let eased = egui::emath::easing::cubic_out(progress);
                    from + (measured_h - from) * eased
                }
                _ => measured_h,
            };
            // Round up to a whole physical pixel: a fractional inner height
            // truncates at the OS/DPI boundary and clips the card's bottom edge.
            quantize_height(&ctx, next)
        } else {
            quantize_height(&ctx, self.last_viewport_height)
        };
        self.last_viewport_height = card_next;

        // Keep the window at least as tall as the card. A growing target gets
        // room up front (the empty strip appears above the card and is
        // invisible); a shrinking card keeps the old window until it has
        // arrived, then the window is resized once. Never resize per frame:
        // resizing while painting makes Windows composite stale frames.
        let arrived = (card_next - target_h).abs() <= 0.5;
        let want_window = if target_h > self.last_window_height + 0.5
            || card_next > self.last_window_height + 0.5
        {
            target_h.max(card_next)
        } else if arrived && target_h < self.last_window_height - 0.5 {
            target_h
        } else {
            self.last_window_height
        };

        if (want_window - self.last_window_height).abs() > 0.5 {
            self.last_window_height = want_window;
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(
                FLYOUT_OUTER_WIDTH,
                want_window,
            )));
            self.size_settle = 3;
            if self.reveal_phase == RevealPhase::Shown {
                self.place(&ctx, want_window);
            } else {
                self.reveal_phase = RevealPhase::Settling;
            }
            ctx.request_repaint();
        } else if self.reveal_phase == RevealPhase::Settling {
            self.place(&ctx, want_window);
            self.parked = false;
            self.reveal_phase = RevealPhase::ReadyToUncloak;
            ctx.request_repaint();
        }
    }
}

impl eframe::App for PiDaTrayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4] // fully transparent window behind the rounded card
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Runs even while the flyout is hidden, so the window is clean before
        // it is ever shown.
        crate::ensure_borderless_window_policy(frame);

        // Tell egui the truth about viewport focus. eframe's 0.37 `App::ui`
        // path can leave `InputState::focused` false for a window that Windows
        // has activated, and egui gates the text caret and focus rings on that
        // flag: the server URL field would accept typing with no visible caret.
        if let Some(has_foreground) = crate::flyout_has_foreground(frame) {
            ctx.input_mut(|input| input.focused = has_foreground);
        }

        if self.reveal_phase == RevealPhase::RevealNow {
            if let Some(pos) = self.pending_reveal.take() {
                let ppp = ctx.pixels_per_point();
                crate::reveal_flyout_at(
                    frame,
                    pos.x * ppp,
                    pos.y * ppp,
                    FLYOUT_OUTER_WIDTH * ppp,
                    self.last_window_height * ppp,
                );
            }
            self.parked = false;
            self.reveal_phase = RevealPhase::Shown;
            self.shown_at = Some(Instant::now());
            self.seen_focused_while_shown = false;
        }

        if self.reveal_phase == RevealPhase::ReadyToUncloak {
            crate::set_window_cloaked(frame, false);
            crate::focus_flyout_window(frame);
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.shown_at = Some(Instant::now());
            self.seen_focused_while_shown = false;
            self.reveal_phase = RevealPhase::Shown;
        }

        self.sync_tray();
        self.drain_tray_actions(ctx);
        self.handle_close_request(ctx);

        if self.visible {
            if self.reveal_phase == RevealPhase::Shown {
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    if self.view == View::Settings {
                        self.settings.reset();
                        self.view = View::Devices;
                    } else if !self.shared.read(|s| s.settings.pin_flyout) {
                        self.hide(ctx);
                    }
                }
                if self.visible {
                    self.apply_focus_policy(ctx, frame);
                }
            }
        } else {
            crate::set_window_cloaked(frame, true);
            self.park_window(ctx);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Hidden: nothing to lay out or paint (0% GPU/CPU while idle).
        if !self.visible {
            crate::set_window_cloaked(frame, true);
            return;
        }
        self.draw(ui);
        if !self.visible {
            crate::set_window_cloaked(frame, true);
        }
    }
}
