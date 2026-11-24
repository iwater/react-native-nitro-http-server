use super::super::*;
use std::ffi::CString;
use crate::types::{HttpRequest, RequestBodyData};
use crate::global::{REQUEST_BODIES, CHUNK_ACCUMULATORS};
use crate::tests::test_utils::{get_free_port, wait_for_port, run_outside_async, test_callback_ok};

extern "C" fn test_callback(_request: *mut HttpRequest) {
    println!("Test callback called");
}

/// Capture `request_id` without responding so the test controls when `send_response` runs.
static PENDING_REQUEST_ID: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

extern "C" fn capture_request_callback(request: *mut HttpRequest) {
    if request.is_null() { return; }
    unsafe {
        let req = &*request;
        let id = std::ffi::CStr::from_ptr(req.request_id).to_string_lossy().into_owned();
        *PENDING_REQUEST_ID.lock().unwrap() = Some(id);
    }
    unsafe {
        crate::c_api::free_http_request(request);
    }
}

async fn await_pending_request_id() -> String {
    for _ in 0..100 {
        if let Some(id) = PENDING_REQUEST_ID.lock().unwrap().take() {
            return id;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("callback never captured a request id");
}

#[test]
fn test_server_creation() {
    let result = start_server(8080, std::ptr::null(), test_callback);
    assert!(result >= 0);
}

#[test]
fn test_free_functions() {
    let c_str = CString::new("test").unwrap();
    let ptr = c_str.into_raw();
    unsafe {
        free_string(ptr);
    }
    
    let request = Box::new(HttpRequest {
        request_id: CString::new("id").unwrap().into_raw(),
        method: CString::new("GET").unwrap().into_raw(),
        path: CString::new("/").unwrap().into_raw(),
        headers_json: CString::new("{}").unwrap().into_raw(),
        body: CString::new("").unwrap().into_raw(),
        body_len: 0,
    });
    
    unsafe {
        free_http_request(Box::into_raw(request));
    }
}

#[test]
fn test_read_request_body_chunk() {
    let request_id = "test-req-id";
    let data = b"Hello World";
    
    // Setup global state
    {
        let mut bodies = REQUEST_BODIES.lock().unwrap();
        bodies.insert(request_id.to_string(), RequestBodyData {
            data: data.to_vec(),
            read_position: 0,
        });
    }
    
    // Test reading in chunks
    let mut buffer = [0u8; 5];
    let request_id_c = CString::new(request_id).unwrap();
    
    let read = read_request_body_chunk(
            request_id_c.as_ptr(),
            buffer.as_mut_ptr() as *mut i8, // c_char is i8 on some platforms
            5
        );
    
    assert_eq!(read, 5);
    assert_eq!(&buffer, b"Hello");
    
    // Read next chunk
    let mut buffer2 = [0u8; 10];
    let read2 = read_request_body_chunk(
            request_id_c.as_ptr(),
            buffer2.as_mut_ptr() as *mut i8,
            10
        );
    
    // Should finish reading (remaining 6 bytes: " World")
    assert_eq!(read2, 6);
    assert_eq!(&buffer2[..6], b" World");
    
    // Cleanup
    {
        let mut bodies = REQUEST_BODIES.lock().unwrap();
        bodies.remove(request_id);
    }
}

#[test]
fn test_write_response_chunk() {
    let request_id = "test-chunk-req";
    let chunk1 = b"Part 1";
    let chunk2 = b"Part 2";
    let request_id_c = CString::new(request_id).unwrap();
    
    // Write chunks via C API
    write_response_chunk(
        request_id_c.as_ptr(),
        chunk1.as_ptr() as *const i8,
        chunk1.len() as i32
    );
    
    write_response_chunk(
        request_id_c.as_ptr(),
        chunk2.as_ptr() as *const i8,
        chunk2.len() as i32
    );
    
    // Verify accumulation
    {
        let mut accs = CHUNK_ACCUMULATORS.lock().unwrap();
        let acc = accs.get(request_id).expect("Accumulator should exist");
        assert_eq!(acc.chunks.len(), 2);
        assert_eq!(acc.chunks[0], chunk1);
        assert_eq!(acc.chunks[1], chunk2);
        
        // Clean up manually since we aren't calling end_response
        accs.remove(request_id);
    }
}

#[tokio::test]
async fn test_invalid_status_code_falls_back_to_500() {
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), capture_request_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let client = reqwest::Client::new();
    let handle = tokio::spawn(async move {
        client.get(format!("http://127.0.0.1:{}/x", actual)).send().await
    });

    let rid = await_pending_request_id().await;

    // Invalid status 99 + header with control char + non-ASCII obs-text header + valid header.
    let rid_c = CString::new(rid).unwrap();
    let headers = CString::new(
        r#"{"X-Bad": "line1\nline2", "X-Name": "中文文件名.pdf", "X-Ok": "yes"}"#,
    )
    .unwrap();
    let body = CString::new("hi").unwrap();
    assert!(crate::c_api::send_response(
        rid_c.as_ptr(), 99, headers.as_ptr(), body.as_ptr() as *const i8, 2));

    let resp = handle.await.unwrap().expect("request must not be reset by a panic");
    assert_eq!(resp.status(), 500, "invalid status must fall back to 500");
    assert_eq!(resp.headers().get("x-ok").unwrap(), "yes", "valid header must survive");
    assert!(resp.headers().get("x-bad").is_none(), "header with control char must be dropped, not panic");
    // http 0.2 HeaderValue allows obs-text (0x80-0xFF), so non-ASCII bytes pass through untouched.
    assert_eq!(
        resp.headers().get("x-name").map(|v| v.as_bytes().to_vec()),
        Some("中文文件名.pdf".as_bytes().to_vec()),
        "non-ASCII obs-text header value should pass through intact"
    );

    run_outside_async(|| crate::c_api::stop_server());
}

/// Reset global `REQUEST_TIMEOUT_SECS` back to 30s on drop.
struct TimeoutResetGuard;
impl Drop for TimeoutResetGuard {
    fn drop(&mut self) {
        crate::global::REQUEST_TIMEOUT_SECS
            .store(30, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Verify `request_timeout_secs` in config overrides the default 30s timeout.
#[tokio::test]
async fn test_request_timeout_configurable() {
    let _reset = TimeoutResetGuard;

    let config = CString::new(r#"{"request_timeout_secs": 1}"#).unwrap();
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server_with_config(
            port as i32,
            host.as_ptr(),
            capture_request_callback,
            config.as_ptr(),
        )
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let start = std::time::Instant::now();
    let resp = reqwest::get(format!("http://127.0.0.1:{}/slow", actual))
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(resp.status(), 500);
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "configured timeout did not take effect: {:?}",
        elapsed
    );

    run_outside_async(|| crate::c_api::stop_app_server());
    let _ = PENDING_REQUEST_ID.lock().unwrap().take();
}

/// Verify timeout (1s) preempts a late response (2s) and cleans up the channel.
#[tokio::test]
async fn test_request_timeout_preempts_late_response() {
    let _reset = TimeoutResetGuard;

    let config = CString::new(r#"{"request_timeout_secs": 1}"#).unwrap();
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server_with_config(
            port as i32,
            host.as_ptr(),
            capture_request_callback,
            config.as_ptr(),
        )
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let client = reqwest::Client::new();
    let handle = tokio::spawn(async move {
        client
            .get(format!("http://127.0.0.1:{}/slow", actual))
            .send()
            .await
            .unwrap()
    });

    let rid = await_pending_request_id().await;

    // Respond after 2s; the 1s timeout should already have returned 500.
    let rid_c = CString::new(rid).unwrap();
    let headers = CString::new("{}").unwrap();
    let responder = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        crate::c_api::send_response(rid_c.as_ptr(), 200, headers.as_ptr(), std::ptr::null(), 0)
    });

    let start = std::time::Instant::now();
    let resp = handle.await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(resp.status(), 500, "1s timeout must fire before 2s response");
    assert!(elapsed < std::time::Duration::from_secs(5), "timeout did not take effect: {:?}", elapsed);

    assert!(!responder.await.unwrap(), "channel must already be cleaned up after timeout");

    run_outside_async(|| crate::c_api::stop_app_server());
}

/// Verify `RequestStateGuard` cleans up global maps when hyper drops the future on client disconnect.
#[tokio::test]
async fn test_global_maps_cleaned_on_client_disconnect() {
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), capture_request_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let base_channels = crate::global::response_channels_len();
    let base_bodies = crate::global::request_bodies_len();

    let client = reqwest::Client::new();
    let handle = tokio::spawn(async move {
        client
            .post(format!("http://127.0.0.1:{}/x", actual))
            .body("payload")
            .send()
            .await
            .unwrap()
    });

    let _rid = await_pending_request_id().await;
    handle.abort();

    for _ in 0..40 {
        if crate::global::response_channels_len() == base_channels
            && crate::global::request_bodies_len() == base_bodies
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    assert_eq!(
        crate::global::response_channels_len(),
        base_channels,
        "RESPONSE_CHANNELS leaked on client disconnect"
    );
    assert_eq!(
        crate::global::request_bodies_len(),
        base_bodies,
        "REQUEST_BODIES leaked on client disconnect"
    );

    run_outside_async(|| crate::c_api::stop_server());
}

#[tokio::test]
async fn test_global_maps_cleaned_after_send_response() {
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), capture_request_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    // Record baseline sizes rather than assuming 0.
    let base_channels = crate::global::response_channels_len();
    let base_bodies = crate::global::request_bodies_len();
    let base_accs = crate::global::chunk_accumulators_len();

    let client = reqwest::Client::new();
    let handle = tokio::spawn(async move {
        client
            .post(format!("http://127.0.0.1:{}/x", actual))
            .body("payload")
            .send()
            .await
            .unwrap()
    });

    let rid = await_pending_request_id().await;
    let rid_c = CString::new(rid).unwrap();
    let headers = CString::new("{}").unwrap();

    // Also accumulate a chunk before send_response to ensure CHUNK_ACCUMULATORS is cleaned up.
    let chunk = CString::new("chunk-data").unwrap();
    assert!(crate::c_api::write_response_chunk(
        rid_c.as_ptr(),
        chunk.as_ptr() as *const i8,
        10
    ));

    assert!(crate::c_api::send_response(
        rid_c.as_ptr(), 200, headers.as_ptr(), std::ptr::null(), 0));
    handle.await.unwrap();

    assert_eq!(crate::global::response_channels_len(), base_channels, "RESPONSE_CHANNELS leaked");
    assert_eq!(crate::global::request_bodies_len(), base_bodies, "REQUEST_BODIES leaked");
    assert_eq!(crate::global::chunk_accumulators_len(), base_accs, "CHUNK_ACCUMULATORS leaked");

    run_outside_async(|| crate::c_api::stop_server());
}

extern "C" fn accel_redirect_callback(request: *mut HttpRequest) {
    if request.is_null() { return; }
    unsafe {
        let req = &*request;
        let headers = CString::new(
            r#"{"X-Accel-Redirect": "/secret.txt", "Content-Type": "application/json", "X-Keep": "kept"}"#,
        )
        .unwrap();
        crate::c_api::send_response(
            req.request_id as *const i8,
            200,
            headers.as_ptr(),
            std::ptr::null(),
            0,
        );
    }
    unsafe {
        crate::c_api::free_http_request(request);
    }
}

#[tokio::test]
async fn test_x_accel_redirect_uses_canonical_root() {
    let temp = std::env::temp_dir().join("test_accel_root");
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::write(temp.join("secret.txt"), "accel-body").unwrap();

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let root = CString::new(temp.to_string_lossy().to_string()).unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_app_server(
            port as i32,
            host.as_ptr(),
            root.as_ptr(),
            accel_redirect_callback,
        )
    });
    assert!(actual > 0, "app server failed to start");
    wait_for_port(actual as u16).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/api/serve", actual))
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "canonical root should serve the redirected file");
    assert_eq!(resp.headers().get("x-keep").unwrap(), "kept", "other headers should be preserved");
    assert!(
        resp.headers().get("x-accel-redirect").is_none(),
        "X-Accel-Redirect must be stripped from the final response"
    );
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/plain; charset=utf-8",
        "Content-Type should match the target file MIME type"
    );
    assert_eq!(resp.text().await.unwrap(), "accel-body");

    run_outside_async(|| crate::c_api::stop_app_server());
    std::fs::remove_dir_all(&temp).ok();
}

