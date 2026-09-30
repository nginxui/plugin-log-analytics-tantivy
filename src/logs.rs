//! The nginx log files the host lists, which ones may be read and how rotated
//! files group under their live log.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use base64::Engine;
use regex::Regex;

/// Prefix of a query value that holds a base64url encoded path, which keeps a
/// web application firewall from reading it as a traversal attempt.
pub const ENCODED_PATH_PREFIX: &str = "b64_";

/// One log file the host lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLog {
    pub path: String,
    /// `access` or `error`.
    pub kind: String,
    /// `config` or `default`.
    pub source: String,
    pub config_file: String,
}

/// A log group: the listed live log and its rotated files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogGroup {
    /// The group key, the main log path of the listed file.
    pub path: String,
    pub kind: String,
    pub config_file: String,
}

#[derive(Default)]
struct Listing {
    entries: Vec<HostLog>,
    listed: HashSet<String>,
}

/// The current answer of the host, replaced as a whole.
#[derive(Default)]
pub struct HostLogs {
    current: RwLock<Arc<Listing>>,
}

/// Lexically cleans a path like Go's `filepath.Clean`.
pub fn clean_path(path: &str) -> String {
    let mut out: Vec<Component> = Vec::new();
    for c in Path::new(path).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(c),
            },
            other => out.push(other),
        }
    }
    if out.is_empty() {
        return ".".to_owned();
    }
    let mut buf = PathBuf::new();
    for c in out {
        buf.push(c.as_os_str());
    }
    buf.to_string_lossy().into_owned()
}

impl HostLogs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the list. Relative paths are dropped, the others are cleaned.
    pub fn set(&self, logs: Vec<HostLog>) {
        let mut next = Listing::default();
        for mut log in logs {
            if log.path.is_empty() || !Path::new(&log.path).is_absolute() {
                continue;
            }
            log.path = clean_path(&log.path);
            if !next.listed.insert(log.path.clone()) {
                continue;
            }
            next.entries.push(log);
        }
        *self.current.write().expect("host logs lock") = Arc::new(next);
    }

    fn snapshot(&self) -> Arc<Listing> {
        self.current.read().expect("host logs lock").clone()
    }

    pub fn list(&self) -> Vec<HostLog> {
        self.snapshot().entries.clone()
    }

    /// The default access log of nginx, empty when the host lists none.
    pub fn default_access_path(&self) -> String {
        self.snapshot()
            .entries
            .iter()
            .find(|l| l.kind == "access" && l.source == "default")
            .map(|l| l.path.clone())
            .unwrap_or_default()
    }

    /// The listed log a path is, or belongs to as a rotated file.
    fn listed_base(&self, path: &str) -> Option<String> {
        let current = self.snapshot();
        if current.listed.contains(path) {
            return Some(path.to_owned());
        }
        let main = main_log_path(path);
        (main != path && current.listed.contains(&main)).then_some(main)
    }

    /// The only gate for reading log files. A path is valid when the host lists
    /// it or a file it is a rotation of, and it is a regular file or a link to
    /// one. A path that does not exist is accepted, since the file may come
    /// later and indexed data outlives it.
    pub fn is_valid_path(&self, path: &str) -> bool {
        if path.is_empty() {
            return false;
        }
        let path = clean_path(path);
        if !Path::new(&path).is_absolute() {
            return false;
        }
        let Some(base) = self.listed_base(&path) else { return false };

        let Ok(meta) = std::fs::symlink_metadata(&path) else { return true };
        if !meta.file_type().is_symlink() {
            return meta.is_file();
        }
        let Ok(resolved) = std::fs::canonicalize(&path) else { return false };
        if path != base && !same_dir(&resolved, Path::new(&base)) {
            return false;
        }
        std::fs::metadata(&resolved).map(|m| m.is_file()).unwrap_or(false)
    }

    /// The groups of the listed logs, one per main log path.
    pub fn groups(&self) -> Vec<LogGroup> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for log in &self.snapshot().entries {
            let key = main_log_path(&log.path);
            if seen.insert(key.clone()) {
                out.push(LogGroup { path: key, kind: log.kind.clone(), config_file: log.config_file.clone() });
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// The group of a path, when it is one of the listed groups.
    pub fn group_of(&self, path: &str) -> Option<LogGroup> {
        let key = main_log_path(&clean_path(path));
        self.groups().into_iter().find(|g| g.path == key)
    }

    /// The files of a group: the base path and its rotated files that exist and
    /// may be read.
    pub fn group_files(&self, base: &str) -> Vec<PathBuf> {
        let base_path = Path::new(base);
        let (Some(dir), Some(name)) = (base_path.parent(), base_path.file_name()) else {
            return Vec::new();
        };
        let prefix = name.to_string_lossy().into_owned();
        let mut names: Vec<PathBuf> = vec![base_path.to_path_buf()];
        if let Ok(read) = std::fs::read_dir(dir) {
            for entry in read.flatten() {
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    names.push(entry.path());
                }
            }
        }
        names.sort();
        names.dedup();
        names
            .into_iter()
            .filter(|p| {
                let s = p.to_string_lossy();
                self.is_valid_path(&s) && std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
            })
            .collect()
    }
}

