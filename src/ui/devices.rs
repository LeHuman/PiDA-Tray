//! Square device tiles plus a full-width expanded card for every option.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align2, Color32, ColorImage, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, Ui, pos2, vec2,
};
use serde_json::{Value, json};

use super::{
    button::{ButtonKind, pill_button},
    icons,
};
use crate::{
    api::{ApiError, ParamKind},
    catalog::{
        DeviceGroup, EnumParam, LegacyActionSpec, RangeParam, ReadoutSpec, SetActionSpec, prettify,
    },
    constants::{CARD_RADIUS, PALETTE, SLIDER_DEBOUNCE},
    poller::{Poller, Shared, Snapshot, StateMap},
    sound::{self, UiSound},
    theme::{border_subtle, hover_overlay},
};

const PILL_MAX_OPTIONS: usize = 4;
const LABEL_WIDTH: f32 = 92.0;
const VALUE_WIDTH: f32 = 46.0;
const ROW_GAP: f32 = 8.0;
const TILE_GAP: f32 = 8.0;
const TILE_COLUMNS: usize = 3;
const TILE_STRIP_WIDTH: f32 = 22.0;
const LONG_PRESS: Duration = Duration::from_millis(280);
const CLICK_AWAY_GRACE: Duration = Duration::from_millis(150);

struct PressState {
    started: Instant,
    sliding: bool,
}

struct PendingNumeric {
    value: f64,
    changed_at: Instant,
    group: String,
    action_path: String,
    required_inputs: Vec<String>,
    is_int: bool,
}

#[derive(Default)]
pub struct DevicesView {
    textures: HashMap<String, (usize, Option<TextureHandle>)>,
    pending: HashMap<(String, String), PendingNumeric>,
    string_edits: HashMap<(String, String), String>,
    press: HashMap<String, PressState>,
    /// At most one tile is expanded; expansion renders full width in place.
    expanded: Option<String>,
    expanded_at: Option<Instant>,
}

fn decode_png(bytes: &[u8]) -> Result<ColorImage, ApiError> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| ApiError::Parse(e.to_string()))?
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

fn play(shared: &Shared, effect: UiSound) {
    if shared.settings().ui_sounds {
        sound::play(effect);
    }
}

fn switch(ui: &mut Ui, on: bool, busy: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(44.0, 24.0), Sense::click());
    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let t = ui.ctx().animate_bool(resp.id, on);
    let mut track = if on {
        PALETTE.primary
    } else {
        PALETTE.surface_container_high
    };
    if hovered {
        track = track.blend(hover_overlay());
    }
    if pressed {
        track = track.gamma_multiply(0.82);
    }
    ui.painter().rect_filled(rect, 12.0, track);
    if !on || hovered {
        ui.painter().rect_stroke(
            rect,
            12.0,
            Stroke::new(
                1.5,
                if hovered {
                    PALETTE.focus
                } else {
                    PALETTE.outline
                },
            ),
            egui::StrokeKind::Inside,
        );
    }
    let r = 6.5 + 2.5 * t;
    let x = egui::lerp((rect.left() + 12.0)..=(rect.right() - 12.0), t);
    let knob = if on || hovered {
        Color32::WHITE
    } else {
        PALETTE.on_surface_variant
    };
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), r, knob);
    if busy {
        ui.ctx().request_repaint();
    }
    resp.clicked()
}

fn move_button(ui: &mut Ui, up: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter()
            .rect_filled(rect, CornerRadius::same(11), hover_overlay());
    }
    let color = if resp.hovered() {
        Color32::WHITE
    } else {
        PALETTE.on_surface_variant
    };
    icons::chevron(ui.painter(), rect.shrink(6.0), color, up);
    resp.on_hover_text(if up { "Move earlier" } else { "Move later" })
        .clicked()
}

fn chevron_button(ui: &mut Ui, expanded: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter()
            .rect_filled(rect, CornerRadius::same(12), hover_overlay());
    }
    let color = if resp.hovered() || resp.is_pointer_button_down_on() {
        Color32::WHITE
    } else {
        PALETTE.on_surface_variant
    };
    icons::chevron(ui.painter(), rect.shrink(6.0), color, expanded);
    resp.on_hover_text(if expanded { "Collapse" } else { "Expand" })
        .clicked()
}

fn control_label(ui: &mut Ui, text: &str) {
    ui.allocate_ui_with_layout(
        vec2(LABEL_WIDTH, 22.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(text)
                        .size(12.0)
                        .color(PALETTE.on_surface_variant),
                )
                .truncate(),
            )
            .on_hover_text(text);
        },
    );
}

