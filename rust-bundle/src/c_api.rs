use std::os::raw::{c_char, c_int};
use std::ffi::{CStr, CString};
use std::sync::{Arc};
use std::path::PathBuf;
use std::collections::HashMap;

use crate::server::http_server::{HttpServer, SERVER_HANDLE};
use crate::server::static_server::{StaticHttpServer, STATIC_SERVER_HANDLE};
use crate::server::app_server::{AppHttpServer, APP_SERVER_HANDLE};
use crate::types::{RequestCallback, HttpRequest, ResponseData, ResponseChunkAccumulator};
use crate::global::{RESPONSE_CHANNELS, REQUEST_BODIES, CHUNK_ACCUMULATORS};

/// Validate that `port` fits within `u16` so `port as u16` never silently truncates.
fn validate_port(port: c_int) -> bool {
    if port < 0 || port > 65535 {
        crate::log_error!("Invalid port {}, must be 0-65535", port);
        return false;
    }
    true
}

/// Start the plain HTTP server using tokio's default worker thread count.
/// Retained for backwards compatibility; use [`start_server_with_options`] to set worker threads.
/// Returns the actual bound port (>0) on success, or <=0 on failure (not a bool).
#[no_mangle]
pub extern "C" fn start_server(
    port: c_int,
    host: *const c_char,
    callback: RequestCallback,
) -> c_int {
    start_server_with_options(port, host, callback, 0)
}

/// Start the plain HTTP server with an explicit tokio worker thread count.
///
/// `worker_threads`: `0` (or negative) uses tokio's CPU-core default; `>0` sets
/// an explicit count. The runtime is created per `start` and dropped on stop, so
/// worker count can change between restarts. Fewer threads reduce concurrency
/// under slow awaits, though lowering worker count on high-core machines can
/// reduce global mutex contention and per-request CPU overhead.
#[no_mangle]
pub extern "C" fn start_server_with_options(
    port: c_int,
    host: *const c_char,
    callback: RequestCallback,
    worker_threads: c_int,
) -> c_int {
    crate::log_info!("Starting HTTP server on port {}...", port);

    if !validate_port(port) {
        return 0;
    }

    let host_str = unsafe {
        if host.is_null() {
            String::from("127.0.0.1")
        } else {
            match CStr::from_ptr(host).to_str() {
                Ok(s) => {
                    if s.is_empty() {
                        String::from("127.0.0.1")
                    } else {
                        s.to_string()
                    }
                }
                Err(_) => {
                    crate::log_error!("Invalid host string, using default 127.0.0.1");
                    String::from("127.0.0.1")
                }
            }
        }
    };
    
    // Clamp negative values to 0 (tokio default) to avoid panicking in builder.
    let workers = if worker_threads > 0 { worker_threads as u32 } else { 0 };
    let mut server = HttpServer::new(port as u16, host_str.clone(), callback);
    let actual_port = server.start(workers);
    
    if actual_port > 0 {
        let mut guard = SERVER_HANDLE.lock().unwrap();
        *guard = Some(Arc::new(server));
        crate::global::mark_server_started();
        crate::log_info!("Server started successfully on {}:{}", host_str, actual_port);
    } else {
        crate::log_error!("Failed to start server on {}:{}", host_str, port);
    }
    
    actual_port as c_int
}

/// Register an optional callback invoked when a client aborts a request before a response is sent.
/// Pass `NULL` to unregister.
///
/// Invoked on a native thread when hyper drops the request future (see `RequestStateGuard`).
///
/// # Safety
/// `cb` must be safe to call from any thread; the `request_id` pointer is valid only
/// for the duration of the callback invocation.
#[no_mangle]
pub extern "C" fn set_request_aborted_callback(
    // Must be an inline fn type rather than a type alias for cbindgen compatibility.
    cb: Option<extern "C" fn(*const std::os::raw::c_char)>,
) -> bool {
    *crate::global::ABORT_CALLBACK.lock().unwrap() = cb;
    crate::log_info!("Request-aborted callback {}", if cb.is_some() { "registered" } else { "cleared" });
    true
}