#[tokio::test]
async fn test_port_out_of_range_rejected() {
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(70000, host.as_ptr(), capture_request_callback)
    });
    assert_eq!(actual, 0, "port 70000 must be rejected, not truncated to 4464");
}

// ---- Header JSON escaping contract across C++ -> Rust send_response ----

fn send_response_and_collect_headers(
    request_id: &str,
    headers_json: &str,
) -> std::collections::HashMap<String, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<crate::types::ResponseData>();
    crate::global::RESPONSE_CHANNELS
        .lock()
        .unwrap()
        .insert(request_id.to_string(), tx);

    let rid = CString::new(request_id).unwrap();
    let hj = CString::new(headers_json).unwrap();
    let body = CString::new("ok").unwrap();

    let ok = crate::c_api::send_response(rid.as_ptr(), 200, hj.as_ptr(), body.as_ptr(), 2);
    assert!(ok, "send_response should succeed when channel is registered");

    rx.blocking_recv().expect("expected ResponseData").headers
}

#[test]
fn test_send_response_parses_escaped_headers() {
    let headers = send_response_and_collect_headers(
        "t9-escaped",
        "{\"X-Multi\":\"line1\\nline2\",\"X-Tab\":\"a\\tb\",\"X-Plain\":\"ok\"}",
    );

    assert_eq!(
        headers.get("X-Multi").map(|s| s.as_str()),
        Some("line1\nline2"),
        "\\n should unescape to newline"
    );
    assert_eq!(
        headers.get("X-Tab").map(|s| s.as_str()),
        Some("a\tb"),
        "\\t should unescape to tab"
    );
    assert_eq!(headers.get("X-Plain").map(|s| s.as_str()), Some("ok"));
}

