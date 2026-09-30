//! The indexing pipeline of one log group: a reader thread, parser threads
//! that feed one shared writer, and a committer that saves the progress of a
//! long import.
//!
//! The committed state of a file never runs ahead of its documents. Every
//! batch has a sequence number and the end offset of its last line, and the
//! position of a file advances only over the batches that finished without a
//! gap before them. A commit takes the writer lock, so no batch is half added
//! while the state is read.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tantivy::query::{BooleanQuery, Query, RangeQuery, TermQuery};
use tantivy::schema::{IndexRecordOption, Term};
use tantivy::{IndexWriter, TantivyDocument};

use crate::filesync::{self, FilePlan, LineReader, PlanOptions, DOC_FINGERPRINT_LEN};
use crate::geo::{Geo, GeoCache};
use crate::parse::{self, TimeCache};
use crate::rollup::{Delta, RollupBuilder, Rollups};
use crate::schema::Fields;
use crate::sizing::Sizing;
use crate::state::{FileRow, IndexState, Snapshot};
use crate::useragent::{self, UaCache};

/// The writer shared by the parser threads, which add documents under the read
/// lock, and the committer, which commits under the write lock.
pub type SharedWriter = Arc<RwLock<IndexWriter<TantivyDocument>>>;

/// Counters of a running group, read for progress events.
#[derive(Default)]
pub struct GroupProgress {
    pub bytes_read: AtomicU64,
    pub bytes_total: AtomicU64,
    pub docs: AtomicU64,
    pub failed: AtomicU64,
    /// Smallest and largest timestamp added, `i64::MAX` and `i64::MIN` until one is.
    pub min_ts: AtomicI64,
    pub max_ts: AtomicI64,
}

impl GroupProgress {
    pub fn new() -> Self {
        Self { min_ts: AtomicI64::new(i64::MAX), max_ts: AtomicI64::new(i64::MIN), ..Default::default() }
    }

    pub fn time_range(&self) -> Option<(i64, i64)> {
        let (lo, hi) = (self.min_ts.load(Ordering::Relaxed), self.max_ts.load(Ordering::Relaxed));
        (lo <= hi).then_some((lo, hi))
    }
}

/// What a finished group run produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GroupOutcome {
    pub docs: u64,
    pub failed: u64,
    pub bytes: u64,
    pub time_range: Option<(i64, i64)>,
}

/// Everything a group run needs from its round.
pub struct GroupRun<'a> {
    pub fields: &'a Fields,
    pub writer: &'a SharedWriter,
    pub geo: Arc<Geo>,
    pub sizing: Sizing,
    pub group: &'a str,
    pub files: &'a [PathBuf],
    pub snapshot: &'a Snapshot,
    /// The state the round is building. Rows are updated as files are read.
    pub working: &'a Mutex<IndexState>,
    pub progress: &'a GroupProgress,
    pub cancel: &'a AtomicBool,
    pub options: PlanOptions,
    /// The hourly rollups, which follow the documents that are added.
    pub rollups: &'a Rollups,
    /// Called with the state that was just committed and the changes of the
    /// rollups that belong to it.
    pub on_commit: &'a CommitHook<'a>,
}

/// What the owner of the index does after a commit.
pub type CommitHook<'a> = dyn Fn(IndexState, Delta) + Sync + 'a;

struct Batch {
    seq: u64,
    file: usize,
    fingerprint: Arc<str>,
    lines: Vec<(String, u64)>,
    end_offset: u64,
    last: bool,
}

/// The final size and time of each file, taken over when its last batch is in.
type Finals = Mutex<Vec<Option<FileRow>>>;

/// Tracks which batches finished and how far each file advanced.
struct Watermark {
    inner: Mutex<WatermarkInner>,
    drained: Condvar,
}

#[derive(Default)]
struct WatermarkInner {
    next: u64,
    sent: u64,
    done: BTreeMap<u64, (usize, u64, bool)>,
}

impl Watermark {
    fn new() -> Self {
        Self { inner: Mutex::new(WatermarkInner::default()), drained: Condvar::new() }
    }

    fn sent(&self) {
        self.inner.lock().expect("watermark").sent += 1;
    }

    /// Records a finished batch and returns the batches that became contiguous.
    fn complete(&self, seq: u64, file: usize, end_offset: u64, last: bool) -> Vec<(usize, u64, bool)> {
        let mut inner = self.inner.lock().expect("watermark");
        inner.done.insert(seq, (file, end_offset, last));
        let mut advanced = Vec::new();
        loop {
            let next = inner.next;
            let Some(entry) = inner.done.remove(&next) else { break };
            advanced.push(entry);
            inner.next += 1;
        }
        if inner.next == inner.sent {
            self.drained.notify_all();
        }
        advanced
    }