fn value_chip(ui: &mut Ui, text: &str) {
    ui.allocate_ui_with_layout(
        vec2(VALUE_WIDTH, 22.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(text)
                        .size(11.0)
                        .monospace()
                        .color(PALETTE.on_surface),
                )
                .truncate(),
            );
        },
    );
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(9.5).strong().color(PALETTE.muted));
    ui.add_space(1.0);
}

fn value_f64(state: Option<&StateMap>, name: &str, fallback: f64) -> f64 {
    state
        .and_then(|s| s.get(name))
        .and_then(Value::as_f64)
        .unwrap_or(fallback)
}

fn option_label(v: &Value) -> String {
    v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string())
}

fn format_number(value: f64, is_int: bool) -> String {
    if is_int {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.2}")
    }
}

fn format_readout_value(v: &Value, kind: ParamKind) -> String {
    match kind {
        ParamKind::Bool => match v.as_bool() {
            Some(true) => "On".into(),
            Some(false) => "Off".into(),
            None => v.to_string(),
        },
        ParamKind::Float => v
            .as_f64()
            .map(|n| format!("{n:.2}"))
            .unwrap_or_else(|| v.to_string()),
        ParamKind::Int => v
            .as_i64()
            .map(|n| n.to_string())
            .unwrap_or_else(|| v.to_string()),
        ParamKind::Enum | ParamKind::String => v
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string()),
    }
}

fn json_numeric(value: f64, is_int: bool) -> Value {
    if is_int {
        json!(value.round() as i64)
    } else {
        json!(value)
    }
}

fn fit_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn first_slider(group: &DeviceGroup) -> Option<&RangeParam> {
    group
        .set
        .as_ref()?
        .range_params
        .iter()
        .find(|param| param.has_slider())
}

fn surface_count(group: &DeviceGroup) -> usize {
    let mut count = group.legacy.len();
    if let Some(set) = &group.set {
        if set.toggle_param.is_some() {
            count += 1;
        }
        count += set.range_params.len() + set.enum_params.len() + set.string_params.len();
    }
    count
}

fn has_more(group: &DeviceGroup) -> bool {
    let surfaces = surface_count(group);
    let has_primary = group.set.as_ref().is_some_and(|set| {
        set.toggle_param.is_some() || set.range_params.iter().any(RangeParam::has_slider)
    }) || (surfaces == 1 && group.legacy.len() == 1);
    surfaces > 1 || !group.readouts.is_empty() || (!has_primary && surfaces > 0)
}

impl DevicesView {
    pub fn collapse_expanded(&mut self) {
        self.expanded = None;
        self.press.clear();
    }

    pub fn flush_all(&mut self, shared: &Shared, poller: &Poller) {
        let keys: Vec<_> = self.pending.keys().cloned().collect();
        for key in keys {
            self.flush_key(&key, shared, poller);
        }
    }

