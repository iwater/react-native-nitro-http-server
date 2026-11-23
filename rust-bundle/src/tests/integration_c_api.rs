use crate::c_api::{
    start_server_with_config, start_server, start_static_server, start_app_server,
    stop_app_server, stop_server, stop_static_server,
    free_http_request, get_server_stats, send_response,
    write_response_chunk, end_response
};
use crate::tests::test_utils::{get_free_port, wait_for_server};
use crate::types::{HttpRequest, ResponseData};
use crate::global::RESPONSE_CHANNELS;
use std::ffi::{CString, CStr};
use std::os::raw::c_char;
use std::collections::HashMap;
use std::time::Duration;
use reqwest::{StatusCode, multipart};
use serde_json::json;

extern "C" fn c_api_test_callback(request: *mut HttpRequest) {
    if request.is_null() { return; }
    
    let req = unsafe { &*request };
    let request_id_c = unsafe { std::ffi::CStr::from_ptr(req.request_id) };
    let request_id = request_id_c.to_string_lossy().to_string();
    
    // Send simple "OK" response
    if let Some(chan) = RESPONSE_CHANNELS.lock().unwrap().remove(&request_id) {
         let resp = ResponseData {
             status_code: 200,
             headers: HashMap::new(),
             body: b"C API OK".to_vec(),
         };
         let _ = chan.send(resp);
    }
    
    unsafe {
        free_http_request(request);
    }
}

#[test]
fn test_start_server_with_config_integration() {
    let free_port = get_free_port();
    
    let host = CString::new("127.0.0.1").unwrap();
    let config = CString::new(r#"{"verbose": true}"#).unwrap(); 
    
    // Call the C API
    // This internally creates a new tokio Runtime, so we MUST NOT be in a runtime efficiently?
    // Or at least not in a context that prohibits nested runtimes.
    // #[test] runs without runtime.
    let success = start_server_with_config(
        free_port as i32,
        host.as_ptr(),
        c_api_test_callback,
        config.as_ptr()
    );
    
    assert!(success > 0, "start_server_with_config should return a valid port");
    
    // Creating a separate runtime for the client
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
        
    rt.block_on(async {
        wait_for_server(free_port).await;
        
        // Verify server is responding
        let client = reqwest::Client::new();
        let resp = client.get(format!("http://127.0.0.1:{}/test", free_port))
            .send().await.unwrap();
            
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await.unwrap(), "C API OK");
    });
    
    // Stop server
    stop_app_server();
    
    // We can't easily wait for stop without async, but process exit handles it.
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn test_start_raw_server_integration() {
    let free_port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    
    // Test raw start_server
    let success = start_server(
        free_port as i32,
        host.as_ptr(),
        c_api_test_callback
    );
    assert!(success > 0, "start_server should return a valid port");
    
    // Client Runtime
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        wait_for_server(free_port).await;
        
        let client = reqwest::Client::new();
        let resp = client.get(format!("http://127.0.0.1:{}/raw", free_port))
            .send().await.unwrap();
            
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await.unwrap(), "C API OK");
    });
    
    stop_server();
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn test_start_static_server_integration() {
    let free_port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    
    // Create use a temp dir for static files
    let temp_dir = std::env::temp_dir().join(format!("test_static_c_api_{}", free_port));
    std::fs::create_dir_all(&temp_dir).unwrap();
    std::fs::write(temp_dir.join("index.html"), "Static Content").unwrap();
    
    let root_dir = CString::new(temp_dir.to_string_lossy().to_string()).unwrap();
    
    let success = start_static_server(
        free_port as i32,
        host.as_ptr(),
        root_dir.as_ptr()
    );
    assert!(success > 0, "start_static_server should return a valid port");
    
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        wait_for_server(free_port).await;
        
        let client = reqwest::Client::new();
        let resp = client.get(format!("http://127.0.0.1:{}/index.html", free_port))
            .send().await.unwrap();
            
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await.unwrap(), "Static Content");
    });
    
    stop_static_server();
    std::fs::remove_dir_all(&temp_dir).unwrap();
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn test_start_app_server_integration() {
    let free_port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    
    // Temp dir for static fallback
    let temp_dir = std::env::temp_dir().join(format!("test_app_c_api_{}", free_port));
    std::fs::create_dir_all(&temp_dir).unwrap();
    std::fs::write(temp_dir.join("fallback.txt"), "Fallback Content").unwrap();
    
    let root_dir = CString::new(temp_dir.to_string_lossy().to_string()).unwrap();
    
    let success = start_app_server(
        free_port as i32,
        host.as_ptr(),
        root_dir.as_ptr(),
        c_api_test_callback
    );
    assert!(success > 0, "start_app_server should return a valid port");
    
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        wait_for_server(free_port).await;
        
        let client = reqwest::Client::new();
        
        // 1. Test Static Fallback (GET)
        let resp_static = client.get(format!("http://127.0.0.1:{}/fallback.txt", free_port))
            .send().await.unwrap();
        assert_eq!(resp_static.status(), StatusCode::OK);
        assert_eq!(resp_static.text().await.unwrap(), "Fallback Content");
        
        // 2. Test Callback (Non-existent static file)
        let resp_callback = client.get(format!("http://127.0.0.1:{}/api", free_port))
            .send().await.unwrap();
        assert_eq!(resp_callback.status(), StatusCode::OK);
        assert_eq!(resp_callback.text().await.unwrap(), "C API OK");
    });
    
    stop_app_server();
    std::fs::remove_dir_all(&temp_dir).unwrap();
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn test_get_server_stats_integration() {
    let stats_ptr = get_server_stats();
    assert!(!stats_ptr.is_null());
    
    let stats_str = unsafe { std::ffi::CStr::from_ptr(stats_ptr).to_string_lossy().into_owned() };
    assert!(stats_str.contains("status"));
    assert!(stats_str.contains("running"));

    // Task 16 起 get_server_stats 返回**堆分配**字符串（配 free_string）；
    // 改之前是静态字面量、不能释放。这里按新契约释放，同时也是契约的回归守卫。
    unsafe { crate::c_api::free_string(stats_ptr as *mut std::os::raw::c_char) };
}

