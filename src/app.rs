//! The plugin process: ties the engine to the host, runs the schedule and
//! stops everything on shutdown.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nginxui_plugin_sdk::protocol::Settings as RawSettings;
use nginxui_plugin_sdk::{current_host, Host};
use tokio::sync::{watch, Notify};

use crate::config::{Dirs, Settings};
use crate::engine::{Engine, Scope};
use crate::logs::HostLog;
use crate::store::StoreError;

/// Entry of the processing indicator of the host and its English label, which
/// the browser bundle translates.
pub const ACTIVITY_KEY: &str = "indexing";
pub const ACTIVITY_LABEL: &str = "Nginx Log Indexing...";

/// How long the first round waits, so the host has listed the logs by then.
const FIRST_ROUND_DELAY: Duration = Duration::from_secs(2);
/// A change of the log files is followed by a round after this quiet time.
const CHANGE_DEBOUNCE: Duration = Duration::from_secs(3);

pub struct App {
    pub engine: Arc<Engine>,
    host: Mutex<Option<Host>>,
    started: AtomicBool,
    shutdown: watch::Sender<bool>,
    reset: Notify,
    /// Set once the host answered the first listing of the logs. Requests that
    /// arrive before it would otherwise refuse every log path.
    listed: watch::Sender<bool>,
    first_round_delay: Duration,
}

impl App {
    /// Opens the index in the data directory.
    pub fn open(data_dir: impl Into<PathBuf>) -> Result<Arc<App>, StoreError> {
        let engine = Engine::open(Dirs::new(data_dir))?;
        Ok(Arc::new(App {
            engine,
            host: Mutex::new(None),
            started: AtomicBool::new(false),
            shutdown: watch::channel(false).0,
            reset: Notify::new(),
            listed: watch::channel(false).0,
            first_round_delay: FIRST_ROUND_DELAY,
        }))
    }

    /// A receiver that changes when the plugin stops, for long lived streams.
    pub fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub fn stopping(&self) -> bool {
        *self.shutdown.borrow()
    }

    /// Takes the log files the host lists and lets the waiting requests go on.
    pub fn set_logs(&self, logs: Vec<HostLog>) {
        self.engine.hostlogs.set(logs);
        self.listed.send_replace(true);
    }

    /// Waits until the host listed the logs for the first time, at most `limit`.
    /// It returns false when the wait ran out.
    pub async fn wait_listed(&self, limit: Duration) -> bool {
        let mut listed = self.listed.subscribe();
        if *listed.borrow() {
            return true;
        }
        let result = tokio::time::timeout(limit, listed.wait_for(|ready| *ready)).await;
        result.is_ok()
    }

    /// Applies new settings. A changed interval restarts the schedule.
    pub fn configure(&self, raw: &RawSettings) {
        let next = Settings::parse(raw);
        let previous = self.engine.settings();
        self.engine.set_settings(next.clone());
        if previous.interval_minutes != next.interval_minutes {
            self.reset.notify_one();
        }
    }

