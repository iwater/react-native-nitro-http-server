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

/// 启动 HTTP 服务器
/// 
/// # Arguments
/// * `port` - 服务器端口号
/// * `host` - 监听的 IP 地址，传入 NULL 则默认为 "127.0.0.1"
/// * `callback` - 请求回调函数，当收到请求时调用
/// 
/// # Returns
/// 成功返回实际端口号（>0），失败返回 <=0（注意：不是 bool——头文件曾一度错标为
/// bool，-O2/-Os 下会让 C 调用方被优化器按 bool 域假设常量折叠）
/// 校验端口号落在 u16 范围内。
/// 不加校验时 `port as u16` 会静默截断（如 70000 -> 4464），监听到一个完全陌生的端口。
fn validate_port(port: c_int) -> bool {
    if port < 0 || port > 65535 {
        crate::log_error!("Invalid port {}, must be 0-65535", port);
        return false;
    }
    true
}

/// 启动普通 HTTP 服务器（tokio worker 数走默认 = CPU 核数）。
///
/// 保留三参数签名是为了**向后兼容**（既有 C 调用方与 15 处 Rust 测试都不必改）；
/// 需要指定 worker 数请用 [`start_server_with_options`]。
#[no_mangle]
pub extern "C" fn start_server(
    port: c_int,
    host: *const c_char,
    callback: RequestCallback,
) -> c_int {
    start_server_with_options(port, host, callback, 0)
}

/// 启动普通 HTTP 服务器，并指定 tokio worker 线程数。
///
/// `worker_threads`：`0` = 按 CPU 核数（tokio 默认）；`>0` = 指定线程数。
/// 非正数一律归 `0`（负值透传给 tokio 的 builder 会 panic）。
///
/// 该 runtime 在**每次调用**时新建、随 server 一起销毁，所以每次启动都可以改
/// （与 `rust_c_net` 那个全局 `OnceCell` runtime 不同）。
///
/// 实测（10 核机，A 路径 native fast path）：默认 10 个 worker 是**过配** ——
/// 各 worker 只有 14~23% 忙碌，而全局 Mutex（`RESPONSE_CHANNELS` /
/// `REQUEST_BODIES`）竞争 + 跨线程唤醒合计占 11.4% CPU；降到 2 个后
/// 高并发吞吐 **+13%**、每请求 CPU **−40%**。
/// ⚠️ 线程越少并发容纳能力越低：handler 里有慢 `await`（DB/网络）时会排队。
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

    // 解析 host 参数
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
    
    // 负值归 0（= tokio 默认），避免把非法值透传下去
    let workers = if worker_threads > 0 { worker_threads as u32 } else { 0 };
    let mut server = HttpServer::new(port as u16, host_str.clone(), callback);
    let actual_port = server.start(workers);
    
    if actual_port > 0 {
        let mut guard = SERVER_HANDLE.lock().unwrap();
        *guard = Some(Arc::new(server));
        crate::global::mark_server_started(); // uptime 的起点
        crate::log_info!("Server started successfully on {}:{}", host_str, actual_port);
    } else {
        crate::log_error!("Failed to start server on {}:{}", host_str, port);
    }
    
    actual_port as c_int
}

/// 注册「请求被中断」的回调（可选）。
///
/// 客户端在 handler 返回之前断开连接时，Rust 侧会调用它，参数是 `request_id`。
/// 传 `NULL` 可注销。
///
/// ⚠️ 回调在**原生线程**上被调用，实现方负责切回 JS 线程。
/// 调用时机是请求 future 被丢弃时（见 `RequestStateGuard` 的注释）。
///
/// # Safety
/// `cb` 必须是可跨线程调用的函数指针；它收到的 `request_id` 指针**仅在调用期间有效**，
/// 需要留存请自行拷贝。
#[no_mangle]
pub extern "C" fn set_request_aborted_callback(
    // ⚠️ fn 类型必须**内联展开**：cbindgen 不穿透 type alias，
    // 写成 Option<RequestAbortedCallback> 会生成 C++ 读不懂的 Option 结构体。
    cb: Option<extern "C" fn(*const std::os::raw::c_char)>,
) -> bool {
    *crate::global::ABORT_CALLBACK.lock().unwrap() = cb;
    crate::log_info!("Request-aborted callback {}", if cb.is_some() { "registered" } else { "cleared" });
    true
}