    /// Waits until every batch sent so far is finished.
    fn drain(&self) {
        let mut inner = self.inner.lock().expect("watermark");
        while inner.next < inner.sent {
            inner = self.drained.wait(inner).expect("watermark");
        }
    }
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The fingerprint every document of a content carries.
pub fn doc_fingerprint(fingerprint: &str) -> &str {
    &fingerprint[..DOC_FINGERPRINT_LEN.min(fingerprint.len())]
}

/// Query that matches the documents of a content from an offset on.
pub fn content_query(fields: &Fields, group: &str, fingerprint: &str, from: u64) -> Box<dyn Query> {
    let term = |field, text: &str| -> Box<dyn Query> {
        Box::new(TermQuery::new(Term::from_field_text(field, text), IndexRecordOption::Basic))
    };
    let range: Box<dyn Query> =
        Box::new(RangeQuery::new(Bound::Included(Term::from_field_u64(fields.off, from)), Bound::Unbounded));
    Box::new(BooleanQuery::intersection(vec![
        term(fields.main_log_path, group),
        term(fields.fp, doc_fingerprint(fingerprint)),
        range,
    ]))
}

/// Builds the document of one valid log line.
#[allow(clippy::too_many_arguments)]
pub fn build_doc(
    f: &Fields,
    entry: &parse::Entry<'_>,
    raw: &str,
    group: &str,
    fingerprint: &str,
    offset: u64,
    ua: &mut UaCache,
    geo: &mut GeoCache,
) -> TantivyDocument {
    let mut d = TantivyDocument::default();
    d.add_i64(f.ts, entry.ts);
    d.add_text(f.ip, entry.ip);
    if let Some(addr) = crate::qsyntax::ip_to_v6(entry.ip) {
        d.add_ip_addr(f.ip_addr, addr);
    }
    d.add_u64(f.status, entry.status);
    d.add_u64(f.bytes_sent, entry.bytes_sent);
    d.add_text(f.raw, raw);
    d.add_text(f.main_log_path, group);
    d.add_text(f.fp, doc_fingerprint(fingerprint));
    d.add_u64(f.off, offset);
    if !entry.method.is_empty() {
        d.add_text(f.method, entry.method);
    }
    if !entry.path.is_empty() {
        d.add_text(f.path, entry.path);
    }
    if !entry.referer.is_empty() {
        d.add_text(f.referer, entry.referer);
    }
    if !entry.user_agent.is_empty() {
        d.add_text(f.user_agent, entry.user_agent);
    }
    let info = ua.info(entry.user_agent);
    if !info.browser.is_empty() {
        d.add_text(f.browser, info.browser);
    }
    if !info.os.is_empty() {
        d.add_text(f.os, info.os);
    }
    if !info.device.is_empty() {
        d.add_text(f.device_type, info.device);
    }
    if let Some(loc) = geo.locate(entry.ip) {
        for (field, value) in [
            (f.region_code, &loc.region_code),
            (f.province, &loc.province),
            (f.city, &loc.city),
            (f.c1, &loc.c1),
            (f.c2, &loc.c2),
            (f.c3, &loc.c3),
            (f.c4, &loc.c4),
        ] {
            if !value.is_empty() {
                d.add_text(field, value);
            }
        }
    }
    if let Some(v) = entry.request_time.filter(|v| *v > 0.0) {
        d.add_f64(f.request_time, v);
    }
    if let Some(v) = entry.upstream_time {
        d.add_f64(f.upstream_time, v);
    }
    d
}

/// Commits with the state as payload and lets the caller reload its reader.
pub fn commit_state(
    writer: &SharedWriter,
    state: &Mutex<IndexState>,
    dirty: bool,
    rollups: &Rollups,
    on_commit: &CommitHook<'_>,
) -> Result<(), String> {
    let mut w = writer.write().expect("writer lock");
    let snapshot = {
        let mut s = state.lock().expect("state lock");
        s.dirty = dirty;
        s.clone()
    };
    // No batch is half added while the writer is locked, so the pending
    // rollup changes are exactly those of the documents this commit saves
    let delta = rollups.take_pending();
    let committed = (|| {
        let mut prepared = w.prepare_commit().map_err(|e| e.to_string())?;
        prepared.set_payload(&snapshot.to_payload());
        prepared.commit().map_err(|e| e.to_string())
    })();
    if let Err(e) = committed {
        rollups.restore(delta);
        return Err(e);
    }
    drop(w);
    on_commit(snapshot, delta);
    Ok(())
}

/// Indexes the files of one group. The caller commits the final state.
pub fn run_group(run: &GroupRun<'_>) -> Result<GroupOutcome, String> {
    let (tx, rx) = sync_channel::<Batch>(run.sizing.threads * 2);
    let rx = Mutex::new(rx);
    let watermark = Watermark::new();
    let error: Mutex<Option<String>> = Mutex::new(None);
    let committed_docs = AtomicU64::new(0);
    let round_high: Mutex<HashMap<String, u64>> = Mutex::new(HashMap::new());
    let finals: Finals = Mutex::new(vec![None; run.files.len()]);
    let failed = |e: String| {
        let mut slot = error.lock().expect("error slot");
        if slot.is_none() {
            *slot = Some(e);
        }
    };
    let parsers_done = AtomicBool::new(false);

    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            let tx = tx;
            let reader_ctx =
                ReaderCtx { run, tx: &tx, watermark: &watermark, round_high: &round_high, finals: &finals };
            let mut seq = 0u64;
            for (index, path) in run.files.iter().enumerate() {
                if run.cancel.load(Ordering::Relaxed) || error.lock().expect("error slot").is_some() {
                    break;
                }
                if let Err(e) = read_file(&reader_ctx, index, path, &mut seq) {
                    failed(format!("{}: {e}", path.display()));
                    break;
                }
            }
        });

        let workers: Vec<_> = (0..run.sizing.threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut ua = UaCache::new(useragent::parser().clone());
                    let mut geo = GeoCache::new(run.geo.clone());
                    let mut times = TimeCache::default();
                    loop {
                        let batch = { rx.lock().expect("receiver").recv() };
                        let Ok(batch) = batch else { break };
                        let w = run.writer.read().expect("writer lock");
                        let now = now_secs();
                        let (mut added, mut bad) = (0u64, 0u64);
                        let mut rollup = RollupBuilder::default();
                        for (line, offset) in &batch.lines {
                            let Some(entry) =
                                parse::parse_line(line, &mut times).filter(|e| parse::is_valid(e, line, now))
                            else {
                                bad += 1;
                                continue;
                            };
                            let doc = build_doc(
                                run.fields,
                                &entry,
                                line,
                                run.group,
                                &batch.fingerprint,
                                *offset,
                                &mut ua,
                                &mut geo,
                            );
                            if let Err(e) = w.add_document(doc) {
                                failed(format!("add document: {e}"));
                                break;
                            }
                            let info = ua.info(entry.user_agent);
                            rollup.add(
                                entry.ts,
                                entry.bytes_sent,
                                entry.status,
                                entry.ip.as_bytes(),
                                [info.browser, info.os, info.device, entry.path],
                            );
                            run.progress.min_ts.fetch_min(entry.ts, Ordering::Relaxed);
                            run.progress.max_ts.fetch_max(entry.ts, Ordering::Relaxed);
                            added += 1;
                        }
                        // Still under the writer lock, like the documents
                        if !rollup.is_empty() {
                            run.rollups.add(run.group, rollup.finish());
                        }
                        run.progress.docs.fetch_add(added, Ordering::Relaxed);
                        run.progress.failed.fetch_add(bad, Ordering::Relaxed);
                        committed_docs.fetch_add(added, Ordering::Relaxed);
                        advance(run, &watermark, &finals, batch.seq, batch.file, batch.end_offset, batch.last);
                        drop(w);
                    }
                })
            })
            .collect();

        // Periodic commits, so a crash during a long import keeps its progress
        let mut next = run.sizing.commit_every;
        while !parsers_done.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(20));
            if workers.iter().all(|w| w.is_finished()) && reader.is_finished() {
                parsers_done.store(true, Ordering::Relaxed);
            }
            if run.sizing.commit_every > 0 && committed_docs.load(Ordering::Relaxed) >= next {
                if let Err(e) = commit_state(run.writer, run.working, true, run.rollups, run.on_commit) {
                    failed(format!("commit: {e}"));
                }
                next = committed_docs.load(Ordering::Relaxed) + run.sizing.commit_every;
            }
        }
    });

    if let Some(e) = error.into_inner().expect("error slot") {
        return Err(e);
    }
    Ok(GroupOutcome {
        docs: run.progress.docs.load(Ordering::Relaxed),
        failed: run.progress.failed.load(Ordering::Relaxed),
        bytes: run.progress.bytes_read.load(Ordering::Relaxed),
        time_range: run.progress.time_range(),
    })
}