    /// Waits until the host accepts calls, then starts the schedule.
    pub async fn run(self: Arc<Self>) {
        let mut stop = self.shutdown.subscribe();
        let host = loop {
            if let Some(h) = current_host().filter(Host::ready) {
                break h;
            }
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(50)) => {}
                _ = stop.changed() => return,
            }
        };
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.host.lock().expect("host lock") = Some(host.clone());
        self.configure(&host.settings());

        let observer = self.clone();
        self.engine.processing.observe(move |indexing| observer.set_activity(indexing));
        self.refresh_logs().await;
        self.schedule().await;
    }

    /// Asks the host which log files the plugin may read.
    pub async fn refresh_logs(&self) {
        let host = self.host.lock().expect("host lock").clone();
        let Some(host) = host else { return };
        let result = host.logs_list().await;
        // A failed first listing ends the wait too, the requests then answer with an error
        self.listed.send_replace(true);
        match result {
            Ok(logs) => {
                let list: Vec<HostLog> = logs
                    .into_iter()
                    .map(|l| HostLog { path: l.path, kind: l.r#type, source: l.source, config_file: l.config_file })
                    .collect();
                nginxui_plugin_sdk::debug!("the host lists {} nginx log file(s)", list.len());
                self.set_logs(list);
            }
            Err(e) => nginxui_plugin_sdk::error!("could not list the nginx log files: {e}"),
        }
    }

    /// The log files changed: list them again and read what is new soon.
    pub async fn logs_changed(&self) {
        self.refresh_logs().await;
        self.engine.request_round();
    }

    fn set_activity(&self, indexing: bool) {
        let host = self.host.lock().expect("host lock").clone();
        let Some(host) = host else { return };
        tokio::spawn(async move {
            let call = host.activity_set(ACTIVITY_KEY, ACTIVITY_LABEL, indexing);
            if let Ok(Err(e)) = tokio::time::timeout(Duration::from_secs(5), call).await {
                nginxui_plugin_sdk::debug!("could not update the host activity: {e}");
            }
        });
    }

    /// Runs the indexing rounds: one after the start, one per interval and one
    /// after a change of the log files.
    async fn schedule(self: &Arc<Self>) {
        let mut stop = self.shutdown.subscribe();
        let mut wait = self.first_round_delay;
        // The first settings arrive before the schedule starts. That is not a
        // change of the interval, the first round still comes after its delay.
        let _ = tokio::time::timeout(Duration::ZERO, self.reset.notified()).await;
        loop {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = self.engine.wait_for_request() => {
                    // A burst of changes makes one round
                    tokio::time::sleep(CHANGE_DEBOUNCE).await;
                }
                () = self.reset.notified() => {
                    wait = self.engine.settings().interval();
                    nginxui_plugin_sdk::info!("log indexing runs every {} minutes", wait.as_secs() / 60);
                    continue;
                }
                _ = stop.changed() => return,
            }
            if self.stopping() {
                return;
            }
            if let Some(report) = self.engine.run_round(Scope::All, false).await {
                nginxui_plugin_sdk::info!(
                    "indexing round: {} group(s), {} document(s), {} failed, {} ms",
                    report.groups,
                    report.docs,
                    report.failed,
                    report.duration_ms
                );
            }
            wait = self.engine.settings().interval();
        }
    }

    /// Stops the schedule and the running round.
    pub async fn stop(&self) {
        self.shutdown.send_replace(true);
        self.engine.cancel();
        // A round ends at its next batch, wait for it a little
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while self.engine.round_running() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let host = self.host.lock().expect("host lock").clone();
        if let Some(host) = host {
            let _ =
                tokio::time::timeout(Duration::from_secs(2), host.activity_set(ACTIVITY_KEY, ACTIVITY_LABEL, false))
                    .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(json: serde_json::Value) -> RawSettings {
        json.as_object().cloned().unwrap_or_default()
    }

    #[tokio::test]
    async fn the_first_round_runs_after_its_delay_not_after_the_interval() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("access.log");
        let line =
            "203.0.113.5 - - [07/Sep/2026:10:00:01 +0000] \"GET /a HTTP/1.1\" 200 10 \"-\" \"curl/8\" 0.001 0.001\n";
        std::fs::write(&log, line.repeat(20)).unwrap();

        let mut app = App::open(dir.path().join("data")).unwrap();
        Arc::get_mut(&mut app).unwrap().first_round_delay = Duration::from_millis(50);
        app.engine.override_sizing(crate::sizing::Sizing {
            heap_mb: 20,
            threads: 1,
            merge_threads: 1,
            batch_lines: 50,
            commit_every: 0,
        });
        app.set_logs(vec![HostLog {
            path: log.to_string_lossy().into_owned(),
            kind: "access".into(),
            source: "config".into(),
            config_file: String::new(),
        }]);
        // The settings come in first, with an interval that is not the default
        app.configure(&settings(serde_json::json!({"incremental_index_interval": 30})));

        let scheduler = tokio::spawn({
            let app = app.clone();
            async move { app.schedule().await }
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while app.engine.total_docs() < 20 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(app.engine.total_docs(), 20);
        assert_eq!(app.engine.settings().interval(), Duration::from_secs(1800));
        app.stop().await;
        scheduler.await.unwrap();
    }

    #[tokio::test]
    async fn requests_wait_for_the_first_listing_of_the_logs() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(dir.path()).unwrap();
        assert!(!app.wait_listed(Duration::from_millis(30)).await);

        let waiter = {
            let app = app.clone();
            tokio::spawn(async move { app.wait_listed(Duration::from_secs(10)).await })
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        app.set_logs(Vec::new());
        assert!(waiter.await.unwrap());
        assert!(app.wait_listed(Duration::from_millis(1)).await);
    }
}
