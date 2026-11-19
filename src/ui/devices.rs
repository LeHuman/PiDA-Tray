//! Scrollable device cards: collapsible groups with toggle, sliders, steppers,
//! enum pills/dropdown, strings, status readouts, and action tiles.

use std::{collections::HashMap, sync::Arc, time::Instant};

use eframe::egui::{
    self, Align, Color32, ColorImage, CornerRadius, Layout, Margin, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, Ui, vec2,
};
use serde_json::{Value, json};

use super::icons;
use crate::{
    api::{ApiError, ParamKind},
    catalog::{DeviceGroup, EnumParam, RangeParam, ReadoutSpec, SetActionSpec, prettify},
    constants::{CARD_RADIUS, PALETTE, SLIDER_DEBOUNCE},
    poller::{Poller, Shared, Snapshot, StateMap},
    theme::{border_subtle, hover_overlay},
};

/// Enums with at most this many options render as segmented pills.
const PILL_MAX_OPTIONS: usize = 4;
/// Shared label column so every control row lines up.
const LABEL_WIDTH: f32 = 92.0;
/// Fixed numeric readout column keeps all slider tracks the same length.
const VALUE_WIDTH: f32 = 46.0;
const ROW_GAP: f32 = 8.0;

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
    /// Cached GPU textures keyed by group path + `Arc::as_ptr` fingerprint so
    /// cache clears or fresh icons reload automatically.
    textures: HashMap<String, (usize, Option<TextureHandle>)>,
    pending: HashMap<(String, String), PendingNumeric>,
    string_edits: HashMap<(String, String), String>,
    /// Device cards the user has collapsed (session-local).
    collapsed: std::collections::HashSet<String>,
}

fn decode_png(bytes: &[u8]) -> Result<ColorImage, ApiError> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| ApiError::Parse(e.to_string()))?
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

