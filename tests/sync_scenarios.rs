//! File tracking scenarios: what the index holds after logs grow, rotate,
//! are copied, compressed, rewritten or read across a restart.

mod common;

use common::*;

#[tokio::test]
async fn appended_lines_are_indexed_once() {
    let bed = Bed::new();
    let log = bed.log("access.log");
    write(&log, &lines(0, 100));
    bed.round().await;
    assert_eq!(bed.docs(), 100);

    append(&log, &lines(100, 150));
    bed.round().await;
    assert_eq!(bed.docs(), 150);

    // Nothing changed, nothing is read
    bed.round().await;
    assert_eq!(bed.docs(), 150);
    bed.assert_no_duplicates();
}

#[tokio::test]
async fn a_partial_last_line_waits_for_its_newline() {
    let bed = Bed::new();
    let log = bed.log("access.log");
    let partial = line(10);
    write(&log, &format!("{}{}", lines(0, 10), &partial[..40]));
    bed.round().await;
    assert_eq!(bed.docs(), 10);

    write(&log, &format!("{}{}\n", lines(0, 10), partial));
    bed.round().await;
    assert_eq!(bed.docs(), 11);
    bed.assert_no_duplicates();
}

#[tokio::test]
async fn rename_rotation_reads_the_unindexed_tail_once() {
    let bed = Bed::new();
    let log = bed.log("access.log");
    write(&log, &lines(0, 100));
    bed.round().await;

    // Lines arrive, then the log is rotated before the next round
    append(&log, &lines(100, 120));
    std::fs::rename(&log, bed.log("access.log.1")).unwrap();
    write(&log, &lines(120, 150));
    bed.round().await;
    assert_eq!(bed.docs(), 150);
    bed.assert_no_duplicates();

    bed.round().await;
    assert_eq!(bed.docs(), 150);
}

#[tokio::test]
async fn rename_rotation_without_a_tail_adds_only_the_new_file() {
    let bed = Bed::new();
    let log = bed.log("access.log");
    write(&log, &lines(0, 100));
    bed.round().await;

    std::fs::rename(&log, bed.log("access.log.1")).unwrap();
    write(&log, &lines(100, 130));
    bed.round().await;
    assert_eq!(bed.docs(), 130);
    bed.assert_no_duplicates();
}

#[tokio::test]
async fn copytruncate_continues_from_the_copy() {
    let bed = Bed::new();
    let log = bed.log("access.log");
    write(&log, &lines(0, 100));
    bed.round().await;

    // The copy holds 20 lines the index has not seen, then the log is truncated
    append(&log, &lines(100, 120));
    std::fs::copy(&log, bed.log("access.log.1")).unwrap();
    write(&log, "");
    append(&log, &lines(120, 140));
    bed.round().await;
    assert_eq!(bed.docs(), 140);
    bed.assert_no_duplicates();
}

#[tokio::test]
async fn copytruncate_to_an_empty_file_waits_for_the_first_line() {
    let bed = Bed::new();
    let log = bed.log("access.log");
    write(&log, &lines(0, 100));
    bed.round().await;

    std::fs::copy(&log, bed.log("access.log.1")).unwrap();
    write(&log, "");
    bed.round().await;
    assert_eq!(bed.docs(), 100);

    append(&log, &lines(100, 105));
    bed.round().await;
    assert_eq!(bed.docs(), 105);
    bed.assert_no_duplicates();
}

#[tokio::test]
async fn a_compressed_copy_of_an_indexed_file_adds_nothing() {
    let bed = Bed::new();
    let log = bed.log("access.log");
    let text = lines(0, 100);
    write(&log, &text);
    bed.round().await;

    std::fs::remove_file(&log).unwrap();
    gzip(&bed.log("access.log.1.gz"), &text);
    write(&log, &lines(100, 110));
    bed.round().await;
    assert_eq!(bed.docs(), 110);
    bed.assert_no_duplicates();
}

