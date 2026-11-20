//! Inline settings form.

use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, Pos2, Rect, RichText,
    Stroke, Ui, Vec2,
};

use crate::{
    config::{normalize_server_url, set_autostart},
    constants::{FLYOUT_WIDTH, IDLE_POLL_CHOICES, PALETTE},
    poller::{Command, Poller, Shared},
    theme::border_subtle,
};

use super::{
    SHEET_RADIUS,
    button::{ButtonKind, pill_button},
};

#[derive(Default)]
pub struct SettingsView {
    url: String,
    idle_poll_seconds: u64,
    pin_flyout: bool,
    launch_at_startup: bool,
    ui_sounds: bool,
    error: Option<String>,
    loaded: bool,
}

#[derive(Default)]
pub struct SettingsOutcome {
    pub close: bool,
    /// Restore content-fit flyout height and forget the dragged height.
    pub reset_size: bool,
    /// Height the form needs without the padding that pushes the footer to the
    /// bottom of the card. Used as the settled flyout height, so it does not
    /// depend on the current window size.
    pub content_height: f32,
}

fn poll_label(secs: u64) -> String {
    if secs == 0 {
        "Off (on open)".into()
    } else {
        format!("Every {secs}s")
    }
}

impl SettingsView {
    /// Re-read saved settings next time the form opens.
    pub fn reset(&mut self) {
        self.loaded = false;
        self.error = None;
    }

    fn save(&mut self, shared: &Arc<Shared>, poller: &Poller) -> bool {
        let normalized = match normalize_server_url(&self.url) {
            Ok(u) => u,
            Err(e) => {
                self.error = Some(e);
                return false;
            }
        };

        let prev_autostart = shared.settings().launch_at_startup;
        if self.launch_at_startup != prev_autostart
            && let Err(e) = set_autostart(self.launch_at_startup)
        {
            self.error = Some(format!("Autostart: {e}"));
            return false;
        }

        let prev_url = shared.settings().server_url;
        self.url = normalized.clone();
        self.error = None;
        shared.update(|s| {
            s.settings.server_url = normalized.clone();
            s.settings.idle_poll_seconds = self.idle_poll_seconds;
            s.settings.pin_flyout = self.pin_flyout;
            s.settings.launch_at_startup = self.launch_at_startup;
            s.settings.ui_sounds = self.ui_sounds;
        });
        shared.save_settings();
        poller.send(Command::SettingsChanged);
        if normalized != prev_url {
            poller.send(Command::Sync);
        }
        true
    }

    pub fn show(&mut self, ui: &mut Ui, shared: &Arc<Shared>, poller: &Poller) -> SettingsOutcome {
        let mut out = SettingsOutcome::default();
        if !self.loaded {
            let current = shared.settings();
            self.url = current.server_url;
            self.idle_poll_seconds = current.idle_poll_seconds;
            self.pin_flyout = current.pin_flyout;
            self.launch_at_startup = current.launch_at_startup;
            self.ui_sounds = current.ui_sounds;
            self.loaded = true;
        }

        let mut submit = false;

        // All clickable settings controls get a pointing cursor.
        ui.style_mut().visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);