fn switch(ui: &mut Ui, on: bool, busy: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(44.0, 24.0), Sense::click());
    let t = ui.ctx().animate_bool(resp.id, on);
    let track = if on {
        PALETTE.primary
    } else {
        PALETTE.surface_container_high
    };
    ui.painter().rect_filled(rect, 12.0, track);
    if !on {
        ui.painter().rect_stroke(
            rect,
            12.0,
            Stroke::new(1.5, PALETTE.outline),
            egui::StrokeKind::Inside,
        );
    }
    let r = 6.5 + 2.5 * t;
    let x = egui::lerp((rect.left() + 12.0)..=(rect.right() - 12.0), t);
    let knob = if on {
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

fn chevron_button(ui: &mut Ui, expanded: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(12), hover_overlay());
    }
    let color = if resp.hovered() {
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
        Layout::left_to_right(Align::Center),
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
        Layout::right_to_left(Align::Center),
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

impl DevicesView {
    /// Flush any debounced slider/stepper edits immediately (e.g. before hiding the flyout).
    pub fn flush_all(&mut self, shared: &Shared, poller: &Poller) {
        let keys: Vec<_> = self.pending.keys().cloned().collect();
        for key in keys {
            if let Some(p) = self.pending.remove(&key) {
                let value = json_numeric(p.value, p.is_int);
                poller.set_param(
                    shared,
                    &p.group,
                    &p.action_path,
                    &p.required_inputs,
                    &key.1,
                    value,
                );
            }
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
        for group in snap.catalog.iter() {
            self.card(ui, group, snap, shared, poller);
        }
        self.flush_due(ui.ctx(), shared, poller);
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
            .and_then(|(_, t)| t.as_ref())
            .map(TextureHandle::id)
    }

    fn card(
        &mut self,
        ui: &mut Ui,
        group: &DeviceGroup,
        snap: &Snapshot,
        shared: &Arc<Shared>,
        poller: &Poller,
    ) {
        let state = snap.states.get(&group.path);
        let tex = self.texture_for(ui.ctx(), snap, &group.path);
        let expanded = !self.collapsed.contains(&group.path);
        let mut toggle_collapse = false;

        egui::Frame::new()
            .fill(PALETTE.surface_container)
            .corner_radius(CornerRadius::same(CARD_RADIUS))
            .stroke(Stroke::new(1.0, border_subtle()))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);

                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                    match tex {
                        Some(id) => {
                            ui.painter().image(
                                id,
                                rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                Color32::WHITE,
                            );
                        }
                        None => {
                            icons::draw_kind(
                                ui.painter(),
                                group.icon,
                                rect.shrink(2.0),
                                PALETTE.focus,
                            );
                        }
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                        ui.label(
                            RichText::new(&group.title)
                                .size(14.0)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        if let Some(sub) = &group.subtitle {
                            ui.label(RichText::new(sub).size(10.5).color(PALETTE.muted));
                        }
                    });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                        toggle_collapse = chevron_button(ui, expanded);

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
                                // Real JSON boolean, never 1/0 or "true".
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
                    });
                });

                if !expanded {
                    return;
                }

                if let Some(set) = &group.set {
                    let has_controls = !set.range_params.is_empty()
                        || !set.enum_params.is_empty()
                        || !set.string_params.is_empty();
                    if has_controls {
                        section_label(ui, "Controls");
                    }
                    for r in &set.range_params {
                        self.numeric_control(ui, group, set, r, state, shared, poller);
                    }
                    for e in &set.enum_params {
                        enum_control(ui, group, set, e, state, shared, poller);
                    }
                    for name in &set.string_params {
                        self.string_control(ui, group, set, name, state, shared, poller);
                    }
                }

                if !group.readouts.is_empty() {
                    section_label(ui, "Status");
                    for ro in &group.readouts {
                        readout_row(ui, ro, state);
                    }
                }

                if !group.legacy.is_empty() {
                    section_label(ui, "Actions");
                    let tile_width = ((ui.available_width() - 6.0) / 2.0).max(80.0);
                    for row in group.legacy.chunks(2) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            for l in row {
                                let busy = snap.in_flight.contains(&l.action_path);
                                let label = if busy {
                                    format!("{} …", l.label)
                                } else {
                                    l.label.clone()
                                };
                                let btn = egui::Button::new(
                                    RichText::new(label).size(11.5).color(PALETTE.on_surface),
                                )
                                .fill(PALETTE.surface_container_high)
                                .stroke(Stroke::new(1.0, border_subtle()))
                                .corner_radius(CornerRadius::same(8))
                                .min_size(vec2(tile_width, 32.0));
                                if ui
                                    .add_enabled(!busy, btn)
                                    .on_hover_text(&l.action_path)
                                    .clicked()
                                {
                                    poller.press(shared, &group.path, &l.action_path);
                                }
                            }
                        });
                    }
                }
            });
        if toggle_collapse {
            if expanded {
                self.collapsed.insert(group.path.clone());
            } else {
                self.collapsed.remove(&group.path);
            }
        }
        ui.add_space(6.0);
    }

    #[allow(clippy::too_many_arguments)]
    fn numeric_control(
        &mut self,
        ui: &mut Ui,
        group: &DeviceGroup,
        set: &SetActionSpec,
        r: &RangeParam,
        state: Option<&StateMap>,
        shared: &Arc<Shared>,
        poller: &Poller,
    ) {
        let key = (set.action_path.clone(), r.name.clone());
        let mut v = self
            .pending
            .get(&key)
            .map(|p| p.value)
            .unwrap_or_else(|| r.clamp(value_f64(state, &r.name, r.default_value())));

        let mut changed = false;
        let mut flush_now = false;
        let row_width = ui.available_width();
        let control_width = (row_width - LABEL_WIDTH - ROW_GAP).max(120.0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(ROW_GAP, 0.0);
            control_label(ui, &prettify(&r.name));

            if r.has_slider() {
                ui.visuals_mut().widgets.inactive.bg_fill = PALETTE.track;
                ui.spacing_mut().slider_width = (control_width - VALUE_WIDTH - ROW_GAP).max(80.0);
                let (lo, hi) = (r.min.unwrap(), r.max.unwrap());
                let mut slider = egui::Slider::new(&mut v, lo..=hi).show_value(false);
                if r.is_int {
                    slider = slider.integer();
                } else if let Some(step) = r.step {
                    slider = slider.step_by(step);
                }
                let resp = ui.add(slider);
                changed = resp.changed();
                flush_now = resp.drag_stopped() || resp.lost_focus();
                value_chip(ui, &format_number(v, r.is_int));
            } else {
                let step = r.step.unwrap_or(1.0);
                ui.allocate_ui_with_layout(
                    vec2(control_width, 22.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                        if ui
                            .add_sized(
                                [24.0, 22.0],
                                egui::Button::new(RichText::new("−").size(14.0)),
                            )
                            .clicked()
                        {
                            v = r.clamp(v - step);
                            changed = true;
                        }

                        let mut dv =
                            egui::DragValue::new(&mut v).speed(if r.is_int { 1.0 } else { 0.1 });
                        if let (Some(lo), Some(hi)) = (r.min, r.max) {
                            dv = dv.range(lo..=hi);
                        } else if let Some(lo) = r.min {
                            dv = dv.range(lo..=f64::MAX);
                        } else if let Some(hi) = r.max {
                            dv = dv.range(f64::MIN..=hi);
                        }
                        let dv_width = (control_width - 56.0).max(60.0);
                        let resp = ui.add_sized([dv_width, 22.0], dv);
                        changed |= resp.changed();
                        flush_now |= resp.drag_stopped() || resp.lost_focus();

                        if ui
                            .add_sized(
                                [24.0, 22.0],
                                egui::Button::new(RichText::new("+").size(14.0)),
                            )
                            .clicked()
                        {
                            v = r.clamp(v + step);
                            changed = true;
                        }
                    },
                );
            }
        });

        if changed {
            let clamped = r.clamp(v);
            let value = if r.is_int { clamped.round() } else { clamped };
            shared.set_pending_param(&group.path, &r.name, true);
            shared.update(|s| {
                s.states
                    .entry(group.path.clone())
                    .or_default()
                    .insert(r.name.clone(), json_numeric(value, r.is_int));
            });
            self.pending.insert(
                key.clone(),
                PendingNumeric {
                    value,
                    changed_at: Instant::now(),
                    group: group.path.clone(),
                    action_path: set.action_path.clone(),
                    required_inputs: set.required_inputs.clone(),
                    is_int: r.is_int,
                },
            );
        }
        if flush_now {
            self.flush_key(&key, shared, poller);
        }
    }

    fn flush_key(&mut self, key: &(String, String), shared: &Shared, poller: &Poller) {
        if let Some(p) = self.pending.remove(key) {
            let value = json_numeric(p.value, p.is_int);
            poller.set_param(
                shared,
                &p.group,
                &p.action_path,
                &p.required_inputs,
                &key.1,
                value,
            );
        }
    }

    /// Send sliders that have been quiet for `SLIDER_DEBOUNCE`.
    fn flush_due(&mut self, ctx: &egui::Context, shared: &Shared, poller: &Poller) {
        let now = Instant::now();
        let due: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| now.duration_since(p.changed_at) >= SLIDER_DEBOUNCE)
            .map(|(k, _)| k.clone())
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
        let buf = self
            .string_edits
            .entry(key.clone())
            .or_insert_with(|| current.clone());
        let row_width = ui.available_width();
        let control_width = (row_width - LABEL_WIDTH - ROW_GAP).max(120.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(ROW_GAP, 0.0);
            control_label(ui, &prettify(name));
            let resp = ui.add_sized(
                [control_width, 24.0],
                egui::TextEdit::singleline(buf).hint_text("value"),
            );
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                poller.set_param(
                    shared,
                    &group.path,
                    &set.action_path,
                    &set.required_inputs,
                    name,
                    json!(buf.clone()),
                );
            } else if !resp.has_focus() && *buf != current {
                *buf = current.clone(); // follow server state when not editing
            }
        });
    }
}

