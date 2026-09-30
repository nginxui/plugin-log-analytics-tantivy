//! The hourly rollups agree with the index: through imports with several
//! commits, appends, rewrites, rebuilds, rotation and a restart, and on random
//! dashboard windows, where the figures have to equal the direct scan.

mod common;

use std::sync::Once;

use common::*;
use plugin_log_analytics_rs::analytics;
use plugin_log_analytics_rs::engine::Scope;
use plugin_log_analytics_rs::query::{self, Filter};
use plugin_log_analytics_rs::rollup::{RollupCollector, Slot};
use plugin_log_analytics_rs::sizing::Sizing;

/// A time zone with a half hour offset puts day boundaries inside an hour. A
/// zone set from outside wins, so other offsets can be tried with `TZ=...`.
fn zone() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if std::env::var_os("TZ").is_none() {
            std::env::set_var("TZ", "Asia/Kolkata");
        }
    });
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const BASE: i64 = 1_788_739_200 + 13 * 60 + 7; // 2026-09-07, not on an hour
const AGENTS: [&str; 6] = [
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0",
    "curl/8.4.0",
    "Googlebot/2.1 (+http://www.google.com/bot.html)",
    "-",
];

/// A line at `ts` with a client, path, status and size taken from `rng`.
fn random_line(rng: &mut Rng, ts: i64) -> String {
    let ip = match rng.below(10) {
        0 => format!("2001:db8::{:x}", rng.below(50)),
        _ => format!("{}.{}.{}.{}", 1 + rng.below(3), rng.below(4), rng.below(8), 1 + rng.below(40)),
    };
    let stamp = chrono::DateTime::from_timestamp(ts, 0).unwrap().format("%d/%b/%Y:%H:%M:%S +0000");
    let path = match rng.below(6) {
        0 => "/".to_owned(),
        1 => format!("/p/{}", rng.below(30)),
        2 => format!("/api/items?id={}", rng.below(200)),
        3 => "/about".to_owned(),
        _ => format!("/a/{}/{}", rng.below(5), rng.below(9)),
    };
    let status = [200, 200, 200, 301, 404, 500][rng.below(6) as usize];
    format!(
        "{ip} - - [{stamp}] \"GET {path} HTTP/1.1\" {status} {} \"-\" \"{}\" 0.0{} 0.0{}",
        rng.below(50_000),
        AGENTS[rng.below(AGENTS.len() as u64) as usize],
        rng.below(9) + 1,
        rng.below(9) + 1
    )
}

/// Lines over `[from, to)`, mostly in time order with some out of place.
fn log_text(rng: &mut Rng, from: i64, to: i64, lines: usize) -> (String, i64, i64) {
    let mut stamps: Vec<i64> = (0..lines).map(|_| from + rng.below((to - from) as u64) as i64).collect();
    stamps.sort_unstable();
    for _ in 0..lines / 20 {
        let (a, b) = (rng.below(lines as u64) as usize, rng.below(lines as u64) as usize);
        stamps.swap(a, b);
    }
    let (lo, hi) = (*stamps.iter().min().unwrap(), *stamps.iter().max().unwrap());
    (stamps.iter().map(|t| random_line(rng, *t) + "\n").collect(), lo, hi)
}

fn bed() -> Bed {
    zone();
    let bed = Bed::new();
    // Two parser threads and a commit every few hundred lines
    bed.engine.override_sizing(Sizing { heap_mb: 30, threads: 2, batch_lines: 50, commit_every: 400 });
    bed
}

/// The rollup of the group, which has to equal a fresh scan of the index.
fn assert_rollup_matches_index(bed: &Bed, context: &str) {
    let group = bed.group();
    let rollup = bed.engine.rollup_of(&group).expect("a rollup");
    let searcher = bed.engine.store.searcher();
    let q = query::build(bed.engine.store.fields(), &Filter { groups: vec![group], ..Default::default() });
    let scanned = searcher.search(q.as_ref(), &RollupCollector).unwrap();
    assert!(*rollup == scanned, "rollup and index differ: {context}");
}

/// Random windows, unaligned, on whole days, short and long, all equal.
fn assert_windows_match(bed: &Bed, rng: &mut Rng, lo: i64, hi: i64, windows: usize, context: &str) {
    let group = bed.group();
    let searcher = bed.engine.store.searcher();
    let fields = bed.engine.store.fields();
    let check = |start: i64, end: i64| {
        let direct = analytics::dashboard(&searcher, fields, &group, start, end).unwrap();
        let rolled = bed.engine.dashboard(&group, start, end).unwrap();
        assert!(direct == rolled, "{context}: window {start}..{end}\n direct {direct:?}\n rolled {rolled:?}");
    };
    // The window of the page: UTC midnight to the end of a UTC day
    let day = |t: i64| t - t.rem_euclid(86400);
    check(day(lo), day(hi) + 86399);
    check(day(lo + 86400), day(hi) + 86399);
    check(lo - 3 * 86400, hi + 3 * 86400);
    for _ in 0..windows {
        let start = lo - 7200 + rng.below((hi - lo + 14_400) as u64) as i64;
        let length = match rng.below(4) {
            0 => 60 + rng.below(7200),
            1 => 3600 + rng.below(86_400),
            2 => 86_400 + rng.below(3 * 86_400),
            _ => rng.below(8 * 86_400) + 1,
        } as i64;
        match rng.below(3) {
            // Whole minutes, then whole hours, then anything
            0 => check(start - start.rem_euclid(60), start - start.rem_euclid(60) + length),
            1 => check(start - start.rem_euclid(3600), start - start.rem_euclid(3600) + length),
            _ => check(start, start + length),
        }
    }
}

