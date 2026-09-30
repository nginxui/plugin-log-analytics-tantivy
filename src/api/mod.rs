//! The HTTP API of the plugin. The host proxies `/api/plugins/<id>/http/*` to
//! it after it authenticated the request. Routes, request bodies and answers
//! are the ones of the Go plugin, which the pages were written against.

mod dashboard;
mod index;
mod respond;
mod search;
mod stream;
mod ws;

use std::sync::Arc;
use std::time::Duration;

use nginxui_plugin_sdk::http::{HttpBody, Incoming, Method, Request, Response};

use crate::app::App;
use crate::logs::{clean_path, decode_path_param, main_log_path};
use crate::status::{self, StatusFilter};

pub use respond::{ApiError, Resp};

/// How long a request waits for the first listing of the logs. The status
/// answers without it, so it waits less.
const START_WAIT: Duration = Duration::from_secs(30);
const STATUS_WAIT: Duration = Duration::from_secs(5);

/// Answers one request.
pub async fn handle(app: Arc<App>, req: Request<Incoming>) -> Response<HttpBody> {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    match route(&app, req, &method, &path).await {
        Ok(response) => response,
        Err(e) => e.into_response(method.as_str(), &path),
    }
}

async fn route(app: &Arc<App>, req: Request<Incoming>, method: &Method, path: &str) -> Result<Resp, ApiError> {
    let get = *method == Method::GET;
    let post = *method == Method::POST;
    // Right after the start the host may not have listed the logs yet
    match path {
        "/logs/status" => {
            app.wait_listed(STATUS_WAIT).await;
        }
        "/search" | "/entries" | "/analytics" | "/preflight" | "/dashboard" | "/geo/world" | "/geo/china"
        | "/geo/china/city" | "/geo/regions" | "/geo/points" | "/geo/stats" | "/index/rebuild" => {
            app.wait_listed(START_WAIT).await;
        }
        _ => {}
    }
    Ok(match path {
        "/logs/status" if get => logs_status(app, &req).await,
        "/search" if post => search::search(app, req).await?,
        "/entries" if get => search::entries(app, req).await?,
        "/analytics" if post => search::analytics(app, req).await?,
        "/preflight" if get => preflight(app, &req).await,
        "/dashboard" if post => dashboard::dashboard(app, req).await?,
        "/geo/world" if post => dashboard::world(app, req).await?,
        "/geo/china" if post => dashboard::china(app, req).await?,
        "/geo/china/city" if post => dashboard::china_city(app, req).await?,
        "/geo/regions" if post => dashboard::regions(app, req).await?,
        "/geo/points" if post => dashboard::points(app, req).await?,
        "/geo/stats" if post => dashboard::stats(app, req).await?,
        "/index/rebuild" if post => index::rebuild(app, req).await?,
        "/warm" if post => index::warm(app),
        "/geolite/status" if get => stream::geolite_status(app),
        "/geolite/download" if get => stream::geolite_download(app, req),
        "/events" if get => stream::events(app, req),
        p if get && p.starts_with("/geo/boundary/") => index::boundary(app, &p["/geo/boundary/".len()..]).await,
        _ => respond::not_found(),
    })
}

async fn logs_status(app: &Arc<App>, req: &Request<Incoming>) -> Resp {
    let filter = StatusFilter {
        kind: respond::query_param(req, "type").unwrap_or_default(),
        name: respond::query_param(req, "name").unwrap_or_default(),
        path: respond::query_param(req, "path").unwrap_or_default(),
        indexed: respond::query_param(req, "indexed").unwrap_or_default(),
    };
    let app = app.clone();
    let response = tokio::task::spawn_blocking(move || status::log_status(&app.engine, &filter)).await;
    match response {
        Ok(body) => respond::ok(&body),
        Err(e) => ApiError::internal(e).into_response("GET", "/logs/status"),
    }
}

async fn preflight(app: &Arc<App>, req: &Request<Incoming>) -> Resp {
    let log_path = respond::query_param(req, "log_path").unwrap_or_default();
    let app = app.clone();
    match tokio::task::spawn_blocking(move || status::preflight(&app.engine, &log_path)).await {
        Ok(body) => respond::ok(&body),
        Err(e) => ApiError::internal(e).into_response("GET", "/preflight"),
    }
}

/// Decodes and checks the log path of a request and returns the log group it
/// names. An empty path means the default access log, and without one the
/// whole index.
pub(crate) fn resolve_group(app: &App, raw: &str, use_default: bool) -> Result<String, ApiError> {
    let mut safe = String::new();
    if !raw.is_empty() {
        safe = clean_path(&decode_path_param(raw));
        if !app.engine.hostlogs.is_valid_path(&safe) {
            return Err(ApiError::path_not_whitelisted());
        }
    }
    if safe.is_empty() && use_default {
        safe = app.engine.hostlogs.default_access_path();
    }
    if safe.is_empty() {
        return Ok(String::new());
    }
    if !app.engine.hostlogs.is_valid_path(&safe) {
        return Err(ApiError::path_not_whitelisted());
    }
    Ok(app.engine.hostlogs.group_of(&safe).map_or_else(|| main_log_path(&safe), |g| g.path))
}
