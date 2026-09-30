//! The manifest of the plugin. `plugin.json` is generated from here with
//! `cargo run --bin manifest`, and the packaging narrows it to one platform
//! per package.

use std::collections::HashMap;

use nginxui_plugin_sdk::protocol::{
    capability, event, lifecycle, permission, Manifest, ManifestHttp, ManifestI18n, ManifestResources, ManifestServer,
    ManifestWebapp, SettingsField, SettingsSchema,
};
use serde_json::{json, Value};

pub const PLUGIN_ID: &str = "com.nginxui.log-analytics-tantivy";
/// The plugin of the same pages and routes this one replaces.
pub const CONFLICTING_PLUGIN: &str = "com.nginxui.log-analytics";
pub const PLUGIN_NAME: &str = "Log Analytics (Preview)";
pub const PLUGIN_DESCRIPTION: &str =
    "Search nginx access and error logs with structured queries and field filters, and see traffic on a dashboard with a visitor map. This preview cannot be enabled together with Log Analytics.";
pub const MIN_NGINX_UI_VERSION: &str = "2.7.0";
/// Memory in MiB advised for the machine. The smallest tier of the indexer
/// (50 MB writer, one thread) indexed 1.4 million lines within 160 MB of
/// resident memory on Linux, the rest is room for the server, the page cache
/// of the index and a search or two.
pub const RECOMMENDED_MEMORY_MB: i32 = 256;

/// Prefix of the packaged executables.
pub const BINARY_PREFIX: &str = "log-analytics-tantivy";

pub const BUNDLE_PATH: &str = "webapp/dist/main.js";
pub const STYLE_PATH: &str = "webapp/dist/style.css";
pub const ICON_PATH: &str = "webapp/dist/icon.svg";

/// Platforms the plugin is packaged for, in manifest order.
pub const PLATFORMS: [(&str, &str); 6] = [
    ("linux", "amd64"),
    ("linux", "arm64"),
    ("darwin", "amd64"),
    ("darwin", "arm64"),
    ("windows", "amd64"),
    ("windows", "arm64"),
];

/// Package relative path of the executable of a platform.
pub fn executable_path(os: &str, arch: &str) -> String {
    let ext = if os == "windows" { ".exe" } else { "" };
    format!("server/dist/{BINARY_PREFIX}-{os}-{arch}{ext}")
}

fn translation(name: &str, description: &str) -> ManifestI18n {
    ManifestI18n { name: name.to_owned(), description: description.to_owned() }
}

fn setting(key: &str, kind: &str, name: &str, help: &str, default: Value) -> SettingsField {
    SettingsField {
        key: key.to_owned(),
        r#type: kind.to_owned(),
        display_name: name.to_owned(),
        help_text: help.to_owned(),
        default,
        ..Default::default()
    }
}

