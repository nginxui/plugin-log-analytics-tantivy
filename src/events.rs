//! Fan out of the indexing events to the `/events` clients. The event types and
//! payloads are the ones the web pages already read.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;

pub const PROCESSING_STATUS: &str = "processing_status";
pub const INDEX_READY: &str = "nginx_log_index_ready";
pub const INDEX_PROGRESS: &str = "nginx_log_index_progress";
pub const INDEX_COMPLETE: &str = "nginx_log_index_complete";

/// One message of the stream.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Event {
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Value,
}

/// How many events a slow client may lag behind before it loses the oldest.
const BUFFER: usize = 64;

/// Delivers events to subscribers without ever waiting for one.
pub struct Hub {
    tx: broadcast::Sender<Event>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Self { tx: broadcast::channel(BUFFER).0 }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn publish(&self, kind: &str, data: Value) {
        // Without a subscriber the event has nobody to reach
        let _ = self.tx.send(Event { kind: kind.to_owned(), data });
    }

    pub fn subscribers(&self) -> usize {
        self.tx.receiver_count()
    }

    pub fn progress(&self, log_path: &str, progress: f64, stage: &str, status: &str, elapsed_ms: i64, remain_ms: i64) {
        self.publish(
            INDEX_PROGRESS,
            json!({
                "log_path": log_path,
                "progress": progress,
                "stage": stage,
                "status": status,
                "elapsed_time": elapsed_ms,
                "estimated_remain": remain_ms,
            }),
        );
    }

    pub fn complete(
        &self,
        log_path: &str,
        success: bool,
        duration_ms: i64,
        total_lines: u64,
        indexed_size: u64,
        error: &str,
    ) {
        let mut data = json!({
            "log_path": log_path,
            "success": success,
            "duration": duration_ms,
            "total_lines": total_lines,
            "indexed_size": indexed_size,
        });
        if !error.is_empty() {
            data["error"] = json!(error);
        }
        self.publish(INDEX_COMPLETE, data);
    }

    pub fn ready(&self, log_path: &str, start_time: i64, end_time: i64) {
        self.publish(
            INDEX_READY,
            json!({
                "log_path": log_path,
                "start_time": start_time,
                "end_time": end_time,
                "available": true,
                "index_status": "ready",
            }),
        );
    }
}

type Observer = Box<dyn Fn(bool) + Send + Sync>;

/// Whether an indexing run is active, reported on every change.
pub struct Processing {
    hub: Arc<Hub>,
    indexing: AtomicBool,
    observer: RwLock<Option<Observer>>,
}

impl Processing {
    pub fn new(hub: Arc<Hub>) -> Self {
        Self { hub, indexing: AtomicBool::new(false), observer: RwLock::new(None) }
    }

    pub fn indexing(&self) -> bool {
        self.indexing.load(Ordering::SeqCst)
    }

    /// Sets a function called with every new state, for the host indicator.
    pub fn observe(&self, observer: impl Fn(bool) + Send + Sync + 'static) {
        *self.observer.write().expect("observer lock") = Some(Box::new(observer));
    }

    /// Records the state. Setting the current value again does nothing.
    pub fn set(&self, indexing: bool) {
        if self.indexing.swap(indexing, Ordering::SeqCst) == indexing {
            return;
        }
        self.broadcast();
        if let Some(observer) = self.observer.read().expect("observer lock").as_ref() {
            observer(indexing);
        }
    }

    /// Publishes the current state, for a client that just connected.
    pub fn broadcast(&self) {
        self.hub.publish(PROCESSING_STATUS, json!({ "nginx_log_indexing": self.indexing() }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn events_serialize_with_type_and_data() {
        let e = Event { kind: INDEX_READY.into(), data: json!({"a": 1}) };
        assert_eq!(serde_json::to_value(&e).unwrap(), json!({"type": "nginx_log_index_ready", "data": {"a": 1}}));
    }

    #[test]
    fn subscribers_receive_and_a_slow_one_does_not_block() {
        let hub = Hub::new();
        let mut rx = hub.subscribe();
        for i in 0..(BUFFER * 2) {
            hub.publish("x", json!(i));
        }
        // The oldest events are gone, the receiver learns it lagged
        assert!(matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Lagged(_))));
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn processing_reports_changes_only() {
        let hub = Arc::new(Hub::new());
        let mut rx = hub.subscribe();
        let p = Processing::new(hub);
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        p.observe(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        p.set(true);
        p.set(true);
        p.set(false);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(rx.try_recv().unwrap().data, json!({"nginx_log_indexing": true}));
        assert_eq!(rx.try_recv().unwrap().data, json!({"nginx_log_indexing": false}));
        assert!(rx.try_recv().is_err());
    }
}