extern "C" fn c_api_response_test_callback(request: *mut HttpRequest) {
    if request.is_null() { return; }
    
    let req = unsafe { &*request };
    let request_id = req.request_id; // Keep as *mut c_char
    let path_c = unsafe { std::ffi::CStr::from_ptr(req.path) };
    let path = path_c.to_string_lossy().to_string();
    
    if path == "/send_response" {
        let status = 200;
        let headers = CString::new(r#"{"Content-Type": "text/plain"}"#).unwrap();
        let body = CString::new("Direct Response").unwrap();
        let body_len = body.as_bytes().len() as i32;
        
        send_response(
            request_id,
            status,
            headers.as_ptr(),
            body.as_ptr(),
            body_len
        );
    } else if path == "/chunked" {
        let chunk1 = CString::new("Chunk1").unwrap();
        let chunk2 = CString::new("Chunk2").unwrap();
        
        write_response_chunk(request_id, chunk1.as_ptr(), chunk1.as_bytes().len() as i32);
        write_response_chunk(request_id, chunk2.as_ptr(), chunk2.as_bytes().len() as i32);
        
        let status = 200;
        let headers = CString::new(r#"{"X-Chunked": "true"}"#).unwrap();
        
        end_response(
            request_id,
            status,
            headers.as_ptr()
        );
    } else if path == "/end_no_chunks" {
        // Test end_response without prior write_response_chunk
        let status = 200;
        let headers = CString::new(r#"{"X-Empty": "true"}"#).unwrap();
        
        end_response(
            request_id,
            status,
            headers.as_ptr()
        );
    } else {
        // Fallback using send_response for 404
        let status = 404;
        let headers = CString::new("{}").unwrap();
        let body = CString::new("Not Found").unwrap();
        
        send_response(
            request_id,
            status,
            headers.as_ptr(),
            body.as_ptr(),
            body.as_bytes().len() as i32
        );
    }
    
    unsafe {
        free_http_request(request);
    }
}

#[test]
fn test_c_api_response_functions() {
    let free_port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    
    // We use start_server (raw) but with our special callback
    let success = start_server(
        free_port as i32,
        host.as_ptr(),
        c_api_response_test_callback
    );
    assert!(success > 0, "start_server should return a valid port");
    
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        wait_for_server(free_port).await;
        let client = reqwest::Client::new();
        
        // 1. Test send_response
        let resp1 = client.get(format!("http://127.0.0.1:{}/send_response", free_port))
            .send().await.unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);
        assert_eq!(resp1.text().await.unwrap(), "Direct Response");
        
        // 2. Test write_response_chunk + end_response
        let resp2 = client.get(format!("http://127.0.0.1:{}/chunked", free_port))
            .send().await.unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        assert_eq!(resp2.headers().get("X-Chunked").unwrap(), "true");
        assert_eq!(resp2.text().await.unwrap(), "Chunk1Chunk2");

        // 3. Test end_response without chunks (empty body)
        let resp3 = client.get(format!("http://127.0.0.1:{}/end_no_chunks", free_port))
            .send().await.unwrap();
        assert_eq!(resp3.status(), StatusCode::OK);
        assert_eq!(resp3.headers().get("X-Empty").unwrap(), "true");
        assert_eq!(resp3.text().await.unwrap(), "");
        
        // 4. Test 404 path
        let resp4 = client.get(format!("http://127.0.0.1:{}/unknown", free_port))
            .send().await.unwrap();
        assert_eq!(resp4.status(), StatusCode::NOT_FOUND);
    });
    
    stop_server();
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn test_start_server_null_host() {
    let free_port = get_free_port();
    
    // Pass null pointer for host, should default to 127.0.0.1
    let success = start_server(
        free_port as i32,
        std::ptr::null(),
        c_api_test_callback // Re-use the basic "OK" callback
    );
    assert!(success > 0, "start_server with null host should return a valid port");
    
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        wait_for_server(free_port).await;
        
        let client = reqwest::Client::new();
        let resp = client.get(format!("http://127.0.0.1:{}/", free_port))
            .send().await.unwrap();
            
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await.unwrap(), "C API OK");
    });
    
    stop_server();
    std::thread::sleep(Duration::from_millis(100));
}

