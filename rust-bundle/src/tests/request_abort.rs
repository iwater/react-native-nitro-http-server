//! Tests for the Rust client-abort callback chain:
//! client disconnect -> hyper drops handler future -> `RequestStateGuard::drop`
//! -> `notify_request_aborted()` -> registered C callback.

use crate::tests::test_utils::{get_free_port, run_outside_async, wait_for_port};
use crate::types::{HttpRequest, ResponseData};
use std::ffi::CString;
use std::sync::Mutex;

static ABORTED_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

extern "C" fn capture_abort_callback(request_id: *const std::os::raw::c_char) {
    if request_id.is_null() {
        return;
    }
    let id = unsafe { std::ffi::CStr::from_ptr(request_id) }
        .to_string_lossy()
        .into_owned();
    ABORTED_IDS.lock().unwrap().push(id);
}

/// Capture `request_id` without sending a response to simulate an in-flight handler.
static PENDING_ID: Mutex<Option<String>> = Mutex::new(None);

extern "C" fn capture_only_callback(request: *mut HttpRequest) {
    if request.is_null() {
        return;
    }
    let id = unsafe {
        let req = &*request;
        std::ffi::CStr::from_ptr(req.request_id)
            .to_string_lossy()
            .into_owned()
    };
    *PENDING_ID.lock().unwrap() = Some(id);
    unsafe {
        crate::c_api::free_http_request(request);
    }
}

/// Capture `request_id` and immediately respond with 200 (negative control).
static RESPONDED_ID: Mutex<Option<String>> = Mutex::new(None);

extern "C" fn capture_and_respond_callback(request: *mut HttpRequest) {
    if request.is_null() {
        return;
    }
    let id = unsafe {
        let req = &*request;
        std::ffi::CStr::from_ptr(req.request_id)
            .to_string_lossy()
            .into_owned()
    };
    *RESPONDED_ID.lock().unwrap() = Some(id.clone());

    if let Some(tx) = crate::global::RESPONSE_CHANNELS.lock().unwrap().remove(&id) {
        let _ = tx.send(ResponseData {
            status_code: 200,
            headers: std::collections::HashMap::new(),
            body: b"ok".to_vec(),
        });
    }

    unsafe {
        crate::c_api::free_http_request(request);
    }
}

fn register_abort_callback() {
    ABORTED_IDS.lock().unwrap().clear();
    assert!(
        crate::c_api::set_request_aborted_callback(Some(capture_abort_callback)),
        "registering abort callback should return true"
    );
}

fn unregister_abort_callback() {
    let _ = crate::c_api::set_request_aborted_callback(None);
}

async fn await_pending_id() -> String {
    for _ in 0..100 {
        if let Some(id) = PENDING_ID.lock().unwrap().take() {
            return id;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("handler was never invoked");
}

async fn await_aborted(expected: &str) -> bool {
    for _ in 0..100 {
        if ABORTED_IDS.lock().unwrap().iter().any(|id| id == expected) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn test_request_aborted_callback_fires_on_client_disconnect() {
    use tokio::io::AsyncWriteExt;

    register_abort_callback();

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), capture_only_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", actual as u16))
        .await
        .expect("connect failed");
    sock.write_all(b"POST /abort HTTP/1.1\r\nHost: localhost\r\nContent-Length: 7\r\n\r\npayload")
        .await
        .expect("write failed");
    sock.flush().await.expect("flush failed");

    // Wait until the handler has been invoked before closing the client socket.
    let rid = await_pending_id().await;
    drop(sock);

    assert!(
        await_aborted(&rid).await,
        "abort callback was not invoked on client disconnect (request_id={rid}, received {:?})",
        ABORTED_IDS.lock().unwrap()
    );

    assert_eq!(
        crate::global::response_channels_len(),
        0,
        "RESPONSE_CHANNELS should be empty after disconnect"
    );

    run_outside_async(|| crate::c_api::stop_server());
    unregister_abort_callback();
}

/// Negative control: normal 200 responses must not trigger the abort callback.
#[tokio::test]
async fn test_request_aborted_callback_not_fired_on_normal_response() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    register_abort_callback();
    *RESPONDED_ID.lock().unwrap() = None;

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), capture_and_respond_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", actual as u16))
        .await
        .expect("connect failed");
    sock.write_all(b"GET /ok HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("write failed");
    sock.flush().await.expect("flush failed");

    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.expect("read failed");
    assert!(
        buf.starts_with(b"HTTP/1.1 200"),
        "expected 200 response, got {:?}",
        String::from_utf8_lossy(&buf[..buf.len().min(40)])
    );

    let rid = RESPONDED_ID
        .lock()
        .unwrap()
        .clone()
        .expect("handler was never invoked");

    // Allow time for RequestStateGuard to drop after the handler future completes.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    assert!(
        !ABORTED_IDS.lock().unwrap().iter().any(|id| id == &rid),
        "normal completed request must not trigger abort callback (request_id={rid})"
    );

    run_outside_async(|| crate::c_api::stop_server());
    unregister_abort_callback();
}

/// Disconnecting when no abort callback is registered must remain safe and still clean up state.
#[tokio::test]
async fn test_no_abort_callback_registered_is_safe() {
    use tokio::io::AsyncWriteExt;

    unregister_abort_callback();

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), capture_only_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", actual as u16))
        .await
        .expect("connect failed");
    sock.write_all(b"POST /abort HTTP/1.1\r\nHost: localhost\r\nContent-Length: 7\r\n\r\npayload")
        .await
        .expect("write failed");
    sock.flush().await.expect("flush failed");

    let _rid = await_pending_id().await;
    drop(sock);

    for _ in 0..40 {
        if crate::global::response_channels_len() == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        crate::global::response_channels_len(),
        0,
        "state must still be cleaned up without an abort callback"
    );

    run_outside_async(|| crate::c_api::stop_server());
}