/// Raw unescaped control characters make the entire JSON parse fail, causing all
/// response headers to be dropped.
#[test]
fn test_send_response_drops_all_headers_on_raw_control_char() {
    let headers = send_response_and_collect_headers(
        "t9-raw",
        "{\"X-Multi\":\"line1\nline2\",\"X-Plain\":\"ok\"}",
    );

    assert!(
        headers.is_empty(),
        "raw control char should fail JSON parsing and drop headers, got: {:?}",
        headers
    );
}

// ---- Merging accumulated chunks with explicit body in send_response ----

fn collect_response_after<F: FnOnce()>(request_id: &str, f: F) -> crate::types::ResponseData {
    let (tx, rx) = tokio::sync::oneshot::channel::<crate::types::ResponseData>();
    crate::global::RESPONSE_CHANNELS
        .lock()
        .unwrap()
        .insert(request_id.to_string(), tx);
    f();
    rx.blocking_recv().expect("expected ResponseData")
}

/// Accumulated chunks (`write_response_chunk`) must be prepended before explicit body (`send_response`).
#[test]
fn test_send_response_merges_accumulated_chunks_before_explicit_body() {
    let rid = "t-merge-accumulated-chunks";
    let rid_c = CString::new(rid).unwrap();
    let hj = CString::new("{}").unwrap();

    let data = collect_response_after(rid, || {
        let chunk = CString::new("a").unwrap();
        assert!(crate::c_api::write_response_chunk(
            rid_c.as_ptr(),
            chunk.as_ptr() as *const i8,
            1
        ));

        let explicit: &[u8] = b"bc";
        assert!(crate::c_api::send_response(
            rid_c.as_ptr(),
            200,
            hj.as_ptr(),
            explicit.as_ptr() as *const i8,
            2
        ));
    });

    assert_eq!(
        data.body, b"abc",
        "accumulated chunks must precede explicit body"
    );
}

