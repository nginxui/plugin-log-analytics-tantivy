//! What the indexer knows about the log files, kept in the commit payload of
//! the index so the file positions and the documents are always committed
//! together.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::schema::FORMAT_VERSION;

/// Rows of files that no longer exist kept per group. A compressed copy of a
/// rotated file still finds the position of its source there.
const KEEP_GONE: usize = 16;

/// The state of one log file.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct FileRow {
    pub path: String,
    /// Hash of the first line of the content, empty while it is unknown.
    pub fingerprint: String,
    /// Offset in the decompressed content after the last consumed line.
    pub position: u64,
    /// Size of the file when it was read.
    pub size: u64,
    /// Modification time of the file when it was read, in nanoseconds.
    pub mtime_ns: i64,
    /// Unix seconds of the last read.
    pub indexed_at: i64,
}

/// The state of one log group.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupState {
    pub files: Vec<FileRow>,
    /// Unix seconds when the last run of the group started.
    pub started_at: i64,
    /// Milliseconds the last run took.
    pub duration_ms: i64,
}

/// Everything the commit payload holds.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct IndexState {
    pub format: u32,
    /// A bulk import committed part of its work. Documents past the recorded
    /// positions may exist, so the next reads remove them first.
    pub dirty: bool,
    pub groups: BTreeMap<String, GroupState>,
}

impl Default for IndexState {
    fn default() -> Self {
        Self { format: FORMAT_VERSION, dirty: false, groups: BTreeMap::new() }
    }
}

impl IndexState {
    pub fn to_payload(&self) -> String {
        serde_json::to_string(self).expect("state serializes")
    }

    /// Reads a payload. `None` for a missing, malformed or foreign one.
    pub fn from_payload(payload: Option<&str>) -> Option<Self> {
        let state: IndexState = serde_json::from_str(payload?).ok()?;
        (state.format == FORMAT_VERSION).then_some(state)
    }

    pub fn group(&self, path: &str) -> Option<&GroupState> {
        self.groups.get(path)
    }

    /// Inserts or replaces the row of a file.
    pub fn set_row(&mut self, group: &str, row: FileRow) {
        let files = &mut self.groups.entry(group.to_owned()).or_default().files;
        match files.iter_mut().find(|f| f.path == row.path) {
            Some(existing) => *existing = row,
            None => files.push(row),
        }
    }

    /// Drops the oldest rows of files that are gone, beyond the ones kept.
    pub fn prune(&mut self, exists: impl Fn(&str) -> bool) {
        for group in self.groups.values_mut() {
            let mut gone: Vec<usize> =
                group.files.iter().enumerate().filter(|(_, f)| !exists(&f.path)).map(|(i, _)| i).collect();
            if gone.len() <= KEEP_GONE {
                continue;
            }
            gone.sort_by_key(|&i| group.files[i].indexed_at);
            let drop: Vec<usize> = gone[..gone.len() - KEEP_GONE].to_vec();
            let mut index = 0;
            group.files.retain(|_| {
                let keep = !drop.contains(&index);
                index += 1;
                keep
            });
        }
    }
}

/// The rows of a group as they were when a round started. After a rotation the
/// new file replaces the row of the old path, and the renamed file still has to
/// find it, so every file of the round reads this copy.
#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    by_path: HashMap<String, FileRow>,
    by_fp: HashMap<String, Vec<FileRow>>,
}

impl Snapshot {
    pub fn new(rows: &[FileRow]) -> Self {
        let mut snap = Snapshot::default();
        for row in rows {
            snap.by_path.insert(row.path.clone(), row.clone());
            if !row.fingerprint.is_empty() {
                snap.by_fp.entry(row.fingerprint.clone()).or_default().push(row.clone());
            }
        }
        snap
    }

    pub fn row(&self, path: &str) -> Option<&FileRow> {
        self.by_path.get(path)
    }

    /// The row of another path that holds the same content and got furthest.
    pub fn inherit(&self, fingerprint: &str, path: &str) -> Option<&FileRow> {
        self.by_fp.get(fingerprint)?.iter().filter(|r| r.path != path).max_by_key(|r| r.position)
    }

    /// The furthest position any file with this content has reached.
    pub fn high_water(&self, fingerprint: &str) -> u64 {
        self.by_fp.get(fingerprint).map_or(0, |rows| rows.iter().map(|r| r.position).max().unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, fp: &str, position: u64) -> FileRow {
        FileRow { path: path.into(), fingerprint: fp.into(), position, ..Default::default() }
    }

    #[test]
    fn payload_round_trips_and_rejects_foreign_ones() {
        let mut state = IndexState::default();
        state.set_row("/l/a.log", row("/l/a.log", "f1", 10));
        state.dirty = true;
        let back = IndexState::from_payload(Some(&state.to_payload())).unwrap();
        assert_eq!(back, state);
        assert!(IndexState::from_payload(None).is_none());
        assert!(IndexState::from_payload(Some("{}")).is_none());
        assert!(IndexState::from_payload(Some("not json")).is_none());
        let other = IndexState { format: FORMAT_VERSION + 1, ..Default::default() };
        assert!(IndexState::from_payload(Some(&other.to_payload())).is_none());
    }

    #[test]
    fn set_row_replaces_by_path() {
        let mut state = IndexState::default();
        state.set_row("g", row("a", "f1", 1));
        state.set_row("g", row("a", "f1", 5));
        state.set_row("g", row("b", "f1", 2));
        assert_eq!(state.group("g").unwrap().files.len(), 2);
        assert_eq!(state.group("g").unwrap().files[0].position, 5);
    }

    #[test]
    fn inherit_picks_the_furthest_other_path() {
        let snap = Snapshot::new(&[row("a", "f", 10), row("b", "f", 30), row("c", "g", 99)]);
        assert_eq!(snap.inherit("f", "a").unwrap().path, "b");
        assert_eq!(snap.inherit("f", "b").unwrap().path, "a");
        assert!(snap.inherit("g", "c").is_none());
        assert!(snap.inherit("none", "a").is_none());
        assert_eq!(snap.high_water("f"), 30);
        assert_eq!(snap.high_water("none"), 0);
    }

    #[test]
    fn prune_keeps_the_newest_rows_of_gone_files() {
        let mut state = IndexState::default();
        for i in 0..20 {
            let mut r = row(&format!("gone{i}"), "f", 1);
            r.indexed_at = i;
            state.set_row("g", r);
        }
        state.set_row("g", row("alive", "f", 1));
        state.prune(|p| p == "alive");
        let files = &state.group("g").unwrap().files;
        assert_eq!(files.len(), KEEP_GONE + 1);
        assert!(files.iter().any(|f| f.path == "alive"));
        assert!(files.iter().any(|f| f.path == "gone19"));
        assert!(!files.iter().any(|f| f.path == "gone0"));
    }
}
