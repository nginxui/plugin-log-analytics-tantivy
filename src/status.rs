//! The index state of the log groups and the preflight check, as the pages
//! read them.

use std::path::Path;

use serde::Serialize;

use crate::engine::{Engine, Phase};
use crate::logs::{clean_path, decode_path_param};

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct TimeRange {
    pub start: i64,
    pub end: i64,
}

/// The index state of one log group. Every field the pages read is always
/// present, zero when there is nothing to say.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LogStatusItem {
    pub path: String,
    pub main_log_path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub config_file: String,
    pub index_status: String,
    pub last_modified: i64,
    pub last_size: u64,
    pub last_indexed: i64,
    pub index_start_time: i64,
    pub index_duration: i64,
    pub is_compressed: bool,
    pub has_timerange: bool,
    pub timerange_start: i64,
    pub timerange_end: i64,
    pub timerange: TimeRange,
    pub document_count: u64,
    pub error_message: String,
    pub error_time: i64,
    pub retry_count: u32,
    pub queue_position: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct LogListSummary {
    pub total_files: usize,
    pub indexed_files: usize,
    pub indexing_files: usize,
    pub document_count: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LogStatusResponse {
    pub items: Vec<LogStatusItem>,
    pub summary: LogListSummary,
}

/// Filters of the status request.
#[derive(Debug, Default, Clone)]
pub struct StatusFilter {
    pub kind: String,
    pub name: String,
    pub path: String,
    pub indexed: String,
}

const NOT_INDEXED: &str = "not_indexed";
const QUEUED: &str = "queued";
const INDEXING: &str = "indexing";
const INDEXED: &str = "indexed";
const ERROR: &str = "error";

fn item(engine: &Engine, group: &crate::logs::LogGroup) -> LogStatusItem {
    let state = engine.state();
    let saved = state.group(&group.path);
    let rows = saved.map(|g| g.files.as_slice()).unwrap_or(&[]);
    let last_indexed = rows.iter().map(|r| r.indexed_at).max().unwrap_or(0);
    let is_access = group.kind == "access";

    let mut it = LogStatusItem {
        path: group.path.clone(),
        main_log_path: group.path.clone(),
        kind: group.kind.clone(),
        name: Path::new(&group.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        config_file: group.config_file.clone(),
        index_status: NOT_INDEXED.to_owned(),
        last_modified: rows.iter().map(|r| r.mtime_ns / 1_000_000_000).max().unwrap_or(0),
        last_size: rows.iter().map(|r| r.size).sum(),
        last_indexed,
        index_start_time: saved.map_or(0, |g| g.started_at),
        index_duration: saved.map_or(0, |g| g.duration_ms),
        is_compressed: false,
        has_timerange: false,
        timerange_start: 0,
        timerange_end: 0,
        timerange: TimeRange::default(),
        document_count: 0,
        error_message: String::new(),
        error_time: 0,
        retry_count: 0,
        queue_position: 0,
    };
    if !is_access {
        return it;
    }

    let stats = engine.group_stats(&group.path);
    it.document_count = stats.docs;
    if let Some((start, end)) = stats.range {
        it.has_timerange = true;
        it.timerange_start = start;
        it.timerange_end = end;
        it.timerange = TimeRange { start, end };
    }
    match engine.phase(&group.path) {
        Some(Phase::Queued(position)) => {
            it.index_status = QUEUED.to_owned();
            it.queue_position = position;
        }
        Some(Phase::Indexing) => it.index_status = INDEXING.to_owned(),
        Some(Phase::Failed { message, at, retries }) => {
            it.index_status = ERROR.to_owned();
            it.error_message = message;
            it.error_time = at;
            it.retry_count = retries;
        }
        None if last_indexed > 0 => it.index_status = INDEXED.to_owned(),
        None => {}
    }
    it
}

/// The state of every listed log group, sorted by path, with the totals.
pub fn log_status(engine: &Engine, filter: &StatusFilter) -> LogStatusResponse {
    let wanted_path = decode_path_param(&filter.path);
    let mut items: Vec<LogStatusItem> = engine
        .hostlogs
        .groups()
        .iter()
        .map(|g| item(engine, g))
        .filter(|i| filter.kind.is_empty() || i.kind == filter.kind)
        .filter(|i| filter.name.is_empty() || i.name.contains(&filter.name))
        .filter(|i| wanted_path.is_empty() || i.path.contains(&wanted_path))
        .filter(|i| match filter.indexed.as_str() {
            "true" => i.index_status == INDEXED,
            "false" => i.index_status == NOT_INDEXED,
            "indexing" => i.index_status == INDEXING,
            _ => true,
        })
        .collect();
    items.sort_by(|a, b| a.path.cmp(&b.path));

    let mut summary = LogListSummary { total_files: items.len(), ..Default::default() };
    let mut stored = 0u64;
    for i in &items {
        stored += i.document_count;
        match i.index_status.as_str() {
            INDEXED => summary.indexed_files += 1,
            INDEXING => summary.indexing_files += 1,
            _ => {}
        }
    }
    summary.document_count = engine.total_docs().max(if summary.indexed_files > 0 { stored } else { 0 });
    LogStatusResponse { items, summary }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileInfo {
    pub exists: bool,
    pub readable: bool,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub size: u64,
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub last_modified: i64,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PreflightResponse {
    pub available: bool,
    pub index_status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_range: Option<TimeRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_info: Option<FileInfo>,
}

fn missing_file() -> FileInfo {
    FileInfo { exists: false, readable: false, size: 0, last_modified: 0 }
}

/// Tells whether the index of a log is ready to be searched. An empty path
/// means the default access log.
pub fn preflight(engine: &Engine, log_path: &str) -> PreflightResponse {
    let mut path = decode_path_param(log_path);
    if path.is_empty() {
        path = engine.hostlogs.default_access_path();
    }

    let indexed_group = |path: &str| {
        let group = engine.hostlogs.group_of(path)?;
        let last = engine.state().group(&group.path)?.files.iter().map(|r| r.indexed_at).max().unwrap_or(0);
        (last > 0).then(|| {
            let range = engine.group_stats(&group.path).range.map(|(start, end)| TimeRange { start, end });
            (group, range)
        })
    };

    if path.is_empty() {
        return PreflightResponse {
            available: true,
            index_status: NOT_INDEXED.to_owned(),
            message: String::new(),
            time_range: None,
            file_info: None,
        };
    }
    if !engine.hostlogs.is_valid_path(&path) {
        return PreflightResponse {
            available: false,
            index_status: ERROR.to_owned(),
            message: format!("Invalid log path: {path}"),
            time_range: None,
            file_info: Some(missing_file()),
        };
    }

    let cleaned = clean_path(&path);
    match std::fs::metadata(&cleaned) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match indexed_group(&cleaned) {
            Some((_, range)) => PreflightResponse {
                available: true,
                index_status: INDEXED.to_owned(),
                message: "File indexed (historical data available)".to_owned(),
                time_range: range,
                file_info: Some(missing_file()),
            },
            None => PreflightResponse {
                available: false,
                index_status: NOT_INDEXED.to_owned(),
                message: "Log file does not exist".to_owned(),
                time_range: None,
                file_info: Some(missing_file()),
            },
        },
        Err(e) => PreflightResponse {
            available: false,
            index_status: ERROR.to_owned(),
            message: format!("Cannot access log file {cleaned}: {e}"),
            time_range: None,
            file_info: Some(FileInfo { exists: true, readable: false, size: 0, last_modified: 0 }),
        },
        Ok(meta) => {
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            let info = FileInfo { exists: true, readable: true, size: meta.len(), last_modified: modified };
            let group = engine.hostlogs.group_of(&cleaned);
            let known = group.as_ref().is_some_and(|g| engine.state().group(&g.path).is_some());
            let mut response = PreflightResponse {
                available: false,
                index_status: NOT_INDEXED.to_owned(),
                message: String::new(),
                time_range: None,
                file_info: Some(info),
            };
            if known {
                if let Some((_, range)) = indexed_group(&cleaned) {
                    response.index_status = INDEXED.to_owned();
                    response.available = true;
                    response.time_range = range;
                } else if engine.processing.indexing() {
                    response.index_status = INDEXING.to_owned();
                }
            } else {
                response.index_status = if engine.processing.indexing() { QUEUED } else { NOT_INDEXED }.to_owned();
                response.message = "Log file not indexed yet".to_owned();
            }
            response
        }
    }
}
