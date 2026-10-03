//! Responses and errors of the HTTP API. The error bodies have the shapes the
//! pages already translate.

use http_body_util::BodyExt;
use nginxui_plugin_sdk::http::{full_body, header, HeaderValue, HttpBody, Incoming, Request, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

pub type Resp = Response<HttpBody>;

/// Largest request body that is read.
const MAX_BODY: usize = 1 << 20;

/// An error of a handler.
#[derive(Debug)]
pub enum ApiError {
    /// An error with a number the pages translate. Answered with 500.
    Coded {
        scope: &'static str,
        code: i32,
        message: String,
        params: Vec<String>,
    },
    /// Anything else. The detail goes to the log, the client sees a generic text.
    Internal(String),
    /// The request body is not what the handler reads.
    Validate(String),
    TooLarge(String),
    /// A plain JSON answer with a status.
    Plain(StatusCode, Value),
}

impl ApiError {
    pub fn coded(scope: &'static str, code: i32, message: &str) -> Self {
        ApiError::Coded { scope, code, message: message.to_owned(), params: Vec::new() }
    }

    pub fn coded_with(scope: &'static str, code: i32, message: &str, params: Vec<String>) -> Self {
        ApiError::Coded { scope, code, message: message.to_owned(), params }
    }

    pub fn internal(e: impl std::fmt::Display) -> Self {
        ApiError::Internal(e.to_string())
    }

    /// The log path is not one of the listed logs.
    pub fn path_not_whitelisted() -> Self {
        Self::coded("nginx_log", 50014, "log path is not under whitelist")
    }

    pub fn cannot_access_log_file() -> Self {
        Self::coded("nginx_log", 50015, "cannot access log file")
    }

    pub fn failed_to_rebuild_index() -> Self {
        Self::coded("nginx_log", 50018, "failed to rebuild index")
    }

    pub fn failed_to_rebuild_file_index() -> Self {
        Self::coded("nginx_log", 50019, "failed to rebuild file index")
    }
}

impl From<crate::analytics::AnalyticsError> for ApiError {
    fn from(e: crate::analytics::AnalyticsError) -> Self {
        ApiError::Internal(e.to_string())
    }
}

impl From<tantivy::TantivyError> for ApiError {
    fn from(e: tantivy::TantivyError) -> Self {
        ApiError::Internal(e.to_string())
    }
}

/// A JSON response.
pub fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Resp {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    let mut response = Response::new(full_body(body));
    *response.status_mut() = status;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
    response
}

pub fn ok<T: Serialize>(value: &T) -> Resp {
    json_response(StatusCode::OK, value)
}

pub fn not_found() -> Resp {
    let mut response = Response::new(full_body("404 page not found"));
    *response.status_mut() = StatusCode::NOT_FOUND;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    response
}

impl ApiError {
    /// Logs the error and builds its response.
    pub fn into_response(self, method: &str, path: &str) -> Resp {
        match self {
            ApiError::Coded { scope, code, message, params } => {
                nginxui_plugin_sdk::error!("{method} {path}: {message}");
                let mut body = json!({"scope": scope, "code": code, "message": message});
                if !params.is_empty() {
                    body["params"] = json!(params);
                }
                json_response(StatusCode::INTERNAL_SERVER_ERROR, &body)
            }
            ApiError::Internal(detail) => {
                nginxui_plugin_sdk::error!("{method} {path}: {detail}");
                json_response(StatusCode::INTERNAL_SERVER_ERROR, &json!({"code": 500, "message": "Server Error"}))
            }
            ApiError::Validate(detail) => json_response(
                StatusCode::NOT_ACCEPTABLE,
                &json!({"scope": "validate", "code": 406, "message": "Validation error", "errors": {"body": detail}}),
            ),
            ApiError::TooLarge(detail) => json_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                &json!({"scope": "validate", "code": 413, "message": "Request body too large", "errors": {"body": detail}}),
            ),
            ApiError::Plain(status, body) => json_response(status, &body),
        }
    }
}

/// Reads the whole body up to a limit.
pub async fn read_body(req: Request<Incoming>) -> Result<Vec<u8>, ApiError> {
    let collected = http_body_util::Limited::new(req.into_body(), MAX_BODY)
        .collect()
        .await
        .map_err(|e| ApiError::TooLarge(e.to_string()))?;
    Ok(collected.to_bytes().to_vec())
}

/// Reads a JSON body. A body that cannot be read is a validation error.
pub async fn read_json<T: DeserializeOwned>(req: Request<Incoming>) -> Result<T, ApiError> {
    let bytes = read_body(req).await?;
    serde_json::from_slice(&bytes).map_err(|e| ApiError::Validate(e.to_string()))
}

/// The values of a query string by name.
pub fn query_param(req: &Request<Incoming>, name: &str) -> Option<String> {
    let query = req.uri().query()?;
    form_urlencoded::parse(query.as_bytes()).find(|(k, _)| k == name).map(|(_, v)| v.into_owned())
}

/// Whether the client asked for Chinese names. The language of the page, sent
/// as `X-Language` or `X-Locale`, decides; the browser's `Accept-Language`
/// only when the page names none.
pub fn wants_chinese(req: &Request<Incoming>) -> bool {
    let header = |name: &str| {
        req.headers().get(name).and_then(|v| v.to_str().ok()).map(|v| v.trim().to_lowercase()).filter(|v| !v.is_empty())
    };
    if let Some(page) = header("x-language").or_else(|| header("x-locale")) {
        return page.starts_with("zh");
    }
    header("accept-language").is_some_and(|v| v.starts_with("zh") || v.contains("zh-") || v.contains("zh_"))
}
