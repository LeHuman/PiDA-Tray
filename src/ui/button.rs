//! Shared pill/tile button with explicit resting, hover, and pressed feedback.

use eframe::egui::{self, Color32, CornerRadius, Response, RichText, Stroke, Vec2};

use crate::{
    constants::PALETTE,
    theme::{active_tint_bg, border_input, border_subtle, hover_overlay},
};

#[derive(Clone, Copy)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Segment { active: bool },
}

pub fn pill_button(
    ui: &mut egui::Ui,
    text: RichText,
    min_size: Vec2,
    corner_radius: u8,
    kind: ButtonKind,
) -> Response {
    let enabled = ui.is_enabled();
    let (rest, hover, pressed, rest_stroke, hover_stroke, fg) = match kind {
        ButtonKind::Primary | ButtonKind::Segment { active: true } => (
            PALETTE.primary,
            PALETTE.primary.blend(hover_overlay()),
            PALETTE.primary.gamma_multiply(0.78),
            Stroke::NONE,
            Stroke::new(1.0, PALETTE.focus),
            Color32::WHITE,
        ),
        ButtonKind::Secondary => (
            PALETTE.surface_container_high,
            PALETTE.surface_container_high.blend(hover_overlay()),
            active_tint_bg(),
            Stroke::new(1.0, border_subtle()),
            Stroke::new(1.0, border_input()),
            PALETTE.on_surface,
        ),
        ButtonKind::Ghost => (
            Color32::TRANSPARENT,
            hover_overlay(),
            active_tint_bg(),
            Stroke::NONE,
            Stroke::new(1.0, border_input()),
            PALETTE.on_surface_variant,
        ),
        ButtonKind::Segment { active: false } => (
            PALETTE.surface_container_high,
            PALETTE.surface_container_high.blend(hover_overlay()),
            active_tint_bg(),
            Stroke::new(1.0, border_subtle()),
            Stroke::new(1.0, border_input()),
            PALETTE.on_surface,
        ),
    };

    let (rest, hover, pressed, fg) = if enabled {
        (rest, hover, pressed, fg)
    } else {
        let dim = rest.gamma_multiply(0.55);
        (dim, dim, dim, PALETTE.muted)
    };

    let response = ui
        .scope(|ui| {
            let v = &mut ui.style_mut().visuals;
            v.widgets.inactive.weak_bg_fill = rest;
            v.widgets.inactive.bg_fill = rest;
            v.widgets.inactive.bg_stroke = rest_stroke;
            v.widgets.inactive.fg_stroke = Stroke::new(1.0, fg);

            v.widgets.hovered.weak_bg_fill = hover;
            v.widgets.hovered.bg_fill = hover;
            v.widgets.hovered.bg_stroke = hover_stroke;
            v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);

            v.widgets.active.weak_bg_fill = pressed;
            v.widgets.active.bg_fill = pressed;
            v.widgets.active.bg_stroke = hover_stroke;
            v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);

            v.widgets.open.weak_bg_fill = hover;
            v.widgets.open.bg_fill = hover;
            v.widgets.open.bg_stroke = hover_stroke;
            v.widgets.open.fg_stroke = Stroke::new(1.0, Color32::WHITE);

            v.selection.stroke = Stroke::new(1.0, PALETTE.focus);
            v.interact_cursor = enabled.then_some(egui::CursorIcon::PointingHand);

            ui.add(
                egui::Button::new(text)
                    .min_size(min_size)
                    .corner_radius(CornerRadius::same(corner_radius)),
            )
        })
        .inner;

    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}
