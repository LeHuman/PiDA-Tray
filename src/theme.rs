//! Local Windows fonts + dark slate visuals (no remote fonts).

use std::{path::PathBuf, sync::Arc};

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke};

use crate::constants::{CARD_RADIUS, PALETTE};

pub fn border_card() -> Color32 {
    Color32::from_rgba_unmultiplied(255, 255, 255, 26)
}

pub fn border_subtle() -> Color32 {
    Color32::from_rgba_unmultiplied(255, 255, 255, 20)
}

pub fn border_input() -> Color32 {
    Color32::from_rgba_unmultiplied(255, 255, 255, 31)
}

pub fn hover_overlay() -> Color32 {
    Color32::from_rgba_unmultiplied(255, 255, 255, 20)
}

pub fn active_tint_bg() -> Color32 {
    Color32::from_rgba_unmultiplied(37, 99, 235, 64)
}

fn read_first_existing(candidates: &[PathBuf]) -> Option<Vec<u8>> {
    candidates.iter().find_map(|p| std::fs::read(p).ok())
}

/// Segoe UI for proportional text and Cascadia Mono / Consolas for monospace,
/// falling back to egui's bundled fonts when system files are unavailable.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    if let Some(root) = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")) {
        let dir = PathBuf::from(root).join("Fonts");
        if let Some(bytes) =
            read_first_existing(&[dir.join("segoeui.ttf"), dir.join("SegoeUI.ttf")])
        {
            fonts
                .font_data
                .insert("segoe-ui".into(), Arc::new(FontData::from_owned(bytes)));
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "segoe-ui".into());
        }
        if let Some(bytes) =
            read_first_existing(&[dir.join("CascadiaMono.ttf"), dir.join("consola.ttf")])
        {
            fonts
                .font_data
                .insert("local-mono".into(), Arc::new(FontData::from_owned(bytes)));
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .insert(0, "local-mono".into());
        }
    }
    ctx.set_fonts(fonts);
}

pub fn apply_visuals(ctx: &egui::Context) {
    let p = &PALETTE;
    ctx.all_styles_mut(|style| {
        let mut v = egui::Visuals::dark();
        v.override_text_color = Some(p.on_surface);
        v.panel_fill = Color32::TRANSPARENT;
        v.window_fill = p.surface_container;
        v.extreme_bg_color = p.surface_container;
        v.faint_bg_color = p.surface_container;
        v.selection.bg_fill = p.primary;
        v.selection.stroke = Stroke::new(1.5, p.focus);
        v.hyperlink_color = p.focus;
        v.window_stroke = Stroke::new(1.0, border_input());

        let r = CornerRadius::same(CARD_RADIUS);
        v.widgets.noninteractive.bg_fill = p.surface_container;
        v.widgets.noninteractive.weak_bg_fill = p.surface_container;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border_subtle());
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.on_surface);
        v.widgets.noninteractive.corner_radius = r;

        v.widgets.inactive.bg_fill = p.surface_container;
        v.widgets.inactive.weak_bg_fill = p.surface_container_high;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, border_input());
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.on_surface);
        v.widgets.inactive.corner_radius = r;

        v.widgets.hovered.bg_fill = Color32::from_rgb(33, 38, 48);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(33, 38, 48);
        v.widgets.hovered.bg_stroke =
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 55));
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        v.widgets.hovered.corner_radius = r;

        v.widgets.active.bg_fill = p.primary;
        v.widgets.active.weak_bg_fill = p.primary;
        v.widgets.active.bg_stroke = Stroke::new(1.5, p.focus);
        v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        v.widgets.active.corner_radius = r;

        v.widgets.open.bg_fill = p.surface_container;
        v.widgets.open.weak_bg_fill = p.surface_container;
        v.widgets.open.bg_stroke = Stroke::new(1.5, p.focus);
        v.widgets.open.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        v.widgets.open.corner_radius = r;

        style.visuals = v;
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.slider_width = 185.0;
        style.spacing.slider_rail_height = 5.0;
    });
}

fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

#[allow(dead_code)] // contrast helper (used by tests and available to UI code)
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[allow(dead_code)]
/// Readable foreground (near-black or near-white) for text drawn on `bg`.
pub fn on_color(bg: Color32) -> Color32 {
    let dark = Color32::from_rgb(0x10, 0x10, 0x14);
    let light = Color32::from_rgb(0xF5, 0xF5, 0xF7);
    if contrast_ratio(bg, dark) >= contrast_ratio(bg, light) {
        dark
    } else {
        light
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_text_is_readable_on_surface() {
        assert!(contrast_ratio(PALETTE.on_surface, PALETTE.surface) >= 7.0);
        assert!(contrast_ratio(PALETTE.on_primary, PALETTE.primary) >= 4.5);
        assert!(contrast_ratio(PALETTE.error, PALETTE.error_surface) >= 4.5);
    }

    #[test]
    fn on_color_picks_the_readable_side() {
        assert!(luminance(on_color(Color32::BLACK)) > 0.5);
        assert!(luminance(on_color(Color32::WHITE)) < 0.1);
    }
}
