//! Disk cache: last schema + last-confirmed group states, and PNG icons.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::api::Schema;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheData {
    pub schema: Option<Schema>,
    /// Last server-confirmed outputs, keyed by device-group path.
    pub group_states: BTreeMap<String, Map<String, Value>>,
}

pub fn cache_path(dir: &Path) -> PathBuf {
    dir.join("cache.json")
}

pub fn icons_dir(dir: &Path) -> PathBuf {
    dir.join("icons")
}

/// Filesystem-safe PNG path for an icon key (`office/fan` -> `office_fan.png`).
pub fn icon_file(dir: &Path, key: &str) -> PathBuf {
    let sanitized: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = if sanitized.is_empty() {
        "_root".to_owned()
    } else {
        sanitized
    };
    icons_dir(dir).join(format!("{sanitized}.png"))
}

impl CacheData {
    /// Corrupt or missing cache files yield an empty cache rather than an error.
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(cache_path(dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Atomic write: temp file + rename.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string(self).map_err(std::io::Error::other)?;
        let path = cache_path(dir);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)
    }

    /// Drop cached state for groups a *fresh* schema no longer contains.
    pub fn prune_to(&mut self, fresh: &Schema) {
        let live: HashSet<&str> = fresh.groups.iter().map(|(k, _)| k.as_str()).collect();
        self.group_states.retain(|k, _| live.contains(k.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CacheData {
        let schema = Schema::from_value(
            &serde_json::json!({"groups": {"b": {"actions": {"x": {}}}, "a": {"actions": {}}}}),
        )
        .unwrap();
        let mut c = CacheData {
            schema: Some(schema),
            ..Default::default()
        };
        c.group_states.insert(
            "b".into(),
            serde_json::json!({"power": true})
                .as_object()
                .unwrap()
                .clone(),
        );
        c.group_states.insert("gone".into(), Map::new());
        c
    }

    #[test]
    fn round_trips_and_keeps_schema_order() {
        let dir = tempfile::tempdir().unwrap();
        let c = sample();
        c.save(dir.path()).unwrap();
        let back = CacheData::load(dir.path());
        assert_eq!(back, c);
        assert_eq!(back.schema.unwrap().groups[0].0, "b");
    }

    #[test]
    fn corrupt_cache_falls_back_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(cache_path(dir.path()), "garbage").unwrap();
        assert_eq!(CacheData::load(dir.path()), CacheData::default());
        assert_eq!(
            CacheData::load(&dir.path().join("missing")),
            CacheData::default()
        );
    }

    #[test]
    fn prunes_only_groups_missing_from_fresh_schema() {
        let mut c = sample();
        let fresh = c.schema.clone().unwrap();
        c.prune_to(&fresh);
        assert!(c.group_states.contains_key("b"));
        assert!(!c.group_states.contains_key("gone"));
    }

    #[test]
    fn icon_names_are_sanitized() {
        let p = icon_file(Path::new("base"), "office/fan set");
        assert_eq!(p.file_name().unwrap(), "office_fan_set.png");
        assert_eq!(
            icon_file(Path::new("base"), "").file_name().unwrap(),
            "_root.png"
        );
        assert!(
            !icon_file(Path::new("base"), "../../x")
                .to_string_lossy()
                .contains("..")
        );
    }
}
