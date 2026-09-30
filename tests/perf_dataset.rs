//! The figures of the performance dataset. They were validated against the Go
//! plugin: 30 day dashboard PV 1,400,000 and UV 59,983, the last 7 days 326,923
//! and 54,091, and 1,399,999 hits for the search over the 30 days (the search
//! range excludes its last second).
//!
//! The dataset is large, so the test only runs on request:
//!
//!     cargo test --release --test perf_dataset -- --ignored --nocapture
//!
//! `PERF_DATA` names the dataset directory, it holds `access.log`,
//! `access.log.1`, `access.log.2.gz` and `meta.json`.

use std::path::PathBuf;
use std::time::Instant;

use plugin_log_analytics_rs::analytics;
use plugin_log_analytics_rs::config::Dirs;
use plugin_log_analytics_rs::engine::{Engine, Scope};
use plugin_log_analytics_rs::logs::HostLog;
use plugin_log_analytics_rs::query::Filter;
use plugin_log_analytics_rs::search::{self, SearchParams};
use plugin_log_analytics_rs::sys;

fn dataset() -> Option<PathBuf> {
    let dir = PathBuf::from(
        std::env::var("PERF_DATA").unwrap_or_else(|_| "/Volumes/Working/Git/.tmp-635799/perf/data".into()),
    );
    dir.join("meta.json").exists().then_some(dir)
}

fn day(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0).unwrap().format("%Y-%m-%d").to_string()
}

fn day_start(ts: i64) -> i64 {
    ts - ts.rem_euclid(86400)
}

#[tokio::test]
#[ignore = "needs the performance dataset, run with --release -- --ignored"]
async fn dashboard_and_search_match_the_validated_figures() {
    let Some(data) = dataset() else {
        eprintln!("the performance dataset is not there, skipped");
        return;
    };
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(data.join("meta.json")).unwrap()).unwrap();
    let (first, last) = (meta["first_ts"].as_i64().unwrap(), meta["last_ts"].as_i64().unwrap());

    let work = tempfile::Builder::new()
        .prefix("perf-index")
        .tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/target"))
        .unwrap();
    let engine = Engine::open(Dirs::new(work.path())).unwrap();
    engine.hostlogs.set(vec![HostLog {
        path: data.join("access.log").to_string_lossy().into_owned(),
        kind: "access".into(),
        source: "config".into(),
        config_file: String::new(),
    }]);
    let group = data.join("access.log").to_string_lossy().into_owned();

    let started = Instant::now();
    let report = engine.run_round(Scope::All, false).await.expect("a round ran");
    eprintln!(
        "indexed {} documents in {:.1} s, peak RSS {} MB, index {} MB",
        report.docs,
        started.elapsed().as_secs_f64(),
        sys::peak_rss_mb(),
        engine.store.disk_size() >> 20
    );
    let rss = || {
        let pid = std::process::id().to_string();
        let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().unwrap_or(0) / 1024
    };
    eprintln!("resident after the round {} MB", rss());
    sys::release_memory();
    eprintln!("resident after release_memory {} MB", rss());
    assert_eq!(report.failed, 0);
    assert_eq!(report.docs, 1_400_000);
    assert_eq!(engine.group_stats(&group).docs, 1_400_000);

    let searcher = engine.store.searcher();
    let fields = engine.store.fields();

    let t = Instant::now();
    let d30 = analytics::dashboard(&searcher, fields, &group, day_start(first), day_start(last) + 86399).unwrap();
    eprintln!("dashboard 30 days: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    assert_eq!((d30.summary.total_pv, d30.summary.total_uv), (1_400_000, 59_983));

    let t = Instant::now();
    let d7 =
        analytics::dashboard(&searcher, fields, &group, day_start(last - 7 * 86400), day_start(last) + 86399).unwrap();
    eprintln!("dashboard 7 days: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    assert_eq!((d7.summary.total_pv, d7.summary.total_uv), (326_923, 54_091));
    assert_eq!(day(day_start(first)).len(), 10);

    let t = Instant::now();
    let params = SearchParams {
        filter: Filter { groups: vec![group.clone()], start: Some(first), end: Some(last), ..Default::default() },
        limit: 50,
        sort_by: "timestamp".into(),
        descending: true,
        ..Default::default()
    };
    let out = search::search(&searcher, fields, &params).unwrap();
    eprintln!("search 30 days: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    assert_eq!(out.summary.docs, 1_399_999);
    assert_eq!(out.hits.len(), 50);
}