#[test]
fn test_send_response_without_chunks_keeps_explicit_body() {
    let rid = "t-no-accumulated-chunks";
    let rid_c = CString::new(rid).unwrap();
    let hj = CString::new("{}").unwrap();

    let data = collect_response_after(rid, || {
        let explicit: &[u8] = b"hello";
        assert!(crate::c_api::send_response(
            rid_c.as_ptr(),
            200,
            hj.as_ptr(),
            explicit.as_ptr() as *const i8,
            5
        ));
    });

    assert_eq!(data.body, b"hello", "explicit body must pass through unchanged");
}

#[test]
fn test_send_response_with_only_accumulated_chunks() {
    let rid = "t-only-accumulated-chunks";
    let rid_c = CString::new(rid).unwrap();
    let hj = CString::new("{}").unwrap();

    let data = collect_response_after(rid, || {
        let chunk = CString::new("streamed").unwrap();
        assert!(crate::c_api::write_response_chunk(
            rid_c.as_ptr(),
            chunk.as_ptr() as *const i8,
            8
        ));
        assert!(crate::c_api::send_response(
            rid_c.as_ptr(),
            200,
            hj.as_ptr(),
            std::ptr::null(),
            0
        ));
    });

    assert_eq!(data.body, b"streamed", "accumulated chunks must still be sent when explicit body is empty");
}


