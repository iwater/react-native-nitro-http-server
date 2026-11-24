use std::collections::HashMap;
use std::sync::{Mutex};
use std::sync::atomic::{AtomicU8, AtomicU64};
use once_cell::sync::Lazy;
use tokio::sync::oneshot;

use crate::types::{ResponseData, RequestBodyData, StreamingResponseData, ResponseChunkAccumulator, RequestAbortedCallback};

// Global logging level: 0=Off, 1=Error, 2=Warn, 3=Info, 4=Debug
pub static VERBOSE: AtomicU8 = AtomicU8::new(0);

// Global response timeout in seconds (default: 30).
// Increase via `request_timeout_secs` in config for long-polling or SSE.
// Global state on the singleton server; re-applied when restarting a config server.
pub static REQUEST_TIMEOUT_SECS: AtomicU64 = AtomicU64::new(30);

// Global request body size limit in bytes (default: 100MB, matching buffer_upload).
// Required because hyper 0.14's `to_bytes` is unbounded.
// Global state on the singleton server; re-applied when restarting a config server.
pub static MAX_BODY_SIZE: AtomicU64 = AtomicU64::new(100 * 1024 * 1024);

// ---- getStats counters -----------------------------------------------------
// Only counters that cover 100% of request paths are tracked here; the remaining
// ServerStats fields stay 0 (see c_api::get_server_stats). Process-wide across restarts.

/// Number of requests entering request handling across all three servers.
pub static TOTAL_REQUESTS: AtomicU64 = AtomicU64::new(0);

/// Number of requests where the server itself failed to produce a response
/// (JS timeout, closed response channel, unavailable static root, plugin I/O error -> 500).
/// Intentional 4xx/5xx responses from JS or plugins are not counted.
pub static ERROR_COUNT: AtomicU64 = AtomicU64::new(0);

/// Unix timestamp (seconds) of the most recent `start_*` call (0 if never started).
pub static SERVER_START_UNIX: AtomicU64 = AtomicU64::new(0);

/// Record server start time for `uptime_secs()`.
pub fn mark_server_started() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    SERVER_START_UNIX.store(now, std::sync::atomic::Ordering::Relaxed);
}

/// Seconds elapsed since the most recent start (0 if never started).
pub fn uptime_secs() -> u64 {
    let start = SERVER_START_UNIX.load(std::sync::atomic::Ordering::Relaxed);
    if start == 0 {
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(start)
}

pub static RESPONSE_CHANNELS: Lazy<Mutex<HashMap<String, oneshot::Sender<ResponseData>>>> = 
    Lazy::new(|| Mutex::new(HashMap::new()));

pub static REQUEST_BODIES: Lazy<Mutex<HashMap<String, RequestBodyData>>> = 
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Reserved for future streaming response enhancements.
#[allow(dead_code)]
pub static STREAMING_RESPONSE_CHANNELS: Lazy<Mutex<HashMap<String, oneshot::Sender<StreamingResponseData>>>> = 
    Lazy::new(|| Mutex::new(HashMap::new()));

pub static CHUNK_ACCUMULATORS: Lazy<Mutex<HashMap<String, ResponseChunkAccumulator>>> = 
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Global custom MIME type map (extension -> MIME type).
pub static CUSTOM_MIME_TYPES: Lazy<Mutex<HashMap<String, String>>> = 
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Idempotently clean up all three global tables for a request.
/// Locks one map at a time to avoid lock-ordering issues.
pub(crate) fn cleanup_request_state(request_id: &str) {
    RESPONSE_CHANNELS.lock().unwrap().remove(request_id);
    REQUEST_BODIES.lock().unwrap().remove(request_id);
    CHUNK_ACCUMULATORS.lock().unwrap().remove(request_id);
}

/// Global client-abort callback registered at server start (singleton native server).
pub static ABORT_CALLBACK: Lazy<Mutex<Option<RequestAbortedCallback>>> = Lazy::new(|| Mutex::new(None));

/// Best-effort notification to JS that the client disconnected.
fn notify_request_aborted(request_id: &str) {
    let cb = *ABORT_CALLBACK.lock().unwrap();
    let Some(cb) = cb else { return };
    match std::ffi::CString::new(request_id) {
        Ok(c) => cb(c.as_ptr()),
        Err(_) => crate::log_warn!("request_id contains NUL, cannot notify abort: {}", request_id),
    }
}

/// Clean up request state and return `true` if no response was ever sent.
/// `send_response` removes the sender before sending, so a remaining entry means no response.
pub(crate) fn cleanup_request_state_checking_response(request_id: &str) -> bool {
    let never_responded = RESPONSE_CHANNELS.lock().unwrap().remove(request_id).is_some();
    REQUEST_BODIES.lock().unwrap().remove(request_id);
    CHUNK_ACCUMULATORS.lock().unwrap().remove(request_id);
    never_responded
}

/// RAII guard ensuring per-request map entries are cleaned up on every exit path,
/// including when hyper drops the handler future on client disconnect (in which case
/// the timeout branch never runs). No-op on normal completion.
pub(crate) struct RequestStateGuard {
    request_id: String,
}

impl RequestStateGuard {
    pub(crate) fn new(request_id: String) -> Self {
        Self { request_id }
    }
}

impl Drop for RequestStateGuard {
    fn drop(&mut self) {
        // Never-responded indicates client disconnect (hyper dropped the future) rather than normal completion.
        if cleanup_request_state_checking_response(&self.request_id) {
            crate::log_info!("Request {} aborted by client (future dropped before response)", self.request_id);
            notify_request_aborted(&self.request_id);
        }
    }
}

// Test-only map length observers for leak assertions.
#[cfg(test)]
pub fn response_channels_len() -> usize {
    RESPONSE_CHANNELS.lock().unwrap().len()
}

#[cfg(test)]
pub fn request_bodies_len() -> usize {
    REQUEST_BODIES.lock().unwrap().len()
}

#[cfg(test)]
pub fn chunk_accumulators_len() -> usize {
    CHUNK_ACCUMULATORS.lock().unwrap().len()
}
