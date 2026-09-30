//! Settings the host pushes and the layout of the data directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use nginxui_plugin_sdk::protocol::Settings as RawSettings;

pub const KEY_INTERVAL: &str = "incremental_index_interval";
pub const KEY_MAX_TASKS: &str = "max_concurrent_index_tasks";
pub const KEY_CUSTOM_MMDB: &str = "index_custom_mmdb";
pub const KEY_MAP_PATH: &str = "geo_map_path";

/// Minutes between two indexing rounds when the setting is empty.
pub const DEFAULT_INTERVAL_MINUTES: i64 = 15;

/// The plugin settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// Minutes between indexing rounds, zero or less for the default.
    pub interval_minutes: i64,
    /// Log groups indexed at the same time, zero or less picks a value.
    pub max_tasks: i64,
    /// A custom IP location database, relative to the geo folder when not absolute.
    pub custom_mmdb: String,
    /// Folder with the map outline files, relative to the data directory when not absolute.
    pub map_path: String,
}

fn int_of(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn text_of(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).map(|s| s.trim().to_owned()).unwrap_or_default()
}

impl Settings {
    /// Reads the map of the host. Numbers may arrive as numbers or strings and
    /// unknown keys are ignored.
    pub fn parse(raw: &RawSettings) -> Self {
        Settings {
            interval_minutes: int_of(raw.get(KEY_INTERVAL)),
            max_tasks: int_of(raw.get(KEY_MAX_TASKS)),
            custom_mmdb: text_of(raw.get(KEY_CUSTOM_MMDB)),
            map_path: text_of(raw.get(KEY_MAP_PATH)),
        }
    }

    pub fn interval(&self) -> Duration {
        let minutes = if self.interval_minutes <= 0 { DEFAULT_INTERVAL_MINUTES } else { self.interval_minutes };
        Duration::from_secs(minutes as u64 * 60)
    }

    /// Whether the locations come from a custom database.
    pub fn uses_custom_mmdb(&self) -> bool {
        !self.custom_mmdb.is_empty()
    }
}

/// Folder of the map files below the data directory when none is set.
pub const DEFAULT_MAP_DIR: &str = "maps";

/// Where the plugin keeps its files.
#[derive(Debug, Clone)]
pub struct Dirs {
    data: PathBuf,
}

impl Dirs {
    pub fn new(data: impl Into<PathBuf>) -> Self {
        Self { data: data.into() }
    }

    pub fn data(&self) -> &Path {
        &self.data
    }

    pub fn index(&self) -> PathBuf {
        self.data.join("index")
    }

    pub fn geolite(&self) -> PathBuf {
        self.data.join("geolite")
    }

    pub fn maps(&self, settings: &Settings) -> PathBuf {
        let base = if settings.map_path.is_empty() { DEFAULT_MAP_DIR } else { settings.map_path.as_str() };
        let path = Path::new(base);
        if path.is_absolute() {
            PathBuf::from(crate::logs::clean_path(base))
        } else {
            PathBuf::from(crate::logs::clean_path(&self.data.join(path).to_string_lossy()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(v: Value) -> RawSettings {
        v.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn settings_accept_numbers_and_strings() {
        let s = Settings::parse(&raw(json!({
            "incremental_index_interval": "30", "max_concurrent_index_tasks": 3.0,
            "index_custom_mmdb": " my.mmdb ", "geo_map_path": "", "unknown": 1
        })));
        assert_eq!(s.interval_minutes, 30);
        assert_eq!(s.max_tasks, 3);
        assert_eq!(s.custom_mmdb, "my.mmdb");
        assert!(s.uses_custom_mmdb());
        assert_eq!(s.interval(), Duration::from_secs(1800));
    }

    #[test]
    fn empty_interval_means_fifteen_minutes() {
        assert_eq!(Settings::default().interval(), Duration::from_secs(900));
        assert_eq!(Settings { interval_minutes: -5, ..Default::default() }.interval(), Duration::from_secs(900));
    }

    #[test]
    fn map_folder_is_relative_to_the_data_directory() {
        let dirs = Dirs::new("/data/p");
        assert_eq!(dirs.maps(&Settings::default()), Path::new("/data/p/maps"));
        let abs = Settings { map_path: "/srv/maps".into(), ..Default::default() };
        assert_eq!(dirs.maps(&abs), Path::new("/srv/maps"));
        let rel = Settings { map_path: "m/x".into(), ..Default::default() };
        assert_eq!(dirs.maps(&rel), Path::new("/data/p/m/x"));
        assert_eq!(dirs.index(), Path::new("/data/p/index"));
    }
}
