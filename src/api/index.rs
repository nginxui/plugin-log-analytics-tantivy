//! Index maintenance: rebuild and warm up.

use std::sync::Arc;

use nginxui_plugin_sdk::http::{Incoming, Request, StatusCode};
use serde::Deserialize;
use serde_json::json;
use tantivy::collector::Count;
use tantivy::query::AllQuery;

use super::respond::{self, json_response, ApiError, Resp};
use crate::app::App;
use crate::engine::RebuildError;
use crate::logs::decode_path_param;

/// The optional body of a rebuild. An empty path rebuilds everything.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RebuildRequest {
    #[serde(rename = "type")]
    _kind: String,
    path: String,
}

/// Starts a rebuild in the background and answers at once.
pub async fn rebuild(app: &Arc<App>, req: Request<Incoming>) -> Result<Resp, ApiError> {
    // No body, or one that cannot be read, means rebuild everything
    let body = respond::read_body(req).await.unwrap_or_default();
    let request: RebuildRequest = serde_json::from_slice(&body).unwrap_or_default();

    let path = decode_path_param(&request.path);
    if !path.is_empty() && !app.engine.hostlogs.is_valid_path(&path) {
        return Err(ApiError::cannot_access_log_file());
    }
    let scope = app.engine.check_rebuild((!path.is_empty()).then_some(path.as_str())).map_err(|e| match e {
        RebuildError::Busy => ApiError::failed_to_rebuild_index(),
        RebuildError::GroupBusy => ApiError::failed_to_rebuild_file_index(),
        RebuildError::NotAllowed => ApiError::cannot_access_log_file(),
    })?;

    let engine = app.engine.clone();
    tokio::spawn(async move {
        if let Some(report) = engine.run_round(scope, true).await {
            nginxui_plugin_sdk::info!(
                "rebuild: {} group(s), {} document(s), {} failed, {} ms",
                report.groups,
                report.docs,
                report.failed,
                report.duration_ms
            );
        }
    });
    Ok(json_response(StatusCode::OK, &json!({"message": "Index rebuild started in background", "status": "started"})))
}

/// Warms the index in the background and answers at once. The page asks for it
/// as soon as a view that searches is shown.
pub fn warm(app: &Arc<App>) -> Resp {
    let engine = app.engine.clone();
    tokio::task::spawn_blocking(move || {
        // A pass over the match all query brings the files of the index into memory
        let _ = engine.store.searcher().search(&AllQuery, &Count);
    });
    json_response(StatusCode::ACCEPTED, &json!({"status": "warming"}))
}
