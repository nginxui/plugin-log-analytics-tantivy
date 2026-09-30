//! The indexing engine: keeps the index up to date with the log files, runs the
//! rounds, and tracks what each log group is doing for the status pages.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tantivy::collector::Count;
use tantivy::query::{Query, TermQuery};
use tantivy::schema::{IndexRecordOption, Term};
use tokio::sync::Notify;

use crate::analytics::{self, AnalyticsError, DashboardResponse};
use crate::collectors::TimeRangeCollector;
use crate::config::{Dirs, Settings};
use crate::events::{Hub, Processing};
use crate::filesync::{self, PlanOptions};
use crate::geo::{Geo, GeoPaths};
use crate::logs::{HostLogs, LogGroup};
use crate::pipeline::{self, CommitHook, GroupOutcome, GroupProgress, GroupRun, SharedWriter};
use crate::rollup::{Delta, GroupRollup, RollupCollector, Rollups, Slot};
use crate::sizing::{self, Sizing};
use crate::state::{IndexState, Snapshot};
use crate::store::{Store, StoreError};

/// Which groups a round covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    Group(String),
}

/// Why a rebuild was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebuildError {
    /// An indexing run is active.
    Busy,
    /// The group is being indexed.
    GroupBusy,
    /// The path is not one of the listed logs.
    NotAllowed,
}

/// What a log group is doing right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Queued(u32),
    Indexing,
    Failed { message: String, at: i64, retries: u32 },
}

/// Documents and time range of a group in the committed index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroupStats {
    pub docs: u64,
    pub range: Option<(i64, i64)>,
}

/// What a round did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoundReport {
    pub groups: usize,
    pub failed: usize,
    pub docs: u64,
    pub duration_ms: u64,
    /// Number of the round since the process started, counting from one.
    pub round: u64,
}

/// One group of a round.
struct PlannedGroup {
    group: LogGroup,
}