/// Send an HTTP response for the given `request_id`.
#[no_mangle]
pub extern "C" fn send_response(
    request_id: *const c_char,
    status_code: c_int,
    headers_json: *const c_char,
    body: *const c_char,
    body_len: c_int,
) -> bool {
    unsafe {
        let request_id_str = if request_id.is_null() {
            crate::log_error!("send_response: request_id is null");
            return false;
        } else {
            CStr::from_ptr(request_id).to_string_lossy().into_owned()
        };
        
        let headers_str = if headers_json.is_null() {
            String::from("{}")
        } else {
            CStr::from_ptr(headers_json).to_string_lossy().into_owned()
        };
        
        let headers: HashMap<String, String> = match serde_json::from_str(&headers_str) {
            Ok(h) => h,
            Err(e) => {
                crate::log_error!("Failed to parse headers JSON: {}", e);
                HashMap::new()
            }
        };
        
        let mut body_data = if !body.is_null() && body_len > 0 {
            let body_slice = std::slice::from_raw_parts(body as *const u8, body_len as usize);
            body_slice.to_vec()
        } else {
            Vec::new()
        };

        // Prepend any chunks previously written via `write_response_chunk` (e.g.
        // `res.write('a'); res.end(arrayBuffer)`) so they are not dropped.
        let accumulated = {
            let mut accumulators = CHUNK_ACCUMULATORS.lock().unwrap();
            accumulators.remove(&request_id_str)
        };
        if let Some(accumulator) = accumulated {
            let mut merged: Vec<u8> = accumulator.chunks.into_iter().flatten().collect();
            if !merged.is_empty() {
                merged.extend_from_slice(&body_data);
                body_data = merged;
            }
        }
        
        crate::log_debug!("Sending response for request {}: status={}, headers={}, body_len={}", 
                 request_id_str, status_code, headers_str, body_len);
        
        let sender = {
            let mut channels = RESPONSE_CHANNELS.lock().unwrap();
            channels.remove(&request_id_str)
        };

        // Clean up remaining per-request state (idempotent).
        crate::global::cleanup_request_state(&request_id_str);

        match sender {
            Some(tx) => {
                let response_data = ResponseData {
                    status_code: status_code as u16,
                    headers,
                    body: body_data,
                };
                
                match tx.send(response_data) {
                    Ok(_) => {
                        crate::log_debug!("Response sent successfully for request {}", request_id_str);
                        true
                    }
                    Err(_) => {
                        crate::log_error!("Failed to send response: receiver dropped for request {}", request_id_str);
                        false
                    }
                }
            }
            None => {
                crate::log_error!("No response channel found for request {}", request_id_str);
                false
            }
        }
    }
}

/// Stop the HTTP server.
#[no_mangle]
pub extern "C" fn stop_server() {
    crate::log_info!("Stopping HTTP server...");
    let mut guard = SERVER_HANDLE.lock().unwrap();
    if let Some(_server) = guard.take() {
        crate::log_info!("Server stopped successfully");
    }
}

