//! Where the memory of an indexing round goes. A counting allocator over
//! mimalloc records the live bytes and the call sites of large allocations,
//! and the report lists the sites at the highest live total.
//!
//!     LOG_ANALYTICS_MEMORY_MB=200 PERF_DATA=... \
//!     cargo test --release --test memory_profile -- --ignored --nocapture

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use plugin_log_analytics_tantivy::config::Dirs;
use plugin_log_analytics_tantivy::engine::{Engine, Scope};
use plugin_log_analytics_tantivy::logs::HostLog;

/// Allocations from this size on are recorded with their call site.
const BIG: usize = 256 * 1024;
/// Smaller allocations are sampled: one per this many bytes allocated, and
/// the sample stands for that many bytes.
const SAMPLE_EVERY: usize = 256 * 1024;

static SAMPLED: AtomicUsize = AtomicUsize::new(0);
static SMALL_LIVE: Mutex<Option<HashMap<usize, String>>> = Mutex::new(None);

struct Tracking;

static LIVE: AtomicUsize = AtomicUsize::new(0);
/// Bytes the profiler holds itself, mostly the symbol cache of the backtraces.
static OWN: AtomicUsize = AtomicUsize::new(0);
static ON: AtomicBool = AtomicBool::new(false);
static BIG_LIVE: Mutex<Option<HashMap<usize, (usize, String)>>> = Mutex::new(None);

thread_local! {
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

fn site() -> String {
    let text = std::backtrace::Backtrace::force_capture().to_string();
    let frames: Vec<String> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("at "))
        .filter(|p| !p.contains("memory_profile.rs"))
        .filter_map(|p| {
            if let Some(own) = p.strip_prefix("./") {
                Some(own.to_owned())
            } else {
                p.find("/tantivy").map(|i| p[i + 1..].to_owned())
            }
        })
        .take(5)
        .collect();
    if frames.is_empty() {
        "(outside the crates)".to_owned()
    } else {
        frames.join(" < ")
    }
}

fn record(ptr: *mut u8, size: usize) {
    if !ON.load(Ordering::Relaxed) || INSIDE.with(Cell::get) {
        return;
    }
    if size < BIG {
        let before = SAMPLED.fetch_add(size, Ordering::Relaxed);
        if before / SAMPLE_EVERY == (before + size) / SAMPLE_EVERY {
            return;
        }
        INSIDE.with(|c| c.set(true));
        let s = site();
        if let Some(map) = SMALL_LIVE.lock().unwrap().as_mut() {
            map.insert(ptr as usize, s);
        }
        INSIDE.with(|c| c.set(false));
        return;
    }
    INSIDE.with(|c| c.set(true));
    let s = site();
    if let Some(map) = BIG_LIVE.lock().unwrap().as_mut() {
        map.insert(ptr as usize, (size, s));
    }
    INSIDE.with(|c| c.set(false));
}

fn forget(ptr: *mut u8, size: usize) {
    if INSIDE.with(Cell::get) {
        return;
    }
    INSIDE.with(|c| c.set(true));
    if size < BIG {
        if let Some(map) = SMALL_LIVE.lock().unwrap().as_mut() {
            map.remove(&(ptr as usize));
        }
        INSIDE.with(|c| c.set(false));
        return;
    }
    if let Some(map) = BIG_LIVE.lock().unwrap().as_mut() {
        map.remove(&(ptr as usize));
    }
    INSIDE.with(|c| c.set(false));
}

unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { mimalloc::MiMalloc.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
            if INSIDE.with(Cell::get) {
                OWN.fetch_add(layout.size(), Ordering::Relaxed);
            }
            record(p, layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        forget(ptr, layout.size());
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        if INSIDE.with(Cell::get) {
            OWN.fetch_sub(layout.size().min(OWN.load(Ordering::Relaxed)), Ordering::Relaxed);
        }
        unsafe { mimalloc::MiMalloc.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { mimalloc::MiMalloc.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            forget(ptr, layout.size());
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            LIVE.fetch_add(new_size, Ordering::Relaxed);
            if INSIDE.with(Cell::get) {
                OWN.fetch_add(new_size, Ordering::Relaxed);
                OWN.fetch_sub(layout.size().min(OWN.load(Ordering::Relaxed)), Ordering::Relaxed);
            }
            record(p, new_size);
        }
        p
    }
}

#[global_allocator]
static GLOBAL: Tracking = Tracking;

/// Live bytes of the code under test.
fn live() -> usize {
    LIVE.load(Ordering::Relaxed).saturating_sub(OWN.load(Ordering::Relaxed))
}

fn rss_anon_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| l.strip_prefix("RssAnon:")?.split_whitespace().next()?.parse::<u64>().ok())
        })
        .unwrap_or(0)
        / 1024
}

struct Snapshot {
    at: Duration,
    live: usize,
    rss: u64,
    sites: Vec<(String, usize, usize)>,
}

