//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use plugin_log_analytics_tantivy::collectors::TermCounts;
use plugin_log_analytics_tantivy::config::Dirs;
use plugin_log_analytics_tantivy::engine::{Engine, Scope};
use plugin_log_analytics_tantivy::logs::HostLog;
use plugin_log_analytics_tantivy::sizing::Sizing;
use tantivy::query::AllQuery;

/// A test bed: a log folder and an engine with its index beside it.
pub struct Bed {
    pub root: tempfile::TempDir,
    pub engine: Arc<Engine>,
}

pub const SMALL: Sizing = Sizing { heap_mb: 20, threads: 1, batch_lines: 50, commit_every: 0 };

impl Bed {
    pub fn new() -> Bed {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("logs")).unwrap();
        let engine = Self::engine_at(root.path());
        Bed { root, engine }
    }

    fn engine_at(root: &Path) -> Arc<Engine> {
        let engine = Engine::open(Dirs::new(root.join("data"))).unwrap();
        engine.override_sizing(SMALL);
        let log = root.join("logs").join("access.log");
        engine.hostlogs.set(vec![HostLog {
            path: log.to_string_lossy().into_owned(),
            kind: "access".into(),
            source: "config".into(),
            config_file: String::new(),
        }]);
        engine
    }

    /// Opens the engine again over the same files, as after a restart.
    pub fn restart(&mut self) {
        self.engine = Self::engine_at(self.root.path());
    }

    pub fn log(&self, name: &str) -> PathBuf {
        self.root.path().join("logs").join(name)
    }

    pub fn group(&self) -> String {
        self.log("access.log").to_string_lossy().into_owned()
    }

    pub async fn round(&self) {
        self.engine.run_round(Scope::All, false).await;
    }

    pub fn docs(&self) -> u64 {
        self.engine.group_stats(&self.group()).docs
    }

    /// How many documents each request path has. Every test line has its own
    /// path, so a count above one is a duplicate.
    pub fn per_path(&self) -> HashMap<String, u64> {
        let searcher = self.engine.store.searcher();
        searcher.search(&AllQuery, &TermCounts { field: "path" }).unwrap()
    }

    pub fn assert_no_duplicates(&self) {
        let dup: Vec<_> = self.per_path().into_iter().filter(|(_, c)| *c > 1).collect();
        assert!(dup.is_empty(), "duplicated documents: {dup:?}");
    }
}

/// One log line, unique by `n`.
pub fn line(n: usize) -> String {
    format!(
        "203.0.113.{} - - [07/Sep/2026:10:{:02}:{:02} +0000] \"GET /p/{n} HTTP/1.1\" 200 {} \"-\" \"curl/8.0\" 0.001 0.001",
        n % 250,
        (n / 60) % 60,
        n % 60,
        100 + n
    )
}

/// Lines `from..to`, each ending with a newline.
pub fn lines(from: usize, to: usize) -> String {
    (from..to).map(|n| line(n) + "\n").collect()
}

pub fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

pub fn append(path: &Path, text: &str) {
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

pub fn gzip(path: &Path, text: &str) {
    let mut enc = flate2::write::GzEncoder::new(std::fs::File::create(path).unwrap(), flate2::Compression::fast());
    enc.write_all(text.as_bytes()).unwrap();
    enc.finish().unwrap();
}

use std::convert::Infallible;

use hyper_util::rt::TokioIo;
use nginxui_plugin_sdk::http::hyper::service::service_fn;
use plugin_log_analytics_tantivy::api;
use plugin_log_analytics_tantivy::app::App;

/// Serves the API of an app on a local port and returns its address.
pub async fn serve(app: Arc<App>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { break };
            let app = app.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req| {
                    let app = app.clone();
                    async move { Ok::<_, Infallible>(api::handle(app, req).await) }
                });
                let conn = nginxui_plugin_sdk::http::hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades();
                let _ = conn.await;
            });
        }
    });
    format!("127.0.0.1:{}", addr.port())
}

/// An app with an access log of mixed clients, indexed.
pub struct Api {
    pub root: tempfile::TempDir,
    pub app: Arc<App>,
    pub addr: String,
    pub client: reqwest::Client,
    pub log: PathBuf,
}

/// One line of a varied log: several clients, paths, statuses and agents.
pub fn varied_line(n: usize, ts_base: i64) -> String {
    let ips = ["8.8.8.8", "1.1.1.1", "9.9.9.9", "203.0.113.7", "46.4.0.1"];
    let uas = [
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
        "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
        "curl/8.4.0",
        "Googlebot/2.1 (+http://www.google.com/bot.html)",
    ];
    let paths = ["/", "/about", "/wp-login.php", "/api/items?id=3", "/how-to/start"];
    let statuses = [200, 200, 404, 200, 500];
    let ts = ts_base + (n as i64) * 37;
    let stamp = chrono::DateTime::from_timestamp(ts, 0).unwrap().format("%d/%b/%Y:%H:%M:%S +0000");
    format!(
        "{} - - [{}] \"GET {} HTTP/1.1\" {} {} \"https://example.com/\" \"{}\" 0.0{} 0.0{}",
        ips[n % ips.len()],
        stamp,
        paths[(n / 2) % paths.len()],
        statuses[n % statuses.len()],
        100 + n,
        uas[n % uas.len()],
        n % 9 + 1,
        n % 7 + 1
    )
}

impl Api {
    pub async fn new(lines: usize) -> Api {
        let root = tempfile::tempdir().unwrap();
        let logs = root.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let log = logs.join("access.log");
        let base = 1_788_739_200i64; // 2026-09-07 00:00:00 UTC
        let text: String = (0..lines).map(|n| varied_line(n, base) + "\n").collect();
        std::fs::write(&log, text).unwrap();

        let app = App::open(root.path().join("data")).unwrap();
        app.engine.override_sizing(SMALL);
        app.set_logs(vec![HostLog {
            path: log.to_string_lossy().into_owned(),
            kind: "access".into(),
            source: "default".into(),
            config_file: String::new(),
        }]);
        app.engine.run_round(Scope::All, false).await;
        let addr = serve(app.clone()).await;
        Api { root, app, addr, client: plain_client(), log }
    }

    pub fn group(&self) -> String {
        self.log.to_string_lossy().into_owned()
    }

    pub async fn get(&self, path: &str) -> (u16, serde_json::Value) {
        let r = self.client.get(format!("http://{}{path}", self.addr)).send().await.unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)))
    }

    pub async fn post(&self, path: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let r = self
            .client
            .post(format!("http://{}{path}", self.addr))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)))
    }
}

/// A client for the local test server. The download client only speaks https,
/// building it first installs the TLS provider this one needs as well.
fn plain_client() -> reqwest::Client {
    let _ = plugin_log_analytics_tantivy::geolite_download::client();
    reqwest::Client::new()
}