/// 发送 HTTP 响应
/// 
/// # Arguments
/// * `request_id` - 请求 ID，必须与回调中的 requestId 匹配
/// * `status_code` - HTTP 状态码
/// * `headers_json` - 响应头的 JSON 字符串
/// * `body` - 响应体数据
/// * `body_len` - 响应体长度
/// 
/// # Returns
/// 如果响应发送成功返回 true，否则返回 false
#[no_mangle]
pub extern "C" fn send_response(
    request_id: *const c_char,
    status_code: c_int,
    headers_json: *const c_char,
    body: *const c_char,
    body_len: c_int,
) -> bool {
    unsafe {
        // 解析 request_id
        let request_id_str = if request_id.is_null() {
            crate::log_error!("send_response: request_id is null");
            return false;
        } else {
            CStr::from_ptr(request_id).to_string_lossy().into_owned()
        };
        
        // 解析 headers
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
        
        // 解析 body
        let mut body_data = if !body.is_null() && body_len > 0 {
            let body_slice = std::slice::from_raw_parts(body as *const u8, body_len as usize);
            body_slice.to_vec()
        } else {
            Vec::new()
        };

        // ⚠️ 必须先把「流式累积器」里已写的块并进来，否则
        //    `res.write('a'); res.end(arrayBuffer)` 里的 `'a'` 会被**静默丢弃**。
        //
        //    为什么会混用两条通道：`send_response` 同时服务
        //      · `sendResponse`（handler 返回普通对象，body 走显式参数）
        //      · `sendBinaryResponse`（`res.end(ArrayBuffer)`，body 也走显式参数）
        //    而 `res.write(...)` 走的是**另一条**通道 `write_response_chunk`（累积器）。
        //    于是「先 write、再 end(二进制)」时两条通道都该收；只取显式 body 就丢数据。
        //
        //    顺序与 Node 一致：先 write 的在前、`end(chunk)` 的在后。
        //    期望值来自真实 node v22.22.2 实测：
        //      `write('a'); end(Buffer.from([0x62,0x63]))` → body `616263`。
        //
        //    非流式路径上累积器恒为空 ⇒ 这里是无副作用的 no-op。
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
        
        // 从全局映射表中取出对应的 channel sender
        let sender = {
            let mut channels = RESPONSE_CHANNELS.lock().unwrap();
            channels.remove(&request_id_str)
        };

        // 其余两张表一并清理（幂等，对已取走的 channels 是 no-op）
        crate::global::cleanup_request_state(&request_id_str);

        match sender {
            Some(tx) => {
                // 构建响应数据
                let response_data = ResponseData {
                    status_code: status_code as u16,
                    headers,
                    body: body_data,
                };
                
                // 发送响应数据
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

/// 停止 HTTP 服务器
#[no_mangle]
pub extern "C" fn stop_server() {
    crate::log_info!("Stopping HTTP server...");
    let mut guard = SERVER_HANDLE.lock().unwrap();
    if let Some(_server) = guard.take() {
        // 如果需要，可以在这里添加清理逻辑
        crate::log_info!("Server stopped successfully");
    }
}

/// 启动静态文件 HTTP 服务器
/// 
/// # Arguments
/// * `port` - 服务器端口号
/// * `host` - 监听的 IP 地址，传入 NULL 则默认为 "127.0.0.1"
/// * `root_dir` - 静态文件根目录路径
/// 
/// # Returns
/// 成功返回实际端口号（>0），失败返回 <=0（注意：不是 bool）
#[no_mangle]
pub extern "C" fn start_static_server(port: c_int, host: *const c_char, root_dir: *const c_char) -> c_int {
    crate::log_info!("Starting static HTTP server on port {}...", port);

    if !validate_port(port) {
        return 0;
    }

    // 解析 host 参数
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
    
    // 检查目录是否存在
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
        crate::global::mark_server_started(); // uptime 的起点
        crate::log_info!("Static server started successfully on {}:{} with root directory: {:?}", host_str, actual_port, root_path);
    } else {
        crate::log_error!("Failed to start static server on {}:{}", host_str, port);
    }
    
    actual_port as c_int
}

/// 停止静态文件 HTTP 服务器
#[no_mangle]
pub extern "C" fn stop_static_server() {
    crate::log_info!("Stopping static HTTP server...");
    let mut guard = STATIC_SERVER_HANDLE.lock().unwrap();
    if let Some(_server) = guard.take() {
        crate::log_info!("Static server stopped successfully");
    }
}

/// 启动 App HTTP 服务器（混合静态文件和回调）
/// 
/// 当接收到 GET 请求时，首先尝试从 root_dir 查找静态文件：
/// - 如果文件存在，直接返回文件内容
/// - 如果文件不存在，将请求交给 callback 处理
/// 
/// 当接收到非 GET 请求（POST、PUT、DELETE 等）时，直接交给 callback 处理
/// 
/// # Arguments
/// * `port` - 服务器端口号
/// * `host` - 监听的 IP 地址，传入 NULL 则默认为 "127.0.0.1"
/// * `root_dir` - 静态文件根目录路径
/// * `callback` - 请求回调函数，当静态文件不存在或不是 GET 请求时调用
/// 
/// # Returns
/// 成功返回实际端口号（>0），失败返回 <=0（注意：不是 bool）
#[no_mangle]
pub extern "C" fn start_app_server(port: c_int, host: *const c_char, root_dir: *const c_char, callback: RequestCallback) -> c_int {
    crate::log_info!("Starting App HTTP server on port {}...", port);

    if !validate_port(port) {
        return 0;
    }

    // 解析 host 参数
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
    
    // 检查目录是否存在
    let root_path_buf = PathBuf::from(&root_path);
    if !root_path_buf.exists() {
        crate::log_error!("Root directory does not exist: {:?}", root_path_buf);
        return 0;
    }
    
    if !root_path_buf.is_dir() {
        crate::log_error!("Root path is not a directory: {:?}", root_path_buf);
        return 0;
    }
    
    // 构建配置 JSON，包含 root_dir
    let config_json = format!(r#"{{"root_dir": "{}"}}"#, root_path.replace("\\", "\\\\").replace("\"", "\\\""));
    
    let mut server = AppHttpServer::new(port as u16, host_str.clone(), callback, &config_json);
    let actual_port = server.start();
    
    if actual_port > 0 {
        let mut guard = APP_SERVER_HANDLE.lock().unwrap();
        *guard = Some(Arc::new(server));
        crate::global::mark_server_started(); // uptime 的起点
        crate::log_info!("App server started successfully on {}:{} with root directory: {}", host_str, actual_port, root_path);
    } else {
        crate::log_error!("Failed to start app server on {}:{}", host_str, port);
    }
    
    actual_port as c_int
}

/// 启动 App HTTP 服务器 (带配置)
/// 
/// root_dir 现在从 config_json 中的 "root_dir" 字段读取（可选）
/// 
/// # Returns
/// 成功返回实际端口号（>0），失败返回 <=0（注意：不是 bool）
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

    // 解析 host 参数
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
        crate::global::mark_server_started(); // uptime 的起点
        crate::log_info!("App Server started successfully on {}:{}", host_str, actual_port);
    } else {
        crate::log_error!("Failed to start App server on {}:{}", host_str, port);
    }
    
    actual_port as c_int
}

/// 停止 App HTTP 服务器
#[no_mangle]
pub extern "C" fn stop_app_server() {
    crate::log_info!("Stopping App HTTP server...");
    let mut guard = APP_SERVER_HANDLE.lock().unwrap();
    if let Some(_server) = guard.take() {
        crate::log_info!("App server stopped successfully");
    }
}

/// 获取服务器统计信息
///
/// # Returns
/// JSON 格式的统计信息字符串。
///
/// ⚠️ **内存所有权**：本函数返回**堆分配**的字符串，调用方**必须**用 `free_string`
/// 释放。改之前返回的是 `'static` 字面量的指针（那时**不能**释放）——语义已经翻转，
/// C++ 侧 `HybridHttpServer::getStats` 是唯一调用点，已同步改为解析后释放。
///
/// 字段口径（哪些是真的、哪些恒 0、为什么）：
/// - `status`            恒 `"running"`。既有调用方依赖它
///   （`tests/integration_c_api.rs::test_get_server_stats_integration` 断言含
///   `status`/`running`），所以**必须保留**。
/// - `totalRequests`     **真实**：三个 server 的请求入口各加一次。
/// - `errorCount`        **真实**：服务器自身没能产出响应的次数（等 JS 超时 /
///                       响应通道被关闭）。JS / 插件主动返回的 4xx/5xx **不算**。
/// - `uptime`            **真实**：最近一次 `start_*` 至今的秒数。
/// - `activeConnections` 恒 0。hyper 0.14 的 `Server::serve` **不暴露每连接的钩子**，
///                       要真实统计得自己写 accept 循环 + `serve_connection`，
///                       超出本任务范围。
/// - `bytesSent`/`bytesReceived` 恒 0。响应体的出口不止一个（回调路径走
///                       `send_response`，插件路径由 hyper 直接写回），
///                       只统计其中一部分会得到一个「看起来真实、其实残缺」的数字，
///                       **比 0 更糟**。
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

    // into_raw 把所有权交给调用方（配 free_string）
    CString::new(json).unwrap_or_default().into_raw() as *const c_char
}

/// 释放 HttpRequest 结构体
/// 
/// # Safety
/// 此函数会释放传入的 HttpRequest 指针及其所有字段
#[no_mangle]
pub unsafe extern "C" fn free_http_request(request: *mut HttpRequest) {
    if request.is_null() {
        return;
    }
    
    let req = Box::from_raw(request);
    
    // 释放所有 C 字符串
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
        // body 使用 Vec 分配，需要用 Vec 来释放
        let _ = Vec::from_raw_parts(req.body as *mut u8, req.body_len as usize, req.body_len as usize);
    }
}