/// Start the static file HTTP server.
/// Returns the actual bound port (>0) on success, or <=0 on failure (not a bool).
#[no_mangle]
pub extern "C" fn start_static_server(port: c_int, host: *const c_char, root_dir: *const c_char) -> c_int {
    crate::log_info!("Starting static HTTP server on port {}...", port);

    if !validate_port(port) {
        return 0;
    }

    let host_str = unsafe {
        if host.is_null() {
            String::from("127.0.0.1")
        } else {
            match CStr::from_ptr(host).to_str() {
                Ok(s) => {
                    if s.is_empty() {
                        String::from("127.0.0.1")
                    } else {
                        s.to_string()
                    }
                }
                Err(_) => {
                    crate::log_error!("Invalid host string, using default 127.0.0.1");
                    String::from("127.0.0.1")
                }
            }
        }
    };
    
    let root_path = unsafe {
        if root_dir.is_null() {
            crate::log_error!("Root directory path is null");
            return 0;
        }
        match CStr::from_ptr(root_dir).to_str() {
            Ok(s) => PathBuf::from(s),
            Err(e) => {
                crate::log_error!("Invalid root directory path: {}", e);
                return 0;
            }
        }
    };
    
    if !root_path.exists() {
        crate::log_error!("Root directory does not exist: {:?}", root_path);
        return 0;
    }
    
    if !root_path.is_dir() {
        crate::log_error!("Root path is not a directory: {:?}", root_path);
        return 0;
    }
    
    let mut server = StaticHttpServer::new(port as u16, host_str.clone(), root_path.clone());
    let actual_port = server.start();
    
    if actual_port > 0 {
        let mut guard = STATIC_SERVER_HANDLE.lock().unwrap();
        *guard = Some(Arc::new(server));
        crate::global::mark_server_started();
        crate::log_info!("Static server started successfully on {}:{} with root directory: {:?}", host_str, actual_port, root_path);
    } else {
        crate::log_error!("Failed to start static server on {}:{}", host_str, port);
    }
    
    actual_port as c_int
}

/// Stop the static file HTTP server.
#[no_mangle]
pub extern "C" fn stop_static_server() {
    crate::log_info!("Stopping static HTTP server...");
    let mut guard = STATIC_SERVER_HANDLE.lock().unwrap();
    if let Some(_server) = guard.take() {
        crate::log_info!("Static server stopped successfully");
    }
}

