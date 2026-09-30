//! The dashboard and the maps.

use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use nginxui_plugin_sdk::http::{Incoming, Request, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};

use super::resolve_group;
use super::respond::{self, json_response, ok, ApiError, Resp};
use super::search::{end_after, AnalyticsRequest};
use crate::analytics::{self, Share};
use crate::app::App;

/// The body of a dashboard request.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DashboardRequest {
    log_path: String,
    /// `YYYY-MM-DD`
    start_date: String,
    end_date: String,
}

fn bad_date(field: &str, e: impl std::fmt::Display) -> ApiError {
    ApiError::Plain(
        StatusCode::BAD_REQUEST,
        json!({"error": format!("Invalid {field} format, expected YYYY-MM-DD: {e}")}),
    )
}

/// Refuses a group that is an error log: its entries have no traffic figures.
fn access_only(app: &App, group: &str) -> Result<(), ApiError> {
    match app.engine.hostlogs.group_of(group) {
        Some(g) if g.kind == crate::logs::ERROR_KIND => Err(ApiError::Plain(
            StatusCode::BAD_REQUEST,
            json!({"error": "The dashboard and the maps are only available for access logs"}),
        )),
        _ => Ok(()),
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The first instant of a calendar day in the local zone of the server.
fn local_midnight(date: NaiveDate) -> i64 {
    crate::localtime::from_local(date.year(), date.month(), date.day(), 0, 0, 0).unwrap_or(0)
}

/// The window `[start, end)` of a dashboard request in unix seconds. The dates
/// are days in the local zone of the server, like the daily and hourly buckets,
/// and the end date is included, so the window ends at the next local
/// midnight. A missing date means the last 30 days.
fn window(request: &DashboardRequest, now: i64) -> Result<(i64, i64), ApiError> {
    let parse = |text: &str, field: &str| -> Result<Option<NaiveDate>, ApiError> {
        if text.is_empty() {
            return Ok(None);
        }
        NaiveDate::parse_from_str(text, "%Y-%m-%d").map(Some).map_err(|e| bad_date(field, e))
    };
    let start = parse(&request.start_date, "start_date")?.map(local_midnight);
    let end = parse(&request.end_date, "end_date")?.and_then(|d| d.succ_opt()).map(local_midnight);
    Ok(match (start, end) {
        (Some(s), Some(e)) => (s, e),
        _ => (now - 30 * 86400, now),
    })
}

pub async fn dashboard(app: &Arc<App>, req: Request<Incoming>) -> Result<Resp, ApiError> {
    let request: DashboardRequest = respond::read_json(req).await?;
    let group = resolve_group(app, &request.log_path, true)?;
    access_only(app, &group)?;
    let (start, end) = window(&request, now())?;

    let app = app.clone();
    tokio::task::spawn_blocking(move || -> Result<Resp, ApiError> {
        let response = app.engine.dashboard(&group, start, end)?;
        Ok(ok(&response))
    })
    .await
    .map_err(ApiError::internal)?
}

fn items(shares: &[Share], key: &str) -> Vec<Value> {
    shares.iter().map(|s| json!({ key: s.key, "value": s.value, "percent": s.percent })).collect()
}

/// Reads a geo request and runs `work` on the index.
async fn geo<F>(app: &Arc<App>, req: Request<Incoming>, work: F) -> Result<Resp, ApiError>
where
    F: FnOnce(&App, &AnalyticsRequest, &str, &tantivy::Searcher) -> Result<Value, ApiError> + Send + 'static,
{
    let request: AnalyticsRequest = respond::read_json(req).await?;
    let group = resolve_group(app, &request.path, true)?;
    access_only(app, &group)?;
    let app = app.clone();
    tokio::task::spawn_blocking(move || -> Result<Resp, ApiError> {
        analytics::validate_range(request.start_time, request.end_time)?;
        let searcher = app.engine.store.searcher();
        Ok(json_response(StatusCode::OK, &work(&app, &request, &group, &searcher)?))
    })
    .await
    .map_err(ApiError::internal)?
}

/// Requests per country, for the world map.
pub async fn world(app: &Arc<App>, req: Request<Incoming>) -> Result<Resp, ApiError> {
    geo(app, req, |app, request, group, searcher| {
        let filter = analytics::range_filter(group, request.start_time, end_after(request.end_time));
        let shares = analytics::countries(searcher, app.engine.store.fields(), &filter, 300)?;
        Ok(json!({ "data": items(&shares, "code") }))
    })
    .await
}

/// Requests per subdivision of a country, keyed by ISO 3166-2 code, for the
/// region map of a country.
pub async fn regions(app: &Arc<App>, req: Request<Incoming>) -> Result<Resp, ApiError> {
    geo(app, req, |app, request, group, searcher| {
        let country = request.country.trim().to_ascii_uppercase();
        if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(ApiError::Validate("country must be a two letter ISO code".into()));
        }
        let filter = analytics::range_filter(group, request.start_time, end_after(request.end_time));
        let shares = analytics::regions(searcher, app.engine.store.fields(), &filter, &country, 500)?;
        Ok(json!({ "data": items(&shares, "code") }))
    })
    .await
}

/// The busiest cities with their coordinates, of one country or of all, for
/// the hotspot map.
pub async fn points(app: &Arc<App>, req: Request<Incoming>) -> Result<Resp, ApiError> {
    geo(app, req, |app, request, group, searcher| {
        let country = request.country.trim().to_ascii_uppercase();
        let limit = if request.limit > 0 { (request.limit as usize).min(2000) } else { 500 };
        let filter = analytics::range_filter(group, request.start_time, end_after(request.end_time));
        let points = analytics::city_points(
            searcher,
            app.engine.store.fields(),
            &filter,
            (!country.is_empty()).then_some(country.as_str()),
            limit,
        )?;
        Ok(json!({ "data": points }))
    })
    .await
}

/// The busiest countries.
pub async fn stats(app: &Arc<App>, req: Request<Incoming>) -> Result<Resp, ApiError> {
    geo(app, req, |app, request, group, searcher| {
        let limit = if request.limit > 0 { request.limit as usize } else { 20 };
        let filter = analytics::range_filter(group, request.start_time, end_after(request.end_time));
        let shares = analytics::countries(searcher, app.engine.store.fields(), &filter, limit)?;
        // The keys are the ones of the Go structure, which has no field tags
        let stats: Vec<Value> = shares.iter().map(|s| json!({"Country": s.key, "Requests": s.value})).collect();
        Ok(json!({ "stats": stats }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(start: &str, end: &str) -> DashboardRequest {
        DashboardRequest { log_path: String::new(), start_date: start.into(), end_date: end.into() }
    }

    #[test]
    fn dates_are_local_days_and_the_end_day_is_included() {
        let (s, e) = window(&request("2026-09-01", "2026-09-30"), 0).unwrap();
        assert_eq!(s, crate::localtime::from_local(2026, 9, 1, 0, 0, 0).unwrap());
        assert_eq!(e, crate::localtime::from_local(2026, 10, 1, 0, 0, 0).unwrap());
        assert_eq!(crate::localtime::format_date(s), "2026-09-01");
        assert_eq!(crate::localtime::format_date(e - 1), "2026-09-30");
    }

    #[test]
    fn a_missing_date_means_the_last_thirty_days() {
        let (s, e) = window(&request("", "2026-09-30"), 10_000_000).unwrap();
        assert_eq!((s, e), (10_000_000 - 30 * 86400, 10_000_000));
        assert_eq!(window(&request("", ""), 10_000_000).unwrap().1, 10_000_000);
    }

    #[test]
    fn a_bad_date_is_a_400_with_the_message() {
        let e = window(&request("not-a-date", "2026-09-30"), 0).unwrap_err();
        match e {
            ApiError::Plain(status, body) => {
                assert_eq!(status, StatusCode::BAD_REQUEST);
                assert!(body["error"]
                    .as_str()
                    .unwrap()
                    .starts_with("Invalid start_date format, expected YYYY-MM-DD: "));
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