    pub fn show(&mut self, ui: &mut Ui, snap: &Snapshot, shared: &Arc<Shared>, poller: &Poller) {
        if snap.catalog.is_empty() {
            let msg = if snap.syncing {
                "Loading devices…"
            } else {
                "No devices yet"
            };
            ui.add_space(4.0);
            ui.label(RichText::new(msg).color(PALETTE.on_surface_variant));
            return;
        }

        let tile_w = ((ui.available_width() - TILE_GAP * (TILE_COLUMNS as f32 - 1.0))
            / TILE_COLUMNS as f32)
            .floor()
            .max(88.0);
        // Rows are spaced explicitly; egui's inherited vertical item spacing
        // would otherwise add a trailing gap under the last row.
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut row: Vec<&DeviceGroup> = Vec::with_capacity(TILE_COLUMNS);
        let mut expanded_rect = None;
        let mut expand_request = None;
        let mut collapse_request = false;
        let mut needs_gap = false;

        for group in snap.catalog.iter() {
            if self.expanded.as_deref() == Some(group.path.as_str()) {
                if !row.is_empty() {
                    if needs_gap {
                        ui.add_space(TILE_GAP);
                    }
                    self.tile_row(ui, &row, snap, shared, poller, tile_w, &mut expand_request);
                    row.clear();
                    needs_gap = true;
                }
                if needs_gap {
                    ui.add_space(TILE_GAP);
                }
                let response =
                    self.expanded_card(ui, group, snap, shared, poller, &mut collapse_request);
                expanded_rect = Some(response);
                needs_gap = true;
            } else {
                row.push(group);
                if row.len() == TILE_COLUMNS {
                    if needs_gap {
                        ui.add_space(TILE_GAP);
                    }
                    self.tile_row(ui, &row, snap, shared, poller, tile_w, &mut expand_request);
                    row.clear();
                    needs_gap = true;
                }
            }
        }
        if !row.is_empty() {
            if needs_gap {
                ui.add_space(TILE_GAP);
            }
            self.tile_row(ui, &row, snap, shared, poller, tile_w, &mut expand_request);
        }

        let now = Instant::now();
        if let Some(path) = expand_request {
            play(shared, UiSound::Open);
            self.expanded = Some(path);
            self.expanded_at = Some(now);
        } else if collapse_request {
            play(shared, UiSound::Close);
            self.expanded = None;
        } else if let Some(rect) = expanded_rect
            && self
                .expanded_at
                .is_none_or(|t| now.duration_since(t) >= CLICK_AWAY_GRACE)
        {
            let clicked = ui.input(|input| input.pointer.any_click());
            let position = ui.input(|input| input.pointer.interact_pos());
            if clicked && position.is_some_and(|position| !rect.contains(position)) {
                play(shared, UiSound::Close);
                self.expanded = None;
            }
        }

        self.flush_due(ui.ctx(), shared, poller);
    }