#[tokio::test]
async fn a_compressed_file_with_an_unindexed_tail_adds_the_tail() {
    let bed = Bed::new();
    let log = bed.log("access.log");
    write(&log, &lines(0, 100));
    bed.round().await;

    std::fs::remove_file(&log).unwrap();
    gzip(&bed.log("access.log.1.gz"), &lines(0, 120));
    write(&log, &lines(120, 130));
    bed.round().await;
    assert_eq!(bed.docs(), 130);
    bed.assert_no_duplicates();
}

#[tokio::test]
async fn two_rotations_between_rounds() {
    let bed = Bed::new();
    let log = bed.log("access.log");
    write(&log, &lines(0, 100));
    bed.round().await;

    // The first file rotates, a second one fills and rotates too
    std::fs::rename(&log, bed.log("access.log.1")).unwrap();
    write(&log, &lines(100, 150));
    std::fs::rename(bed.log("access.log.1"), bed.log("access.log.2")).unwrap();
    std::fs::rename(&log, bed.log("access.log.1")).unwrap();
    write(&log, &lines(150, 160));
    bed.round().await;
    assert_eq!(bed.docs(), 160);
    bed.assert_no_duplicates();
}

#[tokio::test]
async fn the_state_survives_a_restart() {
    let mut bed = Bed::new();
    let log = bed.log("access.log");
    write(&log, &lines(0, 100));
    bed.round().await;
    assert_eq!(bed.docs(), 100);

    bed.restart();
    assert_eq!(bed.docs(), 100);
    append(&log, &lines(100, 130));
    bed.round().await;
    assert_eq!(bed.docs(), 130);
    bed.assert_no_duplicates();
}

#[tokio::test]
async fn a_rewritten_file_replaces_its_documents() {
    let bed = Bed::new();
    let log = bed.log("access.log");
    write(&log, &lines(0, 100));
    bed.round().await;

    // Same first line, shorter and different after it
    write(&log, &format!("{}{}", lines(0, 1), lines(500, 530)));
    bed.round().await;
    assert_eq!(bed.docs(), 31);
    bed.assert_no_duplicates();
    let paths = bed.per_path();
    assert!(paths.contains_key("/p/0") && paths.contains_key("/p/500"));
    assert!(!paths.contains_key("/p/5"));
}

#[tokio::test]
async fn a_stopped_import_resumes_without_duplicates() {
    // Large enough that the import is still running when it is stopped
    const TOTAL: usize = 60_000;
    let mut bed = Bed::new();
    bed.engine.override_sizing(plugin_log_analytics_rs::sizing::Sizing { commit_every: 100, batch_lines: 10, ..SMALL });
    let log = bed.log("access.log");
    write(&log, &lines(0, TOTAL));

    let engine = bed.engine.clone();
    let runner = tokio::spawn({
        let engine = engine.clone();
        async move { engine.run_round(plugin_log_analytics_rs::engine::Scope::All, false).await }
    });
    // Stop after the import made some progress
    for _ in 0..500 {
        if engine.total_docs() > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    engine.cancel();
    runner.await.unwrap();
    assert!(engine.state().dirty, "a stopped import leaves the state dirty");

    bed.restart();
    bed.round().await;
    assert_eq!(bed.docs(), TOTAL as u64);
    bed.assert_no_duplicates();
    assert!(!bed.engine.state().dirty);
}

#[tokio::test]
async fn rebuild_reads_everything_again() {
    let bed = Bed::new();
    let log = bed.log("access.log");
    write(&log, &lines(0, 100));
    bed.round().await;
    append(&log, &lines(100, 120));
    bed.engine.run_round(plugin_log_analytics_rs::engine::Scope::All, true).await;
    assert_eq!(bed.docs(), 120);
    bed.assert_no_duplicates();

    bed.engine.run_round(plugin_log_analytics_rs::engine::Scope::Group(bed.group()), true).await;
    assert_eq!(bed.docs(), 120);
    bed.assert_no_duplicates();
}