fn same_dir(resolved: &Path, base: &Path) -> bool {
    let base_dir =
        base.parent().and_then(|d| std::fs::canonicalize(d).ok()).or_else(|| base.parent().map(Path::to_path_buf));
    resolved.parent().map(Path::to_path_buf) == base_dir
}

struct RotationRules {
    dash_date: Regex,
    dot_date: Regex,
    numbered: Regex,
    middle_numbered: Regex,
    multi_part_date: Regex,
    full_dates: [Regex; 3],
}

fn rules() -> &'static RotationRules {
    static RULES: OnceLock<RotationRules> = OnceLock::new();
    RULES.get_or_init(|| {
        let re = |p: &str| Regex::new(p).expect("rotation pattern");
        RotationRules {
            dash_date: re(r"^(.+\..+)-(\d{8}|\d{4}-\d{2}-\d{2})$"),
            dot_date: re(r"^\d{4}\.\d{2}\.\d{2}$"),
            numbered: re(r"^(.+)\.(\d{1,3})$"),
            middle_numbered: re(r"^(.+)\.(\d{1,3})\.log$"),
            multi_part_date: re(r"^2\d{3}\.\d{2}\.\d{2}$"),
            full_dates: [re(r"^\d{8}$"), re(r"^\d{4}-\d{2}-\d{2}$"), re(r"^\d{6}$")],
        }
    })
}

fn is_full_date(s: &str) -> bool {
    rules().full_dates.iter().any(|r| r.is_match(s))
}

fn join(dir: &Path, name: &str) -> String {
    clean_path(&dir.join(name).to_string_lossy())
}

/// Collapses rotated and compressed files (`access.log.1`, `access.log.2.gz`,
/// `access.log-20231201`, ...) onto the live log of their group. It is the
/// same function as in the Go plugin, since the value is stored with every
/// document.
pub fn main_log_path(file_path: &str) -> String {
    let path = Path::new(file_path);
    let dir = path.parent().unwrap_or(Path::new(""));
    let Some(name) = path.file_name() else { return file_path.to_owned() };
    let mut filename = name.to_string_lossy().into_owned();
    for ext in [".gz", ".bz2", ".xz", ".lz4"] {
        if let Some(stripped) = filename.strip_suffix(ext) {
            filename = stripped.to_owned();
        }
    }
    let r = rules();

    if let Some(c) = r.dash_date.captures(&filename) {
        return join(dir, &c[1]);
    }

    let parts: Vec<&str> = filename.split('.').collect();
    if parts.len() >= 3 {
        if parts.len() >= 4 {
            let last_three = parts[parts.len() - 3..].join(".");
            if r.dot_date.is_match(&last_three) {
                return join(dir, &parts[..parts.len() - 3].join("."));
            }
        }
        if is_full_date(parts[parts.len() - 1]) {
            return join(dir, &parts[..parts.len() - 1].join("."));
        }
    }

    if let Some(c) = r.numbered.captures(&filename) {
        return join(dir, &c[1]);
    }
    if let Some(c) = r.middle_numbered.captures(&filename) {
        return join(dir, &format!("{}.log", &c[1]));
    }
    if is_full_date(&filename) || r.multi_part_date.is_match(&filename) {
        return join(dir, "access.log");
    }
    file_path.to_owned()
}