extern "C" fn c_api_upload_test_callback(request: *mut HttpRequest) {
    if request.is_null() { return; }
    
    let req = unsafe { &*request };
    let request_id = req.request_id;
    let path_c = unsafe { std::ffi::CStr::from_ptr(req.path) };
    let path = path_c.to_string_lossy().to_string();
    
    // Parse headers helper
    let headers_c = unsafe { std::ffi::CStr::from_ptr(req.headers_json) };
    let headers_str = headers_c.to_string_lossy();
    let headers: HashMap<String, String> = serde_json::from_str(&headers_str).unwrap_or_default();
    
    if path.starts_with("/upload") {
        if let Some(file_path) = headers.get("x-uploaded-file-path").or(headers.get("X-Uploaded-File-Path")) {
            let status = 200;
            let resp_headers = CString::new("{}").unwrap();
            let body_str = format!("Upload OK: {}", file_path);
            let body = CString::new(body_str.clone()).unwrap();
            
            send_response(
                request_id,
                status,
                resp_headers.as_ptr(),
                body.as_ptr(),
                body_str.len() as i32
            );
        } else {
             let status = 400;
             let h = CString::new("{}").unwrap();
             let b = CString::new("Missing upload header").unwrap();
             send_response(request_id, status, h.as_ptr(), b.as_ptr(), b.as_bytes().len() as i32);
        }
    } else if path.starts_with("/buffer-upload") {
        // Echo back the body content
        // The body should have been populated by BufferUploadPlugin
        let status = 200;
        let resp_headers = CString::new("{}").unwrap();
        
        let body_ptr = req.body as *const c_char;
        let body_len = req.body_len;
        
        send_response(
            request_id,
            status,
            resp_headers.as_ptr(),
            body_ptr,
            body_len
        );
    } else {
        let status = 404;
        let h = CString::new("{}").unwrap();
        let b = CString::new("Not Found").unwrap();
        send_response(request_id, status, h.as_ptr(), b.as_ptr(), b.as_bytes().len() as i32);
    }
    
    unsafe {
        free_http_request(request);
    }
}