fn readout_row(ui: &mut Ui, ro: &ReadoutSpec, state: Option<&StateMap>) {
    ui.add_space(1.0);
    ui.horizontal_wrapped(|ui| {
        if ro.fields.len() > 1 {
            ui.label(
                RichText::new(format!("{}:", ro.label))
                    .size(11.0)
                    .color(PALETTE.muted),
            );
        }
        for f in &ro.fields {
            let val_text = state
                .and_then(|s| s.get(&f.name))
                .map(|v| format_readout_value(v, f.kind))
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
                            RichText::new(prettify(&f.name))
                                .size(11.0)
                                .color(PALETTE.muted),
                        );
                        ui.label(
                            RichText::new(val_text)
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
    e: &EnumParam,
    state: Option<&StateMap>,
    shared: &Arc<Shared>,
    poller: &Poller,
) {
    let current = state.and_then(|s| s.get(&e.name));
    let row_width = ui.available_width();
    let control_width = (row_width - LABEL_WIDTH - ROW_GAP).max(120.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(ROW_GAP, 0.0);
        control_label(ui, &prettify(&e.name));

        if e.options.len() <= PILL_MAX_OPTIONS {
            ui.allocate_ui_with_layout(
                vec2(control_width, 24.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                    for opt in &e.options {
                        let selected = current == Some(opt);
                        let text = option_label(opt);
                        let btn =
                            egui::Button::new(RichText::new(text).size(11.5).color(if selected {
                                Color32::WHITE
                            } else {
                                PALETTE.on_surface
                            }))
                            .fill(if selected {
                                PALETTE.primary
                            } else {
                                PALETTE.surface_container_high
                            })
                            .stroke(if selected {
                                Stroke::NONE
                            } else {
                                Stroke::new(1.0, border_subtle())
                            })
                            .corner_radius(CornerRadius::same(255));
                        if ui.add(btn).clicked() && !selected {
                            poller.set_param(
                                shared,
                                &group.path,
                                &set.action_path,
                                &set.required_inputs,
                                &e.name,
                                opt.clone(),
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
                Layout::left_to_right(Align::Center),
                |ui| {
                    egui::ComboBox::from_id_salt((&set.action_path, &e.name))
                        .width(control_width)
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for opt in &e.options {
                                let selected = current == Some(opt);
                                if ui.selectable_label(selected, option_label(opt)).clicked()
                                    && !selected
                                {
                                    poller.set_param(
                                        shared,
                                        &group.path,
                                        &set.action_path,
                                        &set.required_inputs,
                                        &e.name,
                                        opt.clone(),
                                    );
                                }
                            }
                        });
                },
            );
        }
    });
}
