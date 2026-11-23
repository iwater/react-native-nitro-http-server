use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::collections::HashMap;
use std::ffi::{CString};
use std::os::raw::{c_int};
use std::time::Duration;

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, StatusCode};
use tokio::runtime::Runtime;
use tokio::sync::{oneshot};
use once_cell::sync::Lazy;

use crate::types::{RequestCallback, HttpRequest, ResponseData, RequestBodyData};
use crate::global::{RESPONSE_CHANNELS, REQUEST_BODIES};

/// 进程内请求序号。
///
/// `request_id` 的唯一用途是**同进程内**把 JS 侧拿到的 id 与 Rust 侧的
/// `RESPONSE_CHANNELS` / `REQUEST_BODIES` 条目配对（见 `send_response` 等入口），
/// 既不落盘、也不跨进程可见，因此**不需要 UUID 的全局唯一性**。
/// 单调计数器比 `Uuid::new_v4()` 更合适：省掉每次 16 字节 `getentropy`
/// 系统调用与 36 字符 String（A 路径 CPU 剖面实测 `getentropy` 占 4.7%）。
static REQUEST_SEQ: AtomicU64 = AtomicU64::new(1);

pub struct HttpServer {
    pub port: u16,
    pub host: String,
    pub callback: RequestCallback,
    pub runtime: Option<Runtime>,
}

pub static SERVER_HANDLE: Lazy<Mutex<Option<Arc<HttpServer>>>> = Lazy::new(|| Mutex::new(None));

impl HttpServer {
    pub fn new(port: u16, host: String, callback: RequestCallback) -> Self {
        Self {
            port,
            host,
            callback,
            runtime: None,
        }
    }
    
    /// 启动服务器，返回实际监听的端口（0 = 失败）。
    ///
    /// `worker_threads`：tokio worker 线程数。
    /// - `0` = 按 CPU 核数（tokio 默认）
    /// - `>0` = 指定线程数
    ///
    /// 该 runtime 在每次 `start()` 时新建、`stop()` 时随 `HttpServer` 一起销毁，
    /// 所以**每次启动都可以改**（与 nitro-net 的全局 `OnceCell` runtime 不同）。
    ///
    /// 实测（10 核机，A 路径 native fast path）：默认 10 个 worker 是**过配** ——
    /// 各 worker 只有 14~23% 忙碌，而全局 Mutex（`RESPONSE_CHANNELS` /
    /// `REQUEST_BODIES`）竞争 + 跨线程唤醒合计占 11.4% CPU。降到 2 个后
    /// 高并发吞吐 **+13%**、每请求 CPU **−40%**。
    /// ⚠️ 但线程越少，并发容纳能力越低：handler 里有慢 `await`（DB/网络）时会排队。
    pub fn start(&mut self, worker_threads: u32) -> u16 {
        let callback = self.callback;
        let host = self.host.clone();
        let port = self.port;
        
        // 解析 host 字符串为 IP 地址
        let addr = crate::utils::resolve_bind_addr(&host, port);

        // 先创建 Runtime，因为 AddrIncoming::bind 需要在 Tokio Runtime 上下文中运行
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        if worker_threads > 0 {
            builder.worker_threads(worker_threads as usize);
            crate::log_info!("HTTP server runtime: {} worker threads", worker_threads);
        } else {
            crate::log_info!("HTTP server runtime: default worker threads (CPU cores)");
        }
        let rt = match builder.enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                crate::log_error!("Failed to create runtime: {}", e);
                return 0;
            }
        };

        // 进入 Runtime 上下文，确保 AddrIncoming::bind 可以找到 reactor
        let _guard = rt.enter();

        // 使用 try_bind 风格的 AddrIncoming::bind 避免 panic
        let incoming = match hyper::server::conn::AddrIncoming::bind(&addr) {
            Ok(inc) => inc,
            Err(e) => {
                crate::log_error!("Failed to bind to {}: {}", addr, e);
                return 0;
            }
        };

        let actual_port = incoming.local_addr().port();
        self.port = actual_port;
        
        rt.spawn(async move {
            if let Err(e) = Self::run_server_with_incoming(incoming, callback).await {
                crate::log_error!("Server error: {}", e);
            }
        });
        
        self.runtime = Some(rt);
        actual_port
    }
    
    async fn run_server_with_incoming(incoming: hyper::server::conn::AddrIncoming, callback: RequestCallback) -> Result<(), hyper::Error> {
        let addr = incoming.local_addr();
        let make_svc = make_service_fn(move |_conn| {
            let callback = callback;
            async move {
                Ok::<_, hyper::Error>(service_fn(move |req| {
                    handle_request(req, callback)
                }))
            }
        });

        let server = Server::builder(incoming).serve(make_svc);
        crate::log_info!("HTTP Server running on http://{}", addr);
        server.await
    }
    
    #[allow(dead_code)]
    pub fn stop(&mut self) {
        self.runtime = None;
    }
}

