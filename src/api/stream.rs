//! The two WebSocket streams, the indexing events and the download of the IP
//! location database, and the status of that database.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nginxui_plugin_sdk::http::{Incoming, Request};
use serde::Serialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::Message;

use super::respond::{ok, Resp};
use super::ws::{upgrade, Socket};
use crate::app::App;
use crate::geolite_download::{self as download, DownloadSlot};
use crate::localtime::format_rfc3339;

/// A quiet connection is pinged this often, so proxies keep it open.
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// Streams the indexing events as `{type, data}` messages. The current
/// processing state is sent right after the connection opens.
pub fn events(app: &Arc<App>, req: Request<Incoming>) -> Resp {
    let app = app.clone();
    upgrade(req, move |socket| events_loop(app, socket))
}

async fn events_loop(app: Arc<App>, socket: Socket) {
    let (mut tx, mut rx) = socket.split();
    let mut events = app.engine.hub.subscribe();
    let mut stop = app.shutdown_signal();
    app.engine.processing.broadcast();

    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    let Ok(text) = serde_json::to_string(&event) else { continue };
                    if tx.send(Message::text(text)).await.is_err() {
                        break;
                    }
                }
                // A slow client loses the oldest events and goes on
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            _ = ping.tick() => {
                if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            frame = rx.next() => match frame {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            _ = stop.changed() => {
                let _ = tx.send(Message::Close(None)).await;
                break;
            }
        }
    }
}

/// Whether the IP location database is in place.
pub fn geolite_status(app: &Arc<App>) -> Resp {
    let paths = app.engine.geo_paths();
    let path = paths.db_path();
    let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file());
    let modified = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| format_rfc3339(d.as_secs() as i64))
        .unwrap_or_default();
    ok(&json!({
        "exists": meta.is_some(),
        "path": path.to_string_lossy(),
        "size": meta.as_ref().map_or(0, std::fs::Metadata::len),
        "last_modified": modified,
    }))
}

#[derive(Serialize)]
struct Progress<'a> {
    status: &'a str,
    progress: f64,
    message: &'a str,
}

async fn send(tx: &mut futures_util::stream::SplitSink<Socket, Message>, status: &str, message: &str, progress: f64) {
    let text = serde_json::to_string(&Progress { status, progress, message }).unwrap_or_default();
    if let Err(e) = tx.send(Message::text(text)).await {
        nginxui_plugin_sdk::debug!("could not send the download progress: {e}");
    }
}

/// Downloads the database and reports progress as `{status, progress, message}`
/// messages. The first half of the progress is the download, the second half
/// the unpacking.
pub fn geolite_download(app: &Arc<App>, req: Request<Incoming>) -> Resp {
    let app = app.clone();
    upgrade(req, move |socket| download_loop(app, socket))
}

async fn download_loop(app: Arc<App>, socket: Socket) {
    let (mut tx, mut rx) = socket.split();
    // Stop the download when the client goes away
    let (gone_tx, mut gone) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        while let Some(Ok(frame)) = rx.next().await {
            if matches!(frame, Message::Close(_)) {
                break;
            }
        }
        let _ = gone_tx.send(true);
    });

    let Some(_slot) = DownloadSlot::acquire() else {
        send(&mut tx, "error", "A download is already running", 0.0).await;
        return;
    };
    let paths = app.engine.geo_paths();
    if paths.default_db().exists() {
        send(&mut tx, "info", "Database already exists, removing old version...", 0.0).await;
    }
    send(&mut tx, "info", "Starting download...", 0.0).await;

    let client = match download::client() {
        Ok(c) => c,
        Err(e) => {
            send(&mut tx, "error", &format!("Download failed: {e}"), 0.0).await;
            return;
        }
    };

    // Progress is reported through a channel, the download task does not own the socket
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<f64>();
    let xz = paths.default_xz();
    let job = tokio::spawn(async move {
        let result = download::download(&client, download::DOWNLOAD_URL, &xz, |p| {
            let _ = progress_tx.send(p);
        })
        .await;
        result
    });
    tokio::pin!(job);

    let result = loop {
        tokio::select! {
            Some(p) = progress_rx.recv() => send(&mut tx, "progress", "Downloading GeoLite2 database...", p * 0.5).await,
            result = &mut job => break result,
            _ = gone.changed() => {
                job.abort();
                let _ = std::fs::remove_file(paths.default_xz());
                return;
            }
        }
    };
    match result {
        Ok(Ok(())) => send(&mut tx, "info", "Download complete", 50.0).await,
        Ok(Err(e)) => {
            send(&mut tx, "error", &format!("Download failed: {e}"), 0.0).await;
            return;
        }
        Err(e) => {
            send(&mut tx, "error", &format!("Download failed: {e}"), 0.0).await;
            return;
        }
    }

    send(&mut tx, "info", "Decompressing database...", 50.0).await;
    let (unpack_tx, mut unpack_rx) = tokio::sync::mpsc::unbounded_channel::<f64>();
    let unpack_paths = paths.clone();
    let mut unpack = tokio::task::spawn_blocking(move || {
        download::decompress(&unpack_paths, |p| {
            let _ = unpack_tx.send(p);
        })
    });
    let result = loop {
        tokio::select! {
            Some(p) = unpack_rx.recv() => send(&mut tx, "progress", "Decompressing database...", 50.0 + p * 0.5).await,
            result = &mut unpack => break result,
        }
    };
    match result {
        Ok(Ok(())) => {
            send(&mut tx, "info", "Database ready", 100.0).await;
            send(&mut tx, "info", "GeoLite2 database downloaded and installed successfully", 100.0).await;
        }
        Ok(Err(e)) => send(&mut tx, "error", &format!("Decompression failed: {e}"), 50.0).await,
        Err(e) => send(&mut tx, "error", &format!("Decompression failed: {e}"), 50.0).await,
    }
    let _ = tx.close().await;
}
