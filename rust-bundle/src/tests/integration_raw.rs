use crate::server::http_server::HttpServer;
use crate::tests::test_utils::{get_free_port, test_callback_ok, wait_for_server};
use crate::types::{HttpRequest, ResponseData};
use crate::global::RESPONSE_CHANNELS;
use std::collections::HashMap;
use std::time::Duration;
use reqwest::StatusCode;

/// 把请求体原样回给客户端：用于验证 body 指针 / 长度在 C 边界两侧的一致性。
/// 注意必须先拷成 Vec 再 free —— free_http_request 会回收 req.body。
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
        server.start(0); // 0 = tokio 默认（按 CPU 核数）
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

/// body 必须原样穿过 C 边界。现有用例虽然 POST 了 "data"，但回调根本不读 body，
/// 所以 body 分配（alloc_request_body）的指针 / 长度正确性此前没有断言覆盖。
#[tokio::test]
async fn test_raw_server_body_roundtrip() {
    let free_port = get_free_port();

    let server_task = tokio::spawn(async move {
        let mut server = HttpServer::new(free_port, "127.0.0.1".to_string(), echo_body_callback);
        server.start(0); // 0 = tokio 默认（按 CPU 核数）
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    wait_for_server(free_port).await;
    let client = reqwest::Client::new();
    let base_url = format!("http://127.0.0.1:{}", free_port);

    for payload in [
        vec![0x41u8; 1],                     // 最小
        "hello 中文".as_bytes().to_vec(),      // 多字节 UTF-8
        vec![0u8, 1, 2, 255, 0, 42],         // 含 \0 与 0xFF，二进制安全
        vec![0x41u8; 70000],                 // 大于常见分配粒度
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
            "body 未原样回传（len={}）",
            payload.len()
        );
    }

    server_task.abort();
}