    #[allow(clippy::too_many_arguments)]
    fn tile_row(
        &mut self,
        ui: &mut Ui,
        row: &[&DeviceGroup],
        snap: &Snapshot,
        shared: &Arc<Shared>,
        poller: &Poller,
        tile_w: f32,
        expand_request: &mut Option<String>,
    ) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = TILE_GAP;
            for group in row {
                self.tile(ui, group, snap, shared, poller, tile_w, expand_request);
            }
        });
    }

    fn texture_for(
        &mut self,
        ctx: &egui::Context,
        snap: &Snapshot,
        key: &str,
    ) -> Option<egui::TextureId> {
        let Some(bytes) = snap.icons.get(key) else {
            self.textures.remove(key);
            return None;
        };
        let ptr = Arc::as_ptr(bytes) as usize;
        let stale = self
            .textures
            .get(key)
            .is_none_or(|(cached_ptr, _)| *cached_ptr != ptr);
        if stale {
            let tex = decode_png(bytes)
                .ok()
                .map(|img| ctx.load_texture(format!("icon:{key}"), img, TextureOptions::LINEAR));
            self.textures.insert(key.to_owned(), (ptr, tex));
        }
        self.textures
            .get(key)
            .and_then(|(_, tex)| tex.as_ref())
            .map(TextureHandle::id)
    }

    #[allow(clippy::too_many_arguments)]
    fn tile(
        &mut self,
        ui: &mut Ui,
        group: &DeviceGroup,
        snap: &Snapshot,
        shared: &Arc<Shared>,
        poller: &Poller,
        tile_w: f32,
        expand_request: &mut Option<String>,
    ) {
        let state = snap.states.get(&group.path);
        let texture = self.texture_for(ui.ctx(), snap, &group.path);
        let toggle = group
            .set
            .as_ref()
            .and_then(|set| set.toggle_param.as_deref());
        let slider = first_slider(group);
        let primary_action = (toggle.is_none() && slider.is_none() && group.legacy.len() == 1)
            .then(|| group.legacy.first())
            .flatten();
        let more = has_more(group);
        let on = toggle
            .and_then(|name| state.and_then(|s| s.get(name)))
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let (rect, _) = ui.allocate_exact_size(vec2(tile_w, tile_w), Sense::hover());
        let id = ui.scope_id().with(("device-tile", &group.path));
        let strip_rect = more.then(|| {
            Rect::from_min_max(
                pos2(rect.right() - TILE_STRIP_WIDTH, rect.top() + 1.0),
                pos2(rect.right() - 1.0, rect.bottom() - 1.0),
            )
        });
        let body_right = strip_rect.map_or(rect.right() - 1.0, |strip| strip.left() - 1.0);
        let body_rect = Rect::from_min_max(
            rect.min + vec2(1.0, 1.0),
            pos2(body_right, rect.bottom() - 1.0),
        );
        let body_sense = if slider.is_some() {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let body = ui.interact(body_rect, id.with("body"), body_sense);
        let strip =
            strip_rect.map(|strip_rect| ui.interact(strip_rect, id.with("more"), Sense::click()));

        let strip_hovered = strip.as_ref().is_some_and(|response| response.hovered());
        if body.hovered() || strip_hovered {
            ui.ctx()
                .set_cursor_icon(if slider.is_some() && body.hovered() {
                    egui::CursorIcon::ResizeVertical
                } else {
                    egui::CursorIcon::PointingHand
                });
        }

        let slider_value = slider.map(|param| {
            value_f64(state, &param.name, param.default_value())
                .clamp(param.min.unwrap(), param.max.unwrap())
        });
        let slider_fraction = slider.map(|param| {
            ((slider_value.unwrap() - param.min.unwrap())
                / (param.max.unwrap() - param.min.unwrap()))
            .clamp(0.0, 1.0) as f32
        });

        let painter = ui.painter();
        let mut bg = if on && slider.is_none() {
            PALETTE
                .surface_container
                .blend(PALETTE.primary.gamma_multiply(0.22))
        } else {
            PALETTE.surface_container
        };
        if body.hovered() || strip_hovered {
            bg = bg.blend(hover_overlay());
        }
        if body.is_pointer_button_down_on() {
            bg = bg.gamma_multiply(0.90);
        }
        painter.rect_filled(rect, CornerRadius::same(CARD_RADIUS), bg);
        if let Some(fraction) = slider_fraction
            && fraction > 0.0
        {
            let fill = Rect::from_min_max(
                pos2(
                    body_rect.left(),
                    rect.bottom() - fraction * body_rect.height(),
                ),
                body_rect.right_bottom(),
            );
            painter.rect_filled(
                fill,
                CornerRadius::same(CARD_RADIUS),
                Color32::from_rgba_unmultiplied(
                    37,
                    99,
                    235,
                    if toggle.is_some() && !on { 42 } else { 78 },
                ),
            );
        }
        if toggle.is_some() {
            painter.circle_filled(
                pos2(body_rect.right() - 8.0, body_rect.top() + 8.0),
                3.0,
                if on { PALETTE.ok } else { PALETTE.outline },
            );
        }
        painter.rect_stroke(
            rect,
            CornerRadius::same(CARD_RADIUS),
            Stroke::new(
                1.0,
                if body.hovered() || strip_hovered {
                    PALETTE.focus.gamma_multiply(0.7)
                } else {
                    border_subtle()
                },
            ),
            egui::StrokeKind::Inside,
        );
        if strip_hovered && let Some(strip_rect) = strip_rect {
            painter.rect_filled(
                strip_rect.shrink2(vec2(0.0, 1.0)),
                CornerRadius {
                    nw: 0,
                    ne: CARD_RADIUS,
                    sw: 0,
                    se: CARD_RADIUS,
                },
                hover_overlay(),
            );
            painter.line_segment(
                [strip_rect.left_top(), strip_rect.left_bottom()],
                Stroke::new(1.0, border_subtle()),
            );
        }

        let inner = body_rect.shrink(9.0);
        let icon_rect = Rect::from_min_size(inner.min, vec2(32.0, 32.0));
        if let Some(texture) = texture {
            painter.image(
                texture,
                icon_rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            icons::draw_kind(painter, group.icon, icon_rect.shrink(1.0), PALETTE.focus);
        }

        let text_x = inner.left() + 40.0;
        let text_right = body_rect.right() - if toggle.is_some() { 14.0 } else { 6.0 };
        let max_chars = (((text_right - text_x) / 6.2).floor() as usize).max(5);
        painter.text(
            pos2(text_x, inner.top()),
            Align2::LEFT_TOP,
            fit_text(&group.title, max_chars),
            FontId::proportional(11.5),
            Color32::WHITE,
        );
        if let Some(subtitle) = &group.subtitle {
            painter.text(
                pos2(text_x, inner.top() + 15.0),
                Align2::LEFT_TOP,
                fit_text(subtitle, max_chars + 2),
                FontId::proportional(8.5),
                PALETTE.muted,
            );
        }

        let mut value_text = None;
        let mut value_label = None;
        if let Some(param) = slider {
            value_label = Some(prettify(&param.name));
            value_text = Some(format_number(slider_value.unwrap(), param.is_int));
        } else if let Some(toggle) = toggle {
            value_label = Some(prettify(toggle));
            value_text = Some(if on { "On".into() } else { "Off".into() });
        } else if let Some(action) = primary_action {
            value_label = Some(action.label.clone());
        } else if let Some(readout) = group.readouts.first()
            && let Some(field) = readout.fields.first()
        {
            let value = state
                .and_then(|s| s.get(&field.name))
                .map(|value| format_readout_value(value, field.kind))
                .unwrap_or_else(|| "—".into());
            value_label = Some(prettify(&field.name));
            value_text = Some(value);
        }
        if let Some(label) = value_label {
            let label_y = if slider.is_some() || value_text.is_none() {
                rect.bottom() - 23.0
            } else {
                rect.bottom() - 37.0
            };
            painter.text(
                pos2(inner.left(), label_y),
                Align2::LEFT_BOTTOM,
                fit_text(&label, max_chars + 2),
                FontId::proportional(9.0),
                PALETTE.muted,
            );
        }
        if let Some(value) = value_text {
            if slider.is_some() {
                painter.text(
                    pos2(inner.right(), rect.bottom() - 23.0),
                    Align2::RIGHT_BOTTOM,
                    value,
                    FontId::proportional(10.5),
                    PALETTE.on_surface_variant,
                );
            } else {
                painter.text(
                    pos2(inner.left(), rect.bottom() - 23.0),
                    Align2::LEFT_BOTTOM,
                    value,
                    FontId::proportional(15.0),
                    Color32::WHITE,
                );
            }
        }

        if let Some(strip_rect) = strip_rect {
            let color = if strip_hovered {
                Color32::WHITE
            } else {
                PALETTE.on_surface_variant
            };
            icons::chevron(
                painter,
                Rect::from_center_size(
                    pos2(strip_rect.center().x, strip_rect.top() + 14.0),
                    vec2(11.0, 11.0),
                ),
                color,
                false,
            );
        }

        if let Some(strip) = strip
            && strip.clicked()
        {
            *expand_request = Some(group.path.clone());
        }

        if let Some(param) = slider {
            let set = group.set.as_ref().unwrap();
            let now = Instant::now();
            let mut press_value = None;
            let mut released_sliding = false;

            if body.is_pointer_button_down_on() {
                let state = self.press.entry(group.path.clone()).or_insert(PressState {
                    started: now,
                    sliding: false,
                });
                let held = now.duration_since(state.started) >= LONG_PRESS;
                if body.dragged() || held {
                    state.sliding = true;
                }
                if state.sliding {
                    if let Some(position) = body.interact_pointer_pos() {
                        let fraction = ((body_rect.bottom() - position.y) / body_rect.height())
                            .clamp(0.0, 1.0) as f64;
                        press_value = Some(
                            param.min.unwrap()
                                + fraction * (param.max.unwrap() - param.min.unwrap()),
                        );
                    }
                } else {
                    let remaining = LONG_PRESS.saturating_sub(now.duration_since(state.started));
                    ui.ctx().request_repaint_after(remaining);
                }
            } else if let Some(state) = self.press.remove(&group.path) {
                released_sliding = state.sliding;
            }

            let mut pending_key = None;
            if let Some(value) = press_value {
                pending_key = Some(self.queue_numeric(group, set, param, value, shared));
            }
            if released_sliding {
                let key = pending_key.unwrap_or((set.action_path.clone(), param.name.clone()));
                self.flush_key(&key, shared, poller);
                play(shared, UiSound::Action);
            } else if body.clicked() {
                if let Some(toggle) = toggle {
                    play(shared, UiSound::Toggle);
                    poller.set_param(
                        shared,
                        &group.path,
                        &set.action_path,
                        &set.required_inputs,
                        toggle,
                        json!(!on),
                    );
                } else if let Some(position) = body.interact_pointer_pos() {
                    let fraction = ((body_rect.bottom() - position.y) / body_rect.height())
                        .clamp(0.0, 1.0) as f64;
                    let value =
                        param.min.unwrap() + fraction * (param.max.unwrap() - param.min.unwrap());
                    let key = self.queue_numeric(group, set, param, value, shared);
                    self.flush_key(&key, shared, poller);
                    play(shared, UiSound::Action);
                }
            }
        } else if body.clicked() {
            if let (Some(set), Some(toggle)) = (&group.set, toggle) {
                play(shared, UiSound::Toggle);
                poller.set_param(
                    shared,
                    &group.path,
                    &set.action_path,
                    &set.required_inputs,
                    toggle,
                    json!(!on),
                );
            } else if let Some(action) = primary_action {
                play(shared, UiSound::Action);
                poller.press(shared, &group.path, &action.action_path);
            } else if surface_count(group) > 0 || !group.readouts.is_empty() {
                *expand_request = Some(group.path.clone());
            }
        }
    }

    fn queue_numeric(
        &mut self,
        group: &DeviceGroup,
        set: &SetActionSpec,
        param: &RangeParam,
        value: f64,
        shared: &Shared,
    ) -> (String, String) {
        let clamped = param.clamp(value);
        let value = if param.is_int {
            clamped.round()
        } else {
            clamped
        };
        let key = (set.action_path.clone(), param.name.clone());
        shared.set_pending_param(&group.path, &param.name, true);
        shared.update(|s| {
            s.states
                .entry(group.path.clone())
                .or_default()
                .insert(param.name.clone(), json_numeric(value, param.is_int));
        });
        self.pending.insert(
            key.clone(),
            PendingNumeric {
                value,
                changed_at: Instant::now(),
                group: group.path.clone(),
                action_path: set.action_path.clone(),
                required_inputs: set.required_inputs.clone(),
                is_int: param.is_int,
            },
        );
        key
    }

    fn expanded_card(
        &mut self,
        ui: &mut Ui,
        group: &DeviceGroup,
        snap: &Snapshot,
        shared: &Arc<Shared>,
        poller: &Poller,
        collapse_request: &mut bool,
    ) -> Rect {
        let state = snap.states.get(&group.path);
        let texture = self.texture_for(ui.ctx(), snap, &group.path);
        let response = egui::Frame::new()
            .fill(PALETTE.surface_container)
            .corner_radius(CornerRadius::same(CARD_RADIUS))
            .stroke(Stroke::new(1.0, border_subtle()))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);

                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                    if let Some(texture) = texture {
                        ui.painter().image(
                            texture,
                            rect,
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    } else {
                        icons::draw_kind(ui.painter(), group.icon, rect.shrink(2.0), PALETTE.focus);
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                        ui.label(
                            RichText::new(&group.title)
                                .size(14.0)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        if let Some(subtitle) = &group.subtitle {
                            ui.label(RichText::new(subtitle).size(10.5).color(PALETTE.muted));
                        }
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                        if chevron_button(ui, true) {
                            *collapse_request = true;
                        }
                        if let Some(set) = &group.set
                            && let Some(toggle) = &set.toggle_param
                        {
                            let on = state
                                .and_then(|s| s.get(toggle))
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            let busy = snap
                                .in_flight
                                .contains(&format!("{}:{toggle}", set.action_path));
                            if switch(ui, on, busy) {
                                play(shared, UiSound::Toggle);
                                poller.set_param(
                                    shared,
                                    &group.path,
                                    &set.action_path,
                                    &set.required_inputs,
                                    toggle,
                                    json!(!on),
                                );
                            }
                        }
                        if move_button(ui, false) {
                            play(shared, UiSound::Action);
                            shared.move_device(&group.path, 1);
                        }
                        if move_button(ui, true) {
                            play(shared, UiSound::Action);
                            shared.move_device(&group.path, -1);
                        }
                    });
                });

                if let Some(set) = &group.set {
                    let has_controls = !set.range_params.is_empty()
                        || !set.enum_params.is_empty()
                        || !set.string_params.is_empty();
                    if has_controls {
                        section_label(ui, "Controls");
                    }
                    for param in &set.range_params {
                        self.numeric_control(ui, group, set, param, state, shared, poller);
                    }
                    for param in &set.enum_params {
                        enum_control(ui, group, set, param, state, shared, poller);
                    }
                    for name in &set.string_params {
                        self.string_control(ui, group, set, name, state, shared, poller);
                    }
                }

                if !group.readouts.is_empty() {
                    section_label(ui, "Status");
                    for readout in &group.readouts {
                        readout_row(ui, readout, state);
                    }
                }

                if !group.legacy.is_empty() {
                    section_label(ui, "Actions");
                    let tile_width = ((ui.available_width() - 6.0) / 2.0).max(80.0);
                    for row in group.legacy.chunks(2) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            for action in row {
                                action_tile(
                                    ui, group, snap, shared, poller, action, tile_width, 32.0,
                                );
                            }
                        });
                    }
                }
            });
        response.response.rect
    }

    #[allow(clippy::too_many_arguments)]
    fn numeric_control(
        &mut self,
        ui: &mut Ui,
        group: &DeviceGroup,
        set: &SetActionSpec,
        param: &RangeParam,
        state: Option<&StateMap>,
        shared: &Arc<Shared>,
        poller: &Poller,
    ) {
        let key = (set.action_path.clone(), param.name.clone());
        let mut value = self
            .pending
            .get(&key)
            .map(|pending| pending.value)
            .unwrap_or_else(|| param.clamp(value_f64(state, &param.name, param.default_value())));

        let mut changed = false;
        let mut flush_now = false;
        let row_width = ui.available_width();
        let control_width = (row_width - LABEL_WIDTH - ROW_GAP).max(120.0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(ROW_GAP, 0.0);
            control_label(ui, &prettify(&param.name));

            if param.has_slider() {
                ui.visuals_mut().widgets.inactive.bg_fill = PALETTE.track;
                ui.spacing_mut().slider_width = (control_width - VALUE_WIDTH - ROW_GAP).max(80.0);
                let (lo, hi) = (param.min.unwrap(), param.max.unwrap());
                let mut slider = egui::Slider::new(&mut value, lo..=hi).show_value(false);
                if param.is_int {
                    slider = slider.integer();
                } else if let Some(step) = param.step {
                    slider = slider.step_by(step);
                }
                let response = ui.add(slider);
                changed = response.changed();
                flush_now = response.drag_stopped() || response.lost_focus();
                value_chip(ui, &format_number(value, param.is_int));
            } else {
                let step = param.step.unwrap_or(1.0);
                ui.allocate_ui_with_layout(
                    vec2(control_width, 22.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                        if ui
                            .add_sized(
                                [24.0, 22.0],
                                egui::Button::new(RichText::new("−").size(14.0)),
                            )
                            .clicked()
                        {
                            value = param.clamp(value - step);
                            changed = true;
                        }
                        let mut drag = egui::DragValue::new(&mut value).speed(if param.is_int {
                            1.0
                        } else {
                            0.1
                        });
                        if let (Some(lo), Some(hi)) = (param.min, param.max) {
                            drag = drag.range(lo..=hi);
                        } else if let Some(lo) = param.min {
                            drag = drag.range(lo..=f64::MAX);
                        } else if let Some(hi) = param.max {
                            drag = drag.range(f64::MIN..=hi);
                        }
                        let drag_width = (control_width - 56.0).max(60.0);
                        let response = ui.add_sized([drag_width, 22.0], drag);
                        changed |= response.changed();
                        flush_now |= response.drag_stopped() || response.lost_focus();
                        if ui
                            .add_sized(
                                [24.0, 22.0],
                                egui::Button::new(RichText::new("+").size(14.0)),
                            )
                            .clicked()
                        {
                            value = param.clamp(value + step);
                            changed = true;
                        }
                    },
                );
            }
        });

        if changed {
            self.queue_numeric(group, set, param, value, shared);
        }
        if flush_now {
            self.flush_key(&key, shared, poller);
            play(shared, UiSound::Action);
        }
    }

    fn flush_key(&mut self, key: &(String, String), shared: &Shared, poller: &Poller) {
        if let Some(pending) = self.pending.remove(key) {
            let value = json_numeric(pending.value, pending.is_int);
            poller.set_param(
                shared,
                &pending.group,
                &pending.action_path,
                &pending.required_inputs,
                &key.1,
                value,
            );
        }
    }

    fn flush_due(&mut self, ctx: &egui::Context, shared: &Shared, poller: &Poller) {
        let now = Instant::now();
        let due: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| now.duration_since(pending.changed_at) >= SLIDER_DEBOUNCE)
            .map(|(key, _)| key.clone())
            .collect();
        for key in due {
            self.flush_key(&key, shared, poller);
        }
        if !self.pending.is_empty() {
            ctx.request_repaint_after(SLIDER_DEBOUNCE);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn string_control(
        &mut self,
        ui: &mut Ui,
        group: &DeviceGroup,
        set: &SetActionSpec,
        name: &str,
        state: Option<&StateMap>,
        shared: &Arc<Shared>,
        poller: &Poller,
    ) {
        let key = (set.action_path.clone(), name.to_owned());
        let current = state
            .and_then(|s| s.get(name))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let buffer = self
            .string_edits
            .entry(key)
            .or_insert_with(|| current.clone());
        let row_width = ui.available_width();
        let control_width = (row_width - LABEL_WIDTH - ROW_GAP).max(120.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(ROW_GAP, 0.0);
            control_label(ui, &prettify(name));
            let response = ui.add_sized(
                [control_width, 24.0],
                egui::TextEdit::singleline(buffer).hint_text("value"),
            );
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                play(shared, UiSound::Action);
                poller.set_param(
                    shared,
                    &group.path,
                    &set.action_path,
                    &set.required_inputs,
                    name,
                    json!(buffer.clone()),
                );
            } else if !response.has_focus() && *buffer != current {
                *buffer = current.clone();
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn action_tile(
    ui: &mut Ui,
    group: &DeviceGroup,
    snap: &Snapshot,
    shared: &Arc<Shared>,
    poller: &Poller,
    action: &LegacyActionSpec,
    width: f32,
    height: f32,
) {
    let busy = snap.in_flight.contains(&action.action_path);
    let label = if busy {
        format!("{} …", action.label)
    } else {
        action.label.clone()
    };
    let response = ui
        .add_enabled_ui(!busy, |ui| {
            pill_button(
                ui,
                RichText::new(label).size(11.5),
                vec2(width, height),
                8,
                ButtonKind::Secondary,
            )
        })
        .inner
        .on_hover_text(&action.action_path);
    if response.clicked() {
        play(shared, UiSound::Action);
        poller.press(shared, &group.path, &action.action_path);
    }
}

fn readout_row(ui: &mut Ui, readout: &ReadoutSpec, state: Option<&StateMap>) {
    ui.add_space(1.0);
    ui.horizontal_wrapped(|ui| {
        if readout.fields.len() > 1 {
            ui.label(
                RichText::new(format!("{}:", readout.label))
                    .size(11.0)
                    .color(PALETTE.muted),
            );
        }
        for field in &readout.fields {
            let value = state
                .and_then(|s| s.get(&field.name))
                .map(|value| format_readout_value(value, field.kind))
                .unwrap_or_else(|| "—".into());
            egui::Frame::new()
                .fill(PALETTE.surface)
                .corner_radius(CornerRadius::same(7))
                .stroke(Stroke::new(1.0, border_subtle()))
                .inner_margin(Margin::symmetric(7, 3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                        ui.label(
                            RichText::new(prettify(&field.name))
                                .size(11.0)
                                .color(PALETTE.muted),
                        );
                        ui.label(
                            RichText::new(value)
                                .size(11.0)
                                .monospace()
                                .color(PALETTE.on_surface),
                        );
                    });
                });
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn enum_control(
    ui: &mut Ui,
    group: &DeviceGroup,
    set: &SetActionSpec,
    param: &EnumParam,
    state: Option<&StateMap>,
    shared: &Arc<Shared>,
    poller: &Poller,
) {
    let current = state.and_then(|s| s.get(&param.name));
    let row_width = ui.available_width();
    let control_width = (row_width - LABEL_WIDTH - ROW_GAP).max(120.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(ROW_GAP, 0.0);
        control_label(ui, &prettify(&param.name));

        if param.options.len() <= PILL_MAX_OPTIONS {
            ui.allocate_ui_with_layout(
                vec2(control_width, 24.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                    for option in &param.options {
                        let selected = current == Some(option);
                        if pill_button(
                            ui,
                            RichText::new(option_label(option)).size(11.5),
                            vec2(0.0, 24.0),
                            255,
                            ButtonKind::Segment { active: selected },
                        )
                        .clicked()
                            && !selected
                        {
                            play(shared, UiSound::Action);
                            poller.set_param(
                                shared,
                                &group.path,
                                &set.action_path,
                                &set.required_inputs,
                                &param.name,
                                option.clone(),
                            );
                        }
                    }
                },
            );
        } else {
            let shown = current
                .map(option_label)
                .unwrap_or_else(|| "Select…".into());
            ui.allocate_ui_with_layout(
                vec2(control_width, 24.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    let combo = egui::ComboBox::from_id_salt((&set.action_path, &param.name))
                        .width(control_width)
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            ui.visuals_mut().interact_cursor = Some(egui::CursorIcon::PointingHand);
                            for option in &param.options {
                                let selected = current == Some(option);
                                if ui
                                    .selectable_label(selected, option_label(option))
                                    .clicked()
                                    && !selected
                                {
                                    play(shared, UiSound::Action);
                                    poller.set_param(
                                        shared,
                                        &group.path,
                                        &set.action_path,
                                        &set.required_inputs,
                                        &param.name,
                                        option.clone(),
                                    );
                                }
                            }
                        });
                    if combo.response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                },
            );
        }
    });
}
