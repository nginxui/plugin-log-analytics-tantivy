//! Index maintenance: rebuild, warm up and the map boundary files.

use std::sync::Arc;

use nginxui_plugin_sdk::http::{full_body, header, HeaderValue, Incoming, Request, Response, StatusCode};
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

fn is_boundary_name(name: &str) -> bool {
    let b = name.as_bytes();
    name.len() == 16 && b[..6].iter().all(u8::is_ascii_digit) && &name[6..] == "_full.json"
}

/// Serves one map outline file. The page falls back to a public mirror when it
/// is not there.
pub async fn boundary(app: &Arc<App>, filename: &str) -> Resp {
    let name = filename.trim();
    if !is_boundary_name(name) {
        return json_response(StatusCode::BAD_REQUEST, &json!({"message": "invalid map file name"}));
    }
    let dir = app.engine.dirs.maps(&app.engine.settings());
    match tokio::fs::read(dir.join(name)).await {
        Ok(bytes) => {
            let mut response = Response::new(full_body(bytes));
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
            response
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            json_response(StatusCode::NOT_FOUND, &json!({"message": "map file not found"}))
        }
        Err(_) => json_response(StatusCode::INTERNAL_SERVER_ERROR, &json!({"message": "failed to read map file"})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_names_are_six_digits_and_a_suffix() {
        assert!(is_boundary_name("100000_full.json"));
        assert!(!is_boundary_name("10000_full.json"));
        assert!(!is_boundary_name("../000000_full.json"));
        assert!(!is_boundary_name("100000_full.jsoN"));
        assert!(!is_boundary_name("abcdef_full.json"));
    }
}