pub struct Engine {
    pub store: Store,
    /// Hourly figures of the dashboard, a cache of the index.
    pub rollups: Rollups,
    pub hostlogs: HostLogs,
    pub hub: Arc<Hub>,
    pub processing: Processing,
    pub dirs: Dirs,
    settings: RwLock<Settings>,
    /// The state of the last commit.
    state: Mutex<IndexState>,
    phases: Mutex<HashMap<String, Phase>>,
    /// Counts the commits, so cached figures know when they are stale.
    generation: AtomicU64,
    stats: Mutex<(u64, HashMap<String, GroupStats>)>,
    round_lock: tokio::sync::Mutex<()>,
    round_active: AtomicBool,
    cancel: AtomicBool,
    trigger: Notify,
    sizing_override: Mutex<Option<Sizing>>,
    last_report: Mutex<Option<RoundReport>>,
    rounds: AtomicU64,
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl Engine {
    /// Opens the index below the data directory.
    pub fn open(dirs: Dirs) -> Result<Arc<Engine>, StoreError> {
        let (store, state) = Store::open(&dirs.index())?;
        let hub = Arc::new(Hub::new());
        Ok(Arc::new(Engine {
            store,
            rollups: Rollups::default(),
            hostlogs: HostLogs::new(),
            processing: Processing::new(hub.clone()),
            hub,
            dirs,
            settings: RwLock::new(Settings::default()),
            state: Mutex::new(state),
            phases: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(1),
            stats: Mutex::new((0, HashMap::new())),
            round_lock: tokio::sync::Mutex::new(()),
            round_active: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            trigger: Notify::new(),
            sizing_override: Mutex::new(None),
            last_report: Mutex::new(None),
            rounds: AtomicU64::new(0),
        }))
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().expect("settings lock").clone()
    }

    pub fn set_settings(&self, settings: Settings) {
        *self.settings.write().expect("settings lock") = settings;
    }

    pub fn geo_paths(&self) -> GeoPaths {
        GeoPaths::new(self.dirs.geolite(), &self.settings().custom_mmdb)
    }

    /// The committed state.
    pub fn state(&self) -> IndexState {
        self.state.lock().expect("state lock").clone()
    }

    pub fn phase(&self, group: &str) -> Option<Phase> {
        self.phases.lock().expect("phases lock").get(group).cloned()
    }

    fn set_phase(&self, group: &str, phase: Option<Phase>) {
        let mut phases = self.phases.lock().expect("phases lock");
        match phase {
            Some(p) => {
                phases.insert(group.to_owned(), p);
            }
            None => {
                phases.remove(group);
            }
        }
    }

    pub fn group_active(&self, group: &str) -> bool {
        matches!(self.phase(group), Some(Phase::Queued(_) | Phase::Indexing))
    }

    /// Asks the scheduler for a round soon.
    pub fn request_round(&self) {
        self.trigger.notify_one();
    }

    pub async fn wait_for_request(&self) {
        self.trigger.notified().await;
    }

    /// Stops the running round at the next batch.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Replaces the sizing, for tests.
    pub fn override_sizing(&self, sizing: Sizing) {
        *self.sizing_override.lock().expect("sizing lock") = Some(sizing);
    }

    fn sizing(&self) -> Sizing {
        self.sizing_override.lock().expect("sizing lock").unwrap_or_else(sizing::current)
    }

    pub fn last_report(&self) -> Option<RoundReport> {
        self.last_report.lock().expect("report lock").clone()
    }

    /// Makes the index follow the phrase search setting. A change empties the
    /// index, the round that follows reads every log again.
    fn apply_layout(&self) {
        let wanted = self.settings().phrase_search;
        if self.store.positions() == wanted {
            return;
        }
        match self.store.set_positions(wanted) {
            Ok(true) => {
                *self.state.lock().expect("state lock") = IndexState::default();
                self.rollups.clear();
                self.generation.fetch_add(1, Ordering::SeqCst);
                nginxui_plugin_sdk::info!("the phrase search setting changed, the logs are indexed again");
            }
            Ok(false) => {}
            Err(e) => nginxui_plugin_sdk::warn!("cannot change the index layout: {e}"),
        }
    }

    /// Whether a round is running.
    pub fn round_running(&self) -> bool {
        self.round_active.load(Ordering::SeqCst)
    }

    fn on_commit(&self, state: IndexState, delta: Delta) {
        *self.state.lock().expect("state lock") = state;
        // The rollups change together with what the readers see
        self.rollups.apply(delta, || self.store.reload());
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn group_query(&self, group: &str) -> Box<dyn Query> {
        Box::new(TermQuery::new(
            Term::from_field_text(self.store.fields().main_log_path, group),
            IndexRecordOption::Basic,
        ))
    }

    /// Documents and time range of a group in the committed index.
    pub fn group_stats(&self, group: &str) -> GroupStats {
        let generation = self.generation.load(Ordering::SeqCst);
        {
            let mut cache = self.stats.lock().expect("stats lock");
            if cache.0 != generation {
                *cache = (generation, HashMap::new());
            }
            if let Some(found) = cache.1.get(group) {
                return *found;
            }
        }
        let searcher = self.store.searcher();
        let found = match searcher.search(self.group_query(group).as_ref(), &(Count, TimeRangeCollector)) {
            Ok((docs, range)) => GroupStats { docs: docs as u64, range },
            Err(_) => GroupStats::default(),
        };
        let mut cache = self.stats.lock().expect("stats lock");
        if cache.0 == generation {
            cache.1.insert(group.to_owned(), found);
        }
        found
    }

    /// The rollup of a group, computed from the index when it is not kept.
    /// `None` when the group is too large for a rollup.
    pub fn rollup_of(&self, group: &str) -> Option<Arc<GroupRollup>> {
        match self.rollups.get(group) {
            Some(Slot::Ready(rollup)) => return Some(rollup),
            Some(Slot::TooLarge) => return None,
            None => {}
        }
        // The version and the searcher of one commit, so a commit that comes
        // in while the group is scanned keeps the result from being kept
        let (version, searcher) = self.rollups.snapshot(|| self.store.searcher());
        let scanned = searcher.search(self.group_query(group).as_ref(), &RollupCollector);
        match scanned {
            Ok(rollup) => match self.rollups.install(version, group, rollup) {
                Slot::Ready(r) => Some(r),
                Slot::TooLarge => None,
            },
            Err(e) => {
                nginxui_plugin_sdk::warn!("cannot scan {group} for its rollup: {e}");
                None
            }
        }
    }

    /// The dashboard of a group, or of all groups when `group` is empty. It
    /// reads the rollups and falls back to the index for a group without one.
    pub fn dashboard(&self, group: &str, start: i64, end: i64) -> Result<DashboardResponse, AnalyticsError> {
        let names: Vec<String> = if group.is_empty() {
            let mut names: Vec<String> = self.state().groups.keys().cloned().collect();
            names.extend(self.rollups.groups());
            names.sort();
            names.dedup();
            names
        } else {
            vec![group.to_owned()]
        };
        let rollups: Option<Vec<Arc<GroupRollup>>> = names.iter().map(|g| self.rollup_of(g)).collect();
        let searcher = self.store.searcher();
        let fields = self.store.fields();
        match rollups {
            Some(rollups) => analytics::dashboard_rollup(&searcher, fields, group, start, end, &rollups),
            None => analytics::dashboard(&searcher, fields, group, start, end),
        }
    }

    /// Documents in the whole index.
    pub fn total_docs(&self) -> u64 {
        self.store.searcher().num_docs()
    }

    /// The groups of a scope that have an access log to index.
    fn groups_in(&self, scope: &Scope) -> Vec<LogGroup> {
        let groups: Vec<LogGroup> = self.hostlogs.groups().into_iter().filter(|g| g.kind == "access").collect();
        match scope {
            Scope::All => groups,
            Scope::Group(path) => groups.into_iter().filter(|g| &g.path == path).collect(),
        }
    }

    /// Whether any file of the group changed since it was read.
    fn group_pending(&self, group: &LogGroup, state: &IndexState) -> bool {
        let files = self.hostlogs.group_files(&group.path);
        let rows = state.group(&group.path).map(|g| g.files.as_slice()).unwrap_or(&[]);
        files.iter().any(|path| {
            let Ok(meta) = std::fs::metadata(path) else { return false };
            let key = path.to_string_lossy();
            let row = rows.iter().find(|r| r.path == key);
            filesync::needs_sync(meta.len(), filesync::mtime_ns(&meta), row)
        })
    }

    /// Checks that a rebuild may start, without starting it.
    pub fn check_rebuild(&self, path: Option<&str>) -> Result<Scope, RebuildError> {
        if self.processing.indexing() || self.round_running() {
            return Err(RebuildError::Busy);
        }
        match path {
            None => Ok(Scope::All),
            Some(p) => {
                let Some(group) = self.hostlogs.group_of(p) else { return Err(RebuildError::NotAllowed) };
                if self.group_active(&group.path) {
                    return Err(RebuildError::GroupBusy);
                }
                Ok(Scope::Group(group.path))
            }
        }
    }

    /// Runs one round. An incremental round reads the groups whose files
    /// changed, a rebuild first removes the documents of its scope and reads
    /// everything again. Rounds never overlap, a second one waits.
    pub async fn run_round(self: &Arc<Self>, scope: Scope, rebuild: bool) -> Option<RoundReport> {
        let guard = self.round_lock.lock().await;
        if self.cancelled() {
            return None;
        }
        self.apply_layout();
        let state = self.state();
        let groups = self.groups_in(&scope);

        let engine = self.clone();
        let scan_state = state.clone();
        let planned: Vec<PlannedGroup> = tokio::task::spawn_blocking(move || {
            groups
                .into_iter()
                .filter(|g| rebuild || engine.group_pending(g, &scan_state))
                .map(|group| PlannedGroup { group })
                .collect()
        })
        .await
        .unwrap_or_default();

        let purge_only = rebuild && planned.is_empty();
        if planned.is_empty() && !purge_only {
            return None;
        }

        self.round_active.store(true, Ordering::SeqCst);
        // The indicator of the host shows a rebuild and a first import, the
        // regular check for new lines stays silent
        let loud = rebuild || planned.iter().any(|p| state.group(&p.group.path).is_none_or(|g| g.files.is_empty()));
        if loud {
            self.processing.set(true);
        }
        for (i, p) in planned.iter().enumerate() {
            self.set_phase(&p.group.path, Some(Phase::Queued(i as u32 + 1)));
        }

        let engine = self.clone();
        let started = Instant::now();
        let scope_for_round = scope.clone();
        let outcome =
            tokio::task::spawn_blocking(move || engine.round_blocking(planned, &scope_for_round, rebuild)).await;

        self.processing.set(false);
        self.round_active.store(false, Ordering::SeqCst);
        drop(guard);

        let mut report = match outcome {
            Ok(report) => report,
            Err(e) => {
                nginxui_plugin_sdk::error!("indexing round stopped unexpectedly: {e}");
                RoundReport { failed: 1, ..Default::default() }
            }
        };
        report.duration_ms = started.elapsed().as_millis() as u64;
        report.round = self.rounds.fetch_add(1, Ordering::SeqCst) + 1;
        *self.last_report.lock().expect("report lock") = Some(report.clone());
        Some(report)
    }

    fn round_blocking(self: &Arc<Self>, planned: Vec<PlannedGroup>, scope: &Scope, rebuild: bool) -> RoundReport {
        let sizing = self.sizing();
        let planned_paths: Vec<String> = planned.iter().map(|p| p.group.path.clone()).collect();
        let mut report = RoundReport { groups: planned.len(), ..Default::default() };

        let writer: SharedWriter = match self.store.writer(&sizing) {
            Ok(w) => Arc::new(RwLock::new(w)),
            Err(e) => {
                nginxui_plugin_sdk::error!("cannot open the index writer: {e}");
                for p in &planned {
                    self.fail_group(&p.group.path, &e.to_string(), 0);
                }
                report.failed = planned.len();
                return report;
            }
        };

        let committed = self.state();
        let working = Mutex::new(committed.clone());
        let on_commit = |s: IndexState, d: Delta| self.on_commit(s, d);

        if rebuild {
            if let Err(e) = self.purge(&writer, &working, scope, &on_commit) {
                nginxui_plugin_sdk::error!("cannot remove the old documents: {e}");
                for p in &planned {
                    self.fail_group(&p.group.path, &e, 0);
                }
                report.failed = planned.len();
                return report;
            }
        }

        let geo = self.open_geo();
        let options = PlanOptions { force: false, dirty: committed.dirty };
        let concurrency = self.concurrency(&sizing);
        let next = AtomicUsize::new(0);
        let failed = AtomicUsize::new(0);
        let docs = AtomicU64::new(0);

        std::thread::scope(|scope| {
            for _ in 0..concurrency.min(planned.len()).max(1) {
                scope.spawn(|| loop {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some(planned) = planned.get(index) else { break };
                    if self.cancelled() {
                        self.set_phase(&planned.group.path, None);
                        continue;
                    }
                    match self.process_group(
                        &planned.group,
                        &writer,
                        &working,
                        geo.clone(),
                        sizing,
                        options,
                        &on_commit,
                    ) {
                        Ok(outcome) => {
                            docs.fetch_add(outcome.docs, Ordering::SeqCst);
                        }
                        Err(_) => {
                            failed.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                });
            }
        });

        report.failed = failed.load(Ordering::SeqCst);
        report.docs = docs.load(Ordering::SeqCst);

        // A failed group may have left documents past its recorded positions, so
        // the next round removes them before it reads again.
        if let Err(e) = pipeline::commit_state(&writer, &working, report.failed > 0, &self.rollups, &on_commit) {
            nginxui_plugin_sdk::error!("final commit failed: {e}");
            report.failed += 1;
        }

        // The writer merges what it can before it is dropped
        match Arc::try_unwrap(writer) {
            Ok(lock) => {
                if let Err(e) = lock.into_inner().expect("writer lock").wait_merging_threads() {
                    nginxui_plugin_sdk::warn!("merging stopped with an error: {e}");
                }
            }
            Err(_) => nginxui_plugin_sdk::warn!("the index writer is still in use"),
        }
        self.store.reload();
        // Groups whose rollup was dropped are ready before the first request
        for p in &planned_paths {
            if self.rollups.get(p).is_none() {
                self.rollup_of(p);
            }
        }
        crate::sys::release_memory();
        report
    }

    fn concurrency(&self, sizing: &Sizing) -> usize {
        let configured = self.settings().max_tasks;
        if configured > 0 {
            return configured as usize;
        }
        // Small budgets index one group at a time
        if sizing.heap_mb <= 50 {
            1
        } else {
            sizing.threads.min(2)
        }
    }

    fn open_geo(&self) -> Arc<Geo> {
        let paths = self.geo_paths();
        let db = paths.db_path();
        if db.is_file() {
            Geo::open(Some(&db))
        } else {
            Geo::countries_only()
        }
    }

    /// Removes the documents and the state of a rebuild scope.
    fn purge(
        &self,
        writer: &SharedWriter,
        working: &Mutex<IndexState>,
        scope: &Scope,
        on_commit: &CommitHook<'_>,
    ) -> Result<(), String> {
        {
            let w = writer.read().expect("writer lock");
            let mut state = working.lock().expect("state lock");
            match scope {
                Scope::All => {
                    w.delete_all_documents().map_err(|e| e.to_string())?;
                    state.groups.clear();
                    self.rollups.invalidate_all();
                }
                Scope::Group(path) => {
                    w.delete_query(self.group_query(path)).map_err(|e| e.to_string())?;
                    state.groups.remove(path);
                    self.rollups.invalidate(path);
                }
            }
        }
        pipeline::commit_state(writer, working, false, &self.rollups, on_commit)
    }

    fn fail_group(&self, group: &str, message: &str, duration_ms: i64) {
        let retries = match self.phase(group) {
            Some(Phase::Failed { retries, .. }) => retries + 1,
            _ => 1,
        };
        self.set_phase(group, Some(Phase::Failed { message: message.to_owned(), at: now_secs(), retries }));
        self.hub.complete(group, false, duration_ms, 0, 0, message);
    }

    #[allow(clippy::too_many_arguments)]
    fn process_group(
        &self,
        group: &LogGroup,
        writer: &SharedWriter,
        working: &Mutex<IndexState>,
        geo: Arc<Geo>,
        sizing: Sizing,
        options: PlanOptions,
        on_commit: &CommitHook<'_>,
    ) -> Result<GroupOutcome, String> {
        let path = group.path.clone();
        // A group without documents counts them into its rollup as they come
        self.rollups.begin_group(&path, || {
            self.store.searcher().search(self.group_query(&path).as_ref(), &Count).map_or(1, |n| n as u64)
        });
        self.set_phase(&path, Some(Phase::Indexing));
        self.hub.progress(&path, 0.0, "scanning", "running", 0, 0);

        let files: Vec<PathBuf> = self.hostlogs.group_files(&path);
        let started_at = now_secs();
        let started = Instant::now();
        let (snapshot, bytes_total) = {
            let state = working.lock().expect("state lock");
            let rows = state.group(&path).map(|g| g.files.clone()).unwrap_or_default();
            let total: u64 = files
                .iter()
                .map(|f| {
                    let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
                    let done = rows.iter().find(|r| r.path == f.to_string_lossy()).map_or(0, |r| r.position);
                    let content = filesync::content_size_estimate(f, size);
                    content.saturating_sub(done.min(content))
                })
                .sum();
            (Snapshot::new(&rows), total)
        };
        working.lock().expect("state lock").groups.entry(path.clone()).or_default();

        let progress = GroupProgress::new();
        progress.bytes_total.store(bytes_total, Ordering::Relaxed);
        let cancel = &self.cancel;
        let finished = AtomicBool::new(false);

        let outcome = std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut last = Instant::now();
                while !finished.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                    if last.elapsed() < Duration::from_secs(1) {
                        continue;
                    }
                    last = Instant::now();
                    let total = progress.bytes_total.load(Ordering::Relaxed).max(1) as f64;
                    let done = progress.bytes_read.load(Ordering::Relaxed) as f64;
                    let percent = (done / total * 100.0).min(99.0);
                    let elapsed = started.elapsed().as_millis() as i64;
                    let remain = if percent > 0.5 { (elapsed as f64 * (100.0 - percent) / percent) as i64 } else { 0 };
                    self.hub.progress(&path, percent, "indexing", "running", elapsed, remain);
                }
            });
            let run = GroupRun {
                fields: self.store.fields(),
                writer,
                geo,
                sizing,
                group: &path,
                files: &files,
                snapshot: &snapshot,
                working,
                progress: &progress,
                cancel,
                options,
                rollups: &self.rollups,
                on_commit,
            };
            let result = pipeline::run_group(&run);
            finished.store(true, Ordering::Relaxed);
            result
        });