/// 释放 C 字符串
/// 
/// # Safety
/// 此函数会释放传入的 C 字符串指针
#[no_mangle]
pub unsafe extern "C" fn free_string(s: *mut c_char) {
    if !s.is_null() {
        let _ = CString::from_raw(s);
    }
}

/// 分块读取请求 body
/// 
/// 从指定请求的 body 中读取数据到缓冲区
/// 
/// # Arguments
/// * `request_id` - 请求 ID
/// * `buffer` - 用于存储数据的缓冲区
/// * `buffer_size` - 缓冲区大小
/// 
/// # Returns
/// 返回实际读取的字节数，如果返回 0 表示已读取完毕，返回 -1 表示错误
#[no_mangle]
pub extern "C" fn read_request_body_chunk(
    request_id: *const c_char,
    buffer: *mut c_char,
    buffer_size: c_int,
) -> c_int {
    unsafe {
        // 解析 request_id
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
                // 已经读取完毕
                return 0;
            }
            
            let mut to_read = std::cmp::min(remaining, buffer_size as usize);
            if body_data.read_position + to_read < body_data.data.len() && to_read >= 4 {
                let end = body_data.read_position + to_read;
                // 如果切点后面的字节是 UTF-8 续字节 (10xxxxxx)，回退到多字节字符起点之前，避免截断 UTF-8 字符
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
            
            // 如果读取完毕，从映射表中移除
            if body_data.read_position >= body_data.data.len() {
                // 保留在映射表中，让调用者可以重新读取或稍后清理
            }
            
            to_read as c_int
        } else {
            crate::log_error!("read_request_body_chunk: no body found for request {}", request_id_str);
            -1
        }
    }
}

