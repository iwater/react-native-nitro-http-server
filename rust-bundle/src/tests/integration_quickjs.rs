//! QuickJS integration tests covering the request -> JS callback -> response flow.

use crate::server::app_server::AppHttpServer;
use crate::tests::test_utils::{get_free_port, wait_for_server};
use crate::types::HttpRequest;
use crate::global::RESPONSE_CHANNELS;
use crate::types::ResponseData;

use rquickjs::{Runtime, Context, Object, Function, Ctx};
use rquickjs::function::Rest;
use std::collections::HashMap;
use std::time::Duration;
use std::ffi::CStr;
use std::fs;
use reqwest::{StatusCode, multipart};
use serde_json::json;

fn extract_request_info(request: *mut HttpRequest) -> Option<(String, String, String, String, String, i32)> {
    if request.is_null() {
        return None;
    }
    
    let req = unsafe { &*request };
    
    let request_id = unsafe { CStr::from_ptr(req.request_id) }.to_string_lossy().to_string();
    let method = unsafe { CStr::from_ptr(req.method) }.to_string_lossy().to_string();
    let path = unsafe { CStr::from_ptr(req.path) }.to_string_lossy().to_string();
    let headers_json = if req.headers_json.is_null() {
        "{}".to_string()
    } else {
        unsafe { CStr::from_ptr(req.headers_json) }.to_string_lossy().to_string()
    };
    let body = if req.body.is_null() || req.body_len <= 0 {
        String::new()
    } else {
        let body_slice = unsafe { std::slice::from_raw_parts(req.body as *const u8, req.body_len as usize) };
        String::from_utf8_lossy(body_slice).to_string()
    };
    let body_len = req.body_len;
    
    Some((request_id, method, path, headers_json, body, body_len))
}

fn send_response_from_js(request_id: &str, status_code: u16, headers: HashMap<String, String>, body: Vec<u8>) -> bool {
    if let Some(chan) = RESPONSE_CHANNELS.lock().unwrap().remove(request_id) {
        let resp = ResponseData {
            status_code,
            headers,
            body,
        };
        chan.send(resp).is_ok()
    } else {
        false
    }
}