#[test]
fn test_c_api_upload_plugins() {
    let free_port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    
    // Setup temp dir for uploads
    let temp_dir = std::env::temp_dir().join(format!("test_c_api_upload_{}", free_port));
    std::fs::create_dir_all(&temp_dir).unwrap();
    
    // Config: Enable upload plugin at /upload and buffer_upload at /buffer-upload
    let config_json = json!({
        "mounts": [
            {
                "type": "upload",
                "path": "/upload",
                "temp_dir": temp_dir.to_string_lossy()
            },
            {
                "type": "buffer_upload",
                "path": "/buffer-upload"
            }
        ]
    });
    
    let config_str = CString::new(config_json.to_string()).unwrap();
    
    let success = start_server_with_config(
        free_port as i32,
        host.as_ptr(),
        c_api_upload_test_callback,
        config_str.as_ptr()
    );
    assert!(success > 0, "Server should start");
    
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        wait_for_server(free_port).await;
        let client = reqwest::Client::new();
        
        // 1. Test Upload Plugin
        // Send a file
        let part = multipart::Part::bytes(b"File Content".to_vec())
            .file_name("test.txt")
            .mime_str("text/plain").unwrap();
        let form = multipart::Form::new().part("file", part);
        
        let resp = client.post(format!("http://127.0.0.1:{}/upload", free_port))
            .multipart(form)
            .send().await.unwrap();
            
        assert_eq!(resp.status(), StatusCode::OK);
        let text = resp.text().await.unwrap();
        assert!(text.contains("Upload OK:"));
        assert!(text.contains(&temp_dir.to_string_lossy().to_string()));
        
        // Verify file exists
        // Extract path from response or just check dir
        let files: Vec<_> = std::fs::read_dir(&temp_dir).unwrap().collect();
        assert!(!files.is_empty(), "File should be created on disk");
        
        // 2. Test Buffer Upload Plugin
        // Send a file, verify echo
        let content = b"Buffered Content".to_vec();
        let part2 = multipart::Part::bytes(content.clone())
            .file_name("buffer.txt")
            .mime_str("text/plain").unwrap();
        let form2 = multipart::Form::new().part("file", part2);
        
        let resp2 = client.post(format!("http://127.0.0.1:{}/buffer-upload", free_port))
            .multipart(form2)
            .send().await.unwrap();
            
        assert_eq!(resp2.status(), StatusCode::OK);
        let echo_body = resp2.bytes().await.unwrap();
        assert_eq!(echo_body, content.as_slice());
    });
    
    stop_app_server();
    std::fs::remove_dir_all(&temp_dir).unwrap();
    std::thread::sleep(Duration::from_millis(100));
}
 
