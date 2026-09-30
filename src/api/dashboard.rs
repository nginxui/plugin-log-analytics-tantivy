//! The dashboard and the maps.

use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
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

/// The window `[start, end)` of a dashboard request in unix seconds. The dates
/// are UTC days and the end date is included, so the window ends at the next
/// midnight. A missing date means the last 30 days.
fn window(request: &DashboardRequest, now: i64) -> Result<(i64, i64), ApiError> {
    let parse = |text: &str, field: &str| -> Result<Option<i64>, ApiError> {
        if text.is_empty() {
            return Ok(None);
        }
        let date = NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|e| bad_date(field, e))?;
        Ok(Some(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight")).timestamp()))
    };
    let start = parse(&request.start_date, "start_date")?;
    let end = parse(&request.end_date, "end_date")?.map(|t| t + 86400);
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

/// Requests per province of China.
pub async fn china(app: &Arc<App>, req: Request<Incoming>) -> Result<Resp, ApiError> {
    geo(app, req, |app, request, group, searcher| {
        let filter = analytics::range_filter(group, request.start_time, end_after(request.end_time));
        let shares = analytics::provinces(searcher, app.engine.store.fields(), &filter, "CN", 100)?;
        Ok(json!({ "data": items(&shares, "name") }))
    })
    .await
}

/// The body of a city request.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CityRequest {
    province: String,
}

/// Requests per city of a province of China.
pub async fn china_city(app: &Arc<App>, req: Request<Incoming>) -> Result<Resp, ApiError> {
    let bytes = respond::read_body(req).await?;
    let province =
        serde_json::from_slice::<CityRequest>(&bytes).map_err(|e| ApiError::Validate(e.to_string()))?.province;
    if province.is_empty() {
        return Err(ApiError::Validate(
            "Key: 'ChinaCityMapRequest.Province' Error:Field validation for 'Province' failed on the 'required' tag"
                .into(),
        ));
    }
    let request: AnalyticsRequest = serde_json::from_slice(&bytes).map_err(|e| ApiError::Validate(e.to_string()))?;
    let group = resolve_group(app, &request.path, true)?;
    access_only(app, &group)?;

    let app = app.clone();
    tokio::task::spawn_blocking(move || -> Result<Resp, ApiError> {
        analytics::validate_range(request.start_time, request.end_time)?;
        let searcher = app.engine.store.searcher();
        let fields = app.engine.store.fields();
        let filter = analytics::range_filter(&group, request.start_time, end_after(request.end_time));
        let cities = analytics::cities(&searcher, fields, &filter, "CN", &province, 100)?;
        let custom = app.engine.settings().uses_custom_mmdb();
        let mut body = json!({ "data": items(&cities, "name"), "custom_mmdb_mode": custom });
        if custom {
            let top = analytics::city_labels(&searcher, fields, &filter, "CN", &province)?;
            if !top.is_empty() {
                body["top_data"] = json!(items(&top, "name"));
            }
        }
        Ok(ok(&body))
    })
    .await
    .map_err(ApiError::internal)?
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
    fn dates_are_utc_days_and_the_end_day_is_included() {
        let (s, e) = window(&request("2026-09-01", "2026-09-30"), 0).unwrap();
        assert_eq!(s, 1_788_220_800);
        assert_eq!(e, 1_790_812_800);
        assert_eq!(e - s, 30 * 86400);
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
