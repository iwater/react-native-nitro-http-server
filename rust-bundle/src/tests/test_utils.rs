use crate::types::{HttpRequest, ResponseData};
use crate::global::RESPONSE_CHANNELS;
use std::collections::HashMap;
use std::net::TcpListener;

// 在独立的 OS 线程上执行闭包。
// start_* / stop_* 会创建或销毁一个 tokio Runtime，而"在 async 上下文里 drop Runtime"
// 会让 tokio 直接 abort 整个测试进程：
//   "Cannot drop a runtime in a context where blocking is not allowed"
// 所以 #[tokio::test] 里调 stop_server() 之前必须套一层 run_outside_async。
pub fn run_outside_async<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(f).join().unwrap()
}

// Helper to find a free port
pub fn get_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

// Common callback function that returns 404
pub extern "C" fn test_callback_404(request: *mut HttpRequest) {
    if request.is_null() { return; }
    
    let req = unsafe { &*request };
    
    let request_id_c = unsafe { std::ffi::CStr::from_ptr(req.request_id) };
    let request_id = request_id_c.to_string_lossy().to_string();
    
    if let Some(chan) = RESPONSE_CHANNELS.lock().unwrap().remove(&request_id) {
         let resp = ResponseData {
             status_code: 404,
             headers: HashMap::new(),
             body: b"Callback 404".to_vec(),
         };
         let _ = chan.send(resp);
    }
    
    unsafe {
        crate::c_api::free_http_request(request);
    }
}

// Callback that returns 200 OK with "Hello World"
pub extern "C" fn test_callback_ok(request: *mut HttpRequest) {
    if request.is_null() { return; }
    
    let req = unsafe { &*request };
    let request_id_c = unsafe { std::ffi::CStr::from_ptr(req.request_id) };
    let request_id = request_id_c.to_string_lossy().to_string();
    
    if let Some(chan) = RESPONSE_CHANNELS.lock().unwrap().remove(&request_id) {
         let resp = ResponseData {
             status_code: 200,
             headers: HashMap::new(),
             body: b"Hello World".to_vec(),
         };
         let _ = chan.send(resp);
    }
    
    unsafe {
        crate::c_api::free_http_request(request);
    }
}

// 回调：把 C 侧收到的请求头（**就是 JS 侧看到的那份 JSON**）原样回显到响应体。
//
// 用来验证「经过完整链路之后，header 值还剩什么」：
// `http_server.rs` / `app_server.rs` 收集 header 时用的是
// `value.to_str().unwrap_or("")` —— 对 obs-text（未编码的非 ASCII 文件名等）
// `to_str()` 返回 Err，值会被**静默置成空串**，调用方察觉不到。
// 在 plugin 层断言 `to_str().is_ok()` 只能证明「设进去的值可读」，
// 只有走到这一层才能证明「JS 实际收到的东西是对的」。
pub extern "C" fn test_callback_echo_headers(request: *mut HttpRequest) {
    if request.is_null() { return; }

    let req = unsafe { &*request };
    let request_id_c = unsafe { std::ffi::CStr::from_ptr(req.request_id) };
    let request_id = request_id_c.to_string_lossy().to_string();

    let headers_json = unsafe { std::ffi::CStr::from_ptr(req.headers_json) }
        .to_string_lossy()
        .to_string();

    if let Some(chan) = RESPONSE_CHANNELS.lock().unwrap().remove(&request_id) {
         let resp = ResponseData {
             status_code: 200,
             headers: HashMap::new(),
             body: headers_json.into_bytes(),
         };
         let _ = chan.send(resp);
    }

    unsafe {
        crate::c_api::free_http_request(request);
    }
}

// 只等待端口可连接，不发 HTTP 请求。
// 用于"回调故意不响应"的测试：wait_for_server 会一直等到 30s 超时才有响应。
pub async fn wait_for_port(port: u16) {
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("Server failed to listen on port {}", port);
}

pub async fn wait_for_server(port: u16) {
    let client = reqwest::Client::new();
    let base_url = format!("http://127.0.0.1:{}", port);
    
    for _ in 0..50 { 
        match client.get(&base_url).send().await {
            Ok(_) => return,
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
    panic!("Server failed to start on port {}", port);
}