// Callback path request body size limit (default 100MB, configurable; 413 without calling JS)

extern "C" fn recording_callback(request: *mut HttpRequest) {
    CALLBACK_HITS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    crate::tests::test_utils::test_callback_ok(request);
}

static CALLBACK_HITS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct MaxBodySizeResetGuard;
impl Drop for MaxBodySizeResetGuard {
    fn drop(&mut self) {
        crate::global::MAX_BODY_SIZE.store(
            100 * 1024 * 1024,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

async fn start_app_server_with(config_json: &'static str) -> u16 {
    let config = CString::new(config_json).unwrap();
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server_with_config(
            port as i32,
            host.as_ptr(),
            recording_callback,
            config.as_ptr(),
        )
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;
    // Reset after wait_for_port because it sends a probe GET that hits the callback.
    CALLBACK_HITS.store(0, std::sync::atomic::Ordering::SeqCst);
    actual as u16
}

#[tokio::test]
async fn test_oversized_body_rejected_413() {
    let _reset = MaxBodySizeResetGuard;
    let port = start_app_server_with(r#"{"max_body_size": 1024}"#).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/big", port))
        .body(vec![0u8; 2048])
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        413,
        "oversized body must be rejected with 413"
    );
    assert_eq!(
        CALLBACK_HITS.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "JS callback must not be invoked when returning 413"
    );

    run_outside_async(|| crate::c_api::stop_app_server());
}

/// Chunked requests without Content-Length must also be rejected when exceeding `max_body_size`.
#[tokio::test]
async fn test_oversized_chunked_body_rejected_413() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let _reset = MaxBodySizeResetGuard;
    let port = start_app_server_with(r#"{"max_body_size": 1024}"#).await;

    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let chunk = vec![b'x'; 2048];
    let mut raw = format!(
        "POST /big HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        port
    )
    .into_bytes();
    raw.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
    raw.extend_from_slice(&chunk);
    raw.extend_from_slice(b"\r\n0\r\n\r\n");
    sock.write_all(&raw).await.unwrap();

    let mut resp = Vec::new();
    sock.read_to_end(&mut resp).await.unwrap();
    let text = String::from_utf8_lossy(&resp);
    assert!(
        text.starts_with("HTTP/1.1 413"),
        "oversized chunked body must return 413, got: {:?}",
        &text[..text.len().min(60)]
    );
    assert_eq!(
        CALLBACK_HITS.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "JS callback must not be invoked for oversized chunked body"
    );

    run_outside_async(|| crate::c_api::stop_app_server());
}

#[tokio::test]
async fn test_body_within_limit_still_passes() {
    let _reset = MaxBodySizeResetGuard;
    let port = start_app_server_with(r#"{"max_body_size": 1024}"#).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/ok", port))
        .body(vec![0u8; 512])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "body within limit must succeed");
    assert_eq!(
        CALLBACK_HITS.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "callback should be invoked once"
    );

    run_outside_async(|| crate::c_api::stop_app_server());
}

#[tokio::test]
async fn test_default_limit_allows_1mb_body() {
    let _reset = MaxBodySizeResetGuard;
    let port = start_app_server_with(r#"{"verbose": false}"#).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/mb", port))
        .body(vec![0u8; 1024 * 1024])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "default 100MB limit must allow 1MB body");

    run_outside_async(|| crate::c_api::stop_app_server());
}

#[tokio::test]
async fn test_plain_http_server_enforces_body_limit() {
    let _reset = MaxBodySizeResetGuard;
    crate::global::MAX_BODY_SIZE.store(1024, std::sync::atomic::Ordering::Relaxed);

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), recording_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;
    CALLBACK_HITS.store(0, std::sync::atomic::Ordering::SeqCst);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/big", actual))
        .body(vec![0u8; 2048])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 413, "plain HttpServer must also enforce MAX_BODY_SIZE");
    assert_eq!(
        CALLBACK_HITS.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "plain HttpServer must not invoke JS callback on 413"
    );

    run_outside_async(|| crate::c_api::stop_server());
}

// getStats counters

fn read_stats() -> serde_json::Value {
    let ptr = crate::c_api::get_server_stats();
    assert!(!ptr.is_null(), "get_server_stats must not return null");
    let text = unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { crate::c_api::free_string(ptr as *mut std::os::raw::c_char) };
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("stats must be valid JSON: {} / {}", e, text))
}