/// Builds the manifest. `shared` is the map of shared libraries the browser
/// bundle was built against, it is empty until the bundle is built.
pub fn build(shared: HashMap<String, String>) -> Manifest {
    let executables = PLATFORMS.iter().map(|(os, arch)| (format!("{os}-{arch}"), executable_path(os, arch))).collect();
    Manifest {
        id: PLUGIN_ID.to_owned(),
        name: PLUGIN_NAME.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        description: PLUGIN_DESCRIPTION.to_owned(),
        icon_path: ICON_PATH.to_owned(),
        api_version: 1,
        min_nginx_ui_version: MIN_NGINX_UI_VERSION.to_owned(),
        i18n: HashMap::from([
            ("zh_CN".to_owned(), translation("日志分析（预览）", "对 Nginx 访问日志和错误日志做结构化搜索和按字段筛选，在面板和访客地图上查看流量。预览版，不能与“日志分析”同时启用。")),
            ("zh_TW".to_owned(), translation("日誌分析（預覽）", "對 Nginx 存取日誌和錯誤日誌做結構化搜尋和依欄位篩選，在面板和訪客地圖上檢視流量。預覽版，不能與「日誌分析」同時啟用。")),
            (
                "ja_JP".to_owned(),
                translation("ログ分析（プレビュー）", "Nginx のアクセスログとエラーログを構造化検索してフィールドで絞り込み、ダッシュボードと訪問者マップでトラフィックを確認します。プレビュー版のため、「ログ分析」と同時に有効にできません。"),
            ),
        ]),
        server: Some(ManifestServer {
            executables,
            // The plugin indexes on a schedule, so it stays up
            lifecycle: lifecycle::RESIDENT.to_owned(),
            resources: Some(ManifestResources { recommended_memory_mb: RECOMMENDED_MEMORY_MB, ..Default::default() }),
            ..Default::default()
        }),
        webapp: Some(ManifestWebapp {
            bundle_path: BUNDLE_PATH.to_owned(),
            style_path: STYLE_PATH.to_owned(),
            shared,
            chunks: HashMap::from([
                ("dashboard".to_owned(), "webapp/dist/chunks/dashboard.js".to_owned()),
                ("search".to_owned(), "webapp/dist/chunks/search.js".to_owned()),
            ]),
            ..Default::default()
        }),
        capabilities: vec![capability::HTTP.to_owned()],
        permissions: vec![
            // The nginx log files, and the event when their set changes
            permission::LOG_FILES.to_owned(),
            // The download of the IP location database
            permission::NETWORK.to_owned(),
        ],
        // Both plugins serve the same pages, only one of them runs
        conflicts: vec![CONFLICTING_PLUGIN.to_owned()],
        events: vec![event::LOG_PATHS_CHANGED.to_owned()],
        network_hosts: vec!["cloud.nginxui.com".to_owned()],
        // A socket keeps websockets and streaming responses possible
        http: Some(ManifestHttp { listen: "unix".to_owned() }),
        settings_schema: Some(SettingsSchema {
            settings: vec![
                setting(
                    "incremental_index_interval",
                    "number",
                    "Indexing interval (minutes)",
                    "How often the logs are checked for new lines. Zero or empty means 15 minutes.",
                    json!(15),
                ),
                setting(
                    "max_concurrent_index_tasks",
                    "number",
                    "Logs indexed at once",
                    "The most log files indexed at the same time. A lower number uses less memory. Zero picks a value from the available CPUs.",
                    json!(0),
                ),
                setting(
                    "index_custom_mmdb",
                    "text",
                    "Custom IP location database",
                    "Path of your own IP location database file. Empty uses the downloaded one.",
                    Value::Null,
                ),
            ],
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Sorts the keys of the objects that come from hash maps, so the output does
/// not change from run to run. The fields of the structures keep their order.
fn sort_maps(value: &mut Value) {
    fn sort(v: &mut Value) {
        if let Value::Object(map) = v {
            let mut entries: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            map.extend(entries);
        }
    }
    sort(&mut value["i18n"]);
    sort(&mut value["server"]["executables"]);
    sort(&mut value["webapp"]["shared"]);
    sort(&mut value["webapp"]["chunks"]);
}

/// The committed layout: two space indentation and a trailing newline.
pub fn render(manifest: &Manifest) -> String {
    let mut value = serde_json::to_value(manifest).expect("manifest serializes");
    sort_maps(&mut value);
    let mut text = serde_json::to_string_pretty(&value).expect("manifest renders");
    text.push('\n');
    text
}

/// Narrows a rendered manifest to one `<os>-<arch>` executable. A per platform
/// package declares exactly the platform it ships.
pub fn narrow(text: &str, platform: &str) -> Result<String, String> {
    let mut value: Value = serde_json::from_str(text).map_err(|e| format!("decode manifest: {e}"))?;
    let executables = value
        .get_mut("server")
        .and_then(|s| s.get_mut("executables"))
        .and_then(Value::as_object_mut)
        .ok_or("the manifest has no server executables")?;
    let path = executables
        .get(platform)
        .cloned()
        .ok_or_else(|| format!("the manifest declares no executable for {platform}"))?;
    executables.clear();
    executables.insert(platform.to_owned(), path);
    let mut out = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(out)
}

/// The shared libraries the bundle was built against, read from the fragment
/// the bundler writes. A missing file gives an empty map.
pub fn shared_from_fragment(text: &str) -> HashMap<String, String> {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("shared").cloned())
        .and_then(|s| serde_json::from_value(s).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_conflict() {
        let m = build(HashMap::new());
        assert_eq!(m.id, "com.nginxui.log-analytics-tantivy");
        assert_eq!(m.version, "0.1.0-beta.1");
        assert_eq!(m.conflicts, ["com.nginxui.log-analytics"]);
        assert_eq!(m.capabilities, ["http"]);
        assert_eq!(m.server.unwrap().resources.unwrap().recommended_memory_mb, RECOMMENDED_MEMORY_MB);
    }

    #[test]
    fn user_facing_text_names_no_engine_or_language() {
        // The id and the file names may name the engine, what people read may not
        let m = build(HashMap::new());
        let mut shown = vec![m.name.clone(), m.description.clone()];
        for t in m.i18n.values() {
            shown.push(t.name.clone());
            shown.push(t.description.clone());
        }
        for field in m.settings_schema.iter().flat_map(|s| &s.settings) {
            shown.push(field.display_name.clone());
            shown.push(field.help_text.clone());
        }
        let text = shown.join("\n").to_lowercase();
        for word in ["tantivy", "bleve", "rust", "golang", "lucene"] {
            assert!(!text.contains(word), "{word} in the manifest");
        }
    }

    #[test]
    fn rendering_is_stable_and_narrowing_keeps_one_platform() {
        let shared = HashMap::from([("vue".to_owned(), ">=3".to_owned()), ("pinia".to_owned(), ">=4".to_owned())]);
        let a = render(&build(shared.clone()));
        assert_eq!(a, render(&build(shared)));
        assert!(a.starts_with("{\n  \"id\": \"com.nginxui.log-analytics-tantivy\""));
        assert!(a.find("\"pinia\"").unwrap() < a.find("\"vue\"").unwrap());

        let narrowed = narrow(&a, "linux-amd64").unwrap();
        let v: Value = serde_json::from_str(&narrowed).unwrap();
        let exes = v["server"]["executables"].as_object().unwrap();
        assert_eq!(exes.len(), 1);
        assert_eq!(exes["linux-amd64"], "server/dist/log-analytics-tantivy-linux-amd64");
        assert!(narrow(&a, "plan9-amd64").is_err());
        assert_eq!(executable_path("windows", "arm64"), "server/dist/log-analytics-tantivy-windows-arm64.exe");
    }

    #[test]
    fn the_committed_manifest_is_the_generated_one() {
        let Ok(committed) = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/plugin.json")) else {
            return;
        };
        let parsed: Manifest = serde_json::from_str(&committed).unwrap();
        let rebuilt = build(parsed.webapp.clone().map(|w| w.shared).unwrap_or_default());
        assert_eq!(parsed, rebuilt, "run `cargo run --bin manifest` to regenerate plugin.json");
    }
}
