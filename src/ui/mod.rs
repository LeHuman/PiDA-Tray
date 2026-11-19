//! `PiDaTrayApp`: flyout show/hide, focus-loss grace, and the card shell.

mod devices;
mod header;
mod icons;
mod settings;

use std::{
    sync::{Arc, mpsc},
    time::Instant,
};

use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, ViewportCommand, vec2};

use crate::{
    constants::{
        CARD_STROKE_WIDTH, FLYOUT_MAX_HEIGHT, FLYOUT_MIN_HEIGHT, FLYOUT_OUTER_WIDTH,
        FLYOUT_PADDING, FLYOUT_RADIUS, FLYOUT_SCREEN_MARGIN, FLYOUT_WIDTH, FOCUS_LOSS_GRACE,
        PALETTE,
    },
    poller::{Command, Connection, Poller, Shared},
    power,
    theme::{self, border_card},
    tray::{self, StatusKey, TrayAction, TrayController},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Devices,
    Settings,
}

pub struct PiDaTrayApp {
    shared: Arc<Shared>,
    poller: Poller,
    actions: mpsc::Receiver<TrayAction>,
    tray: Option<TrayController>,

    visible: bool,
    hid_startup_window: bool,
    view: View,
    /// Tray-icon centre in physical pixels, used to (re)place the flyout.
    anchor: Option<(f32, f32)>,
    last_height: f32,
    shown_at: Option<Instant>,
    seen_focused_while_shown: bool,
    quitting: bool,

    devices: devices::DevicesView,
    settings: settings::SettingsView,
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