/// Start the App HTTP server (serves static files on GET hits, falls back to callback).
/// Returns the actual bound port (>0) on success, or <=0 on failure (not a bool).
#[no_mangle]
pub extern "C" fn start_app_server(port: c_int, host: *const c_char, root_dir: *const c_char, callback: RequestCallback) -> c_int {
    crate::log_info!("Starting App HTTP server on port {}...", port);

    if !validate_port(port) {
        return 0;
    }

    let host_str = unsafe {
        if host.is_null() {
            String::from("127.0.0.1")
        } else {
            match CStr::from_ptr(host).to_str() {
                Ok(s) => {
                    if s.is_empty() {
                        String::from("127.0.0.1")
                    } else {
                        s.to_string()
                    }
                }
                Err(_) => {
                    crate::log_error!("Invalid host string, using default 127.0.0.1");
                    String::from("127.0.0.1")
                }
            }
        }
    };
    
    let root_path = unsafe {
        if root_dir.is_null() {
            crate::log_error!("Root directory path is null");
            return 0;
        }
        match CStr::from_ptr(root_dir).to_str() {
            Ok(s) => s.to_string(),
            Err(e) => {
                crate::log_error!("Invalid root directory path: {}", e);
                return 0;
            }
        }
    };
    
    let root_path_buf = PathBuf::from(&root_path);
    if !root_path_buf.exists() {
        crate::log_error!("Root directory does not exist: {:?}", root_path_buf);
        return 0;
    }
    
    if !root_path_buf.is_dir() {
        crate::log_error!("Root path is not a directory: {:?}", root_path_buf);
        return 0;
    }
    
    let config_json = format!(r#"{{"root_dir": "{}"}}"#, root_path.replace("\\", "\\\\").replace("\"", "\\\""));
    
    let mut server = AppHttpServer::new(port as u16, host_str.clone(), callback, &config_json);
    let actual_port = server.start();
    
    if actual_port > 0 {
        let mut guard = APP_SERVER_HANDLE.lock().unwrap();
        *guard = Some(Arc::new(server));
        crate::global::mark_server_started();
        crate::log_info!("App server started successfully on {}:{} with root directory: {}", host_str, actual_port, root_path);
    } else {
        crate::log_error!("Failed to start app server on {}:{}", host_str, port);
    }
    
    actual_port as c_int
}

/// Start the App HTTP server with JSON configuration (`config_json` may specify `root_dir`).
/// Returns the actual bound port (>0) on success, or <=0 on failure (not a bool).
#[no_mangle]
pub extern "C" fn start_server_with_config(
    port: c_int,
    host: *const c_char,
    callback: RequestCallback,
    config_json: *const c_char,
) -> c_int {
    crate::log_info!("Starting App HTTP server with config on port {}...", port);

    if !validate_port(port) {
        return 0;
    }

    let host_str = unsafe {
        if host.is_null() {
            String::from("127.0.0.1")
        } else {
            match CStr::from_ptr(host).to_str() {
                Ok(s) => {
                    if s.is_empty() {
                        String::from("127.0.0.1")
                    } else {
                        s.to_string()
                    }
                }
                Err(_) => {
                    crate::log_error!("Invalid host string, using default 127.0.0.1");
                    String::from("127.0.0.1")
                }
            }
        }
    };

    let config_str = unsafe {
        if config_json.is_null() {
            ""
        } else {
            match CStr::from_ptr(config_json).to_str() {
                Ok(s) => s,
                Err(_) => ""
            }
        }
    };
    
    let mut server = AppHttpServer::new(port as u16, host_str.clone(), callback, config_str);
    let actual_port = server.start();
    
    if actual_port > 0 {
        let mut guard = APP_SERVER_HANDLE.lock().unwrap();
        *guard = Some(Arc::new(server));
        crate::global::mark_server_started();
        crate::log_info!("App Server started successfully on {}:{}", host_str, actual_port);
    } else {
        crate::log_error!("Failed to start App server on {}:{}", host_str, port);
    }
    
    actual_port as c_int
}

/// Stop the App HTTP server.
#[no_mangle]
pub extern "C" fn stop_app_server() {
    crate::log_info!("Stopping App HTTP server...");
    let mut guard = APP_SERVER_HANDLE.lock().unwrap();
    if let Some(_server) = guard.take() {
        crate::log_info!("App server stopped successfully");
    }
}

/// Return server statistics as a heap-allocated JSON string.
///
/// The caller must free the returned pointer with `free_string`.
///
/// Field semantics:
/// - `status`: always `"running"` (retained for backwards compatibility).
/// - `totalRequests`: total requests entered across all three server entry points.
/// - `errorCount`: internal server errors (timeout / closed response channel /
///   plugin I/O failure), excluding 4xx/5xx returned by JS or plugins.
/// - `uptime`: seconds since the most recent `start_*` call.
/// - `activeConnections`, `bytesSent`, `bytesReceived`: currently 0 (not tracked
///   across all hyper/plugin response paths).
#[no_mangle]
pub extern "C" fn get_server_stats() -> *const c_char {
    use std::sync::atomic::Ordering;

    let json = format!(
        concat!(
            r#"{{"status":"running","totalRequests":{},"activeConnections":0,"#,
            r#""bytesSent":0,"bytesReceived":0,"uptime":{},"errorCount":{}}}"#
        ),
        crate::global::TOTAL_REQUESTS.load(Ordering::Relaxed),
        crate::global::uptime_secs(),
        crate::global::ERROR_COUNT.load(Ordering::Relaxed),
    );

    // Transfer ownership to caller (paired with free_string).
    CString::new(json).unwrap_or_default().into_raw() as *const c_char
}

/// Free an `HttpRequest` struct and all of its owned fields.
///
/// # Safety
/// `request` must be a valid pointer previously allocated by this library, or null.
#[no_mangle]
pub unsafe extern "C" fn free_http_request(request: *mut HttpRequest) {
    if request.is_null() {
        return;
    }
    
    let req = Box::from_raw(request);
    
    if !req.request_id.is_null() {
        let _ = CString::from_raw(req.request_id);
    }
    if !req.method.is_null() {
        let _ = CString::from_raw(req.method);
    }
    if !req.path.is_null() {
        let _ = CString::from_raw(req.path);
    }
    if !req.headers_json.is_null() {
        let _ = CString::from_raw(req.headers_json);
    }
    if !req.body.is_null() && req.body_len > 0 {
        // Reclaim body allocated via `alloc_request_body`.
        let _ = Vec::from_raw_parts(req.body as *mut u8, req.body_len as usize, req.body_len as usize);
    }
}

/// Free a C string allocated by this library.
///
/// # Safety
/// `s` must be a valid CString pointer previously returned by this library, or null.
#[no_mangle]
pub unsafe extern "C" fn free_string(s: *mut c_char) {
    if !s.is_null() {
        let _ = CString::from_raw(s);
    }
}

/// Read the next chunk of the request body into `buffer`.
/// Returns bytes read (>0), 0 at EOF, or -1 on error.
#[no_mangle]
pub extern "C" fn read_request_body_chunk(
    request_id: *const c_char,
    buffer: *mut c_char,
    buffer_size: c_int,
) -> c_int {
    unsafe {
        let request_id_str = if request_id.is_null() {
            crate::log_error!("read_request_body_chunk: request_id is null");
            return -1;
        } else {
            CStr::from_ptr(request_id).to_string_lossy().into_owned()
        };
        
        if buffer.is_null() || buffer_size <= 0 {
            crate::log_error!("read_request_body_chunk: invalid buffer");
            return -1;
        }
        
        let mut bodies = REQUEST_BODIES.lock().unwrap();
        
        if let Some(body_data) = bodies.get_mut(&request_id_str) {
            let remaining = body_data.data.len() - body_data.read_position;
            
            if remaining == 0 {
                return 0;
            }
            
            let mut to_read = std::cmp::min(remaining, buffer_size as usize);
            if body_data.read_position + to_read < body_data.data.len() && to_read >= 4 {
                let end = body_data.read_position + to_read;
                // Back up before a UTF-8 continuation byte (10xxxxxx) to avoid splitting a multi-byte character.
                if (body_data.data[end] & 0xC0) == 0x80 {
                    let mut back = 0usize;
                    while back < 3 && back < to_read {
                        let idx = end - 1 - back;
                        let b = body_data.data[idx];
                        back += 1;
                        if (b & 0xC0) != 0x80 {
                            if (b & 0x80) != 0 {
                                to_read -= back;
                            }
                            break;
                        }
                    }
                }
            }
            let buffer_slice = std::slice::from_raw_parts_mut(buffer as *mut u8, to_read);
            
            buffer_slice.copy_from_slice(
                &body_data.data[body_data.read_position..body_data.read_position + to_read]
            );
            
            body_data.read_position += to_read;
            
            to_read as c_int
        } else {
            crate::log_error!("read_request_body_chunk: no body found for request {}", request_id_str);
            -1
        }
    }
}

/// Write a chunk to the response stream accumulator for `request_id`.
#[no_mangle]
pub extern "C" fn write_response_chunk(
    request_id: *const c_char,
    chunk: *const c_char,
    chunk_len: c_int,
) -> bool {
    unsafe {
        let request_id_str = if request_id.is_null() {
            crate::log_error!("write_response_chunk: request_id is null");
            return false;
        } else {
            CStr::from_ptr(request_id).to_string_lossy().into_owned()
        };
        
        if chunk.is_null() || chunk_len < 0 {
            crate::log_error!("write_response_chunk: invalid chunk");
            return false;
        }
        
        let chunk_data = if chunk_len > 0 {
            let chunk_slice = std::slice::from_raw_parts(chunk as *const u8, chunk_len as usize);
            chunk_slice.to_vec()
        } else {
            Vec::new()
        };
        
        let mut accumulators = CHUNK_ACCUMULATORS.lock().unwrap();
        let accumulator = accumulators.entry(request_id_str.clone())
            .or_insert_with(ResponseChunkAccumulator::new);
        
        accumulator.add_chunk(chunk_data);
        
        crate::log_debug!("write_response_chunk: accumulated {} bytes for request {} (total chunks: {})", 
                 chunk_len, request_id_str, accumulator.chunks.len());
        
        true
    }
}

/// Finalize and send a streamed response for `request_id`.
#[no_mangle]
pub extern "C" fn end_response(
    request_id: *const c_char,
    status_code: c_int,
    headers_json: *const c_char,
) -> bool {
    unsafe {
        let request_id_str = if request_id.is_null() {
            crate::log_error!("end_response: request_id is null");
            return false;
        } else {
            CStr::from_ptr(request_id).to_string_lossy().into_owned()
        };
        
        let headers_str = if headers_json.is_null() {
            String::from("{}")
        } else {
            CStr::from_ptr(headers_json).to_string_lossy().into_owned()
        };
        
        let headers: HashMap<String, String> = match serde_json::from_str(&headers_str) {
            Ok(h) => h,
            Err(e) => {
                crate::log_error!("Failed to parse headers JSON: {}", e);
                HashMap::new()
            }
        };

        let accumulator = {
            let mut accumulators = CHUNK_ACCUMULATORS.lock().unwrap();
            accumulators.remove(&request_id_str)
        };
        
        if let Some(accumulator) = accumulator {
            let body: Vec<u8> = accumulator.chunks.into_iter().flatten().collect();
            let response_data = ResponseData {
                status_code: status_code as u16,
                headers,
                body,
            };
            
            crate::log_debug!("end_response: sending accumulated response for request {} ({} bytes)", 
                     request_id_str, response_data.body.len());
            
            let sender = {
                let mut channels = RESPONSE_CHANNELS.lock().unwrap();
                channels.remove(&request_id_str)
            };
            
            {
                let mut bodies = REQUEST_BODIES.lock().unwrap();
                bodies.remove(&request_id_str);
            }
            
            match sender {
                Some(tx) => {
                    match tx.send(response_data) {
                        Ok(_) => {
                            crate::log_debug!("end_response: response sent successfully for request {}", request_id_str);
                            true
                        }
                        Err(_) => {
                            crate::log_error!("end_response: failed to send response: receiver dropped for request {}", request_id_str);
                            false
                        }
                    }
                }
                None => {
                    crate::log_error!("end_response: no response channel found for request {}", request_id_str);
                    false
                }
            }
        } else {
             crate::log_debug!("end_response: no accumulated chunks for request {}, sending empty body", request_id_str);
            
            let response_data = ResponseData {
                status_code: status_code as u16,
                headers,
                body: Vec::new(),
            };
            
            let sender = {
                let mut channels = RESPONSE_CHANNELS.lock().unwrap();
                channels.remove(&request_id_str)
            };
            
            {
                let mut bodies = REQUEST_BODIES.lock().unwrap();
                bodies.remove(&request_id_str);
            }
            
            match sender {
                Some(tx) => {
                    match tx.send(response_data) {
                         Ok(_) => true,
                         Err(_) => false,
                    }
                }
                None => {
                    let mut channels = RESPONSE_CHANNELS.lock().unwrap();
                    let mut bodies = REQUEST_BODIES.lock().unwrap();
                    channels.remove(&request_id_str);
                    bodies.remove(&request_id_str);
                    false
                }
            }
        }
    }
}

// ==================== WebSocket API ====================

use crate::plugins::websocket::{
    WebSocketEvent, WebSocketEventType, WebSocketPlugin,
    get_all_global_websocket_plugins,
};
use std::sync::atomic::{AtomicPtr, Ordering};

#[repr(C)]
pub enum CWebSocketEventType {
    Open = 1,
    Message = 2,
    Close = 3,
    Error = 4,
}

#[repr(C)]
pub struct CWebSocketEvent {
    pub connection_id: *const c_char,
    pub event_type: c_int,
    pub path: *const c_char,
    pub query: *const c_char,
    pub headers_json: *const c_char,
    pub text_data: *const c_char,
    pub text_len: c_int,
    pub binary_data: *const c_char,
    pub binary_len: c_int,
    pub close_code: c_int,
    pub close_reason: *const c_char,
    pub error_message: *const c_char,  // Error event only
}

pub type CWebSocketCallback = extern "C" fn(*const CWebSocketEvent);

/// Thread-safe function pointer wrapper.
struct SendSyncCallback(CWebSocketCallback);
unsafe impl Send for SendSyncCallback {}
unsafe impl Sync for SendSyncCallback {}

static WEBSOCKET_CALLBACK: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

/// Attach the C callback to a single `WebSocketPlugin` instance.
fn attach_callback_to_plugin(plugin: &WebSocketPlugin, callback: CWebSocketCallback) {
    let wrapped_callback = SendSyncCallback(callback);
    plugin.set_event_callback(move |event: WebSocketEvent| {
        crate::log_info!("WebSocket event received: type={:?}, conn_id={}", event.event_type, event.connection_id);
        let callback_fn = wrapped_callback.0;
        
        let event_type = match event.event_type {
            WebSocketEventType::Open => 1,
            WebSocketEventType::Message => 2,
            WebSocketEventType::Close => 3,
            WebSocketEventType::Error => 4,
        };
        
        let connection_id_c = CString::new(event.connection_id.clone()).unwrap_or_default();
        let path_c = event.path.as_ref()
            .map(|s| CString::new(s.as_str()).unwrap_or_default());
        let query_c = event.query.as_ref()
            .map(|s| CString::new(s.as_str()).unwrap_or_default());
        let headers_json_c = event.headers.as_ref()
            .map(|h| serde_json::to_string(h).unwrap_or_default())
            .map(|s| CString::new(s).unwrap_or_default());
        let text_buf: Option<Vec<u8>> = event.text_data.as_ref().map(|s| {
            let bytes = s.as_bytes();
            let mut v = Vec::with_capacity(bytes.len() + 1);
            v.extend_from_slice(bytes);
            v.push(0);
            v
        });
        let close_reason_c = event.close_reason.as_ref()
            .map(|s| CString::new(s.as_str()).unwrap_or_default());
        let error_message_c = event.error_message.as_ref()
            .map(|s| CString::new(s.as_str()).unwrap_or_default());
        
        let c_event = CWebSocketEvent {
            connection_id: connection_id_c.as_ptr(),
            event_type,
            path: path_c.as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null()),
            query: query_c.as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null()),
            headers_json: headers_json_c.as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null()),
            text_data: text_buf.as_ref().map(|v| v.as_ptr() as *const c_char).unwrap_or(std::ptr::null()),
            text_len: event.text_data.as_ref().map(|s| s.len() as c_int).unwrap_or(0),
            binary_data: event.binary_data.as_ref()
                .map(|b| b.as_ptr() as *const c_char)
                .unwrap_or(std::ptr::null()),
            binary_len: event.binary_data.as_ref().map(|b| b.len() as c_int).unwrap_or(0),
            close_code: event.close_code.map(|c| c as c_int).unwrap_or(0),
            close_reason: close_reason_c.as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null()),
            error_message: error_message_c.as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null()),
        };
        
        crate::log_info!("Calling C callback with event type {}", event_type);
        callback_fn(&c_event);
    });
}

