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

/// Monotonic in-process request sequence number.
/// `request_id` only pairs JS responses with in-memory `RESPONSE_CHANNELS` / `REQUEST_BODIES`
/// entries in the same process, avoiding per-request `Uuid::new_v4()` entropy syscalls.
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
    
    /// Start the server and return the actual bound port (0 on failure).
    ///
    /// `worker_threads`: `0` uses tokio's CPU-core default; `>0` sets an explicit count.
    /// The runtime is created per `start()` and dropped with `HttpServer` on stop, so
    /// worker count can vary across starts. Fewer threads reduce concurrency under slow
    /// awaits, though lowering worker count on high-core machines can lower mutex contention.
    pub fn start(&mut self, worker_threads: u32) -> u16 {
        let callback = self.callback;
        let host = self.host.clone();
        let port = self.port;
        
        let addr = crate::utils::resolve_bind_addr(&host, port);

        // Build runtime before binding because AddrIncoming::bind requires an active Tokio reactor.
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

        let _guard = rt.enter();

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

    // Answer preflight directly without invoking JS.
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
    crate::global::TOTAL_REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let method = req.method().to_string();
    let path = req.uri().path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    
    let mut headers = HashMap::new();
    for (key, value) in req.headers() {
        headers.insert(key.to_string(), value.to_str().unwrap_or("").to_string());
    }
    
    // Enforce global body size limit; reject with 413 without invoking JS.
    let body_bytes = match crate::utils::read_body_limited(
        req.into_body(),
        crate::global::MAX_BODY_SIZE.load(std::sync::atomic::Ordering::Relaxed),
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(resp) => return Ok(resp),
    };
    
    let request_id = REQUEST_SEQ.fetch_add(1, AtomicOrdering::Relaxed).to_string();
    
    let headers_json = match serde_json::to_string(&headers) {
        Ok(json) => json,
        Err(e) => {
            crate::log_error!("Failed to serialize headers: {}", e);
            String::from("{}")
        }
    };
    
    let (tx, rx) = oneshot::channel::<ResponseData>();
    
    {
        let mut channels = RESPONSE_CHANNELS.lock().unwrap();
        channels.insert(request_id.clone(), tx);
    }
    
    {
        let mut bodies = REQUEST_BODIES.lock().unwrap();
        bodies.insert(request_id.clone(), RequestBodyData {
            data: body_bytes.to_vec(),
            read_position: 0,
        });
    }
    
    // Ensure global request maps are cleaned up on return, timeout, or hyper future drop.
    let _state_guard = crate::global::RequestStateGuard::new(request_id.clone());

    let request_id_c = CString::new(request_id.clone()).unwrap_or_default();
    let method_c = CString::new(method).unwrap_or_default();
    let path_c = CString::new(path).unwrap_or_default();
    let headers_json_c = CString::new(headers_json).unwrap_or_default();
    
    // Binary body may contain NUL bytes, so pass raw buffer + length instead of CString.
    let (body_ptr, body_len) = crate::utils::alloc_request_body(&body_bytes);
    
    let http_request = Box::new(HttpRequest {
        request_id: request_id_c.into_raw(),
        method: method_c.into_raw(),
        path: path_c.into_raw(),
        headers_json: headers_json_c.into_raw(),
        body: body_ptr,
        body_len: body_len as c_int,
    });
    
    callback(Box::into_raw(http_request));
    
    let timeout_secs = crate::global::REQUEST_TIMEOUT_SECS.load(std::sync::atomic::Ordering::Relaxed);
    match tokio::time::timeout(Duration::from_secs(timeout_secs), rx).await {
        Ok(Ok(response_data)) => {
            Ok(crate::utils::build_response(
                response_data.status_code,
                response_data.headers,
                response_data.body,
            ))
        }
        Ok(Err(_)) => {
            crate::log_error!("Response channel closed for request {}", request_id);
            crate::global::ERROR_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            
            crate::global::cleanup_request_state(&request_id);

            Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Response channel closed"))
                .unwrap())
        }
        Err(_) => {
            crate::log_error!("Response timeout for request {}", request_id);
            crate::global::ERROR_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            
            crate::global::cleanup_request_state(&request_id);

            Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Request timeout"))
                .unwrap())
        }
    }
}