        Self {
            shared,
            poller,
            actions,
            tray,
            visible: false,
            hid_startup_window: false,
            view: View::Devices,
            anchor: None,
            last_height: FLYOUT_MIN_HEIGHT + 120.0,
            shown_at: None,
            seen_focused_while_shown: false,
            quitting: false,
            devices: Default::default(),
            settings: Default::default(),
        }
    }

    // -- window management ---------------------------------------------------

    fn place(&self, ctx: &egui::Context) {
        let Some(anchor) = self.anchor else { return };
        let Some((work, monitor)) = tray::monitor_rects_at(anchor.0, anchor.1) else {
            return;
        };
        let current_ppp = ctx.pixels_per_point().max(0.5);
        let target_ppp = tray::monitor_dpi_scale_at(anchor.0, anchor.1)
            .unwrap_or(current_ppp)
            .max(0.5);
        let size_px = (
            FLYOUT_OUTER_WIDTH * target_ppp,
            self.last_height * target_ppp,
        );
        let (x, y) = tray::compute_flyout_pos(
            anchor,
            size_px,
            work,
            monitor,
            FLYOUT_SCREEN_MARGIN * target_ppp,
        );
        // `egui-winit` multiplies `OuterPosition` by the window's *current* `pixels_per_point`.
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(
            x / current_ppp,
            y / current_ppp,
        )));
    }

    fn show(&mut self, ctx: &egui::Context, anchor: Option<(f32, f32)>) {
        self.anchor = anchor.or(self.anchor).or_else(tray::cursor_pos);
        self.visible = true;
        self.view = View::Devices;
        self.shown_at = Some(Instant::now());
        self.seen_focused_while_shown = false;
        self.shared.set_visible(true);
        self.place(ctx);
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        self.poller.send(Command::Sync);
        ctx.request_repaint();
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.devices.flush_all(&self.shared, &self.poller);
        self.visible = false;
        self.view = View::Devices;
        self.settings.reset();
        self.shown_at = None;
        self.seen_focused_while_shown = false;
        self.shared.set_visible(false);
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
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
                TrayAction::Toggle(anchor) => self.toggle(ctx, anchor),
                TrayAction::ToggleFromMenu => self.toggle(ctx, tray::cursor_pos()),
                TrayAction::Refresh => self.poller.send(Command::Sync),
                TrayAction::Settings => {
                    self.show(ctx, tray::cursor_pos());
                    self.view = View::Settings;
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
            self.show(ctx, anchor)
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

    fn apply_focus_policy(&mut self, ctx: &egui::Context, pinned: bool) {
        if pinned {
            self.seen_focused_while_shown = false;
            return;
        }
        let focused = ctx.input(|i| i.viewport().focused);
        if focused == Some(true) {
            self.seen_focused_while_shown = true;
            return;
        }
        if focused == Some(false) {
            let elapsed = self
                .shown_at
                .map(|t| t.elapsed())
                .unwrap_or(FOCUS_LOSS_GRACE);
            if self.seen_focused_while_shown || elapsed >= FOCUS_LOSS_GRACE {
                self.hide(ctx);
            } else {
                ctx.request_repaint_after(FOCUS_LOSS_GRACE - elapsed);
            }
        }
    }

    // -- drawing ---------------------------------------------------------------

    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let snap = self.shared.snapshot();

        let outer = egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                ui.set_width(FLYOUT_OUTER_WIDTH);

                let card = egui::Frame::new()
                    .fill(PALETTE.surface)
                    .corner_radius(CornerRadius::same(FLYOUT_RADIUS))
                    .stroke(Stroke::new(CARD_STROKE_WIDTH, border_card()))
                    .inner_margin(Margin::same(0));

                let outcome = card.show(ui, |ui| {
                    ui.set_width(FLYOUT_WIDTH);
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

                    let head = header::show(ui, &snap, self.view == View::Settings);
                    let mut close_settings = false;

                    match self.view {
                        View::Settings => {
                            close_settings =
                                self.settings.show(ui, &self.shared, &self.poller).close;
                        }
                        View::Devices => {
                            egui::Frame::new()
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
                                                        RichText::new(msg)
                                                            .size(11.5)
                                                            .color(PALETTE.error),
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
                                            RichText::new("No server configured")
                                                .color(PALETTE.on_surface_variant),
                                        );
                                        let btn = egui::Button::new(
                                            RichText::new("Configure Server URL")
                                                .strong()
                                                .color(Color32::WHITE),
                                        )
                                        .fill(PALETTE.primary)
                                        .stroke(Stroke::NONE)
                                        .corner_radius(CornerRadius::same(6));
                                        if ui.add(btn).clicked() {
                                            self.view = View::Settings;
                                        }
                                    } else {
                                        if snap.connection == Connection::Offline
                                            && !snap.catalog.is_empty()
                                        {
                                            ui.label(
                                                RichText::new("Offline — showing last known state")
                                                    .color(PALETTE.warn)
                                                    .size(11.0),
                                            );
                                        }
                                        let scroll_max = FLYOUT_MAX_HEIGHT
                                            - 84.0
                                            - if snap.banner.is_some() { 46.0 } else { 0.0 }
                                            - if snap.connection == Connection::Offline
                                                && !snap.catalog.is_empty()
                                            {
                                                24.0
                                            } else {
                                                0.0
                                            };
                                        egui::ScrollArea::vertical()
                                            .max_height(scroll_max.max(120.0))
                                            .auto_shrink([false, true])
                                            .show(ui, |ui| {
                                                self.devices.show(
                                                    ui,
                                                    &snap,
                                                    &self.shared,
                                                    &self.poller,
                                                )
                                            });
                                    }
                                });
                        }
                    }
                    (head, close_settings)
                });

                let (head, close_settings) = outcome.inner;
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
                    return None;
                }

                Some(outcome.response.rect.height())
            });

        // Content-hugging height, clamped.
        if let Some(measured) = outer.inner {
            let target = measured.ceil().clamp(FLYOUT_MIN_HEIGHT, FLYOUT_MAX_HEIGHT);
            if (target - self.last_height).abs() > 1.5 {
                self.last_height = target;
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(FLYOUT_OUTER_WIDTH, target)));
                self.place(&ctx);
                ctx.request_repaint();
            }
        }
    }
}

use eframe::egui::Color32;

impl eframe::App for PiDaTrayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4] // fully transparent window behind the rounded card
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        crate::ensure_borderless_window_policy(frame);

        // `eframe` 0.37 ignores `ViewportBuilder::with_visible(false)` on first map,
        // so hide explicitly once on frame 0 until a tray click shows it.
        if !self.hid_startup_window {
            self.hid_startup_window = true;
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }

        self.sync_tray();
        self.drain_tray_actions(ctx);
        self.handle_close_request(ctx);

        if self.visible {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                if self.view == View::Settings {
                    self.settings.reset();
                    self.view = View::Devices;
                } else {
                    self.hide(ctx);
                    return;
                }
            }
            let pinned = self.shared.read(|s| s.settings.pin_flyout);
            self.apply_focus_policy(ctx, pinned);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Hidden: nothing to lay out or paint (0% GPU/CPU while idle).
        if !self.visible {
            return;
        }
        self.draw(ui);
    }
}