/// Attach a previously registered `set_websocket_callback` to a newly created plugin.
pub(crate) fn apply_saved_websocket_callback(plugin: &WebSocketPlugin) {
    let ptr = WEBSOCKET_CALLBACK.load(Ordering::SeqCst);
    if !ptr.is_null() {
        let callback: CWebSocketCallback = unsafe { std::mem::transmute(ptr) };
        attach_callback_to_plugin(plugin, callback);
    }
}

/// Register the global WebSocket event callback.
#[no_mangle]
pub extern "C" fn set_websocket_callback(callback: CWebSocketCallback) {
    crate::log_info!("Setting WebSocket callback");
    
    // Save pointer so plugins registered on subsequent server starts attach it automatically.
    WEBSOCKET_CALLBACK.store(callback as *mut (), Ordering::SeqCst);
    
    let plugins = get_all_global_websocket_plugins();
    if plugins.is_empty() {
        crate::log_info!("No WebSocket plugin registered yet; callback saved for future server start.");
    } else {
        for plugin in plugins {
            attach_callback_to_plugin(&plugin, callback);
        }
    }
}

/// Send a WebSocket text message.
#[no_mangle]
pub extern "C" fn ws_send_text(connection_id: *const c_char, message: *const c_char) -> bool {
    unsafe {
        let conn_id = if connection_id.is_null() {
            crate::log_error!("ws_send_text: connection_id is null");
            return false;
        } else {
            CStr::from_ptr(connection_id).to_string_lossy().into_owned()
        };
        
        let msg = if message.is_null() {
            String::new()
        } else {
            CStr::from_ptr(message).to_string_lossy().into_owned()
        };
        
        let plugins = get_all_global_websocket_plugins();
        if plugins.is_empty() {
            crate::log_error!("ws_send_text: WebSocket plugin not initialized");
            return false;
        }
        plugins.iter().any(|p| p.send_text(&conn_id, &msg))
    }
}