/// 分块写入响应 body
/// 
/// 向指定请求的响应流中写入数据块
/// 
/// # Arguments
/// * `request_id` - 请求 ID
/// * `chunk` - 要写入的数据块
/// * `chunk_len` - 数据块长度
/// 
/// # Returns
/// 如果写入成功返回 true，否则返回 false
#[no_mangle]
pub extern "C" fn write_response_chunk(
    request_id: *const c_char,
    chunk: *const c_char,
    chunk_len: c_int,
) -> bool {
    unsafe {
        // 解析 request_id
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
        
        // 获取数据
        let chunk_data = if chunk_len > 0 {
            let chunk_slice = std::slice::from_raw_parts(chunk as *const u8, chunk_len as usize);
            chunk_slice.to_vec()
        } else {
            Vec::new()
        };
        
        // 获取或创建块累积器
        let mut accumulators = CHUNK_ACCUMULATORS.lock().unwrap();
        let accumulator = accumulators.entry(request_id_str.clone())
            .or_insert_with(ResponseChunkAccumulator::new);
        
        // 添加块到累积器
        accumulator.add_chunk(chunk_data);
        
        crate::log_debug!("write_response_chunk: accumulated {} bytes for request {} (total chunks: {})", 
                 chunk_len, request_id_str, accumulator.chunks.len());
        
        true
    }
}

