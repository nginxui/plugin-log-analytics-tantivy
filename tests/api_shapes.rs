//! Every endpoint answers with the keys and kinds the web pages declare in
//! `webapp/src/api/types.ts`.

mod common;

use std::collections::HashMap;

use common::*;
use serde_json::{json, Value};

/// A field of a TypeScript interface.
#[derive(Debug, Clone)]
struct Field {
    optional: bool,
    ty: String,
}

type Interfaces = HashMap<String, HashMap<String, Field>>;

/// Reads the interfaces of types.ts. Only the fields at the top level of an
/// interface are read, which is what the pages declare.
fn parse_types() -> Interfaces {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/webapp/src/api/types.ts")).unwrap();
    let mut out: Interfaces = HashMap::new();
    let mut current: Option<String> = None;
    let mut depth = 0i32;
    for line in text.lines() {
        let trimmed = line.trim();
        if current.is_none() {
            if let Some(rest) = trimmed.strip_prefix("export interface ") {
                let name: String = rest.chars().take_while(|c| c.is_alphanumeric()).collect();
                out.insert(name.clone(), HashMap::new());
                current = Some(name);
                depth = 1;
                if rest.contains(" extends ") {
                    let base: String =
                        rest.split(" extends ").nth(1).unwrap().chars().take_while(|c| c.is_alphanumeric()).collect();
                    let inherited = out.get(&base).cloned().unwrap_or_default();
                    out.get_mut(current.as_ref().unwrap()).unwrap().extend(inherited);
                }
            }
            continue;
        }
        let name = current.clone().unwrap();
        if depth == 1 && !trimmed.starts_with("/**") && !trimmed.starts_with('*') && !trimmed.starts_with("//") {
            if let Some((key, ty)) = trimmed.split_once(':') {
                let optional = key.ends_with('?');
                let key = key.trim_end_matches('?').trim();
                if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    out.get_mut(&name).unwrap().insert(key.to_owned(), Field { optional, ty: ty.trim().to_owned() });
                }
            }
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if depth <= 0 {
            current = None;
        }
    }
    out
}

/// Checks a value against an interface: every declared field that is not
/// optional is there, and every field has the kind its type says.
fn check(types: &Interfaces, interface: &str, value: &Value, path: &str) {
    let fields = types.get(interface).unwrap_or_else(|| panic!("no interface {interface}"));
    let object = value.as_object().unwrap_or_else(|| panic!("{path}: expected an object for {interface}, got {value}"));
    for (key, field) in fields {
        match object.get(key) {
            None => assert!(field.optional, "{path}.{key} is missing ({interface})"),
            Some(v) => check_kind(types, &field.ty, v, &format!("{path}.{key}")),
        }
    }
}

fn check_kind(types: &Interfaces, ty: &str, value: &Value, path: &str) {
    let ty = ty.trim().trim_end_matches(',');
    if let Some(item) = ty.strip_suffix("[]") {
        let array = value.as_array().unwrap_or_else(|| panic!("{path}: expected an array, got {value}"));
        for (i, v) in array.iter().take(3).enumerate() {
            check_kind(types, item, v, &format!("{path}[{i}]"));
        }
    } else if ty.starts_with("number") {
        assert!(value.is_number(), "{path}: expected a number, got {value}");
    } else if ty.starts_with("boolean") {
        assert!(value.is_boolean(), "{path}: expected a boolean, got {value}");
    } else if ty.starts_with("string") || ty.contains("'") || ty.contains("| string") {
        assert!(value.is_string(), "{path}: expected a string, got {value}");
    } else if ty.starts_with('{') {
        assert!(value.is_object(), "{path}: expected an object, got {value}");
    } else if types.contains_key(ty) {
        check(types, ty, value, path);
    }
}

#[tokio::test]
async fn the_parser_of_types_reads_the_declarations() {
    let types = parse_types();
    assert!(types["AccessLogEntry"].contains_key("browser_version"));
    assert!(types["PreflightResponse"]["time_range"].optional);
    assert!(!types["DashboardSummary"]["total_uv"].optional);
    assert!(types["ChinaCityMapRequest"].contains_key("province"));
}