        let footer_h = 50.0;
        Frame::new()
            .inner_margin(Margin::symmetric(12, 12))
            .show(ui, |ui| {
                ui.set_width(FLYOUT_WIDTH - 24.0);
                ui.spacing_mut().item_spacing = Vec2::new(0.0, 10.0);
                ui.style_mut().visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);

                // 1. Server URL
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(0.0, 4.0);
                    ui.label(
                        RichText::new("PiDA Server URL")
                            .size(11.5)
                            .strong()
                            .color(PALETTE.on_surface),
                    );
                    let edit = egui::TextEdit::singleline(&mut self.url)
                        .hint_text("http://192.0.2.10:9999")
                        .font(FontId::monospace(12.0))
                        .text_color(Color32::WHITE)
                        .background_color(PALETTE.surface_container)
                        .margin(Margin::symmetric(10, 7))
                        .desired_width(FLYOUT_WIDTH - 24.0);
                    // Keep the focus stroke the same width as the resting one.
                    // egui's focused TextEdit uses `selection.stroke` (1.5px)
                    // and paints it half outside the field, which grows the
                    // widget enough that the whole card loses a pixel or two
                    // at the bottom. The accent colour alone signals focus.
                    let resp = ui
                        .scope(|ui| {
                            let visuals = &mut ui.style_mut().visuals;
                            visuals.selection.stroke = Stroke::new(1.0, PALETTE.focus);
                            visuals.widgets.active.bg_stroke = Stroke::new(1.0, PALETTE.focus);
                            visuals.widgets.open.bg_stroke = Stroke::new(1.0, PALETTE.focus);
                            ui.add(edit)
                        })
                        .inner;
                    if resp.changed() && self.error.is_some() {
                        self.error = None;
                    }
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                    if let Some(e) = &self.error {
                        ui.label(RichText::new(e).size(11.0).color(PALETTE.error));
                    } else {
                        ui.label(
                            RichText::new("Local LAN address (e.g. http://192.0.2.10:9999)")
                                .size(10.5)
                                .color(PALETTE.muted),
                        );
                    }
                });

                // 2. Options card
                Frame::new()
                    .fill(PALETTE.surface_container)
                    .corner_radius(CornerRadius::same(8))
                    .stroke(Stroke::new(1.0, border_subtle()))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(FLYOUT_WIDTH - 44.0);
                        ui.spacing_mut().item_spacing = Vec2::new(0.0, 10.0);

                        ui.horizontal(|ui| {
                            ui.set_width(FLYOUT_WIDTH - 44.0);
                            ui.label(
                                RichText::new("Idle health check")
                                    .size(12.0)
                                    .color(PALETTE.on_surface),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                egui::ComboBox::from_id_salt("idle_poll")
                                    .width(130.0)
                                    .selected_text(
                                        RichText::new(poll_label(self.idle_poll_seconds))
                                            .size(11.5)
                                            .monospace(),
                                    )
                                    .show_ui(ui, |ui| {
                                        for c in IDLE_POLL_CHOICES {
                                            ui.selectable_value(
                                                &mut self.idle_poll_seconds,
                                                c,
                                                poll_label(c),
                                            );
                                        }
                                    });
                            });
                        });

                        ui.horizontal(|ui| {
                            ui.set_width(FLYOUT_WIDTH - 44.0);
                            ui.label(
                                RichText::new("Keep flyout open")
                                    .size(12.0)
                                    .color(PALETTE.on_surface),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.checkbox(&mut self.pin_flyout, "");
                            });
                        });

                        ui.horizontal(|ui| {
                            ui.set_width(FLYOUT_WIDTH - 44.0);
                            ui.label(
                                RichText::new("Launch at Windows startup")
                                    .size(12.0)
                                    .color(PALETTE.on_surface),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.checkbox(&mut self.launch_at_startup, "");
                            });
                        });

                        ui.horizontal(|ui| {
                            ui.set_width(FLYOUT_WIDTH - 44.0);
                            ui.label(
                                RichText::new("Subtle UI sounds")
                                    .size(12.0)
                                    .color(PALETTE.on_surface),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.checkbox(&mut self.ui_sounds, "");
                            });
                        });
                    });

                // 3. Cache + window size controls
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                    if pill_button(
                        ui,
                        RichText::new("Reset flyout size").size(11.5),
                        Vec2::new(0.0, 28.0),
                        6,
                        ButtonKind::Secondary,
                    )
                    .on_hover_text("Return to the default 720 pt height")
                    .clicked()
                    {
                        out.reset_size = true;
                    }

                    if pill_button(
                        ui,
                        RichText::new("Clear cached schema & icons").size(11.5),
                        Vec2::new(0.0, 28.0),
                        6,
                        ButtonKind::Secondary,
                    )
                    .clicked()
                    {
                        poller.send(Command::ClearCache);
                    }
                });
            });

        if submit && self.save(shared, poller) {
            self.reset();
            out.close = true;
            out.content_height = ui.min_rect().height();
            return out;
        }

        // Push the action bar to the bottom of the card so it does not float
        // directly under the form when the flyout is taller than the content.
        let remaining = (ui.available_height() - footer_h).max(0.0);
        let stretch = remaining;
        if remaining > 0.0 {
            ui.add_space(remaining);
        }

        // Bottom footer action bar
        let sep_start = ui.next_widget_position();
        ui.painter().line_segment(
            [
                sep_start,
                Pos2::new(sep_start.x + FLYOUT_WIDTH, sep_start.y),
            ],
            Stroke::new(1.0, border_subtle()),
        );

        // Footer background. Anchored to the card's inner bottom instead of
        // the sheet rect: while the settings sheet slides, its bottom is below
        // the card and the scene clip would cut this fill's rounded corners
        // square, painting over the card's bottom arcs. Ending the fill at the
        // clip bottom keeps the radius concentric with the card shell.
        let footer_bg = Rect::from_min_max(
            sep_start,
            Pos2::new(sep_start.x + FLYOUT_WIDTH, ui.clip_rect().max.y),
        );
        if footer_bg.height() > 0.0 {
            ui.painter().rect_filled(
                footer_bg,
                CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: SHEET_RADIUS,
                    se: SHEET_RADIUS,
                },
                PALETTE.surface_header,
            );
        }

        // Transparent frame: only lays out the buttons now that the fill is
        // painted above.
        Frame::new()
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(FLYOUT_WIDTH - 24.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);

                    if pill_button(
                        ui,
                        RichText::new("Save").size(12.0).strong(),
                        Vec2::new(66.0, 30.0),
                        6,
                        ButtonKind::Primary,
                    )
                    .clicked()
                        && self.save(shared, poller)
                    {
                        self.reset();
                        out.close = true;
                    }

                    if pill_button(
                        ui,
                        RichText::new("Cancel").size(12.0).strong(),
                        Vec2::new(62.0, 30.0),
                        6,
                        ButtonKind::Ghost,
                    )
                    .clicked()
                    {
                        self.reset();
                        out.close = true;
                    }
                });
            });

        out.content_height = (ui.min_rect().height() - stretch).max(0.0);
        out
    }
}