fn snapshot(started: Instant) -> Snapshot {
    INSIDE.with(|c| c.set(true));
    let mut by_site: HashMap<String, (usize, usize)> = HashMap::new();
    if let Some(map) = BIG_LIVE.lock().unwrap().as_ref() {
        for (size, s) in map.values() {
            let e = by_site.entry(s.clone()).or_default();
            e.0 += size;
            e.1 += 1;
        }
    }
    if let Some(map) = SMALL_LIVE.lock().unwrap().as_ref() {
        for s in map.values() {
            let e = by_site.entry(format!("[small] {s}")).or_default();
            e.0 += SAMPLE_EVERY;
            e.1 += 1;
        }
    }
    let mut sites: Vec<(String, usize, usize)> = by_site.into_iter().map(|(s, (b, n))| (s, b, n)).collect();
    sites.sort_by_key(|b| std::cmp::Reverse(b.1));
    INSIDE.with(|c| c.set(false));
    Snapshot { at: started.elapsed(), live: live(), rss: rss_anon_mb(), sites }
}

fn print(label: &str, s: &Snapshot) {
    let big: usize = s.sites.iter().filter(|x| !x.0.starts_with("[small]")).map(|x| x.1).sum();
    eprintln!(
        "\n== {label} at {:.1} s: live {} MB, large {} MB, small {} MB, RssAnon {} MB (profiler {} MB)",
        s.at.as_secs_f64(),
        s.live >> 20,
        big >> 20,
        (s.live - big.min(s.live)) >> 20,
        s.rss,
        OWN.load(Ordering::Relaxed) >> 20
    );
    for (site, bytes, n) in s.sites.iter().take(25) {
        eprintln!("{:>6.1} MB {:>6}x  {site}", *bytes as f64 / 1048576.0, n);
    }
}

#[tokio::test]
#[ignore = "needs the performance dataset, run with --release -- --ignored"]
async fn where_the_memory_of_an_indexing_round_goes() {
    let data = PathBuf::from(std::env::var("PERF_DATA").expect("PERF_DATA names the dataset"));
    let work = tempfile::Builder::new()
        .prefix("mem-index")
        .tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/target"))
        .unwrap();
    let engine = Engine::open(Dirs::new(work.path())).unwrap();
    engine.hostlogs.set(vec![HostLog {
        path: data.join("access.log").to_string_lossy().into_owned(),
        kind: "access".into(),
        source: "config".into(),
        config_file: String::new(),
    }]);
    if std::env::var("TUNE_ALLOCATOR").is_ok_and(|v| v == "1") {
        plugin_log_analytics_tantivy::sys::tune_allocator();
    }
    eprintln!("sizing {:?}", plugin_log_analytics_tantivy::sizing::current());
    eprintln!("before the round: live {} MB, RssAnon {} MB", LIVE.load(Ordering::Relaxed) >> 20, rss_anon_mb());

    *BIG_LIVE.lock().unwrap() = Some(HashMap::new());
    *SMALL_LIVE.lock().unwrap() = Some(HashMap::new());
    // PROFILE_SITES=0 counts the bytes only, without the cost of backtraces
    ON.store(std::env::var("PROFILE_SITES").map_or(true, |v| v != "0"), Ordering::SeqCst);
    let started = Instant::now();
    let done = std::sync::Arc::new(AtomicBool::new(false));
    let watcher = {
        let done = done.clone();
        std::thread::spawn(move || {
            let mut peak_live: Option<Snapshot> = None;
            let mut peak_rss: Option<Snapshot> = None;
            let mut timeline = Vec::new();
            let mut tick = 0u32;
            while !done.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(100));
                let live = live();
                let rss = rss_anon_mb();
                tick += 1;
                if tick.is_multiple_of(10) {
                    timeline.push((started.elapsed().as_secs(), live >> 20, rss));
                }
                if peak_live.as_ref().is_none_or(|p| live > p.live + (2 << 20)) {
                    peak_live = Some(snapshot(started));
                }
                if peak_rss.as_ref().is_none_or(|p| rss > p.rss + 2) {
                    peak_rss = Some(snapshot(started));
                }
            }
            (peak_live, peak_rss, timeline)
        })
    };

    let report = engine.run_round(Scope::All, false).await.expect("a round ran");
    done.store(true, Ordering::SeqCst);
    let (peak_live, peak_rss, timeline) = watcher.join().unwrap();
    ON.store(false, Ordering::SeqCst);

    eprintln!("\nindexed {} documents in {:.1} s", report.docs, started.elapsed().as_secs_f64());
    eprintln!("timeline (s, live MB, RssAnon MB): {timeline:?}");
    if let Some(s) = &peak_live {
        print("highest live heap", s);
    }
    if let Some(s) = &peak_rss {
        print("highest RssAnon", s);
    }
    let end = snapshot(started);
    print("after the round", &end);

    let group = data.join("access.log").to_string_lossy().into_owned();
    if let Some(r) = engine.rollup_of(&group) {
        let ips: usize = r.hours.values().map(|h| h.ips.len()).sum();
        let terms: Vec<usize> = (0..4).map(|k| r.hours.values().map(|h| h.terms[k].len()).sum()).collect();
        let names: Vec<usize> = r.names.iter().map(|n| n.len()).collect();
        eprintln!(
            "rollup: {} hours, {} MB estimated, ips {ips}, terms {terms:?}, names {names:?}",
            r.hours.len(),
            r.bytes_used() >> 20
        );
    }
    let before = live();
    drop(engine);
    eprintln!("dropping the engine freed {} MB, live {} MB", (before.saturating_sub(live())) >> 20, live() >> 20);
}
