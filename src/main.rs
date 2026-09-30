//! The log analytics plugin process. The host starts it, speaks JSON-RPC to it
//! on stdin and stdout and proxies the HTTP API of the pages to its socket.

use std::sync::Arc;

use nginxui_plugin_sdk::protocol::event;
use nginxui_plugin_sdk::{env, Options, Plugin};
use plugin_log_analytics_tantivy::api;
use plugin_log_analytics_tantivy::app::App;
use plugin_log_analytics_tantivy::sys;

// The C allocator does not build for Windows with zig, which has a good heap of its own
#[cfg(not(windows))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn data_dir() -> std::path::PathBuf {
    std::env::var_os(env::PLUGIN_DATA_DIR).map(Into::into).unwrap_or_else(|| std::path::PathBuf::from("data"))
}

fn plugin(app: Arc<App>) -> Plugin {
    let http = app.clone();
    let configured = app.clone();
    let changed = app.clone();
    let stopped = app;
    Plugin::new()
        .http(move |req| {
            let app = http.clone();
            async move { api::handle(app, req).await }
        })
        .configure(move |_ctx, settings| {
            configured.configure(&settings);
            async { Ok(()) }
        })
        .event(event::LOG_PATHS_CHANGED, move |_ctx, _event| {
            let app = changed.clone();
            async move { app.logs_changed().await }
        })
        .shutdown(move |_ctx| {
            let app = stopped.clone();
            async move {
                app.stop().await;
                Ok(())
            }
        })
}

fn main() {
    sys::tune_allocator();
    let dir = data_dir();
    let app = match App::open(&dir) {
        Ok(app) => app,
        Err(e) => {
            // Before the handshake this goes to stderr, never to stdout
            nginxui_plugin_sdk::error!("cannot open the index in {}: {e}", dir.display());
            std::process::exit(1);
        }
    };

    let workers = sys::available_cpus().clamp(2, 4);
    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(workers).enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            nginxui_plugin_sdk::error!("cannot build the runtime: {e}");
            std::process::exit(1);
        }
    };
    runtime.block_on(async move {
        tokio::spawn(app.clone().run());
        nginxui_plugin_sdk::serve_with(plugin(app), Options::new()).await
    })
}