#[tokio::test]
async fn the_rollup_follows_an_import_with_several_commits() {
    let bed = bed();
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let (text, lo, hi) = log_text(&mut rng, BASE, BASE + 6 * 86_400, 12_000);
    write(&bed.log("access.log"), &text);
    bed.round().await;
    assert_eq!(bed.docs(), 12_000);

    assert_rollup_matches_index(&bed, "after the import");
    assert_windows_match(&bed, &mut rng, lo, hi, 150, "after the import");
}

#[tokio::test]
async fn appends_rewrites_rebuilds_and_rotation_keep_the_rollup_exact() {
    let bed = bed();
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    let log = bed.log("access.log");
    let (text, lo, mut hi) = log_text(&mut rng, BASE, BASE + 3 * 86_400, 4_000);
    write(&log, &text);
    bed.round().await;
    assert_rollup_matches_index(&bed, "first import");

    // Lines appended, some into hours that already have lines
    let (more, _, more_hi) = log_text(&mut rng, BASE + 2 * 86_400, BASE + 4 * 86_400, 1_500);
    append(&log, &more);
    hi = hi.max(more_hi);
    bed.round().await;
    assert_eq!(bed.docs(), 5_500);
    assert_rollup_matches_index(&bed, "after an append");
    assert_windows_match(&bed, &mut rng, lo, hi, 60, "after an append");

    // The file is rewritten from its start, with the same first line
    let (rest, rest_lo, rest_hi) = log_text(&mut rng, BASE + 86_400, BASE + 5 * 86_400, 2_500);
    let first_line = text.lines().next().unwrap();
    write(&log, &format!("{first_line}\n{rest}"));
    let (other_lo, other_hi) = (rest_lo.min(lo), rest_hi.max(BASE + 3 * 86_400));
    bed.round().await;
    assert_eq!(bed.docs(), 2_501);
    assert_rollup_matches_index(&bed, "after a rewrite");
    assert_windows_match(&bed, &mut rng, other_lo, other_hi, 60, "after a rewrite");

    // A rebuild reads it all again
    bed.engine.run_round(Scope::All, true).await;
    assert_eq!(bed.docs(), 2_501);
    assert_rollup_matches_index(&bed, "after a rebuild");
    assert_windows_match(&bed, &mut rng, other_lo, other_hi, 40, "after a rebuild");

    // The log rotates and a new one starts
    let (fresh, fresh_lo, fresh_hi) = log_text(&mut rng, BASE + 4 * 86_400, BASE + 6 * 86_400, 1_000);
    append(&log, "");
    std::fs::rename(&log, bed.log("access.log.1")).unwrap();
    write(&log, &fresh);
    bed.round().await;
    assert_eq!(bed.docs(), 3_501);
    assert_rollup_matches_index(&bed, "after a rotation");
    assert_windows_match(&bed, &mut rng, other_lo.min(fresh_lo), other_hi.max(fresh_hi), 60, "after a rotation");
}

#[tokio::test]
async fn a_restart_computes_the_rollup_from_the_index_on_the_first_request() {
    let mut bed = bed();
    let mut rng = Rng(0x0BAD_C0FF_EE12_3457);
    let (text, lo, hi) = log_text(&mut rng, BASE, BASE + 4 * 86_400, 3_000);
    write(&bed.log("access.log"), &text);
    bed.round().await;
    assert_rollup_matches_index(&bed, "before the restart");

    bed.restart();
    assert!(bed.engine.rollups.get(&bed.group()).is_none());
    assert_windows_match(&bed, &mut rng, lo, hi, 30, "after the restart");
    assert!(matches!(bed.engine.rollups.get(&bed.group()), Some(Slot::Ready(_))));
    assert_rollup_matches_index(&bed, "after the restart");
}

#[tokio::test]
async fn a_group_too_large_for_a_rollup_is_served_from_the_index() {
    let bed = bed();
    let mut rng = Rng(0x5EED_5EED_5EED_5EED);
    bed.engine.rollups.set_max_bytes(10_000);
    let (text, lo, hi) = log_text(&mut rng, BASE, BASE + 3 * 86_400, 3_000);
    write(&bed.log("access.log"), &text);
    bed.round().await;
    assert!(matches!(bed.engine.rollups.get(&bed.group()), Some(Slot::TooLarge)));
    assert!(bed.engine.rollup_of(&bed.group()).is_none());
    assert_windows_match(&bed, &mut rng, lo, hi, 20, "without a rollup");
}
