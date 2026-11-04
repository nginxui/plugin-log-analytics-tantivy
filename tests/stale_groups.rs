//! Log groups the host stopped listing and whose log file is gone.

mod common;

use std::collections::HashSet;
use std::path::Path;

use common::*;
use plugin_log_analytics_tantivy::logs::HostLog;

fn host_log(path: &Path) -> HostLog {
    HostLog {
        path: path.to_string_lossy().into_owned(),
        kind: "access".into(),
        source: "config".into(),
        config_file: String::new(),
    }
}

/// A bed with 10 documents in the listed access log and 5 in a deleted site:
/// the host no longer lists it and its file is gone.
async fn bed_with_deleted_site() -> (Bed, String) {
    let bed = Bed::new();
    let live = bed.log("access.log");
    let site = bed.log("site.log");
    write(&live, &lines(0, 10));
    write(&site, &lines(100, 105));
    bed.engine.hostlogs.set(vec![host_log(&live), host_log(&site)]);
    bed.round().await;
    assert_eq!(bed.engine.total_docs(), 15);

    bed.engine.hostlogs.set(vec![host_log(&live)]);
    std::fs::remove_file(&site).unwrap();
    (bed, site.to_string_lossy().into_owned())
}

fn pv(bed: &Bed) -> u64 {
    bed.engine.dashboard("", 0, 4_000_000_000).unwrap().summary.total_pv
}

#[tokio::test]
async fn a_group_stale_in_two_rounds_is_removed() {
    let (bed, site) = bed_with_deleted_site().await;
    assert_eq!(pv(&bed), 15);

    let first = bed.engine.prune_stale_groups(HashSet::new()).await;
    assert_eq!(first, HashSet::from([site.clone()]));
    assert_eq!(bed.engine.total_docs(), 15, "one round is not enough");

    let second = bed.engine.prune_stale_groups(first).await;
    assert!(second.is_empty());
    assert_eq!(bed.engine.total_docs(), 10);
    assert_eq!(bed.engine.group_stats(&site).docs, 0);
    assert!(bed.engine.state().group(&site).is_none());
    assert!(!bed.engine.rollups.groups().contains(&site));

    assert_eq!(bed.docs(), 10, "the listed group stays");
    assert_eq!(pv(&bed), 10, "the dashboard of all groups no longer counts it");
    assert!(bed.log("access.log").exists());
}

#[tokio::test]
async fn the_removal_survives_a_restart() {
    let (mut bed, site) = bed_with_deleted_site().await;
    let first = bed.engine.prune_stale_groups(HashSet::new()).await;
    bed.engine.prune_stale_groups(first).await;

    bed.restart();
    assert!(bed.engine.state().group(&site).is_none());
    assert_eq!(bed.engine.total_docs(), 10);
}

#[tokio::test]
async fn a_group_that_came_back_starts_over() {
    let (bed, site) = bed_with_deleted_site().await;
    let live = bed.log("access.log");
    let first = bed.engine.prune_stale_groups(HashSet::new()).await;

    bed.engine.hostlogs.set(vec![host_log(&live), host_log(Path::new(&site))]);
    let second = bed.engine.prune_stale_groups(first).await;
    assert!(second.is_empty());
    assert_eq!(bed.engine.total_docs(), 15);

    // Stale again: it has to wait for a second round again.
    bed.engine.hostlogs.set(vec![host_log(&live)]);
    let third = bed.engine.prune_stale_groups(second).await;
    assert_eq!(third, HashSet::from([site]));
    assert_eq!(bed.engine.total_docs(), 15);
}

#[tokio::test]
async fn a_log_still_on_disk_or_still_listed_stays() {
    let bed = Bed::new();
    let live = bed.log("access.log");
    let unlisted = bed.log("unlisted.log");
    let missing = bed.log("rotated-away.log");
    write(&live, &lines(0, 10));
    write(&unlisted, &lines(100, 105));
    write(&missing, &lines(200, 203));
    bed.engine.hostlogs.set(vec![host_log(&live), host_log(&unlisted), host_log(&missing)]);
    bed.round().await;

    // One is no longer listed but on disk, the other listed but missing.
    bed.engine.hostlogs.set(vec![host_log(&live), host_log(&missing)]);
    std::fs::remove_file(&missing).unwrap();

    let first = bed.engine.prune_stale_groups(HashSet::new()).await;
    assert!(first.is_empty());
    let everything: HashSet<String> = bed.engine.state().groups.into_keys().collect();
    assert!(bed.engine.prune_stale_groups(everything).await.is_empty());
    assert_eq!(bed.engine.total_docs(), 18);
}

#[tokio::test]
async fn an_empty_host_list_removes_nothing() {
    let (bed, site) = bed_with_deleted_site().await;
    bed.engine.hostlogs.set(vec![]);

    let seen = HashSet::from([site]);
    assert_eq!(bed.engine.prune_stale_groups(seen.clone()).await, seen);
    assert_eq!(bed.engine.total_docs(), 15);
}

#[tokio::test]
async fn a_round_without_a_stale_group_keeps_the_index_as_it_is() {
    let bed = Bed::new();
    write(&bed.log("access.log"), &lines(0, 10));
    bed.round().await;
    let state = bed.engine.state();

    assert!(bed.engine.prune_stale_groups(HashSet::new()).await.is_empty());
    assert_eq!(bed.engine.state(), state);
    assert_eq!(bed.docs(), 10);
}

#[tokio::test]
async fn the_removal_keeps_the_dirty_mark_of_a_stopped_import() {
    // Large enough that the import is still running when it is stopped
    const TOTAL: usize = 60_000;
    let mut bed = Bed::new();
    let live = bed.log("access.log");
    let site = bed.log("site.log");
    write(&site, &lines(100_000, 100_005));
    bed.engine.hostlogs.set(vec![host_log(&site)]);
    bed.round().await;
    bed.engine.hostlogs.set(vec![host_log(&live), host_log(&site)]);

    bed.engine.override_sizing(plugin_log_analytics_tantivy::sizing::Sizing {
        commit_every: 100,
        batch_lines: 10,
        ..SMALL
    });
    write(&live, &lines(0, TOTAL));
    let engine = bed.engine.clone();
    let runner = tokio::spawn({
        let engine = engine.clone();
        async move { engine.run_round(plugin_log_analytics_tantivy::engine::Scope::All, false).await }
    });
    for _ in 0..500 {
        if engine.group_stats(&bed.group()).docs > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    engine.cancel();
    runner.await.unwrap();
    assert!(engine.state().dirty, "a stopped import leaves the state dirty");

    // The restart lists the access log only, and the site is gone.
    bed.restart();
    std::fs::remove_file(&site).unwrap();
    let group = site.to_string_lossy().into_owned();
    let first = bed.engine.prune_stale_groups(HashSet::new()).await;
    assert!(bed.engine.prune_stale_groups(first).await.is_empty());
    assert!(bed.engine.state().group(&group).is_none());
    assert!(bed.engine.state().dirty, "the import still has to clean up after itself");

    bed.round().await;
    assert_eq!(bed.docs(), TOTAL as u64);
    bed.assert_no_duplicates();
    assert!(!bed.engine.state().dirty);
}