#[tokio::test]
async fn test_get_server_stats_total_requests_counts() {
    let before = read_stats()["totalRequests"].as_u64().unwrap();

    let port = start_app_server_with(r#"{"verbose": false}"#).await;
    let client = reqwest::Client::new();
    for _ in 0..3 {
        let _ = client
            .get(format!("http://127.0.0.1:{}/x", port))
            .send()
            .await
            .unwrap();
    }

    let after = read_stats();
    assert_eq!(
        after["totalRequests"].as_u64().unwrap() - before,
        3,
        "totalRequests must increment by 3 after 3 requests"
    );

    assert_eq!(after["status"].as_str(), Some("running"));
    for key in [
        "totalRequests",
        "activeConnections",
        "bytesSent",
        "bytesReceived",
        "uptime",
        "errorCount",
    ] {
        assert!(
            after.get(key).and_then(|v| v.as_u64()).is_some(),
            "field {} must be a non-negative integer, stats = {}",
            key,
            after
        );
    }

    run_outside_async(|| crate::c_api::stop_app_server());
}

#[tokio::test]
async fn test_get_server_stats_error_count_and_uptime() {
    let _reset = TimeoutResetGuard;
    let before_errors = read_stats()["errorCount"].as_u64().unwrap();

    // 1s timeout + non-responding callback triggers timeout error branch.
    let config = CString::new(r#"{"request_timeout_secs": 1}"#).unwrap();
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server_with_config(
            port as i32,
            host.as_ptr(),
            capture_request_callback,
            config.as_ptr(),
        )
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;
    let _ = PENDING_REQUEST_ID.lock().unwrap().take();

    let resp = reqwest::get(format!("http://127.0.0.1:{}/slow", actual))
        .await
        .unwrap();
    assert_eq!(resp.status(), 500, "timeout should return 500");

    let after = read_stats();
    assert_eq!(
        after["errorCount"].as_u64().unwrap() - before_errors,
        1,
        "timeout must increment errorCount by 1"
    );
    assert!(
        after["uptime"].as_u64().unwrap() >= 1,
        "uptime must be >= 1 after 1s timeout, got {}",
        after["uptime"]
    );

    run_outside_async(|| crate::c_api::stop_app_server());
    let _ = PENDING_REQUEST_ID.lock().unwrap().take();
}

#[tokio::test]
async fn test_get_server_stats_counts_plain_and_static_servers() {
    // Plain HttpServer
    let before = read_stats()["totalRequests"].as_u64().unwrap();
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), recording_callback)
    });
    assert!(actual > 0);
    wait_for_port(actual as u16).await;
    let _ = reqwest::get(format!("http://127.0.0.1:{}/a", actual)).await.unwrap();
    assert_eq!(
        read_stats()["totalRequests"].as_u64().unwrap() - before,
        1,
        "plain HttpServer requests must count toward totalRequests"
    );
    run_outside_async(|| crate::c_api::stop_server());

    // StaticHttpServer
    let root = std::env::temp_dir().join(format!("t16_static_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"hi").unwrap();

    let before = read_stats()["totalRequests"].as_u64().unwrap();
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let root_str = CString::new(root.to_string_lossy().to_string()).unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_static_server(port as i32, host.as_ptr(), root_str.as_ptr())
    });
    assert!(actual > 0);
    wait_for_port(actual as u16).await;
    let _ = reqwest::get(format!("http://127.0.0.1:{}/a.txt", actual)).await.unwrap();
    assert_eq!(
        read_stats()["totalRequests"].as_u64().unwrap() - before,
        1,
        "StaticHttpServer requests must count toward totalRequests"
    );
    run_outside_async(|| crate::c_api::stop_static_server());

    std::fs::remove_dir_all(&root).ok();
}