        let duration_ms = started.elapsed().as_millis() as i64;
        match outcome {
            Ok(out) => {
                {
                    let mut state = working.lock().expect("state lock");
                    if let Some(g) = state.groups.get_mut(&path) {
                        g.started_at = started_at;
                        g.duration_ms = duration_ms;
                    }
                    state.prune(|p| std::path::Path::new(p).exists());
                }
                // The documents are visible before the group is reported as ready
                if let Err(e) = pipeline::commit_state(writer, working, true, &self.rollups, on_commit) {
                    self.fail_group(&path, &e, duration_ms);
                    return Err(e);
                }
                self.set_phase(&path, None);
                let (lo, hi) = out.time_range.unwrap_or_else(|| {
                    let s = self.group_stats(&path);
                    s.range.unwrap_or((now_secs(), now_secs()))
                });
                self.hub.progress(&path, 100.0, "indexing", "completed", duration_ms, 0);
                self.hub.complete(&path, true, duration_ms, out.docs, out.bytes, "");
                self.hub.ready(&path, lo, hi);
                Ok(out)
            }
            Err(e) => {
                nginxui_plugin_sdk::warn!("indexing {path} failed: {e}");
                self.fail_group(&path, &e, duration_ms);
                Err(e)
            }
        }
    }
}
