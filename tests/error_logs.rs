//! Error logs: indexed beside the access log, searched by level and text, and
//! kept out of the rollups.

mod common;

use common::*;
use plugin_log_analytics_tantivy::logs::HostLog;
use plugin_log_analytics_tantivy::query::Filter;
use plugin_log_analytics_tantivy::search::{self, EntryLoader, SearchParams};

const ERROR_LINES: &str = r#"2026/09/07 15:32:29 [error] 12#12: *1 open() "/srv/favicon.ico" failed (2: No such file or directory), client: 203.0.113.9, server: example.com, request: "GET /favicon.ico HTTP/1.1", host: "example.com"
2026/09/07 15:32:30 [warn] 12#12: *2 an upstream response is buffered to a temporary file, client: 198.51.100.4, server: example.com, request: "POST /upload HTTP/1.1", upstream: "http://127.0.0.1:9000/upload", host: "example.com"
2026/09/07 15:33:00 [notice] 1#1: signal process started
 a line that continues the previous message
2026/09/07 15:34:00 [crit] 12#12: *3 SSL_do_handshake() failed, client: 203.0.113.9, server: 0.0.0.0:443
"#;

fn with_error_log(bed: &Bed) -> String {
    let access = bed.group();
    let error = bed.log("error.log").to_string_lossy().into_owned();
    bed.engine.hostlogs.set(vec![
        HostLog { path: access, kind: "access".into(), source: "config".into(), config_file: String::new() },
        HostLog { path: error.clone(), kind: "error".into(), source: "config".into(), config_file: String::new() },
    ]);
    error
}

#[tokio::test]
async fn an_error_log_is_indexed_and_searched_by_level() {
    let bed = Bed::new();
    write(&bed.log("access.log"), &lines(0, 20));
    write(&bed.log("error.log"), ERROR_LINES);
    let error = with_error_log(&bed);
    bed.round().await;

    // The continuation line is not an entry of its own
    assert_eq!(bed.engine.group_stats(&error).docs, 4);
    assert_eq!(bed.docs(), 20);
    assert!(bed.engine.rollups.get(&error).is_none(), "an error log has no rollup");

    let searcher = bed.engine.store.searcher();
    let fields = bed.engine.store.fields();
    let find = |text: &str, levels: &[&str]| {
        let filter = Filter {
            text: text.into(),
            groups: vec![error.clone()],
            levels: levels.iter().map(|l| (*l).to_owned()).collect(),
            ..Default::default()
        };
        let params = SearchParams { filter, limit: 10, descending: true, ..Default::default() };
        search::search(&searcher, fields, &params).unwrap()
    };
    assert_eq!(find("", &["error", "crit"]).summary.docs, 2);
    assert_eq!(find("level:warn", &[]).summary.docs, 1);
    assert_eq!(find("ip:203.0.113.9", &[]).summary.docs, 2);
    assert_eq!(find("handshake", &[]).summary.docs, 1);

    let out = find("path:/upload", &[]);
    assert_eq!(out.summary.docs, 1);
    let entry = EntryLoader::new(&searcher, fields).load(out.hits[0]).unwrap();
    assert_eq!(entry["level"], "warn");
    assert_eq!(entry["message"], "an upstream response is buffered to a temporary file");
    assert_eq!(entry["ip"], "198.51.100.4");
    assert_eq!(entry["method"], "POST");
    assert_eq!(entry["upstream"], "http://127.0.0.1:9000/upload");

    // Appended entries are read once
    append(&bed.log("error.log"), "2026/09/07 15:35:00 [error] 12#12: *4 late entry, client: 192.0.2.1\n");
    bed.round().await;
    assert_eq!(bed.engine.group_stats(&error).docs, 5);
}
