use crate::types::{HttpRequest, ResponseData};
use crate::global::RESPONSE_CHANNELS;
use std::collections::HashMap;
use std::net::TcpListener;

// Run a closure on a dedicated OS thread because dropping a tokio Runtime
// inside an async context panics.
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

// Callback that echoes received request headers JSON (what JS sees) back as the body.
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

// Wait only for the TCP port to accept connections without sending an HTTP request
// (used by tests where the callback intentionally does not respond).
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
