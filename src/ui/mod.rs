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
    self, CornerRadius, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Ui, ViewportCommand,
    vec2,
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
/// Settings card height assumed before the form has been measured. The settings
/// page must fit on the very first open, so this covers the form plus one
/// wrapped validation error.
const SETTINGS_HEIGHT_GUESS: f32 = 400.0;

/// Round a logical height up to a whole physical pixel. Fractional inner sizes
/// truncate when DPI scaling converts them, which clips the card's bottom edge.
fn quantize_height(ctx: &egui::Context, height: f32) -> f32 {
    let ppp = ctx.pixels_per_point().max(0.5);
    (height * ppp).ceil() / ppp
}

/// Ease-in-out on [0, 1] with zero slope at both ends (the close curve).
fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Inverse of `cubic_out`, so a mid-flight reversal can keep the settled
/// fraction it is currently showing.
fn invert_cubic_out(y: f32) -> f32 {
    let y = y.clamp(0.0, 1.0);
    1.0 - (1.0 - y).cbrt()
}

/// Inverse of `smoothstep`, for the same reason on the close curve.
fn invert_smoothstep(y: f32) -> f32 {
    let y = y.clamp(0.0, 1.0);
    0.5 - ((1.0 - 2.0 * y).asin() / 3.0).sin()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Devices,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RevealPhase {
    Hidden,
    /// Position and uncloak in one tick, using the frame the parked window is
    /// already holding.
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
    /// Height the card is painted at.
    card_height: f32,
    /// Client height held for the shown session. Windows composites the
    /// previous surface across a native resize, so changing the window while it
    /// is visible shows one stale frame (the page jumping, or the card's bottom
    /// edge going missing). The card animates inside this canvas instead, and
    /// the window only changes size while it is cloaked.
    canvas_height: f32,
    /// Set when the user drags the flyout to a custom height.
    manual_height: Option<f32>,
    /// Content-fit cap for this monitor's work area.
    max_height: f32,
    /// Frames spent waiting for a cloaked resize to land, so a window the OS
    /// refuses to size cannot keep the flyout cloaked forever.
    settle_frames: u8,
    /// A dragged height is waiting to be persisted.
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
    /// Ease value the current leg of the transition started from, so a
    /// reversal continues the card from its current height.
    eased_from: f32,
    /// Last applied window region, including the client size it was built for.
    region: Option<crate::WindowRegion>,
    /// Content-fit card heights measured last session, used to size the next
    /// canvas before anything is laid out.
    devices_card_h: f32,
    settings_card_h: f32,
    /// Height and pointer y of an in-flight top-edge drag.
    resize_drag: Option<(f32, f32)>,
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
        let card_height = manual_height.unwrap_or(FLYOUT_DEFAULT_HEIGHT);

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
            card_height,
            canvas_height: FLYOUT_DEFAULT_HEIGHT,
            manual_height,
            max_height: FLYOUT_MAX_HEIGHT,
            settle_frames: 0,
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
            eased_from: 0.0,
            region: None,
            devices_card_h: 0.0,
            settings_card_h: 0.0,
            resize_drag: None,
        }
    }

    // -- window management ---------------------------------------------------

    /// Physical window rect (x, y, w, h) for the current anchor and height.
    fn compute_placement_px(
        &mut self,
        ctx: &egui::Context,
        height: f32,
    ) -> Option<(i32, i32, i32, i32)> {
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
        Some((
            x.round() as i32,
            y.round() as i32,
            size_px.0.round() as i32,
            size_px.1.round() as i32,
        ))
    }

    /// Logical `OuterPosition` for the current anchor and height. Also refreshes
    /// `max_height` from the monitor work area.
    fn compute_placement(&mut self, ctx: &egui::Context, height: f32) -> Option<egui::Pos2> {
        let current_ppp = ctx.pixels_per_point().max(0.5);
        self.compute_placement_px(ctx, height)
            .map(|(x, y, _, _)| egui::pos2(x as f32 / current_ppp, y as f32 / current_ppp))
    }

    fn place(&mut self, ctx: &egui::Context, height: f32) {
        if let Some(pos) = self.compute_placement(ctx, height) {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        }
    }

    /// Client height for the shown session: the tallest card the content can
    /// need, so no transition has to resize the visible window. A dragged
    /// height only ever shrinks the card, so it can never exceed this.
    fn canvas_target(&self, ctx: &egui::Context) -> f32 {
        let devices = if self.devices_card_h > 0.0 {
            self.devices_card_h
        } else {
            FLYOUT_MIN_HEIGHT + devices::EXPANDED_HEIGHT_GUESS
        };
        let settings = if self.settings_card_h > 0.0 {
            self.settings_card_h
        } else {
            SETTINGS_HEIGHT_GUESS
        };
        // One expanded card must fit without resizing the window.
        let expanded = devices + devices::EXPANDED_HEIGHT_GUESS;
        quantize_height(
            ctx,
            settings
                .max(expanded)
                .clamp(FLYOUT_MIN_HEIGHT, self.max_height),
        )
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

        // Refresh the work-area cap for the monitor this flyout is opening on
        // before anything is derived from it.
        let _ = self.compute_placement_px(ctx, self.canvas_height);
        let previous_height = self.canvas_height;
        let unmeasured = self.devices_card_h <= 0.0;
        // With nothing measured yet, keep the parked size for one cloaked frame
        // so the first layout can measure the real content and ask for a single
        // canvas resize instead of two speculative ones.
        let target = if unmeasured {
            previous_height
        } else {
            let desired = self.canvas_target(ctx);
            quantize_height(ctx, desired.clamp(FLYOUT_MIN_HEIGHT, self.max_height))
        };
        let pos = self.compute_placement(ctx, target);

        self.visible = true;
        self.view = View::Devices;
        self.view_mix = 0.0;
        self.view_from_height = None;
        self.view_dir = 0.0;
        self.eased_from = 0.0;
        self.shown_at = None;
        self.seen_focused_while_shown = false;
        self.shared.set_visible(true);
        // Start the card at its known content height, not at the canvas: the
        // first painted frame is the one the reveal uncloaks.
        self.card_height = self.manual_height.unwrap_or(if self.devices_card_h > 0.0 {
            self.devices_card_h
        } else {
            target
        });
        self.canvas_height = target;
        self.settle_frames = 0;
        self.resize_drag = None;
        self.region = None;

        // Fast path: the parked swap chain already holds a frame at exactly
        // this size and the same DPI, so `logic` can move and uncloak the window
        // in one tick instead of waiting for viewport commands to round-trip.
        // A different monitor scale makes the parked frame the wrong size, so
        // that case goes through `Settling` instead.
        let same_scale = self
            .anchor
            .and_then(|anchor| tray::monitor_dpi_scale_at(anchor.0, anchor.1))
            .is_none_or(|scale| (scale - ctx.pixels_per_point()).abs() <= f32::EPSILON);
        let fast = cfg!(windows)
            && !settle
            && !unmeasured
            && same_scale
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
        self.eased_from = 0.0;
        self.resize_drag = None;
        self.region = None;
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
            self.eased_from = 0.0;
            return;
        }
        let dir = (target - self.view_mix).signum();
        if self.view_dir != 0.0 && dir != self.view_dir {
            // Reversed mid-flight. Re-parameterise `view_mix` so the new
            // direction's curve continues from the settled fraction already on
            // screen: the two curves differ, and switching them directly jumped
            // the sheet and the card by a large step in one frame. The card also
            // restarts from its current height with its ease offset recorded, so
            // it continues from where it is instead of re-applying the curve.
            let settled = self.settled_fraction();
            self.view_mix = if target > 0.0 {
                invert_cubic_out(settled)
            } else {
                invert_smoothstep(settled)
            };
            self.view_from_height = Some(self.card_height);
            self.eased_from = if target > 0.0 { settled } else { 1.0 - settled };
        }
        self.view_dir = dir;
        if self.view_from_height.is_none() {
            self.view_from_height = Some(self.card_height);
            self.eased_from = 0.0;
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

    /// Fraction of the settings page that is settled: 1.0 when it fully covers
    /// the card, 0.0 when it is fully slid out. The curve depends on the
    /// direction of travel so open and close each keep their feel.
    fn settled_fraction(&self) -> f32 {
        let m = self.view_mix.clamp(0.0, 1.0);
        if self.view_dir < 0.0 {
            smoothstep(m)
        } else {
            egui::emath::easing::cubic_out(m)
        }
    }

    /// Device grid body; returns the body height its content needs.
    fn devices_body(&mut self, ui: &mut Ui, snap: &crate::poller::Snapshot) -> f32 {
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
                    // The card height trails the animated reveal by a frame,
                    // and the view crossfade changes the tile width while
                    // the card is still moving. Hide the scrollbar during
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
            // The frame's own rect is the safe base: a banner plus margins can
            // overflow a short card, where `available` under-reports.
            frame.response.rect.height() + scroll_delta
        }
    }

    /// Paint and lay out the settings sheet: the sliding page, its rounded
    /// backdrop clipped to the card, and the form itself.
    fn show_settings_sheet(
        &mut self,
        content: &mut Ui,
        scene: Rect,
        sheet_rect: Rect,
    ) -> settings::SettingsOutcome {
        let mut sheet = content.new_child(egui::UiBuilder::new().max_rect(sheet_rect));
        sheet.set_clip_rect(scene);
        // Backdrop: sheet colour from the reveal line down to the card's inner
        // bottom, rounded there. Painting the whole translated rect would leave
        // its square bottom corners covering the card's arcs while it slides.
        let backdrop = Rect::from_min_max(sheet_rect.min, scene.max);
        if backdrop.height() > 0.0 {
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
        }
        self.settings.show(&mut sheet, &self.shared, &self.poller)
    }

    fn draw(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.advance_view_mix(&ctx);
        let snap = self.shared.snapshot();
        // The view this frame's layout used. Header actions can switch views
        // later in the same frame, and bookkeeping keyed by "the current view"
        // would then record the old layout's height under the new view.
        let laid_out_view = self.view;

        // Height the card is painted at in this frame. The settle checks at the
        // end of the frame use it, so the window region never shrinks before the
        // final-height card has actually been painted.
        let card_painted = self.card_height;

        // Fraction of the settings page that is settled (1.0 = fully in place,
        // 0.0 = fully slid out). The card height and the sheet's slide both
        // follow this one value, so the page never gets squeezed by a card that
        // shrinks before the sheet has moved.
        // One settled fraction for the sheet and the card. The curve follows the
        // direction of travel (rise quickly / slide away gently), and a reversal
        // re-parameterises `view_mix` so this stays continuous.
        let settle_p = self.settled_fraction();

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
                let card_h =
                    card_painted.clamp(FLYOUT_MIN_HEIGHT, viewport.height().max(FLYOUT_MIN_HEIGHT));
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
                // height it will need once settled (drives the card's ease).
                let body_required;
                let body_target;

                // The settings sheet slides up over the device grid; closing
                // slides it back down. The sheet is laid out at an offset rect
                // but masked by a *fixed* screen-space clip: visual transforms
                // also move the clip, letting a square backdrop paint outside
                // the card's rounded bottom.
                let settings_target = laid_out_view == View::Settings;
                let fade = settle_p;
                let scene = content.available_rect_before_wrap();

                // The settings sheet slides as a page of constant size: laying
                // it out at the animated card height would re-flow the form on
                // every frame, squeezing it into the tile height as the card
                // shrinks. Its height is the settled settings body height, and
                // the slide is measured against that, so the page is fully out
                // of the card when `fade` reaches zero.
                let sheet_h = if self.settings_card_h > 0.0 {
                    (self.settings_card_h - 2.0 * (CARD_BLEED + CARD_STROKE_WIDTH) - header_h)
                        .max(FLYOUT_MIN_HEIGHT)
                } else {
                    scene.height()
                };
                let slide = (1.0 - fade) * sheet_h;
                let sheet_rect = Rect::from_min_size(
                    egui::pos2(scene.left(), scene.max.y - sheet_h + slide),
                    vec2(scene.width(), sheet_h),
                );

                if settings_target {
                    if fade < 0.999 {
                        let mut base = content.new_child(egui::UiBuilder::new().max_rect(scene));
                        base.set_clip_rect(scene);
                        self.devices_body(&mut base, &snap);
                    }
                    let outcome = self.show_settings_sheet(&mut content, scene, sheet_rect);
                    close_settings = outcome.close;
                    reset_size = outcome.reset_size;
                    // The card follows the form's own height. It used to be
                    // `max(..., scene.height())`, but with a session canvas the
                    // scene can be much taller than the form, which pinned the
                    // card at the canvas and left the page at its bottom.
                    body_target = if outcome.content_height > 0.0 {
                        outcome.content_height
                    } else {
                        sheet_h
                    };
                    body_required = body_target;
                } else {
                    body_required = self.devices_body(&mut content, &snap);
                    body_target = body_required + self.devices.expansion_extra();
                    if fade > 0.001 {
                        self.show_settings_sheet(&mut content, scene, sheet_rect);
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
        // Snapshot of the state the layout just painted, before the header
        // actions below can change it.
        let laid_out_expanded = self.devices.has_expanded();

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
            self.dirty_height = false;
            self.resize_drag = None;
            self.shared.update(|s| s.settings.flyout_height = None);
            self.shared.save_settings();
            // Explicit user action, so the window may change size; go through
            // eframe so its render surface follows the new client size.
            let target = self.canvas_target(&ctx);
            if (target - self.canvas_height).abs() > 0.5 {
                self.canvas_height = target;
                self.region = None;
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(FLYOUT_OUTER_WIDTH, target)));
                self.place(&ctx, target);
            }
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

        // Natural height = the fixed shell's chrome + header + the body height
        // the content actually needs. The shell itself never grows with content,
        // so measure the body independently of the current window height.
        let chrome = 2.0 * (CARD_BLEED + CARD_STROKE_WIDTH);
        let fit = |body: f32| (chrome + header_h + body).ceil().max(FLYOUT_MIN_HEIGHT);

        // Remember what each view needs, so the next session's canvas covers it.
        // An expanded card is not representative: counting it would add the
        // expansion headroom on top of itself next session.
        match laid_out_view {
            View::Devices if !laid_out_expanded => self.devices_card_h = fit(body_target),
            View::Settings => self.settings_card_h = fit(body_target),
            _ => {}
        }

        // Bounds for the dragged height, taken from the device grid (the only
        // view it applies to). Only meaningful once the grid has been measured.
        let devices_h = self.devices_card_h.max(FLYOUT_MIN_HEIGHT);
        let bounds_known = self.devices_card_h > 0.0;
        let content_min_h = if bounds_known {
            match self.devices.one_row_height() {
                Some(row) => fit(2.0 * FLYOUT_PADDING + row).min(devices_h),
                None => devices_h,
            }
            .max(FLYOUT_MIN_HEIGHT)
        } else {
            FLYOUT_MIN_HEIGHT
        };
        if bounds_known
            && !(snap.catalog.is_empty() && snap.syncing)
            && let Some(manual) = self.manual_height
        {
            let bounded = manual.clamp(content_min_h, devices_h);
            if (bounded - manual).abs() > 0.5 {
                self.manual_height = Some(bounded);
                self.dirty_height = true;
            }
        }
        let content_h = fit(body_target).clamp(FLYOUT_MIN_HEIGHT, self.max_height);

        // Session canvas. While the flyout is still cloaked the window may
        // change size; once it is on screen the geometry is frozen until hide.
        // Windows composites the previous surface across a native resize, so a
        // visible resize always shows one stale frame: the page jumps, or the
        // card's bottom edge disappears into the crop.
        if self.reveal_phase == RevealPhase::Settling {
            let want_canvas = self.canvas_target(&ctx);
            if (want_canvas - self.canvas_height).abs() > 0.5 {
                self.canvas_height = want_canvas;
                // Seed the card at the height this layout actually needs, so
                // the cloaked frames never paint a canvas-height card.
                self.card_height = match laid_out_view {
                    View::Devices => self.manual_height.unwrap_or(fit(body_target)),
                    View::Settings => fit(body_target),
                }
                .min(want_canvas);
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(
                    FLYOUT_OUTER_WIDTH,
                    want_canvas,
                )));
                self.settle_frames = 0;
                ctx.request_repaint();
                return;
            }
            // Uncloak only once the window really has the canvas size: the
            // frame that is revealed must be laid out for the window it lands
            // in. The counter is the escape hatch for an OS-adjusted size.
            let landed = ctx
                .input(|i| i.viewport().inner_rect.map(|r| r.height()))
                .is_some_and(|h| (h - self.canvas_height).abs() <= 1.0);
            self.settle_frames += 1;
            if landed || self.settle_frames > 4 {
                self.place(&ctx, self.canvas_height);
                self.parked = false;
                self.reveal_phase = RevealPhase::ReadyToUncloak;
                ctx.request_repaint();
            } else {
                ctx.request_repaint();
                return;
            }
        }

        // The card never exceeds the window that actually exists.
        let canvas = self.canvas_height.max(FLYOUT_MIN_HEIGHT);
        // A dragged height applies to the device grid only: the settings form
        // is always shown whole, and its own content height is inside the
        // canvas, so that never resizes the window either. Keyed by the layout's
        // view, so the flip frame keeps the old view's height instead of
        // snapping straight to the new view's target.
        let manual = match laid_out_view {
            View::Devices => self.manual_height,
            View::Settings => None,
        };
        let measured_h = manual
            .unwrap_or(fit(body_required))
            .clamp(FLYOUT_MIN_HEIGHT, canvas);
        // Settled height, independent of the current card height so the ease
        // has a fixed destination.
        let target_h = manual
            .unwrap_or(fit(body_target))
            .min(content_h)
            .clamp(FLYOUT_MIN_HEIGHT, canvas);

        // Ease the card toward the height its content needs right now. Only
        // the view transition animates; content changes (card expand/collapse)
        // supply their own animated height and snap here.
        let card_next = if (measured_h - self.card_height).abs() > 0.5 {
            // Views have different natural heights (settings is taller than a
            // few rows of tiles). Ease the card between them so switching
            // doesn't snap; the incoming content is revealed as it grows.
            let next = match self.view_from_height {
                Some(from) if self.view_mix > 0.0 && self.view_mix < 1.0 => {
                    // Track the sheet's own progress: its slide and the card
                    // height follow one curve, so the page is never squeezed by
                    // a card that shrinks before the sheet has moved. The ease is
                    // offset by the value it had when this leg of the transition
                    // started, so a mid-flight reversal continues from the card's
                    // current height instead of re-applying the curve.
                    let eased = if laid_out_view == View::Settings {
                        settle_p
                    } else {
                        1.0 - settle_p
                    };
                    let room = 1.0 - self.eased_from;
                    let progress = if room <= 0.001 {
                        1.0
                    } else {
                        ((eased - self.eased_from) / room).clamp(0.0, 1.0)
                    };
                    from + (measured_h - from) * progress
                }
                _ => measured_h,
            };
            // Round up to a whole physical pixel: a fractional inner height
            // truncates at the OS/DPI boundary and clips the card's bottom edge.
            quantize_height(&ctx, next)
        } else {
            quantize_height(&ctx, self.card_height)
        };
        self.card_height = card_next;

        // Clip the window to the card. Above a short card the canvas is empty
        // and transparent; left in the window it would swallow clicks meant for
        // whatever is behind. While the card animates, or the user is dragging
        // its edge, the region stays open so the card is never clipped, and the
        // temporary input band is the same tradeoff the animated card had. The
        // region is only touched while shown: while cloaked it would be built
        // from the pre-resize window and then clip the card after the resize.
        let view_animating = self.view_mix > 0.001 && self.view_mix < 0.999;
        let settled = (card_painted - target_h).abs() <= 0.5
            && (card_next - target_h).abs() <= 0.5
            && !view_animating
            && self.resize_drag.is_none()
            && self.reveal_phase == RevealPhase::Shown;
        let ppp = ctx.pixels_per_point().max(0.5);
        // The grip sits `4 pt` above the card's top edge.
        let grip_margin = (4.0 * ppp).ceil() as i32;
        let top_px = if settled {
            (((canvas - card_next) * ppp).round() as i32 - grip_margin).max(0)
        } else {
            0
        };
        // The cache key carries the client size: Windows keeps a region in
        // window coordinates and does not resize it, so a client-size change
        // with an unchanged card top still has to be re-applied.
        let desired_region = if top_px <= 0 {
            Some(crate::WindowRegion::Full)
        } else {
            ctx.input(|i| i.viewport().inner_rect)
                .map(|r| crate::WindowRegion::Clipped {
                    top: top_px,
                    width: (r.width() * ppp).round() as i32,
                    height: (r.height() * ppp).round() as i32,
                })
        };
        if let Some(desired) = desired_region
            && self.region != Some(desired)
            && crate::set_window_region(frame, top_px).is_some()
        {
            self.region = Some(desired);
        }

        // Height grip on the card's top edge. The native top border sits at the
        // canvas top, far above the card and outside the window region, so the
        // drag lives on the card itself: it only moves the painted height and
        // never resizes the native window.
        let grip = Rect::from_min_max(
            egui::pos2(outer.left(), outer.top() - 4.0),
            egui::pos2(outer.right(), outer.top() + 3.0),
        );
        let grip = ui.interact(grip, ui.scope_id().with("card-resize-grip"), Sense::drag());
        // The settings page always shows whole, so there is nothing to resize
        // while it is open.
        let grip_enabled = laid_out_view == View::Devices;
        if grip_enabled && (grip.hovered() || grip.dragged()) {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if grip_enabled
            && grip.drag_started()
            && let Some(y) = ctx.input(|i| i.pointer.interact_pos().map(|p| p.y))
        {
            self.resize_drag = Some((self.card_height, y));
        }
        if grip.dragged()
            && let Some((from, start_y)) = self.resize_drag
            && let Some(pointer_y) = ctx.input(|i| i.pointer.interact_pos().map(|p| p.y))
        {
            let lo = content_min_h.max(FLYOUT_MIN_HEIGHT);
            let hi = devices_h.min(canvas).min(self.max_height).max(lo);
            let height = quantize_height(&ctx, (from - (pointer_y - start_y)).clamp(lo, hi));
            self.manual_height = Some(height);
            self.card_height = height;
            self.dirty_height = true;
        }
        if !grip_enabled || grip.drag_stopped() {
            self.resize_drag = None;
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
                    self.canvas_height * ppp,
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
        self.draw(ui, frame);
        if !self.visible {
            crate::set_window_cloaked(frame, true);
        }
    }
}
