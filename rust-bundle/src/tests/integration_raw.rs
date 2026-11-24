use crate::server::http_server::HttpServer;
use crate::tests::test_utils::{get_free_port, test_callback_ok, wait_for_server};
use crate::types::{HttpRequest, ResponseData};
use crate::global::RESPONSE_CHANNELS;
use std::collections::HashMap;
use std::time::Duration;
use reqwest::StatusCode;

/// Echo the request body back to verify pointer/length integrity across the C boundary.
/// Must copy into a Vec before `free_http_request` frees `req.body`.
extern "C" fn echo_body_callback(request: *mut HttpRequest) {
    if request.is_null() { return; }
    unsafe {
        let req = &*request;
        let id = std::ffi::CStr::from_ptr(req.request_id).to_string_lossy().into_owned();
        let body = if req.body.is_null() || req.body_len <= 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(req.body as *const u8, req.body_len as usize).to_vec()
        };

        if let Some(chan) = RESPONSE_CHANNELS.lock().unwrap().remove(&id) {
            let _ = chan.send(ResponseData {
                status_code: 200,
                headers: HashMap::new(),
                body,
            });
        }

        crate::c_api::free_http_request(request);
    }
}

#[tokio::test]
async fn test_raw_server_integration() {
    let free_port = get_free_port();
    
    let server_task = tokio::spawn(async move {
        let mut server = HttpServer::new(free_port, "127.0.0.1".to_string(), test_callback_ok);
        server.start(0); // 0 = tokio default (CPU core count)
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    wait_for_server(free_port).await;
    let base_url = format!("http://127.0.0.1:{}", free_port);
    let client = reqwest::Client::new();

    // 1. Any request should hit callback
    let resp = client.get(format!("{}/any-path", base_url)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "Hello World");
    
    // 2. Another method
    let resp_post = client.post(format!("{}/submit", base_url))
        .body("data")
        .send().await.unwrap();
    assert_eq!(resp_post.status(), StatusCode::OK);
    assert_eq!(resp_post.text().await.unwrap(), "Hello World");

    server_task.abort();
}

/// Verify binary and UTF-8 request bodies round-trip intact across the C boundary.
#[tokio::test]
async fn test_raw_server_body_roundtrip() {
    let free_port = get_free_port();

    let server_task = tokio::spawn(async move {
        let mut server = HttpServer::new(free_port, "127.0.0.1".to_string(), echo_body_callback);
        server.start(0); // 0 = tokio default (CPU core count)
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    wait_for_server(free_port).await;
    let client = reqwest::Client::new();
    let base_url = format!("http://127.0.0.1:{}", free_port);

    for payload in [
        vec![0x41u8; 1],                     // 1-byte minimal payload
        "hello 中文".as_bytes().to_vec(),      // Multi-byte UTF-8
        vec![0u8, 1, 2, 255, 0, 42],         // Binary-safe with NUL and 0xFF
        vec![0x41u8; 70000],                 // Larger than typical buffer sizes
    ] {
        let resp = client
            .post(format!("{}/echo", base_url))
            .body(payload.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.bytes().await.unwrap().to_vec(),
            payload,
            "body roundtrip mismatch (len={})",
            payload.len()
        );
    }

    server_task.abort();
}
