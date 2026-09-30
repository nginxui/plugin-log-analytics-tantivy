//! Search, entries and the statistics of a match set.

use std::sync::Arc;
use std::time::Instant;

use nginxui_plugin_sdk::http::{Incoming, Request};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::resolve_group;
use super::respond::{self, ok, ApiError, Resp};
use crate::analytics;
use crate::app::App;
use crate::query::Filter;
use crate::search::{self, EntryLoader, SearchParams};

/// Splits a comma joined filter value, as the multi selects of the page send it.
fn split_comma(value: &str) -> Vec<String> {
    value.split(',').map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned).collect()
}

fn one(value: &str) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        vec![value.to_owned()]
    }
}

/// The body of a search.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SearchRequest {
    pub query: String,
    pub log_path: String,
    pub start_time: i64,
    pub end_time: i64,
    pub ip: String,
    pub method: String,
    pub status: Vec<i64>,
    pub path: String,
    pub user_agent: String,
    pub referer: String,
    pub browser: String,
    pub os: String,
    pub device: String,
    pub limit: i64,
    pub offset: i64,
    pub sort_by: String,
    pub sort_order: String,
}

fn filter_of(request: &SearchRequest, group: &str, now: i64) -> Filter {
    let (mut start, mut end) =
        ((request.start_time > 0).then_some(request.start_time), (request.end_time > 0).then_some(request.end_time));
    // Without a range the search covers all time up to now
    if start.is_none() && end.is_none() {
        start = Some(0);
        end = Some(now);
    }
    Filter {
        text: request.query.clone(),
        groups: one(group),
        start,
        end,
        ips: one(&request.ip),
        methods: one(&request.method),
        statuses: request.status.iter().filter(|s| **s >= 0).map(|s| *s as u64).collect(),
        paths: one(&request.path),
        user_agents: one(&request.user_agent),
        referers: one(&request.referer),
        browsers: split_comma(&request.browser),
        systems: split_comma(&request.os),
        devices: split_comma(&request.device),
        ..Default::default()
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Loads the entries of the hits and adds their location label.
fn entries_of(
    app: &App,
    searcher: &tantivy::Searcher,
    hits: &[tantivy::DocAddress],
    chinese: bool,
) -> Result<Vec<Value>, ApiError> {
    let mut loader = EntryLoader::new(searcher, app.engine.store.fields());
    hits.iter()
        .map(|addr| {
            let mut entry: Map<String, Value> = loader.load(*addr)?;
            let label = search::location_label(&entry, chinese);
            entry.insert("ip_location_label".into(), json!(label));
            Ok(Value::Object(entry))
        })
        .collect()
}

pub async fn search(app: &Arc<App>, req: Request<Incoming>) -> Result<Resp, ApiError> {
    let chinese = respond::wants_chinese(&req);
    let request: SearchRequest = respond::read_json(req).await?;
    let group = resolve_group(app, &request.log_path, true)?;

    let app = app.clone();
    tokio::task::spawn_blocking(move || -> Result<Resp, ApiError> {
        let started = Instant::now();
        let searcher = app.engine.store.searcher();
        let (sort_by, descending) = match request.sort_by.as_str() {
            "" => ("timestamp".to_owned(), true),
            other => (other.to_owned(), request.sort_order != "asc"),
        };
        let params = SearchParams {
            filter: filter_of(&request, &group, now()),
            // A negative limit asks for the figures without hits
            limit: if request.limit < 0 { 1 } else { request.limit as usize },
            offset: request.offset.max(0) as usize,
            sort_by,
            descending,
        };
        let out = search::search(&searcher, app.engine.store.fields(), &params)?;
        let hits = if request.limit < 0 { &[][..] } else { &out.hits[..] };
        let entries = entries_of(&app, &searcher, hits, chinese)?;

        let docs = out.summary.docs;
        let summary = json!({
            "uv": out.summary.ips.len(),
            "pv": docs,
            "total_traffic": out.summary.bytes,
            "unique_pages": out.summary.pages.len(),
            "avg_traffic_per_pv": if docs > 0 { out.summary.bytes as f64 / docs as f64 } else { 0.0 },
            "traffic_approximate": false,
        });
        Ok(ok(&json!({
            "entries": entries,
            "total": docs,
            "took": started.elapsed().as_millis() as i64,
            "query": request.query,
            "summary": summary,
        })))
    })
    .await
    .map_err(ApiError::internal)?
}

pub async fn entries(app: &Arc<App>, req: Request<Incoming>) -> Result<Resp, ApiError> {
    let chinese = respond::wants_chinese(&req);
    let path = respond::query_param(&req, "path").unwrap_or_default();
    let limit = respond::query_param(&req, "limit").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(0);
    let tail = respond::query_param(&req, "tail")
        .is_some_and(|v| matches!(v.as_str(), "true" | "1" | "t" | "T" | "TRUE" | "True"));
    let group = resolve_group(app, &path, false)?;

    let app = app.clone();
    tokio::task::spawn_blocking(move || -> Result<Resp, ApiError> {
        let searcher = app.engine.store.searcher();
        let params = SearchParams {
            filter: Filter { groups: one(&group), ..Default::default() },
            limit: if limit == 0 { 100 } else { limit.clamp(1, search::MAX_LIMIT as i64) as usize },
            offset: 0,
            sort_by: "timestamp".into(),
            descending: tail,
        };
        let out = search::search(&searcher, app.engine.store.fields(), &params)?;
        let entries = entries_of(&app, &searcher, &out.hits, chinese)?;
        Ok(ok(&json!({"count": entries.len(), "entries": entries})))
    })
    .await
    .map_err(ApiError::internal)?
}

/// The body of the analytics, geo and statistics requests.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AnalyticsRequest {
    pub path: String,
    pub start_time: i64,
    pub end_time: i64,
    pub limit: i64,
}

pub async fn analytics(app: &Arc<App>, req: Request<Incoming>) -> Result<Resp, ApiError> {
    let request: AnalyticsRequest = respond::read_json(req).await?;
    let group = resolve_group(app, &request.path, false)?;
    let app = app.clone();
    tokio::task::spawn_blocking(move || -> Result<Resp, ApiError> {
        analytics::validate_range(request.start_time, request.end_time)?;
        let filter = analytics::range_filter(&group, request.start_time, request.end_time);
        let stats = analytics::entries_stats(&app.engine.store.searcher(), app.engine.store.fields(), &filter)?;
        Ok(ok(&stats))
    })
    .await
    .map_err(ApiError::internal)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comma_values_split_and_trim() {
        assert_eq!(split_comma("Chrome, Firefox,,"), ["Chrome", "Firefox"]);
        assert!(split_comma("").is_empty());
    }

    #[test]
    fn a_search_without_a_range_covers_all_time_up_to_now() {
        let request = SearchRequest::default();
        let f = filter_of(&request, "", 1000);
        assert_eq!((f.start, f.end), (Some(0), Some(1000)));
        let request = SearchRequest { start_time: 5, ..Default::default() };
        let f = filter_of(&request, "/a.log", 1000);
        assert_eq!((f.start, f.end), (Some(5), None));
        assert_eq!(f.groups, ["/a.log"]);
    }

    #[test]
    fn filters_follow_the_request() {
        let request = SearchRequest {
            query: "x".into(),
            ip: "1.1.1.1".into(),
            status: vec![404, 500],
            browser: "Chrome,Safari".into(),
            ..Default::default()
        };
        let f = filter_of(&request, "", 10);
        assert_eq!(f.text, "x");
        assert_eq!(f.ips, ["1.1.1.1"]);
        assert_eq!(f.statuses, [404, 500]);
        assert_eq!(f.browsers, ["Chrome", "Safari"]);
    }
}