async fn handle_request(
    req: Request<Body>,
    callback: RequestCallback,
) -> Result<Response<Body>, hyper::Error> {
    let origin = req.headers().get(hyper::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // preflight 直接应答，不回调 JS
    if let Some(resp) = crate::cors::preflight_response(&req) {
        return Ok(resp);
    }

    let mut resp = handle_request_inner(req, callback).await?;
    crate::cors::apply_cors(&mut resp, origin.as_deref());
    Ok(resp)
}

async fn handle_request_inner(
    req: Request<Body>,
    callback: RequestCallback,
) -> Result<Response<Body>, hyper::Error> {
    // getStats 的 totalRequests：每个进入处理的请求加一次（含插件/回调两条路）
    crate::global::TOTAL_REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let method = req.method().to_string();
    // 使用 path_and_query() 获取完整路径（包含 query 字符串）
    let path = req.uri().path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    
    // 收集头部
    let mut headers = HashMap::new();
    for (key, value) in req.headers() {
        headers.insert(key.to_string(), value.to_str().unwrap_or("").to_string());
    }
    
    // 读取请求体（带全局上限；超限直接 413，**不回调 JS**）
    let body_bytes = match crate::utils::read_body_limited(
        req.into_body(),
        crate::global::MAX_BODY_SIZE.load(std::sync::atomic::Ordering::Relaxed),
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(resp) => return Ok(resp),
    };
    
    // 生成请求 ID（进程内单调序号，见 REQUEST_SEQ 的说明）
    let request_id = REQUEST_SEQ.fetch_add(1, AtomicOrdering::Relaxed).to_string();
    
    // 将 headers 转换为 JSON 字符串
    let headers_json = match serde_json::to_string(&headers) {
        Ok(json) => json,
        Err(e) => {
            crate::log_error!("Failed to serialize headers: {}", e);
            String::from("{}")
        }
    };
    
    // 创建响应通道
    let (tx, rx) = oneshot::channel::<ResponseData>();
    
    // 将发送端存储到全局映射表
    {
        let mut channels = RESPONSE_CHANNELS.lock().unwrap();
        channels.insert(request_id.clone(), tx);
    }
    
    // 将请求体存储到全局映射表（用于分块读取）
    {
        let mut bodies = REQUEST_BODIES.lock().unwrap();
        bodies.insert(request_id.clone(), RequestBodyData {
            data: body_bytes.to_vec(),
            read_position: 0,
        });
    }
    
    // 兜底清理：条目一旦入表就交给 guard，无论后续是正常返回、超时、还是
    // future 被 hyper 丢弃（客户端断开），都不会留下残留
    let _state_guard = crate::global::RequestStateGuard::new(request_id.clone());

    // 创建 C 字符串
    let request_id_c = CString::new(request_id.clone()).unwrap_or_default();
    let method_c = CString::new(method).unwrap_or_default();
    let path_c = CString::new(path).unwrap_or_default();
    let headers_json_c = CString::new(headers_json).unwrap_or_default();
    
    // 对于 body，不能使用 CString，因为二进制数据可能包含 \0 字节
    // 直接分配内存并复制数据（capacity 契约见 utils::alloc_request_body）
    let (body_ptr, body_len) = crate::utils::alloc_request_body(&body_bytes);
    
    // 创建 HttpRequest 结构体
    let http_request = Box::new(HttpRequest {
        request_id: request_id_c.into_raw(),
        method: method_c.into_raw(),
        path: path_c.into_raw(),
        headers_json: headers_json_c.into_raw(),
        body: body_ptr,
        body_len: body_len as c_int,
    });
    
    // 回调到 C 代码
    callback(Box::into_raw(http_request));
    
    // 等待响应（超时可配，默认 30s；长轮询/SSE 需调大，见 REQUEST_TIMEOUT_SECS）
    let timeout_secs = crate::global::REQUEST_TIMEOUT_SECS.load(std::sync::atomic::Ordering::Relaxed);
    match tokio::time::timeout(Duration::from_secs(timeout_secs), rx).await {
        Ok(Ok(response_data)) => {
            // 收到响应，构建 HTTP 响应（非法 status/header 由 build_response 兜底，不 panic）
            Ok(crate::utils::build_response(
                response_data.status_code,
                response_data.headers,
                response_data.body,
            ))
        }
        Ok(Err(_)) => {
            // Channel 被关闭（发送端被 drop）
            crate::log_error!("Response channel closed for request {}", request_id);
            crate::global::ERROR_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            
            // 清理该请求的全部全局状态（channels + bodies + accumulators）
            crate::global::cleanup_request_state(&request_id);

            Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Response channel closed"))
                .unwrap())
        }
        Err(_) => {
            // 超时
            crate::log_error!("Response timeout for request {}", request_id);
            crate::global::ERROR_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            
            // 清理该请求的全部全局状态（channels + bodies + accumulators）
            crate::global::cleanup_request_state(&request_id);

            Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Request timeout"))
                .unwrap())
        }
    }
}
