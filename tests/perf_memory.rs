#![allow(unexpected_cfgs)]
//! Memory of the process after a bulk import, per sizing tier. The test runs on
//! request with the real allocator of the plugin:
//!
//!     PERF_TIER=small cargo test --release --test perf_memory -- --ignored --nocapture
//!
//! `PERF_TIER` is `small`, `mid` or `large`. `PERF_DATA` names the dataset
//! directory like in `perf_dataset`, `PERF_MI` sets mimalloc options as
//! `name=value,...` (`purge_delay`, `purge_decommits`, `arena_purge_mult`,
//! `abandoned_page_purge`).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use plugin_log_analytics_tantivy::config::Dirs;
use plugin_log_analytics_tantivy::engine::{Engine, Scope};
use plugin_log_analytics_tantivy::logs::HostLog;
use plugin_log_analytics_tantivy::sizing;
use plugin_log_analytics_tantivy::sys;

#[cfg(not(any(windows, sysalloc)))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn dataset() -> Option<PathBuf> {
    let dir = PathBuf::from(
        std::env::var("PERF_DATA").unwrap_or_else(|_| "/Volumes/Working/Git/.tmp-635799/perf/data".into()),
    );
    dir.join("meta.json").exists().then_some(dir)
}

/// Resident set size in MiB from the platform, and what the platform calls
/// the memory the process owns: anonymous memory on Linux, the footprint on macOS.
fn sample(label: &str) {
    let pid = std::process::id().to_string();
    let rss_kb = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().unwrap_or(0))
        .unwrap_or(0);
    let owned = if cfg!(target_os = "linux") {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let field = |name: &str| {
            status
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
                / 1024
        };
        format!("anon {} MB, file {} MB", field("RssAnon:"), field("RssFile:"))
    } else {
        let out = std::process::Command::new("footprint").args(["-p", &pid]).output();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        let footprint = text
            .lines()
            .find(|l| l.contains("Footprint:"))
            .map(|l| l.split("Footprint:").nth(1).unwrap_or("").trim().to_owned())
            .unwrap_or_default();
        format!("footprint {footprint}")
    };
    eprintln!("{label:<34} rss {} MB, {owned}", rss_kb / 1024);
    if cfg!(target_os = "macos") && std::env::var_os("PERF_VMMAP").is_some() {
        let out = std::process::Command::new("vmmap").args(["-summary", &pid]).output();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        for line in text.lines().filter(|l| {
            ["mapped file", "MALLOC", "TOTAL", "Physical footprint", "REGION TYPE", "VM_ALLOCATE", "MALLOC_LARGE"]
                .iter()
                .any(|k| l.contains(k))
        }) {
            eprintln!("    {line}");
        }
    }
}

#[cfg(not(any(windows, target_arch = "loongarch64")))]
fn mimalloc_options() {
    use libmimalloc_sys::mi_option_set;
    let names = [("purge_decommits", 5), ("abandoned_page_purge", 12), ("purge_delay", 15), ("arena_purge_mult", 24)];
    for pair in std::env::var("PERF_MI").unwrap_or_default().split(',').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').expect("name=value");
        let (_, id) = names.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("unknown option {name}"));
        // SAFETY: the option numbers come from the mimalloc header of the bundled version.
        unsafe { mi_option_set(*id, value.parse().expect("a number")) };
        eprintln!("mimalloc {name} = {value}");
    }
}

#[tokio::test]
#[ignore = "needs the performance dataset, run with --release -- --ignored"]
async fn memory_after_a_bulk_import() {
    let Some(data) = dataset() else {
        eprintln!("the performance dataset is not there, skipped");
        return;
    };
    #[cfg(not(any(windows, target_arch = "loongarch64")))]
    mimalloc_options();
    let tier = std::env::var("PERF_TIER").unwrap_or_else(|_| "large".into());
    let memory = match tier.as_str() {
        "small" => 512 << 20,
        "mid" => 2u64 << 30,
        _ => 8u64 << 30,
    };
    let sizing = sizing::sizing(Some(memory), 4);
    eprintln!("tier {tier}: {sizing:?}");

    let work = tempfile::Builder::new()
        .prefix("perf-memory")
        .tempdir_in(
            std::env::var("PERF_WORK").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/target").into()),
        )
        .unwrap();
    sample("process start");
    let engine = Engine::open(Dirs::new(work.path())).unwrap();
    engine.override_sizing(sizing);
    engine.hostlogs.set(vec![HostLog {
        path: data.join("access.log").to_string_lossy().into_owned(),
        kind: "access".into(),
        source: "config".into(),
        config_file: String::new(),
    }]);
    sample("engine open");

    let started = Instant::now();
    let report = engine.run_round(Scope::All, false).await.expect("a round ran");
    eprintln!(
        "indexed {} documents in {:.1} s, peak RSS {} MB, index {} MB",
        report.docs,
        started.elapsed().as_secs_f64(),
        sys::peak_rss_mb(),
        engine.store.disk_size() >> 20
    );
    sample("after the round (release done)");
    sys::release_memory();
    sample("after release_memory again");
    tokio::time::sleep(Duration::from_secs(3)).await;
    sample("3 s later");
    // A request that touches the index, then idle again
    let searcher = engine.store.searcher();
    let _ = engine.dashboard(&data.join("access.log").to_string_lossy(), 1_788_048_000, 1_790_640_000).unwrap();
    drop(searcher);
    sample("after a dashboard request");
    sys::release_memory();
    sample("after a dashboard + release");
    if std::env::var_os("PERF_DROP").is_some() {
        drop(engine);
        sample("engine dropped");
        sys::release_memory();
        sample("engine dropped, released");
    }
    if let Some(secs) = std::env::var("PERF_HOLD").ok().and_then(|v| v.parse::<u64>().ok()) {
        eprintln!("pid {}, holding {secs} s", std::process::id());
        tokio::time::sleep(Duration::from_secs(secs)).await;
    }
}
