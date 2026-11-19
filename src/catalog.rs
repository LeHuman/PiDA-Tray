//! Schema -> device cards. Mirrors the Android client's `ControlCatalog`, plus
//! enum controls, read-only state readouts, unbounded numeric steppers, and sub-group merging.

use crate::api::{ParamKind, Schema, action_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Light,
    Fan,
    Monitor,
    Computer,
    Power,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RangeParam {
    pub name: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub is_int: bool,
}

impl RangeParam {
    /// Whether both bounds exist for a bounded slider; otherwise use a numeric stepper.
    pub fn has_slider(&self) -> bool {
        self.min.is_some() && self.max.is_some()
    }

    pub fn clamp(&self, x: f64) -> f64 {
        let x = self.min.map_or(x, |m| x.max(m));
        self.max.map_or(x, |m| x.min(m))
    }

    pub fn default_value(&self) -> f64 {
        match (self.min, self.max) {
            (Some(lo), _) => lo,
            (None, Some(hi)) => hi.min(0.0),
            (None, None) => 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumParam {
    pub name: String,
    pub options: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetActionSpec {
    /// Endpoint path used for both `GET` readback and `POST` dispatch.
    pub action_path: String,
    pub toggle_param: Option<String>,
    pub range_params: Vec<RangeParam>,
    pub enum_params: Vec<EnumParam>,
    pub string_params: Vec<String>,
    /// Inputs marked `required: true` that must accompany partial-delta POSTs when known.
    pub required_inputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadoutField {
    pub name: String,
    pub kind: ParamKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadoutSpec {
    /// Endpoint path polled via `GET` (never used for legacy no-param actions).
    pub action_path: String,
    pub label: String,
    pub fields: Vec<ReadoutField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyActionSpec {
    pub action_path: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceGroup {
    /// Schema group key; also the key for cached state and icons.
    pub path: String,
    pub title: String,
    /// Parent namespace when nested (e.g. `"Living Room"` for `"living_room/lamp"`).
    pub subtitle: Option<String>,
    pub icon: IconKind,
    pub set: Option<SetActionSpec>,
    pub readouts: Vec<ReadoutSpec>,
    pub legacy: Vec<LegacyActionSpec>,
}

impl DeviceGroup {
    /// Endpoint paths to try for `GET /api/icon/<path>`. The server mirrors an
    /// `icons/` tree per *endpoint* (`icons/<group>/<action>.png`), never per group,
    /// so try the set action, readouts, then each legacy action.
    pub fn icon_candidates(&self) -> Vec<String> {
        self.set
            .iter()
            .map(|s| s.action_path.clone())
            .chain(self.readouts.iter().map(|r| r.action_path.clone()))
            .chain(self.legacy.iter().map(|l| l.action_path.clone()))
            .collect()
    }
}

/// `"night_light"` -> `"Night Light"`.
pub fn prettify(name: &str) -> String {
    name.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f
                    .to_uppercase()
                    .chain(c.flat_map(char::to_lowercase))
                    .collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub fn infer_icon(leaf_name: &str) -> IconKind {
    let l = leaf_name.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| l.contains(n));
    if has(&["light", "lamp"]) {
        IconKind::Light
    } else if has(&["fan"]) {
        IconKind::Fan
    } else if has(&["monitor", "tv"]) {
        IconKind::Monitor
    } else if has(&["computer", "pc"]) {
        IconKind::Computer
    } else {
        IconKind::Power
    }
}

fn group_title_and_subtitle(path: &str) -> (String, Option<String>) {
    if path.is_empty() {
        return ("General".into(), None);
    }
    match path.rsplit_once('/') {
        Some((parent, leaf_name)) => {
            let sub = parent
                .split('/')
                .filter(|s| !s.is_empty())
                .map(prettify)
                .collect::<Vec<_>>()
                .join(" · ");
            (prettify(leaf_name), (!sub.is_empty()).then_some(sub))
        }
        None => (prettify(path), None),
    }
}

pub fn build_catalog(schema: &Schema) -> Vec<DeviceGroup> {
    let mut groups: Vec<DeviceGroup> = Vec::new();

    for (path, g) in &schema.groups {
        if g.actions.is_empty() {
            continue;
        }
        let mut set: Option<SetActionSpec> = None;
        let mut readouts = Vec::new();
        let mut legacy = Vec::new();

        for (name, action) in &g.actions {
            let ap = action_path(path, name);
            if set.is_none() && !action.params.is_empty() && !action.inputs.is_empty() {
                let mut spec = SetActionSpec {
                    action_path: ap,
                    toggle_param: None,
                    range_params: Vec::new(),
                    enum_params: Vec::new(),
                    string_params: Vec::new(),
                    required_inputs: Vec::new(),
                };
                // Walk `inputs` (declaration order), looking each one up in `params`.
                for input in &action.inputs {
                    let Some(p) = action.param(input) else {
                        continue;
                    };
                    if p.required {
                        spec.required_inputs.push(input.clone());
                    }
                    match p.kind {
                        ParamKind::Bool => {
                            if spec.toggle_param.is_none() {
                                spec.toggle_param = Some(input.clone());
                            }
                        }
                        ParamKind::Int | ParamKind::Float => {
                            spec.range_params.push(RangeParam {
                                name: input.clone(),
                                min: p.min,
                                max: p.max,
                                step: p.step,
                                is_int: p.kind == ParamKind::Int,
                            });
                        }
                        ParamKind::Enum if !p.values.is_empty() => {
                            spec.enum_params.push(EnumParam {
                                name: input.clone(),
                                options: p.values.clone(),
                            })
                        }
                        ParamKind::Enum => {}
                        ParamKind::String => spec.string_params.push(input.clone()),
                    }
                }
                set = Some(spec);
            } else if !action.params.is_empty() && action.inputs.is_empty() {
                // Read-only state endpoint (`params` declared, no `inputs`).
                let field_names: Vec<&str> = if action.outputs.is_empty() {
                    action.params.iter().map(|(k, _)| k.as_str()).collect()
                } else {
                    action.outputs.iter().map(String::as_str).collect()
                };
                let fields = field_names
                    .into_iter()
                    .filter_map(|fname| {
                        action.param(fname).map(|p| ReadoutField {
                            name: fname.to_owned(),
                            kind: p.kind,
                        })
                    })
                    .collect::<Vec<_>>();
                if !fields.is_empty() {
                    readouts.push(ReadoutSpec {
                        action_path: ap,
                        label: prettify(name),
                        fields,
                    });
                }
            } else if action.params.is_empty() {
                legacy.push(LegacyActionSpec {
                    action_path: ap,
                    label: prettify(name),
                });
            }
        }

        if set.is_none() && readouts.is_empty() && legacy.is_empty() {
            continue;
        }
        let (title, subtitle) = group_title_and_subtitle(path);
        groups.push(DeviceGroup {
            path: path.clone(),
            title,
            subtitle,
            icon: infer_icon(leaf(path)),
            set,
            readouts,
            legacy,
        });
    }

    merge_legacy_subgroups(groups)
}

/// Legacy-only sub-groups (e.g. `room/light/cycle`) fold into the longest-prefix
/// parent group that has a set or readout action (`room/light`), labelled `"<Sub> · <Action>"`.
fn merge_legacy_subgroups(groups: Vec<DeviceGroup>) -> Vec<DeviceGroup> {
    let parent_of = |child: &DeviceGroup| -> Option<usize> {
        if child.set.is_some() || !child.readouts.is_empty() || child.path.is_empty() {
            return None;
        }
        groups
            .iter()
            .enumerate()
            .filter(|(_, g)| {
                (g.set.is_some() || !g.readouts.is_empty())
                    && !g.path.is_empty()
                    && child.path.len() > g.path.len()
                    && child.path.starts_with(g.path.as_str())
                    && child.path.as_bytes()[g.path.len()] == b'/'
            })
            .max_by_key(|(_, g)| g.path.len())
            .map(|(i, _)| i)
    };

    let parents: Vec<Option<usize>> = groups.iter().map(parent_of).collect();
    let mut extra: Vec<Vec<LegacyActionSpec>> = vec![Vec::new(); groups.len()];
    for (i, g) in groups.iter().enumerate() {
        if let Some(p) = parents[i] {
            let sub = prettify(leaf(&g.path));
            extra[p].extend(g.legacy.iter().map(|l| LegacyActionSpec {
                action_path: l.action_path.clone(),
                label: format!("{sub} · {}", l.label),
            }));
        }
    }

    groups
        .into_iter()
        .enumerate()
        .filter(|(i, _)| parents[*i].is_none())
        .map(|(i, mut g)| {
            g.legacy.append(&mut extra[i]);
            g
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(json: &str) -> Schema {
        Schema::from_value(&serde_json::from_str(json).unwrap()).unwrap()
    }

    #[test]
    fn prettifies_names() {
        assert_eq!(prettify("night_light"), "Night Light");
        assert_eq!(prettify("FAN"), "Fan");
        assert_eq!(prettify("__a__b_"), "A B");
    }

    #[test]
    fn infers_icons() {
        assert_eq!(infer_icon("desk_lamp"), IconKind::Light);
        assert_eq!(infer_icon("ceiling_fan"), IconKind::Fan);
        assert_eq!(infer_icon("tv"), IconKind::Monitor);
        assert_eq!(infer_icon("gaming_pc"), IconKind::Computer);
        assert_eq!(infer_icon("heater"), IconKind::Power);
    }

    #[test]
    fn builds_set_spec_in_declaration_order() {
        let s = schema(
            r#"{"groups": {"living_room/lamp": {"actions": {"set": {
            "params": {
              "level": {"type": "int", "range": [0, 10]},
              "power": {"type": "bool", "required": true},
              "other": {"type": "bool"},
              "temp": {"type": "float", "min": 1.0, "max": 2.0},
              "mode": {"type": "enum", "enum": ["a", "b"]},
              "label": {"type": "string"}
            },
            "inputs": ["level", "power", "other", "temp", "mode", "label"]
        }}}}}"#,
        );
        let cat = build_catalog(&s);
        assert_eq!(cat.len(), 1);
        let g = &cat[0];
        assert_eq!(
            (g.title.as_str(), g.subtitle.as_deref(), g.icon),
            ("Lamp", Some("Living Room"), IconKind::Light)
        );
        let set = g.set.as_ref().unwrap();
        assert_eq!(set.action_path, "living_room/lamp/set");
        assert_eq!(set.toggle_param.as_deref(), Some("power"));
        assert_eq!(set.required_inputs, ["power"]);
        let ranges: Vec<_> = set.range_params.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(ranges, ["level", "temp"]);
        assert!(set.range_params[0].is_int && set.range_params[0].has_slider());
        assert!(!set.range_params[1].is_int && set.range_params[1].has_slider());
        assert_eq!(set.enum_params[0].options.len(), 2);
        assert_eq!(set.string_params, ["label"]);
    }

    #[test]
    fn keeps_read_only_state_actions_as_readouts() {
        let s = schema(
            r#"{"groups": {"office/sensor": {"actions": {"status": {
            "params": {
              "temp": {"type": "float"},
              "active": {"type": "bool"}
            },
            "outputs": ["temp", "active"]
        }}}}}"#,
        );
        let cat = build_catalog(&s);
        assert_eq!(cat.len(), 1);
        assert!(cat[0].set.is_none());
        assert_eq!(cat[0].readouts.len(), 1);
        assert_eq!(cat[0].readouts[0].action_path, "office/sensor/status");
        assert_eq!(cat[0].readouts[0].fields.len(), 2);
        assert_eq!(cat[0].icon_candidates(), ["office/sensor/status"]);
    }

    #[test]
    fn keeps_numeric_params_without_both_bounds_as_steppers() {
        let s = schema(
            r#"{"groups": {"a/b": {"actions": {"set": {
            "params": {
              "unbounded": {"type": "int"},
              "min_only": {"type": "float", "min": 5.0}
            },
            "inputs": ["unbounded", "min_only"]
        }}}}}"#,
        );
        let cat = build_catalog(&s);
        let ranges = &cat[0].set.as_ref().unwrap().range_params;
        assert_eq!(ranges.len(), 2);
        assert!(!ranges[0].has_slider());
        assert_eq!(ranges[0].clamp(-99.0), -99.0);
        assert!(!ranges[1].has_slider());
        assert_eq!(ranges[1].clamp(1.0), 5.0);
    }

    #[test]
    fn legacy_only_group_stays_standalone() {
        let cat = build_catalog(&schema(
            r#"{"groups": {"office/fan": {"actions": {"boost_mode": {}}}}}"#,
        ));
        assert_eq!(cat.len(), 1);
        assert!(cat[0].set.is_none());
        assert_eq!(cat[0].legacy[0].label, "Boost Mode");
        assert_eq!(cat[0].legacy[0].action_path, "office/fan/boost_mode");
    }

    #[test]
    fn merges_legacy_subgroups_into_longest_prefix_parent() {
        let s = schema(
            r#"{"groups": {
          "room": {"actions": {"set": {"params": {"power": {"type": "bool"}}, "inputs": ["power"]}}},
          "room/light": {"actions": {"set": {"params": {"power": {"type": "bool"}}, "inputs": ["power"]}}},
          "room/light/cycle": {"actions": {"next": {}, "prev": {}}},
          "roomy/other": {"actions": {"go": {}}}
        }}"#,
        );
        let cat = build_catalog(&s);
        let paths: Vec<_> = cat.iter().map(|g| g.path.as_str()).collect();
        assert_eq!(paths, ["room", "room/light", "roomy/other"]);
        let light = &cat[1];
        let labels: Vec<_> = light.legacy.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["Cycle · Next", "Cycle · Prev"]);
        assert_eq!(light.legacy[0].action_path, "room/light/cycle/next");
        assert!(cat[0].legacy.is_empty());
    }

    #[test]
    fn top_level_group_is_general_and_never_a_parent() {
        let s =
            schema(r#"{"groups": {"": {"actions": {"ping": {}}}, "a": {"actions": {"x": {}}}}}"#);
        let cat = build_catalog(&s);
        assert_eq!(cat[0].title, "General");
        assert!(cat[0].subtitle.is_none());
        assert_eq!(cat[0].legacy[0].action_path, "ping");
        assert_eq!(cat.len(), 2);
    }

    #[test]
    fn icon_candidates_are_endpoint_paths_set_first() {
        let s = schema(
            r#"{"groups": {"office/fan": {"actions": {
            "boost": {},
            "set": {"params": {"power": {"type": "bool"}}, "inputs": ["power"]}
        }}}}"#,
        );
        let cat = build_catalog(&s);
        assert_eq!(
            cat[0].icon_candidates(),
            ["office/fan/set", "office/fan/boost"]
        );
    }
}