/// Moves the position of the files over the batches that became contiguous. A
/// file that is done takes its final size and time, so a file an import stopped
/// in is read again.
fn advance(
    run: &GroupRun<'_>,
    watermark: &Watermark,
    finals: &Finals,
    seq: u64,
    file: usize,
    end_offset: u64,
    last: bool,
) {
    let advanced = watermark.complete(seq, file, end_offset, last);
    if advanced.is_empty() {
        return;
    }
    let mut state = run.working.lock().expect("state lock");
    let finals = finals.lock().expect("finals");
    for (file, offset, last) in advanced {
        let Some(path) = run.files.get(file) else { continue };
        let key = path.to_string_lossy();
        let Some(group) = state.groups.get_mut(run.group) else { continue };
        let Some(row) = group.files.iter_mut().find(|r| r.path == key) else { continue };
        row.position = offset;
        if last {
            if let Some(done) = finals.get(file).and_then(Option::as_ref) {
                row.size = done.size;
                row.mtime_ns = done.mtime_ns;
            }
            row.indexed_at = now_secs();
        }
    }
}

/// What the reader thread shares with the rest of the pipeline.
struct ReaderCtx<'a> {
    run: &'a GroupRun<'a>,
    tx: &'a std::sync::mpsc::SyncSender<Batch>,
    watermark: &'a Watermark,
    round_high: &'a Mutex<HashMap<String, u64>>,
    finals: &'a Finals,
}

