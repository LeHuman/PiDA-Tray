//! Flyout header: status dot, title, and Refresh / Pin / Settings / Hide buttons.

use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, Sense, Stroke, StrokeKind,
    Ui, Vec2, vec2,
};

use super::icons;
use crate::{
    constants::{FLYOUT_RADIUS, FLYOUT_WIDTH, PALETTE},
    poller::{Connection, Snapshot},
    theme::{active_tint_bg, border_input, border_subtle, hover_overlay},
};

#[derive(Default)]
pub struct HeaderOutput {
    pub refresh: bool,
    pub toggle_pin: bool,
    pub settings: bool,
    pub hide: bool,
    pub back: bool,
}

pub fn status_color(snap: &Snapshot) -> Color32 {
    match snap.connection {
        Connection::Online if snap.banner.is_some() => PALETTE.warn,
        Connection::Online => PALETTE.ok,
        Connection::Offline => PALETTE.error,
        Connection::Unconfigured | Connection::Unknown => PALETTE.outline,
    }
}

fn status_label(snap: &Snapshot, in_settings: bool) -> &'static str {
    if in_settings {
        return "Settings";
    }
    match snap.connection {
        Connection::Online if snap.syncing => "Syncing",
        Connection::Online => "Connected",
        Connection::Offline => "Offline",
        Connection::Unconfigured => "Setup needed",
        Connection::Unknown => "Checking",
    }
}

fn icon_button(
    ui: &mut Ui,
    active: bool,
    tooltip: &str,
    draw: impl FnOnce(&egui::Painter, egui::Rect, Color32),
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let (bg, stroke, color) = if active && pressed {
        (
            active_tint_bg(),
            Stroke::new(1.0, PALETTE.focus),
            Color32::WHITE,
        )
    } else if active && hovered {
        (
            active_tint_bg(),
            Stroke::new(1.0, PALETTE.focus.gamma_multiply(0.75)),
            Color32::WHITE,
        )
    } else if active {
        (active_tint_bg(), Stroke::NONE, PALETTE.focus)
    } else if pressed {
        (
            active_tint_bg(),
            Stroke::new(1.0, PALETTE.focus.gamma_multiply(0.75)),
            PALETTE.focus,
        )
    } else if hovered {
        (
            hover_overlay(),
            Stroke::new(1.0, border_input()),
            Color32::WHITE,
        )
    } else {
        (
            Color32::TRANSPARENT,
            Stroke::NONE,
            PALETTE.on_surface_variant,
        )
    };

    if bg != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, CornerRadius::same(6), bg);
    }
    if stroke != Stroke::NONE {
        ui.painter()
            .rect_stroke(rect, CornerRadius::same(6), stroke, StrokeKind::Inside);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(1.5, PALETTE.focus),
            StrokeKind::Inside,
        );
    }
    draw(ui.painter(), rect.shrink(5.0), color);
    resp.on_hover_text(tooltip).clicked()
}

pub fn show(ui: &mut Ui, snap: &Snapshot, in_settings: bool) -> HeaderOutput {
    let mut out = HeaderOutput::default();
    let is_live = snap.connection == Connection::Online;

    let resp = Frame::new()
        .fill(PALETTE.surface_header)
        .corner_radius(CornerRadius {
            nw: FLYOUT_RADIUS,
            ne: FLYOUT_RADIUS,
            sw: 0,
            se: 0,
        })
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(FLYOUT_WIDTH - 24.0, 28.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);

                    if in_settings {
                        out.back = icon_button(ui, false, "Back to devices", icons::back);
                    } else {
                        let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                        ui.painter()
                            .circle_filled(dot.center(), 5.0, status_color(snap));
                        ui.painter().circle_stroke(
                            dot.center(),
                            5.0,
                            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 38)),
                        );
                    }

                    ui.label(
                        RichText::new("PiDA")
                            .size(13.5)
                            .strong()
                            .color(Color32::WHITE),
                    );
                    ui.label(RichText::new("·").size(11.0).color(PALETTE.muted));
                    ui.add(
                        egui::Label::new(
                            RichText::new(status_label(snap, in_settings))
                                .size(11.0)
                                .color(if is_live && !in_settings {
                                    PALETTE.on_surface
                                } else {
                                    PALETTE.on_surface_variant
                                }),
                        )
                        .truncate(),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
                        out.hide = icon_button(ui, false, "Hide flyout (Esc)", icons::close);
                        out.settings = icon_button(
                            ui,
                            in_settings,
                            if in_settings {
                                "Back to devices"
                            } else {
                                "Settings"
                            },
                            icons::gear,
                        );
                        if !in_settings {
                            let pinned = snap.settings.pin_flyout;
                            out.toggle_pin = icon_button(
                                ui,
                                pinned,
                                if pinned {
                                    "Unpin flyout (auto-hide on blur)"
                                } else {
                                    "Pin flyout open"
                                },
                                |p, r, c| icons::pin(p, r, c, pinned),
                            );
                            out.refresh = icon_button(ui, false, "Refresh devices", icons::refresh);
                        }
                    });
                },
            );
        });

    let rect = resp.response.rect;
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, border_subtle()),
    );
    out
}