/// Verify `start_server_with_options` handles explicit, negative (clamped to 0), and 0 worker_threads.
#[tokio::test]
async fn test_start_server_with_options_worker_threads() {
    for workers in [2i32, -1, 0] {
        let port = get_free_port();
        let host = CString::new("127.0.0.1").unwrap();
        let actual = run_outside_async(move || {
            crate::c_api::start_server_with_options(
                port as i32,
                host.as_ptr(),
                test_callback_ok,
                workers,
            )
        });
        assert!(actual > 0, "failed to start with worker_threads={}", workers);
        wait_for_port(actual as u16).await;

        let resp = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{}/probe", actual))
            .send()
            .await
            .expect("request failed");
        assert_eq!(resp.status(), 200, "unexpected status for worker_threads={}", workers);
        assert_eq!(
            resp.text().await.unwrap(),
            "Hello World",
            "unexpected body for worker_threads={}",
            workers
        );

        run_outside_async(|| crate::c_api::stop_server());
    }
}

/// Ensure legacy 3-arg `start_server` keeps working for C ABI backwards compatibility.
#[tokio::test]
async fn test_start_server_legacy_equals_default_options() {
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), test_callback_ok)
    });
    assert!(actual > 0, "legacy start_server failed");
    wait_for_port(actual as u16).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/probe", actual))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    run_outside_async(|| crate::c_api::stop_server());
}

#[test]
fn test_read_request_body_chunk_preserves_utf8_boundaries() {
    let request_id = "test-utf8-chunk";
    // 10 bytes total; reading with a 4-byte buffer must not split multi-byte UTF-8 characters.
    let data = "ab中文cd".as_bytes();
    {
        let mut bodies = REQUEST_BODIES.lock().unwrap();
        bodies.insert(request_id.to_string(), RequestBodyData {
            data: data.to_vec(),
            read_position: 0,
        });
    }

    let request_id_c = CString::new(request_id).unwrap();
    let mut chunks: Vec<String> = Vec::new();
    let mut buf = [0u8; 4];
    loop {
        let n = read_request_body_chunk(
            request_id_c.as_ptr(),
            buf.as_mut_ptr() as *mut std::os::raw::c_char,
            4,
        );
        assert!(n >= 0);
        if n == 0 {
            break;
        }
        let s = std::str::from_utf8(&buf[..n as usize])
            .expect("each chunk must be valid UTF-8 without splitting multi-byte chars");
        chunks.push(s.to_string());
    }

    assert_eq!(chunks.concat(), "ab中文cd");
    REQUEST_BODIES.lock().unwrap().remove(request_id);
}