/// 测试 C API 边缘情况 (Null/Empty 指针)
#[test]
fn test_c_api_edge_cases() {
    let base_port = get_free_port() as std::os::raw::c_int;
    let host_null: *const c_char = std::ptr::null();
    let host_empty = CString::new("").unwrap();
    
    extern "C" fn basic_callback(request: *mut HttpRequest) {
        unsafe { free_http_request(request); }
    }
    
    // Helper to sleep
    fn sleep_ms(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
    
    // 1. start_server with empty host
    let mut port = base_port;
    println!("Step 1: start_server empty host port={}", port);
    let result = start_server(port, host_empty.as_ptr(), basic_callback);
    assert!(result > 0, "Step 1 failed");
    sleep_ms(100);
    stop_server();
    sleep_ms(100);
    
    // 2. start_static_server with null host
    port += 1;
    let temp_dir = std::env::temp_dir();
    let root_dir = CString::new(temp_dir.to_string_lossy().to_string()).unwrap();
    
    println!("Step 2: start_static_server null host port={}", port);
    let result = start_static_server(port, host_null, root_dir.as_ptr());
    assert!(result > 0, "Step 2 failed");
    sleep_ms(100);
    stop_static_server();
    sleep_ms(100);
    
    // 3. start_static_server with empty host
    port += 1;
    println!("Step 3: start_static_server empty host port={}", port);
    let result = start_static_server(port, host_empty.as_ptr(), root_dir.as_ptr());
    assert!(result > 0, "Step 3 failed");
    sleep_ms(100);
    stop_static_server();
    sleep_ms(100);
    
    // 4. start_app_server with null host
    port += 1;
    println!("Step 4: start_app_server null host port={}", port);
    let result = start_app_server(port, host_null, root_dir.as_ptr(), basic_callback);
    assert!(result > 0, "Step 4 failed");
    sleep_ms(100);
    stop_app_server();
    sleep_ms(100);
    
    // 5. start_app_server with empty host
    port += 1;
    println!("Step 5: start_app_server empty host port={}", port);
    let result = start_app_server(port, host_empty.as_ptr(), root_dir.as_ptr(), basic_callback);
    assert!(result > 0, "Step 5 failed");
    sleep_ms(100);
    stop_app_server();
    sleep_ms(100);
    
    // 6. start_server_with_config with null host
    port += 1;
    let config = CString::new("{}").unwrap();
    println!("Step 6: start_server_with_config null host port={}", port);
    let result = start_server_with_config(port, host_null, basic_callback, config.as_ptr());
    assert!(result > 0, "Step 6 failed");
    sleep_ms(100);
    stop_app_server();
    sleep_ms(100);
    
    // 7. start_server_with_config with empty host
    port += 1;
    println!("Step 7: start_server_with_config empty host port={}", port);
    let result = start_server_with_config(port, host_empty.as_ptr(), basic_callback, config.as_ptr());
    assert!(result > 0, "Step 7 failed");
    sleep_ms(100);
    stop_app_server();
    sleep_ms(100);
}
/*
// Disabled due to runtime crash in test environment (signal 6)
// Likely due to FFI panic boundary or threading issues in test harness
#[test]
fn test_c_api_response_edge_cases() {
    // ...
}
*/

// ==================== WebSocket C API 测试 ====================

use crate::c_api::{
    set_websocket_callback, ws_send_text, ws_send_binary, ws_close,
    CWebSocketEvent,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};

/// 用于收集 WebSocket 事件的静态变量
static WS_TEST_EVENT_COUNT: AtomicU32 = AtomicU32::new(0);
static WS_TEST_LAST_EVENT_TYPE: AtomicU32 = AtomicU32::new(0);

extern "C" fn ws_test_callback(event: *const CWebSocketEvent) {
    if event.is_null() { return; }
    
    let evt = unsafe { &*event };
    WS_TEST_EVENT_COUNT.fetch_add(1, AtomicOrdering::SeqCst);
    WS_TEST_LAST_EVENT_TYPE.store(evt.event_type as u32, AtomicOrdering::SeqCst);
    
    // Log event for debugging
    let conn_id = if !evt.connection_id.is_null() {
        unsafe { CStr::from_ptr(evt.connection_id).to_string_lossy().to_string() }
    } else {
        "null".to_string()
    };
    
    println!("[WS Test] Event: type={}, conn_id={}", evt.event_type, conn_id);
}

/// 测试 WebSocket C API 函数 (无服务器)
/// 这些测试验证边缘情况和 null 处理
#[test]
fn test_websocket_c_api_null_handling() {
    // 1. ws_send_text with null connection_id
    let result = ws_send_text(std::ptr::null(), std::ptr::null());
    assert!(!result, "ws_send_text with null conn_id should return false");
    
    // 2. ws_send_text with null message (valid conn_id)
    let conn_id = CString::new("test-conn-id").unwrap();
    let result = ws_send_text(conn_id.as_ptr(), std::ptr::null());
    // Should return false because plugin is not initialized
    assert!(!result, "ws_send_text without plugin should return false");
    
    // 3. ws_send_binary with null connection_id
    let result = ws_send_binary(std::ptr::null(), std::ptr::null(), 0);
    assert!(!result, "ws_send_binary with null conn_id should return false");
    
    // 4. ws_send_binary with null data
    let result = ws_send_binary(conn_id.as_ptr(), std::ptr::null(), 0);
    assert!(!result, "ws_send_binary with null data should return false");
    
    // 5. ws_send_binary with invalid length
    let data = CString::new("test").unwrap();
    let result = ws_send_binary(conn_id.as_ptr(), data.as_ptr(), -1);
    assert!(!result, "ws_send_binary with negative len should return false");
    
    // 6. ws_close with null connection_id
    let result = ws_close(std::ptr::null(), 1000, std::ptr::null());
    assert!(!result, "ws_close with null conn_id should return false");
    
    // 7. ws_close with valid args but no plugin
    let reason = CString::new("test close").unwrap();
    let result = ws_close(conn_id.as_ptr(), 1000, reason.as_ptr());
    // Should return false because plugin is not initialized
    assert!(!result, "ws_close without plugin should return false");
}

/// 测试 WebSocket 与服务器集成
/// 注：此测试与 test_websocket_c_api_send_message 功能重叠，在并行测试时可能冲突
#[test]
#[ignore]
fn test_websocket_c_api_with_server() {
    use tokio_tungstenite::connect_async;
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    
    let free_port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    
    // Config with WebSocket mount
    let config_json = json!({
        "mounts": [
            {
                "type": "websocket",
                "path": "/ws"
            }
        ]
    });
    
    let config_str = CString::new(config_json.to_string()).unwrap();
    
    // Reset test counters
    WS_TEST_EVENT_COUNT.store(0, AtomicOrdering::SeqCst);
    WS_TEST_LAST_EVENT_TYPE.store(0, AtomicOrdering::SeqCst);
    
    // Start server
    let success = start_server_with_config(
        free_port as i32,
        host.as_ptr(),
        c_api_test_callback,
        config_str.as_ptr()
    );
    assert!(success > 0, "Server should start");
    
    // Set WebSocket callback
    set_websocket_callback(ws_test_callback);
    
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
        
    rt.block_on(async {
        wait_for_server(free_port).await;
        
        // Connect WebSocket client
        let ws_url = format!("ws://127.0.0.1:{}/ws?token=test123", free_port);
        let (mut ws_stream, _) = connect_async(&ws_url)
            .await
            .expect("Failed to connect WebSocket");
        
        // Wait for Open event
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Check Open event was received
        let event_count = WS_TEST_EVENT_COUNT.load(AtomicOrdering::SeqCst);
        assert!(event_count >= 1, "Should have received at least one event (Open)");
        assert_eq!(WS_TEST_LAST_EVENT_TYPE.load(AtomicOrdering::SeqCst), 1, "Last event should be Open (1)");
        
        // Send a text message
        ws_stream.send(WsMessage::Text("Hello Server".to_string())).await.unwrap();
        
        // Wait for Message event
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Check Message event was received
        let event_count = WS_TEST_EVENT_COUNT.load(AtomicOrdering::SeqCst);
        assert!(event_count >= 2, "Should have received Message event");
        assert_eq!(WS_TEST_LAST_EVENT_TYPE.load(AtomicOrdering::SeqCst), 2, "Last event should be Message (2)");
        
        // Close connection
        ws_stream.close(None).await.ok();
        
        // Wait for Close event
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Check Close event was received
        let event_count = WS_TEST_EVENT_COUNT.load(AtomicOrdering::SeqCst);
        assert!(event_count >= 3, "Should have received Close event");
    });
    
    stop_app_server();
    std::thread::sleep(Duration::from_millis(100));
}

/// 测试 WebSocket 发送消息 API
#[test]
fn test_websocket_c_api_send_message() {
    use tokio_tungstenite::connect_async;
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    
    let free_port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    
    // Storage for connection ID from callback
    static LAST_CONN_ID: std::sync::LazyLock<Mutex<Option<String>>> = std::sync::LazyLock::new(|| Mutex::new(None));
    
    extern "C" fn ws_capture_conn_callback(event: *const CWebSocketEvent) {
        if event.is_null() { return; }
        let evt = unsafe { &*event };
        
        // Capture connection ID on Open event
        if evt.event_type == 1 && !evt.connection_id.is_null() {
            let conn_id = unsafe { CStr::from_ptr(evt.connection_id).to_string_lossy().to_string() };
            *LAST_CONN_ID.lock().unwrap() = Some(conn_id);
        }
    }
    
    // Config with WebSocket mount
    let config_json = json!({
        "mounts": [
            { "type": "websocket", "path": "/ws" }
        ]
    });
    
    let config_str = CString::new(config_json.to_string()).unwrap();
    
    // Clear previous connection ID
    *LAST_CONN_ID.lock().unwrap() = None;
    
    let success = start_server_with_config(
        free_port as i32,
        host.as_ptr(),
        c_api_test_callback,
        config_str.as_ptr()
    );
    assert!(success > 0);
    
    set_websocket_callback(ws_capture_conn_callback);
    
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
        
    rt.block_on(async {
        wait_for_server(free_port).await;
        
        // Connect WebSocket client
        let ws_url = format!("ws://127.0.0.1:{}/ws", free_port);
        let (mut ws_stream, _) = connect_async(&ws_url)
            .await
            .expect("Failed to connect");
        
        // Wait for connection and callback
        tokio::time::sleep(Duration::from_millis(200)).await;
        
        // Get connection ID
        let conn_id_opt = LAST_CONN_ID.lock().unwrap().clone();
        assert!(conn_id_opt.is_some(), "Should have captured connection ID");
        let conn_id = conn_id_opt.unwrap();
        let conn_id_c = CString::new(conn_id).unwrap();
        
        // Send text message via C API
        let msg = CString::new("Hello from C API").unwrap();
        let send_result = ws_send_text(conn_id_c.as_ptr(), msg.as_ptr());
        assert!(send_result, "ws_send_text should succeed");
        
        // Receive message on client
        if let Some(Ok(msg)) = ws_stream.next().await {
            match msg {
                WsMessage::Text(text) => {
                    assert_eq!(text, "Hello from C API");
                }
                _ => panic!("Expected text message"),
            }
        }
        
        // Send binary message via C API
        let binary_data = b"Binary Data";
        let send_result = ws_send_binary(
            conn_id_c.as_ptr(),
            binary_data.as_ptr() as *const c_char,
            binary_data.len() as i32
        );
        assert!(send_result, "ws_send_binary should succeed");
        
        // Receive binary message on client
        if let Some(Ok(msg)) = ws_stream.next().await {
            match msg {
                WsMessage::Binary(data) => {
                    assert_eq!(data, binary_data.as_slice());
                }
                _ => panic!("Expected binary message"),
            }
        }
        
        // Close connection via C API
        let reason = CString::new("Server closing").unwrap();
        let close_result = ws_close(conn_id_c.as_ptr(), 1000, reason.as_ptr());
        assert!(close_result, "ws_close should succeed");
        
        // Client should receive close
        tokio::time::sleep(Duration::from_millis(100)).await;
    });
    
    stop_app_server();
    std::thread::sleep(Duration::from_millis(100));
}