#[tokio::test]
async fn status_search_entries_and_dashboard_follow_the_types() {
    let types = parse_types();
    let api = Api::new(600).await;

    let (code, body) = api.get("/logs/status").await;
    assert_eq!(code, 200);
    check(&types, "LogStatusResponse", &body, "status");
    assert_eq!(body["items"][0]["index_status"], "indexed");
    assert_eq!(body["items"][0]["document_count"], 600);
    assert_eq!(body["summary"]["indexed_files"], 1);

    let (code, body) = api.post("/search", json!({"query": "", "limit": 20, "log_path": api.group()})).await;
    assert_eq!(code, 200, "{body}");
    check(&types, "AdvancedSearchResponse", &body, "search");
    assert_eq!(body["total"], 600);
    assert_eq!(body["entries"].as_array().unwrap().len(), 20);
    assert_eq!(body["summary"]["pv"], 600);
    assert_eq!(body["summary"]["uv"], 5);
    let first = &body["entries"][0];
    assert!(first["ip_location_label"].is_string());
    // Newest first by default
    assert!(first["timestamp"].as_i64() >= body["entries"][1]["timestamp"].as_i64());

    let (_, body) = api.post("/search", json!({"query": "wp-login.php", "status": [500], "limit": 5})).await;
    check(&types, "AdvancedSearchResponse", &body, "search filtered");
    assert!(body["total"].as_u64().unwrap() > 0);

    let (code, body) = api.get("/entries?limit=7&tail=true").await;
    assert_eq!(code, 200);
    assert_eq!(body["count"], 7);
    assert!(body["entries"].is_array());
    check(&types, "AccessLogEntry", &body["entries"][0], "entries[0]");

    let (code, body) = api
        .post("/dashboard", json!({"log_path": api.group(), "start_date": "2026-09-07", "end_date": "2026-09-08"}))
        .await;
    assert_eq!(code, 200, "{body}");
    check(&types, "DashboardAnalytics", &body, "dashboard");
    assert_eq!(body["summary"]["total_pv"], 600);
    assert_eq!(body["summary"]["total_uv"], 5);
    assert!(!body["top_urls"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn maps_preflight_and_geolite_follow_the_types() {
    let types = parse_types();
    let api = Api::new(300).await;
    let range = json!({"path": api.group(), "start_time": 1_788_739_200i64, "end_time": 1_788_939_200i64});

    let (code, body) = api.post("/geo/world", range.clone()).await;
    assert_eq!(code, 200, "{body}");
    check_kind(&types, "WorldMapData[]", &body["data"], "world.data");
    assert!(body["data"].as_array().unwrap().iter().any(|i| i["code"] == "US"));

    let (code, body) = api.post("/geo/china", range.clone()).await;
    assert_eq!(code, 200);
    check_kind(&types, "ChinaMapData[]", &body["data"], "china.data");

    let mut city = range.clone();
    city["province"] = json!("广东");
    let (code, body) = api.post("/geo/china/city", city).await;
    assert_eq!(code, 200);
    check_kind(&types, "CityData[]", &body["data"], "city.data");
    assert_eq!(body["custom_mmdb_mode"], false);
    let (code, body) = api.post("/geo/china/city", range.clone()).await;
    assert_eq!(code, 406);
    assert_eq!(body["scope"], "validate");

    let (code, body) = api.post("/geo/stats", json!({"path": api.group(), "limit": 2})).await;
    assert_eq!(code, 200);
    let stats = body["stats"].as_array().unwrap();
    assert!(stats.len() <= 2 && stats[0]["Country"].is_string() && stats[0]["Requests"].is_number());

    let (code, body) = api
        .get(&format!("/preflight?log_path={}", plugin_log_analytics_rs::logs::encode_path_param(&api.group())))
        .await;
    assert_eq!(code, 200);
    check(&types, "PreflightResponse", &body, "preflight");
    assert_eq!(body["index_status"], "indexed");
    assert_eq!(body["available"], true);
    let (_, body) = api.get("/preflight?log_path=/etc/passwd").await;
    assert_eq!(body["index_status"], "error");
    check(&types, "PreflightResponse", &body, "preflight invalid");

    let (code, body) = api.get("/geolite/status").await;
    assert_eq!(code, 200);
    check(&types, "GeoLiteStatus", &body, "geolite");
    assert_eq!(body["exists"], false);

    let (code, body) = api.post("/analytics", json!({"path": api.group()})).await;
    assert_eq!(code, 200, "{body}");
    for key in [
        "total_entries",
        "status_code_distribution",
        "method_distribution",
        "top_paths",
        "top_ips",
        "top_user_agents",
        "bytes_stats",
        "response_time_stats",
    ] {
        assert!(body.get(key).is_some(), "analytics lacks {key}");
    }
    assert_eq!(body["total_entries"], 300);
    assert!(body["top_user_agents"].as_array().unwrap().len() <= 10);
}

#[tokio::test]
async fn errors_warm_rebuild_and_boundaries() {
    let api = Api::new(100).await;

    let (code, body) = api.post("/search", json!({"log_path": "/etc/passwd"})).await;
    assert_eq!(code, 500);
    assert_eq!(body["scope"], "nginx_log");
    assert_eq!(body["code"], 50014);
    assert_eq!(body["message"], "log path is not under whitelist");

    let (code, body) = api.post("/search", json!("not an object")).await;
    assert_eq!(code, 406);
    assert_eq!(body["message"], "Validation error");

    let (code, body) = api.post("/dashboard", json!({"start_date": "soon", "end_date": "2026-09-08"})).await;
    assert_eq!(code, 400);
    assert!(body["error"].as_str().unwrap().contains("start_date"));

    let (code, body) = api.post("/geo/stats", json!({"start_time": 50, "end_time": 10})).await;
    assert_eq!(code, 500);
    assert_eq!(body, json!({"code": 500, "message": "Server Error"}));

    let (code, body) = api.post("/warm", json!({})).await;
    assert_eq!((code, &body["status"]), (202, &json!("warming")));

    let (code, body) = api.get("/geo/boundary/110000_full.json").await;
    assert_eq!((code, body["message"].as_str()), (404, Some("map file not found")));
    let (code, _) = api.get("/geo/boundary/evil.json").await;
    assert_eq!(code, 400);
    let maps = api.root.path().join("data").join("maps");
    std::fs::create_dir_all(&maps).unwrap();
    std::fs::write(maps.join("110000_full.json"), r#"{"type":"FeatureCollection"}"#).unwrap();
    let (code, body) = api.get("/geo/boundary/110000_full.json").await;
    assert_eq!((code, body["type"].as_str()), (200, Some("FeatureCollection")));

    let (code, _) = api.get("/nothing").await;
    assert_eq!(code, 404);

    let (code, body) = api.post("/index/rebuild", json!({"path": "/etc/passwd"})).await;
    assert_eq!((code, &body["code"]), (500, &json!(50015)));
    let before = api.app.engine.last_report().map_or(0, |r| r.round);
    let (code, body) = api.post("/index/rebuild", json!({})).await;
    assert_eq!(
        (code, &body["status"], &body["message"]),
        (200, &json!("started"), &json!("Index rebuild started in background"))
    );
    // The rebuild runs in the background and ends with the same documents
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        if !api.app.engine.round_running() && api.app.engine.last_report().is_some_and(|r| r.round > before) {
            break;
        }
    }
    assert_eq!(api.app.engine.group_stats(&api.group()).docs, 100);
}

#[tokio::test]
async fn the_events_socket_sends_the_processing_state_first() {
    use futures_util::StreamExt;
    let api = Api::new(50).await;
    let tcp = tokio::net::TcpStream::connect(&api.addr).await.unwrap();
    let (mut ws, response) = tokio_tungstenite::client_async(format!("ws://{}/events", api.addr), tcp).await.unwrap();
    assert_eq!(response.status().as_u16(), 101);
    let first = ws.next().await.unwrap().unwrap();
    let message: Value = serde_json::from_str(first.to_text().unwrap()).unwrap();
    assert_eq!(message["type"], "processing_status");
    assert_eq!(message["data"]["nginx_log_indexing"], false);

    // A rebuild shows up as progress, completion and readiness
    let (code, _) = api.post("/index/rebuild", json!({})).await;
    assert_eq!(code, 200);
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(Ok(frame))) = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next()).await else {
            break;
        };
        let Ok(text) = frame.to_text() else { continue };
        let Ok(v) = serde_json::from_str::<Value>(text) else { continue };
        let kind = v["type"].as_str().unwrap_or_default().to_owned();
        if kind == "nginx_log_index_complete" {
            assert_eq!(v["data"]["success"], true);
            assert_eq!(v["data"]["total_lines"], 50);
        }
        seen.push(kind.clone());
        if kind == "nginx_log_index_ready" {
            break;
        }
    }
    assert!(seen.contains(&"nginx_log_index_progress".to_owned()), "{seen:?}");
    assert!(seen.contains(&"nginx_log_index_complete".to_owned()), "{seen:?}");
    assert!(seen.contains(&"nginx_log_index_ready".to_owned()), "{seen:?}");
}