/// The real path behind a query value, which may be base64url encoded. A value
/// is taken as encoded only when it has the prefix, decodes, and holds valid
/// UTF-8 without NUL. Any other value comes back unchanged.
pub fn decode_path_param(value: &str) -> String {
    let Some(rest) = value.strip_prefix(ENCODED_PATH_PREFIX).filter(|r| !r.is_empty()) else {
        return value.to_owned();
    };
    let engine = &base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let decoded = engine.decode(rest).or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(rest));
    match decoded {
        Ok(bytes) if !bytes.contains(&0) => String::from_utf8(bytes).unwrap_or_else(|_| value.to_owned()),
        _ => value.to_owned(),
    }
}

/// Encodes a path the way [`decode_path_param`] reads it.
pub fn encode_path_param(path: &str) -> String {
    format!("{ENCODED_PATH_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(paths: &[&str]) -> HostLogs {
        let logs = HostLogs::new();
        logs.set(
            paths
                .iter()
                .map(|p| HostLog {
                    path: (*p).to_owned(),
                    kind: "access".into(),
                    source: "config".into(),
                    config_file: String::new(),
                })
                .collect(),
        );
        logs
    }

    #[test]
    fn rotations_collapse_onto_the_live_log() {
        for (file, want) in [
            ("/var/log/nginx/access.log", "/var/log/nginx/access.log"),
            ("/var/log/nginx/access.log.1", "/var/log/nginx/access.log"),
            ("/var/log/nginx/access.log.2.gz", "/var/log/nginx/access.log"),
            ("/var/log/nginx/access.log-20231201", "/var/log/nginx/access.log"),
            ("/var/log/nginx/access.log-20231201.gz", "/var/log/nginx/access.log"),
            ("/var/log/nginx/access.log-2023-12-01", "/var/log/nginx/access.log"),
            ("/var/log/nginx/access.log.20231201", "/var/log/nginx/access.log"),
            ("/var/log/nginx/access.log.2023.12.01", "/var/log/nginx/access.log"),
            ("/var/log/nginx/site.1.log", "/var/log/nginx/site.log"),
            ("/var/log/nginx/site-2024", "/var/log/nginx/site-2024"),
            ("/var/log/nginx/error.log", "/var/log/nginx/error.log"),
        ] {
            assert_eq!(main_log_path(file), want, "{file}");
        }
    }

    #[test]
    fn only_listed_logs_and_their_rotations_are_valid() {
        let logs = listing(&["/var/log/nginx/access.log", "relative.log"]);
        assert!(logs.is_valid_path("/var/log/nginx/access.log"));
        assert!(logs.is_valid_path("/var/log/nginx/access.log.3.gz"));
        assert!(logs.is_valid_path("/var/log/nginx/../nginx/access.log"));
        assert!(!logs.is_valid_path("/var/log/nginx/other.log"));
        assert!(!logs.is_valid_path("/etc/passwd"));
        assert!(!logs.is_valid_path("relative.log"));
        assert!(!logs.is_valid_path(""));
    }

    #[test]
    fn group_files_lists_the_rotations_that_exist() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("access.log");
        for name in ["access.log", "access.log.1", "access.log.2.gz", "access.log.bak", "other.log"] {
            std::fs::write(dir.path().join(name), "x\n").unwrap();
        }
        let logs = listing(&[base.to_str().unwrap()]);
        let names: Vec<String> = logs
            .group_files(base.to_str().unwrap())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["access.log", "access.log.1", "access.log.2.gz"]);
    }

    #[test]
    fn path_parameters_round_trip_and_leave_plain_values_alone() {
        let path = "/var/log/nginx/access.log";
        assert_eq!(decode_path_param(&encode_path_param(path)), path);
        assert_eq!(decode_path_param(path), path);
        assert_eq!(decode_path_param("defaultsite"), "defaultsite");
        assert_eq!(decode_path_param("b64_"), "b64_");
        assert_eq!(decode_path_param("b64_!!"), "b64_!!");
    }

    #[test]
    fn clean_path_resolves_dots() {
        assert_eq!(clean_path("/a/b/../c/./d"), "/a/c/d");
        assert_eq!(clean_path("/a//b/"), "/a/b");
        assert_eq!(clean_path("/.."), "/");
    }

    #[test]
    fn groups_are_keyed_by_main_path() {
        let logs = listing(&["/v/a.log", "/v/a.log.1", "/v/b.log"]);
        let keys: Vec<String> = logs.groups().into_iter().map(|g| g.path).collect();
        assert_eq!(keys, ["/v/a.log", "/v/b.log"]);
    }
}