/// 结束响应流
/// 
/// 完成指定请求的响应流，关闭发送通道
/// 
/// # Arguments
/// * `request_id` - 请求 ID
/// * `status_code` - HTTP 状态码
/// * `headers_json` - 响应头 JSON 字符串
/// 
/// # Returns
/// 如果成功结束响应返回 true，否则返回 false
#[no_mangle]
pub extern "C" fn end_response(
    request_id: *const c_char,
    status_code: c_int,
    headers_json: *const c_char,
) -> bool {
    unsafe {
        // 解析 request_id
        let request_id_str = if request_id.is_null() {
            crate::log_error!("end_response: request_id is null");
            return false;
        } else {
            CStr::from_ptr(request_id).to_string_lossy().into_owned()
        };
        
        // 解析 headers
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

        // 检查是否有累积的块
        let accumulator = {
            let mut accumulators = CHUNK_ACCUMULATORS.lock().unwrap();
            accumulators.remove(&request_id_str)
        };
        
        // 如果有累积的块，发送累积的响应
        if let Some(accumulator) = accumulator {
            // 使用传入的状态码和头部，以及累积的体数据
            let body: Vec<u8> = accumulator.chunks.into_iter().flatten().collect();
            let response_data = ResponseData {
                status_code: status_code as u16,
                headers,
                body,
            };
            
            crate::log_debug!("end_response: sending accumulated response for request {} ({} bytes)", 
                     request_id_str, response_data.body.len());
            
            // 从响应通道中取出 sender 并发送
            let sender = {
                let mut channels = RESPONSE_CHANNELS.lock().unwrap();
                channels.remove(&request_id_str)
            };
            
            // 清理请求体数据
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
            // 没有累积的块，尝试直接发送空响应（或仅头部）
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
            
             // 清理请求体
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
                     // 清理资源（如果上面的 remove 没找到，这里可能 redundant，但安全）
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

/// WebSocket 事件类型 (C API)
#[repr(C)]
pub enum CWebSocketEventType {
    Open = 1,
    Message = 2,
    Close = 3,
    Error = 4,
}

/// WebSocket 事件结构体 (C API)
#[repr(C)]
pub struct CWebSocketEvent {
    pub connection_id: *const c_char,
    pub event_type: c_int,
    pub path: *const c_char,          // 连接路径
    pub query: *const c_char,         // 查询字符串
    pub headers_json: *const c_char,  // HTTP 头 (JSON 格式)
    pub text_data: *const c_char,
    pub text_len: c_int,
    pub binary_data: *const c_char,
    pub binary_len: c_int,
    pub close_code: c_int,
    pub close_reason: *const c_char,
    pub error_message: *const c_char,  // 错误信息 (仅 Error 事件)
}

/// WebSocket 事件回调函数类型 (C API)
pub type CWebSocketCallback = extern "C" fn(*const CWebSocketEvent);

/// 线程安全的函数指针包装器
struct SendSyncCallback(CWebSocketCallback);
unsafe impl Send for SendSyncCallback {}
unsafe impl Sync for SendSyncCallback {}

/// 全局 WebSocket 回调
static WEBSOCKET_CALLBACK: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

/// 将 C 回调挂载到单个 WebSocketPlugin 实例
fn attach_callback_to_plugin(plugin: &WebSocketPlugin, callback: CWebSocketCallback) {
    let wrapped_callback = SendSyncCallback(callback);
    plugin.set_event_callback(move |event: WebSocketEvent| {
        crate::log_info!("WebSocket event received: type={:?}, conn_id={}", event.event_type, event.connection_id);
        let callback_fn = wrapped_callback.0;
        
        // 转换事件类型
        let event_type = match event.event_type {
            WebSocketEventType::Open => 1,
            WebSocketEventType::Message => 2,
            WebSocketEventType::Close => 3,
            WebSocketEventType::Error => 4,
        };
        
        // 创建 C 字符串
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

/// 若已提前调用过 set_websocket_callback，则将其挂载到新注册的插件上
pub(crate) fn apply_saved_websocket_callback(plugin: &WebSocketPlugin) {
    let ptr = WEBSOCKET_CALLBACK.load(Ordering::SeqCst);
    if !ptr.is_null() {
        let callback: CWebSocketCallback = unsafe { std::mem::transmute(ptr) };
        attach_callback_to_plugin(plugin, callback);
    }
}

/// 设置 WebSocket 事件回调
/// 
/// @param callback WebSocket 事件回调函数
#[no_mangle]
pub extern "C" fn set_websocket_callback(callback: CWebSocketCallback) {
    crate::log_info!("Setting WebSocket callback");
    
    // 存储回调指针（无论是否在 server 启动前调用，新插件注册时都会自动挂载）
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

/// 发送 WebSocket 文本消息
/// 
/// @param connection_id 连接 ID
/// @param message 要发送的文本消息
/// @return 发送成功返回 true
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

/// 发送 WebSocket 二进制消息
/// 
/// @param connection_id 连接 ID
/// @param data 要发送的二进制数据
/// @param len 数据长度
/// @return 发送成功返回 true
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

/// 关闭 WebSocket 连接
/// 
/// @param connection_id 连接 ID
/// @param code 关闭代码 (1000 = 正常关闭)
/// @param reason 关闭原因
/// @return 关闭成功返回 true
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


/// 设置全局 CORS 配置
///
/// # Arguments
/// * `config_json` - CORS 配置 JSON 字符串："true" 启用默认配置，对象自定义，
///   传入 NULL 或 "false" 关闭
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