/// Send a WebSocket binary message.
#[no_mangle]
pub extern "C" fn ws_send_binary(connection_id: *const c_char, data: *const c_char, len: c_int) -> bool {
    unsafe {
        let conn_id = if connection_id.is_null() {
            crate::log_error!("ws_send_binary: connection_id is null");
            return false;
        } else {
            CStr::from_ptr(connection_id).to_string_lossy().into_owned()
        };
        
        if data.is_null() || len < 0 {
            crate::log_error!("ws_send_binary: invalid data");
            return false;
        }
        
        let data_slice = if len == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(data as *const u8, len as usize)
        };
        
        let plugins = get_all_global_websocket_plugins();
        if plugins.is_empty() {
            crate::log_error!("ws_send_binary: WebSocket plugin not initialized");
            return false;
        }
        plugins.iter().any(|p| p.send_binary(&conn_id, data_slice))
    }
}

/// Close a WebSocket connection (`code` 1000 indicates normal closure).
#[no_mangle]
pub extern "C" fn ws_close(connection_id: *const c_char, code: c_int, reason: *const c_char) -> bool {
    unsafe {
        let conn_id = if connection_id.is_null() {
            crate::log_error!("ws_close: connection_id is null");
            return false;
        } else {
            CStr::from_ptr(connection_id).to_string_lossy().into_owned()
        };
        
        let reason_str = if reason.is_null() {
            String::new()
        } else {
            CStr::from_ptr(reason).to_string_lossy().into_owned()
        };
        
        let plugins = get_all_global_websocket_plugins();
        if plugins.is_empty() {
            crate::log_error!("ws_close: WebSocket plugin not initialized");
            return false;
        }
        plugins.iter().any(|p| p.close_connection(&conn_id, code as u16, &reason_str))
    }
}


/// Set global CORS configuration (`"true"` for defaults, JSON object to customize, NULL or `"false"` to disable).
#[no_mangle]
pub extern "C" fn set_cors_config(config_json: *const c_char) {
    let json_str = unsafe {
        if config_json.is_null() {
            None
        } else {
            match CStr::from_ptr(config_json).to_str() {
                Ok(s) => Some(s),
                Err(_) => {
                    crate::log_warn!("Invalid UTF-8 in CORS config, disabling CORS");
                    None
                }
            }
        }
    };
    crate::cors::set_cors_config_json(json_str);
}