/// Plans and reads one file, sending its lines in batches.
fn read_file(ctx: &ReaderCtx<'_>, index: usize, path: &Path, seq: &mut u64) -> Result<(), String> {
    let ReaderCtx { run, tx, watermark, round_high, finals } = *ctx;
    let key = path.to_string_lossy().into_owned();
    let plan: FilePlan = {
        let high = |fp: &str| {
            let mine = round_high.lock().expect("high water").get(fp).copied().unwrap_or(0);
            run.snapshot.high_water(fp).max(mine)
        };
        filesync::plan_file(path, run.snapshot, high, run.options).map_err(|e| e.to_string())?
    };

    let fingerprint = plan.fingerprint.clone().unwrap_or_default();
    let row = FileRow {
        path: key.clone(),
        fingerprint: fingerprint.clone(),
        position: plan.start,
        // Unknown until the file is finished, see `advance`
        size: 0,
        mtime_ns: 0,
        indexed_at: now_secs(),
    };
    let finished_row = FileRow { size: plan.size, mtime_ns: plan.mtime_ns, ..row.clone() };

    if plan.skip {
        run.working.lock().expect("state lock").set_row(run.group, finished_row);
        return Ok(());
    }
    run.working.lock().expect("state lock").set_row(run.group, row);
    finals.lock().expect("finals")[index] = Some(finished_row);
    let fingerprint_arc: Arc<str> = Arc::from(fingerprint.as_str());

    if let Some(from) = plan.replace_from {
        // The delete only sees documents added before it, so what is in
        // flight has to be added first
        watermark.drain();
        let query = content_query(run.fields, run.group, &fingerprint, from);
        run.writer.read().expect("writer lock").delete_query(query).map_err(|e| e.to_string())?;
        // What was deleted cannot be taken out of the hours
        run.rollups.invalidate(run.group);
    }

    let stream = filesync::open_content(path, plan.size, plan.compressed, plan.start).map_err(|e| e.to_string())?;
    let mut lines = LineReader::new(stream, plan.compressed, plan.start);
    let mut batch: Vec<(String, u64)> = Vec::with_capacity(run.sizing.batch_lines);
    let mut last_consumed = plan.start;

    let send = |seq: &mut u64, lines: Vec<(String, u64)>, end: u64, last: bool| -> Result<(), String> {
        watermark.sent();
        let b = Batch { seq: *seq, file: index, fingerprint: fingerprint_arc.clone(), lines, end_offset: end, last };
        *seq += 1;
        tx.send(b).map_err(|_| "the parsers stopped".to_owned())
    };

    loop {
        if run.cancel.load(Ordering::Relaxed) {
            return Err("the run was cancelled".to_owned());
        }
        match lines.next_line().map_err(|e| e.to_string())? {
            Some(line) => {
                run.progress.bytes_read.fetch_add(lines.consumed() - last_consumed, Ordering::Relaxed);
                last_consumed = lines.consumed();
                batch.push((line.text, line.offset));
                if batch.len() >= run.sizing.batch_lines {
                    let full = std::mem::replace(&mut batch, Vec::with_capacity(run.sizing.batch_lines));
                    send(seq, full, lines.consumed(), false)?;
                }
            }
            None => break,
        }
    }
    run.progress.bytes_read.fetch_add(lines.consumed().saturating_sub(last_consumed), Ordering::Relaxed);
    send(seq, batch, lines.consumed(), true)?;
    round_high
        .lock()
        .expect("high water")
        .entry(fingerprint)
        .and_modify(|h| *h = (*h).max(lines.consumed()))
        .or_insert(lines.consumed());
    Ok(())
}