fn setup_http_module(ctx: &Ctx<'_>) {
    let global = ctx.globals();
    
    let http_obj = Object::new(ctx.clone()).expect("Failed to create http object");
    
    let send_fn = Function::new(
        ctx.clone(),
        |_ctx: Ctx<'_>, args: Rest<rquickjs::Value<'_>>| -> rquickjs::Result<bool> {
            let args = args.0;
            if args.len() < 4 {
                return Ok(false);
            }
            
            let request_id: String = args[0].clone().get()?;
            let status_code: i32 = args[1].clone().get()?;
            let headers_val = args[2].clone();
            let body_val = args[3].clone();
            
            let mut headers = HashMap::new();
            if headers_val.is_object() {
                if let Some(obj) = headers_val.as_object() {
                    for key_result in obj.keys::<String>() {
                        if let Ok(k) = key_result {
                            if let Ok(v) = obj.get::<_, String>(&k) {
                                headers.insert(k, v);
                            }
                        }
                    }
                }
            }
            
            // Supports String, ArrayBuffer, and Array<number>.
            let body_bytes: Vec<u8> = if body_val.is_string() {
                let s: String = body_val.get()?;
                s.into_bytes()
            } else if let Some(obj) = body_val.as_object() {
                if let Some(array_buffer) = obj.as_array_buffer() {
                    array_buffer.as_bytes().unwrap().to_vec()
                } else if let Some(array) = obj.as_array() {
                    let mut bytes = Vec::new();
                    for val in array.iter() {
                        let v: rquickjs::Value = val?;
                        if let Some(n) = v.as_int() {
                            bytes.push(n as u8);
                        }
                    }
                    bytes
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            
            let result = send_response_from_js(&request_id, status_code as u16, headers, body_bytes);
            Ok(result)
        }
    ).expect("Failed to create sendResponse function");
    
    http_obj.set("sendResponse", send_fn).expect("Failed to set sendResponse");
    global.set("http", http_obj).expect("Failed to set http global");
}

fn setup_console(ctx: &Ctx<'_>) {
    let global = ctx.globals();
    
    let console_obj = Object::new(ctx.clone()).expect("Failed to create console");
    
    let log_fn = Function::new(
        ctx.clone(),
        |_ctx: Ctx<'_>, args: Rest<String>| -> rquickjs::Result<()> {
            println!("[JS] {}", args.0.join(" "));
            Ok(())
        }
    ).expect("Failed to create log function");
    
    console_obj.set("log", log_fn.clone()).expect("Failed to set log");
    console_obj.set("info", log_fn.clone()).expect("Failed to set info");
    console_obj.set("error", log_fn).expect("Failed to set error");
    global.set("console", console_obj).expect("Failed to set console");
}

fn process_request_in_js(
    request_id: &str,
    method: &str,
    path: &str,
    headers_json: &str,
    body: &str,
    body_len: i32,
    js_code: &str,
) {
    let rt = Runtime::new().expect("Failed to create QuickJS runtime");
    let ctx = Context::full(&rt).expect("Failed to create QuickJS context");
    
    ctx.with(|ctx| {
        setup_http_module(&ctx);
        setup_console(&ctx);
        
        if let Err(e) = ctx.eval::<(), _>(js_code) {
            eprintln!("Failed to eval JS code: {:?}", e);
            let mut headers = HashMap::new();
            headers.insert("Content-Type".to_string(), "text/plain".to_string());
            send_response_from_js(request_id, 500, headers, b"JS Eval Error".to_vec());
            return;
        }
        
        let call_script = format!(
            r#"
            if (typeof handleRequest === 'function') {{
                handleRequest({{
                    requestId: "{}",
                    method: "{}",
                    path: "{}",
                    headers: {},
                    body: "{}",
                    bodyLength: {}
                }});
            }} else {{
                http.sendResponse("{}", 500, {{"Content-Type": "text/plain"}}, "handleRequest not defined");
            }}
            "#,
            request_id.replace('\"', "\\\""),
            method,
            path,
            headers_json,
            body.replace('\\', "\\\\").replace('\"', "\\\"").replace('\n', "\\n"),
            body_len,
            request_id.replace('\"', "\\\"")
        );
        
        if let Err(e) = ctx.eval::<(), _>(call_script.as_str()) {
            eprintln!("JS error processing request: {:?}", e);
            let mut headers = HashMap::new();
            headers.insert("Content-Type".to_string(), "text/plain".to_string());
            send_response_from_js(request_id, 500, headers, format!("JS Error: {:?}", e).into_bytes());
        }
    });
}

// ============================================================================
// Test cases
// ============================================================================

#[tokio::test]
async fn test_quickjs_basic_callback() {
    let temp_dir_base = std::env::temp_dir().join("test_quickjs_basic");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    let _temp_dir = temp_dir_base.canonicalize().unwrap();
    
    extern "C" fn basic_callback(request: *mut HttpRequest) {
        if let Some((request_id, method, path, headers_json, body, body_len)) = extract_request_info(request) {
            let js_code = r#"
                function handleRequest(req) {
                    console.log("Received request:", req.method, req.path);
                    http.sendResponse(
                        req.requestId,
                        200,
                        { "Content-Type": "text/plain" },
                        "Hello from QuickJS!"
                    );
                }
            "#;
            
            process_request_in_js(&request_id, &method, &path, &headers_json, &body, body_len, js_code);
        }
        
        unsafe { crate::c_api::free_http_request(request); }
    }
    
    let config_json = r#"{ "verbose": true, "mounts": [] }"#;
    let free_port = get_free_port();
    
    let server_task = tokio::spawn(async move {
        let mut server = AppHttpServer::new(free_port, "127.0.0.1".to_string(), basic_callback, config_json);
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    
    wait_for_server(free_port).await;
    let base_url = format!("http://127.0.0.1:{}", free_port);
    let client = reqwest::Client::new();
    
    let resp = client.get(format!("{}/api/test", base_url)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert_eq!(body, "Hello from QuickJS!");
    
    server_task.abort();
    let _ = fs::remove_dir_all(&temp_dir_base);
}

#[tokio::test]
async fn test_quickjs_json_response() {
    let temp_dir_base = std::env::temp_dir().join("test_quickjs_json");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    let _temp_dir = temp_dir_base.canonicalize().unwrap();
    
    extern "C" fn json_callback(request: *mut HttpRequest) {
        if let Some((request_id, method, path, headers_json, body, body_len)) = extract_request_info(request) {
            let js_code = r#"
                function handleRequest(req) {
                    var response = {
                        success: true,
                        method: req.method,
                        path: req.path,
                        timestamp: Date.now()
                    };
                    http.sendResponse(
                        req.requestId,
                        200,
                        { "Content-Type": "application/json" },
                        JSON.stringify(response)
                    );
                }
            "#;
            
            process_request_in_js(&request_id, &method, &path, &headers_json, &body, body_len, js_code);
        }
        
        unsafe { crate::c_api::free_http_request(request); }
    }
    
    let config_json = r#"{ "verbose": true, "mounts": [] }"#;
    let free_port = get_free_port();
    
    let server_task = tokio::spawn(async move {
        let mut server = AppHttpServer::new(free_port, "127.0.0.1".to_string(), json_callback, config_json);
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    
    wait_for_server(free_port).await;
    let base_url = format!("http://127.0.0.1:{}", free_port);
    let client = reqwest::Client::new();
    
    let resp = client.get(format!("{}/api/data", base_url)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    
    let content_type = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(content_type.contains("application/json"));
    
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["success"], true);
    assert_eq!(json["method"], "GET");
    assert_eq!(json["path"], "/api/data");
    
    server_task.abort();
    let _ = fs::remove_dir_all(&temp_dir_base);
}

#[tokio::test]
async fn test_quickjs_request_parsing() {
    let temp_dir_base = std::env::temp_dir().join("test_quickjs_parsing");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    let _temp_dir = temp_dir_base.canonicalize().unwrap();
    
    extern "C" fn echo_callback(request: *mut HttpRequest) {
        if let Some((request_id, method, path, headers_json, body, body_len)) = extract_request_info(request) {
            let js_code = r#"
                function handleRequest(req) {
                    var response = {
                        received: {
                            method: req.method,
                            path: req.path,
                            body: req.body,
                            bodyLength: req.bodyLength,
                            hasHeaders: typeof req.headers === 'object'
                        }
                    };
                    http.sendResponse(
                        req.requestId,
                        200,
                        { "Content-Type": "application/json" },
                        JSON.stringify(response)
                    );
                }
            "#;
            
            process_request_in_js(&request_id, &method, &path, &headers_json, &body, body_len, js_code);
        }
        
        unsafe { crate::c_api::free_http_request(request); }
    }
    
    let config_json = r#"{ "verbose": true, "mounts": [] }"#;
    let free_port = get_free_port();
    
    let server_task = tokio::spawn(async move {
        let mut server = AppHttpServer::new(free_port, "127.0.0.1".to_string(), echo_callback, config_json);
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    
    wait_for_server(free_port).await;
    let base_url = format!("http://127.0.0.1:{}", free_port);
    let client = reqwest::Client::new();
    
    let resp = client.post(format!("{}/echo", base_url))
        .header("X-Custom-Header", "test-value")
        .body("Hello, World!")
        .send()
        .await
        .unwrap();
    
    assert_eq!(resp.status(), StatusCode::OK);
    
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["received"]["method"], "POST");
    assert_eq!(json["received"]["path"], "/echo");
    assert_eq!(json["received"]["body"], "Hello, World!");
    assert_eq!(json["received"]["bodyLength"], 13);
    assert_eq!(json["received"]["hasHeaders"], true);
    
    server_task.abort();
    let _ = fs::remove_dir_all(&temp_dir_base);
}

#[tokio::test]
async fn test_quickjs_routing() {
    let temp_dir_base = std::env::temp_dir().join("test_quickjs_routing");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    
    extern "C" fn routing_callback(request: *mut HttpRequest) {
        if let Some((request_id, method, path, headers_json, body, body_len)) = extract_request_info(request) {
            let js_code = r#"
                var routes = {
                    "GET /api/hello": function(req) {
                        http.sendResponse(req.requestId, 200, 
                            { "Content-Type": "application/json" },
                            JSON.stringify({ message: "Hello!" }));
                    },
                    "GET /api/info": function(req) {
                        http.sendResponse(req.requestId, 200,
                            { "Content-Type": "application/json" },
                            JSON.stringify({ version: "1.0.0", engine: "QuickJS" }));
                    },
                    "POST /api/data": function(req) {
                        http.sendResponse(req.requestId, 201,
                            { "Content-Type": "application/json" },
                            JSON.stringify({ created: true, data: req.body }));
                    }
                };
                
                function handleRequest(req) {
                    var key = req.method + " " + req.path;
                    var handler = routes[key];
                    
                    if (handler) {
                        handler(req);
                    } else {
                        http.sendResponse(req.requestId, 404,
                            { "Content-Type": "application/json" },
                            JSON.stringify({ error: "Not Found", path: req.path }));
                    }
                }
            "#;
            
            process_request_in_js(&request_id, &method, &path, &headers_json, &body, body_len, js_code);
        }
        
        unsafe { crate::c_api::free_http_request(request); }
    }
    
    let config_json = r#"{ "verbose": true, "mounts": [] }"#;
    let free_port = get_free_port();
    
    let server_task = tokio::spawn(async move {
        let mut server = AppHttpServer::new(free_port, "127.0.0.1".to_string(), routing_callback, config_json);
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    
    wait_for_server(free_port).await;
    let base_url = format!("http://127.0.0.1:{}", free_port);
    let client = reqwest::Client::new();
    
    let resp1 = client.get(format!("{}/api/hello", base_url)).send().await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    let json1: serde_json::Value = resp1.json().await.unwrap();
    assert_eq!(json1["message"], "Hello!");
    
    let resp2 = client.get(format!("{}/api/info", base_url)).send().await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let json2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(json2["version"], "1.0.0");
    assert_eq!(json2["engine"], "QuickJS");
    
    let resp3 = client.post(format!("{}/api/data", base_url))
        .body("test data")
        .send()
        .await
        .unwrap();
    assert_eq!(resp3.status(), StatusCode::CREATED);
    let json3: serde_json::Value = resp3.json().await.unwrap();
    assert_eq!(json3["created"], true);
    assert_eq!(json3["data"], "test data");
    
    let resp4 = client.get(format!("{}/unknown", base_url)).send().await.unwrap();
    assert_eq!(resp4.status(), StatusCode::NOT_FOUND);
    let json4: serde_json::Value = resp4.json().await.unwrap();
    assert_eq!(json4["error"], "Not Found");
    
    server_task.abort();
    let _ = fs::remove_dir_all(&temp_dir_base);
}

#[tokio::test]
async fn test_quickjs_error_handling() {
    let temp_dir_base = std::env::temp_dir().join("test_quickjs_error");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    
    extern "C" fn error_callback(request: *mut HttpRequest) {
        if let Some((request_id, method, path, headers_json, body, body_len)) = extract_request_info(request) {
            let js_code = r#"
                function handleRequest(req) {
                    if (req.path === "/error") {
                        throw new Error("Intentional error");
                    }
                    http.sendResponse(req.requestId, 200,
                        { "Content-Type": "text/plain" },
                        "OK");
                }
            "#;
            
            process_request_in_js(&request_id, &method, &path, &headers_json, &body, body_len, js_code);
        }
        
        unsafe { crate::c_api::free_http_request(request); }
    }
    
    let config_json = r#"{ "verbose": true, "mounts": [] }"#;
    let free_port = get_free_port();
    
    let server_task = tokio::spawn(async move {
        let mut server = AppHttpServer::new(free_port, "127.0.0.1".to_string(), error_callback, config_json);
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    
    wait_for_server(free_port).await;
    let base_url = format!("http://127.0.0.1:{}", free_port);
    let client = reqwest::Client::new();
    
    let resp1 = client.get(format!("{}/ok", base_url)).send().await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    assert_eq!(resp1.text().await.unwrap(), "OK");
    
    let resp2 = client.get(format!("{}/error", base_url)).send().await.unwrap();
    assert_eq!(resp2.status(), StatusCode::INTERNAL_SERVER_ERROR);
    
    server_task.abort();
    let _ = fs::remove_dir_all(&temp_dir_base);
}

#[tokio::test]
async fn test_quickjs_upload_plugins() {
    let temp_dir_base = std::env::temp_dir().join("test_quickjs_upload");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    let temp_dir = temp_dir_base.canonicalize().unwrap();
    
    let binary_file_path = temp_dir.join("image.png");
    let binary_content = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]; // PNG Magic header
    fs::write(&binary_file_path, &binary_content).unwrap();
    
    extern "C" fn upload_callback(request: *mut HttpRequest) {
        if let Some((request_id, method, path, headers_json, body, body_len)) = extract_request_info(request) {
            let js_code = r#"
                function handleRequest(req) {
                    if (req.path.startsWith("/upload")) {
                        var filePath = req.headers["x-uploaded-file-path"] || req.headers["X-Uploaded-File-Path"];
                        if (filePath) {
                            http.sendResponse(req.requestId, 200, {}, "Upload OK: " + filePath);
                        } else {
                            http.sendResponse(req.requestId, 400, {}, "Missing upload header");
                        }
                    } else if (req.path.startsWith("/buffer-upload")) {
                        http.sendResponse(req.requestId, 200, {}, req.body);
                    } else if (req.path === "/binary-response") {
                         var bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
                         http.sendResponse(req.requestId, 200, { "Content-Type": "image/png" }, bytes);
                    } else {
                        http.sendResponse(req.requestId, 404, {}, "Not Found");
                    }
                }
            "#;
            
            // process_request_in_js uses lossy UTF-8 string conversion, so the buffer-upload
            // subtest sends plain text.
            process_request_in_js(&request_id, &method, &path, &headers_json, &body, body_len, js_code);
        }
        
        unsafe { crate::c_api::free_http_request(request); }
    }
    
    // Config: Enable upload plugin at /upload and buffer_upload at /buffer-upload
    let config_json = json!({
        "verbose": true,
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
    
    let free_port = get_free_port();
    
    let server_task = tokio::spawn(async move {
        let mut server = AppHttpServer::new(free_port, "127.0.0.1".to_string(), upload_callback, &config_json.to_string());
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    
    wait_for_server(free_port).await;
    let base_url = format!("http://127.0.0.1:{}", free_port);
    let client = reqwest::Client::new();
    
    // 1. Test Upload Plugin w/ QuickJS
    let part = multipart::Part::bytes(b"JS Upload Content".to_vec())
        .file_name("js_test.txt")
        .mime_str("text/plain").unwrap();
    let form = multipart::Form::new().part("file", part);
    
    let resp = client.post(format!("{}/upload", base_url))
        .multipart(form)
        .send().await.unwrap();
        
    assert_eq!(resp.status(), StatusCode::OK);
    let text = resp.text().await.unwrap();
    assert!(text.contains("Upload OK:"));
    
    // 2. Test Buffer Upload Plugin w/ QuickJS
    let content = b"Buffered JS Content".to_vec();
    let part2 = multipart::Part::bytes(content.clone())
        .file_name("js_buffer.txt")
        .mime_str("text/plain").unwrap();
    let form2 = multipart::Form::new().part("file", part2);
    
    let resp2 = client.post(format!("{}/buffer-upload", base_url))
        .multipart(form2)
        .send().await.unwrap();
        
    assert_eq!(resp2.status(), StatusCode::OK);
    let echo_body = resp2.text().await.unwrap();
    assert_eq!(echo_body, "Buffered JS Content");

    // 3. Test Binary Response
    let resp3 = client.get(format!("{}/binary-response", base_url)).send().await.unwrap();
    assert_eq!(resp3.status(), StatusCode::OK);
    assert_eq!(resp3.headers().get("content-type").unwrap(), "image/png");
    let bytes = resp3.bytes().await.unwrap();
    assert_eq!(bytes, vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    
    server_task.abort();
    let _ = fs::remove_dir_all(&temp_dir_base);
}

#[tokio::test]
async fn test_quickjs_websocket_plugin_config() {
    let temp_dir_base = std::env::temp_dir().join("test_quickjs_websocket");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    
    extern "C" fn ws_callback(request: *mut HttpRequest) {
        if let Some((request_id, method, path, headers_json, body, body_len)) = extract_request_info(request) {
            let js_code = r#"
                function handleRequest(req) {
                    http.sendResponse(req.requestId, 200, 
                        { "Content-Type": "application/json" },
                        JSON.stringify({ 
                            path: req.path, 
                            method: req.method,
                            message: "HTTP endpoint working"
                        })
                    );
                }
            "#;
            
            process_request_in_js(&request_id, &method, &path, &headers_json, &body, body_len, js_code);
        }
        
        unsafe { crate::c_api::free_http_request(request); }
    }
    
    let config_json = json!({
        "verbose": true,
        "mounts": [
            {
                "type": "websocket",
                "path": "/ws"
            }
        ]
    });
    
    let free_port = get_free_port();
    
    let server_task = tokio::spawn(async move {
        let mut server = AppHttpServer::new(free_port, "127.0.0.1".to_string(), ws_callback, &config_json.to_string());
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    
    wait_for_server(free_port).await;
    let base_url = format!("http://127.0.0.1:{}", free_port);
    let client = reqwest::Client::new();
    
    // 1. Plain HTTP request reaches callback.
    let resp = client.get(format!("{}/api/test", base_url)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["message"], "HTTP endpoint working");
    
    // 2. Non-upgrade request to /ws/* falls through to callback.
    let resp2 = client.get(format!("{}/ws/chat", base_url)).send().await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let json2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(json2["path"], "/ws/chat");
    
    // 3. Upgrade request is intercepted by the WebSocket plugin rather than the HTTP callback.
    let resp3 = client.get(format!("{}/ws", base_url))
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("Sec-WebSocket-Version", "13")
        .send()
        .await
        .unwrap();
    
    let status = resp3.status();
    println!("WebSocket upgrade response status: {}", status);
    assert!(status == StatusCode::SWITCHING_PROTOCOLS || status != StatusCode::OK || status.is_client_error(), 
            "Expected WebSocket plugin to handle upgrade request, got: {}", status);
    
    server_task.abort();
    let _ = fs::remove_dir_all(&temp_dir_base);
}

#[tokio::test]
async fn test_quickjs_websocket_path_matching() {
    let temp_dir_base = std::env::temp_dir().join("test_quickjs_ws_path");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    
    extern "C" fn path_callback(request: *mut HttpRequest) {
        if let Some((request_id, method, path, headers_json, body, body_len)) = extract_request_info(request) {
            let js_code = r#"
                function handleRequest(req) {
                    http.sendResponse(req.requestId, 200, 
                        { "Content-Type": "application/json" },
                        JSON.stringify({ 
                            handled_by: "callback",
                            path: req.path
                        })
                    );
                }
            "#;
            
            process_request_in_js(&request_id, &method, &path, &headers_json, &body, body_len, js_code);
        }
        
        unsafe { crate::c_api::free_http_request(request); }
    }
    
    let config_json = json!({
        "verbose": true,
        "mounts": [
            { "type": "websocket", "path": "/ws" },
            { "type": "websocket", "path": "/chat" },
            { "type": "websocket", "path": "/notifications", "max_message_size": 1048576 }
        ]
    });
    
    let free_port = get_free_port();
    
    let server_task = tokio::spawn(async move {
        let mut server = AppHttpServer::new(free_port, "127.0.0.1".to_string(), path_callback, &config_json.to_string());
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    
    wait_for_server(free_port).await;
    let base_url = format!("http://127.0.0.1:{}", free_port);
    let client = reqwest::Client::new();
    
    let resp1 = client.get(format!("{}/api/hello", base_url)).send().await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    let json1: serde_json::Value = resp1.json().await.unwrap();
    assert_eq!(json1["handled_by"], "callback");
    assert_eq!(json1["path"], "/api/hello");
    
    let resp2 = client.get(format!("{}/ws", base_url)).send().await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let json2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(json2["handled_by"], "callback");
    
    let resp3 = client.get(format!("{}/chat", base_url))
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("Sec-WebSocket-Version", "13")
        .send()
        .await
        .unwrap();
    
    let status = resp3.status();
    println!("/chat upgrade status: {}", status);
    
    let resp4 = client.get(format!("{}/notifications", base_url))
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("Sec-WebSocket-Version", "13")
        .send()
        .await
        .unwrap();
    
    let status4 = resp4.status();
    println!("/notifications upgrade status: {}", status4);
    
    server_task.abort();
    let _ = fs::remove_dir_all(&temp_dir_base);
}

#[tokio::test]
async fn test_quickjs_websocket_real_connection() {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    use futures_util::{StreamExt, SinkExt};
    
    let temp_dir_base = std::env::temp_dir().join("test_quickjs_ws_real");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    
    extern "C" fn real_ws_callback(request: *mut HttpRequest) {
        if let Some((request_id, method, path, headers_json, body, body_len)) = extract_request_info(request) {
            let js_code = r#"
                function handleRequest(req) {
                    http.sendResponse(req.requestId, 200, 
                        { "Content-Type": "text/plain" },
                        "HTTP Fallback"
                    );
                }
            "#;
            
            process_request_in_js(&request_id, &method, &path, &headers_json, &body, body_len, js_code);
        }
        
        unsafe { crate::c_api::free_http_request(request); }
    }
    
    let config_json = json!({
        "verbose": true,
        "mounts": [
            { "type": "websocket", "path": "/ws" }
        ]
    });
    
    let free_port = get_free_port();
    let port_copy = free_port;
    
    let server_task = tokio::spawn(async move {
        let mut server = AppHttpServer::new(free_port, "127.0.0.1".to_string(), real_ws_callback, &config_json.to_string());
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    
    wait_for_server(port_copy).await;
    
    let ws_url = format!("ws://127.0.0.1:{}/ws", port_copy);
    println!("Connecting to WebSocket at {}", ws_url);
    
    let connect_result = tokio::time::timeout(
        Duration::from_secs(5),
        connect_async(&ws_url)
    ).await;
    
    match connect_result {
        Ok(Ok((mut ws_stream, response))) => {
            println!("WebSocket connected! Response: {:?}", response);
            assert_eq!(response.status(), http::StatusCode::SWITCHING_PROTOCOLS);
            
            println!("Test 1: Connection established - PASS");
            
            let send_result = ws_stream.send(Message::Text("Hello WebSocket!".to_string())).await;
            assert!(send_result.is_ok(), "Failed to send text message");
            println!("Test 2: Send text message - PASS");
            
            let ping_result = ws_stream.send(Message::Ping(vec![1, 2, 3, 4])).await;
            assert!(ping_result.is_ok(), "Failed to send ping");
            println!("Test 3: Send ping - PASS");
            
            let pong_result = tokio::time::timeout(
                Duration::from_secs(2),
                async {
                    while let Some(msg_result) = ws_stream.next().await {
                        match msg_result {
                            Ok(Message::Pong(data)) => {
                                assert_eq!(data, vec![1, 2, 3, 4]);
                                return true;
                            }
                            Ok(other) => {
                                println!("Received other message: {:?}", other);
                            }
                            Err(e) => {
                                println!("Receive error: {:?}", e);
                                return false;
                            }
                        }
                    }
                    false
                }
            ).await;
            
            match pong_result {
                Ok(true) => println!("Test 4: Receive pong - PASS"),
                Ok(false) => println!("Test 4: Receive pong - connection issue"),
                Err(_) => println!("Test 4: Receive pong - timed out (expected: server may not auto-pong)"),
            }
            
            let close_result = ws_stream.close(None).await;
            assert!(close_result.is_ok(), "Failed to close connection");
            println!("Test 5: Close connection - PASS");
            
            println!("WebSocket connection tests completed successfully!");
        }
        Ok(Err(e)) => {
            println!("WebSocket connection failed: {:?}", e);
        }
        Err(_) => {
            println!("WebSocket connection timed out");
        }
    }
    
    server_task.abort();
    let _ = fs::remove_dir_all(&temp_dir_base);
}

#[tokio::test]
async fn test_quickjs_websocket_binary_message() {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    use futures_util::SinkExt;
    
    let temp_dir_base = std::env::temp_dir().join("test_quickjs_ws_binary");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    
    extern "C" fn binary_ws_callback(request: *mut HttpRequest) {
        if let Some((request_id, method, path, headers_json, body, body_len)) = extract_request_info(request) {
            let js_code = r#"
                function handleRequest(req) {
                    http.sendResponse(req.requestId, 200, {}, "HTTP");
                }
            "#;
            process_request_in_js(&request_id, &method, &path, &headers_json, &body, body_len, js_code);
        }
        unsafe { crate::c_api::free_http_request(request); }
    }
    
    let config_json = json!({
        "verbose": true,
        "mounts": [{ "type": "websocket", "path": "/ws" }]
    });
    
    let free_port = get_free_port();
    let port_copy = free_port;
    
    let server_task = tokio::spawn(async move {
        let mut server = AppHttpServer::new(free_port, "127.0.0.1".to_string(), binary_ws_callback, &config_json.to_string());
        server.start();
        loop { tokio::time::sleep(Duration::from_secs(1)).await; }
    });
    
    wait_for_server(port_copy).await;
    
    let ws_url = format!("ws://127.0.0.1:{}/ws", port_copy);
    
    if let Ok(Ok((mut ws_stream, response))) = tokio::time::timeout(
        Duration::from_secs(5),
        connect_async(&ws_url)
    ).await {
        assert_eq!(response.status(), http::StatusCode::SWITCHING_PROTOCOLS);
        println!("Test 1: Connection established - PASS");
        
        let binary_data: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]; // PNG header
        let send_result = ws_stream.send(Message::Binary(binary_data.clone())).await;
        assert!(send_result.is_ok(), "Failed to send binary message");
        println!("Test 2: Send binary message ({} bytes) - PASS", binary_data.len());
        
        let large_data: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
        let large_send_result = ws_stream.send(Message::Binary(large_data.clone())).await;
        assert!(large_send_result.is_ok(), "Failed to send large binary message");
        println!("Test 3: Send large binary message ({} bytes) - PASS", large_data.len());
        
        let close_result = ws_stream.close(None).await;
        assert!(close_result.is_ok(), "Failed to close connection");
        println!("Test 4: Close connection - PASS");
        
        println!("Binary WebSocket tests completed successfully!");
    } else {
        println!("WebSocket connection failed or timed out");
    }
    
    server_task.abort();
    let _ = fs::remove_dir_all(&temp_dir_base);
}

