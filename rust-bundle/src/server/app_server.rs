use std::sync::{Arc, Mutex};
use std::sync::atomic::{Ordering};
use std::collections::HashMap;
use std::path::{PathBuf};
use std::ffi::{CString};
use std::os::raw::{c_int};
use std::time::Duration;

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, StatusCode};
use tokio::runtime::Runtime;
use tokio::sync::{oneshot};
use once_cell::sync::Lazy;
use uuid::Uuid;

use crate::types::{RequestCallback, HttpRequest, ResponseData, RequestBodyData};
use crate::global::{RESPONSE_CHANNELS, REQUEST_BODIES, VERBOSE, CUSTOM_MIME_TYPES, REQUEST_TIMEOUT_SECS, MAX_BODY_SIZE};
use crate::logger::{LOG_OFF, LOG_ERROR, LOG_WARN, LOG_INFO, LOG_DEBUG};
use crate::utils::get_content_type;
use crate::plugins::Plugin;
use crate::plugins::manager::PluginManager;
use crate::plugins::webdav::WebDavPlugin;
use crate::plugins::zip_mount::ZipMountPlugin;
use crate::plugins::static_file::StaticFilePlugin;
use crate::plugins::upload::UploadPlugin;
use crate::plugins::buffer_upload::BufferUploadPlugin;
use crate::plugins::rewrite::RewritePlugin;
use crate::plugins::websocket::WebSocketPlugin;

pub struct AppHttpServer {
    pub port: u16,
    pub host: String,
    pub root_dir: Option<PathBuf>,
    pub callback: RequestCallback,
    pub plugin_manager: Arc<PluginManager>,
    pub runtime: Option<Runtime>,
}

pub static APP_SERVER_HANDLE: Lazy<Mutex<Option<Arc<AppHttpServer>>>> = Lazy::new(|| Mutex::new(None));

impl AppHttpServer {
    pub fn new(port: u16, host: String, callback: RequestCallback, config_json: &str) -> Self {
        let mut plugin_manager = PluginManager::new();
        crate::plugins::websocket::clear_global_websocket_plugins();
        
        // 解析配置
        let config: Option<serde_json::Value> = if !config_json.is_empty() {
            serde_json::from_str(config_json).ok()
        } else {
            None
        };
        
        if let Some(ref cfg) = config {
            // 0. 解析 verbose 配置
            if let Some(v_val) = cfg.get("verbose") {
                let level = if let Some(b) = v_val.as_bool() {
                    // 兼容 boolean: true -> Info, false -> Off (or Error?)
                    if b { LOG_INFO } else { LOG_OFF }
                } else if let Some(s) = v_val.as_str() {
                    match s.to_lowercase().as_str() {
                        "off" | "none" => LOG_OFF,
                        "error" => LOG_ERROR,
                        "warn" | "warning" => LOG_WARN,
                        "info" => LOG_INFO,
                        "debug" => LOG_DEBUG,
                        _ => LOG_INFO, // Default to info if unknown string
                    }
                } else if let Some(n) = v_val.as_u64() {
                    n as u8
                } else {
                    LOG_OFF
                };
                
                VERBOSE.store(level, Ordering::Relaxed);
                
                // Self-log active level
                if level >= LOG_INFO {
                    crate::log_info!("Logging enabled at level: {}", level);
                }
            }

            // 0.1 解析响应超时（秒）：长轮询 / SSE 场景需要调大默认的 30s
            if let Some(v) = cfg.get("request_timeout_secs") {
                match v.as_u64() {
                    Some(secs) if secs > 0 => {
                        REQUEST_TIMEOUT_SECS.store(secs, Ordering::Relaxed);
                        crate::log_info!("Request timeout set to {}s", secs);
                    }
                    _ => crate::log_warn!(
                        "Invalid request_timeout_secs {:?}, must be a positive integer; keeping {}s",
                        v,
                        REQUEST_TIMEOUT_SECS.load(Ordering::Relaxed)
                    ),
                }
            }

            // `host` 是**启动参数**（第 4 个），不是 config 字段 —— 放进 config 会被
            // 静默忽略。GitHub issue #7 就是踩了这个（报「host: '0.0.0.0' 无效」），
            // 所以这里显式提醒一次，别让下一个人再花时间。
            if let Some(v) = cfg.get("host") {
                crate::log_warn!(
                    "`host` inside the config object is IGNORED (got {:?}). Pass it as the 4th \
                     argument instead, e.g. start(port, handler, config, \"0.0.0.0\").",
                    v
                );
            }

            // 0.2 解析请求体上限（字节）：默认 100MB，大文件上传场景可调大
            if let Some(v) = cfg.get("max_body_size") {
                match v.as_u64() {
                    Some(size) if size > 0 => {
                        MAX_BODY_SIZE.store(size, Ordering::Relaxed);
                        crate::log_info!("Max request body size set to {} bytes", size);
                    }
                    _ => crate::log_warn!(
                        "Invalid max_body_size {:?}, must be a positive integer; keeping {} bytes",
                        v,
                        MAX_BODY_SIZE.load(Ordering::Relaxed)
                    ),
                }
            }

            // 1. 解析自定义 MIME types 配置（全局）
            if let Some(mime_types) = cfg.get("mime_types").and_then(|v| v.as_object()) {
                let mut custom_types = CUSTOM_MIME_TYPES.lock().unwrap();
                for (ext, mime_type) in mime_types {
                    if let Some(mime_str) = mime_type.as_str() {
                        let ext_clean = ext.trim_start_matches('.').to_lowercase();
                        custom_types.insert(ext_clean, mime_str.to_string());
                        crate::log_debug!("Registered custom MIME type: .{} -> {}", ext.trim_start_matches('.'), mime_str);
                    }
                }
            }

            // 2. 解析挂载列表 (mounts)
            if let Some(mounts) = cfg.get("mounts").and_then(|v| v.as_array()) {
                for mount in mounts {
                    let type_str = mount.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let path = mount.get("path").and_then(|v| v.as_str()).unwrap_or("/");
                    
                    match type_str {
                        "webdav" => {
                            if let Some(root) = mount.get("root").and_then(|v| v.as_str()) {
                                let plugin = WebDavPlugin::new(path.to_string(), root.to_string());
                                // 允许 on_load 处理其他配置 (如有)
                                if let Err(e) = plugin.on_load(mount) {
                                    crate::log_error!("Failed to configure WebDav plugin: {}", e);
                                }
                                plugin_manager.register_plugin(Box::new(plugin));
                                crate::log_info!("Mounted WebDAV: {} -> {}", path, root);
                            }
                        },
                        "zip" => {
                            if let Some(zip_file) = mount.get("zip_file").and_then(|v| v.as_str()) {
                                let plugin = ZipMountPlugin::new(path.to_string(), zip_file.to_string());
                                if let Err(e) = plugin.on_load(mount) {
                                    crate::log_error!("Failed to configure Zip plugin: {}", e);
                                }
                                plugin_manager.register_plugin(Box::new(plugin));
                                crate::log_info!("Mounted Zip: {} -> {}", path, zip_file);
                            }
                        },
                        "static" | "alias" => {
                            if let Some(root) = mount.get("root").and_then(|v| v.as_str()) {
                                let plugin = StaticFilePlugin::new(path.to_string(), PathBuf::from(root));
                                if let Err(e) = plugin.on_load(mount) {
                                    crate::log_error!("Failed to configure Static plugin: {}", e);
                                }
                                plugin_manager.register_plugin(Box::new(plugin));
                                crate::log_info!("Mounted Static: {} -> {}", path, root);
                            }
                        },
                        "upload" => {
                            if let Some(temp_dir) = mount.get("temp_dir").and_then(|v| v.as_str()) {
                                let plugin = UploadPlugin::new(path.to_string(), temp_dir.to_string());
                                if let Err(e) = plugin.on_load(mount) {
                                    crate::log_error!("Failed to configure Upload plugin: {}", e);
                                }
                                plugin_manager.register_plugin(Box::new(plugin));
                                crate::log_info!("Mounted Upload: {} -> {}", path, temp_dir);
                            }
                        },
                        "buffer_upload" => {
                            let plugin = BufferUploadPlugin::new(path.to_string());
                            if let Err(e) = plugin.on_load(mount) {
                                    crate::log_error!("Failed to configure BufferUpload plugin: {}", e);
                            }
                            plugin_manager.register_plugin(Box::new(plugin));
                            crate::log_info!("Mounted BufferUpload: {}", path);
                        },
                        "rewrite" => {
                            let plugin = RewritePlugin::new();
                            if let Err(e) = plugin.on_load(mount) {
                                crate::log_error!("Failed to configure Rewrite plugin: {}", e);
                            } else {
                                plugin_manager.register_plugin(Box::new(plugin));
                                crate::log_info!("Mounted Rewrite plugin");
                            }
                        },
                        "websocket" => {
                            use crate::plugins::websocket::register_global_websocket_plugin;
                            use std::sync::Arc;
                            
                            let plugin = Arc::new(WebSocketPlugin::new(path.to_string()));
                            if let Err(e) = plugin.on_load(mount) {
                                crate::log_error!("Failed to configure WebSocket plugin: {}", e);
                            } else {
                                // 注册全局实例以便 C API 访问（支持多挂载点）
                                register_global_websocket_plugin(plugin.clone());
                                
                                // 创建一个包装器用于 PluginManager
                                struct WebSocketPluginWrapper(Arc<WebSocketPlugin>);
                                
                                #[async_trait::async_trait]
                                impl crate::plugins::Plugin for WebSocketPluginWrapper {
                                    fn name(&self) -> &str { self.0.name() }
                                    fn on_load(&self, config: &serde_json::Value) -> Result<(), anyhow::Error> { 
                                        self.0.on_load(config) 
                                    }
                                    fn should_handle(&self, req: &hyper::Request<hyper::Body>) -> bool { 
                                        self.0.should_handle(req) 
                                    }
                                    async fn handle_request(&self, req: hyper::Request<hyper::Body>) 
                                        -> Result<crate::plugins::PluginAction, anyhow::Error> { 
                                        self.0.handle_request(req).await 
                                    }
                                    fn as_any(&self) -> &dyn std::any::Any { self }
                                }
                                
                                plugin_manager.register_plugin(Box::new(WebSocketPluginWrapper(plugin)));
                                crate::log_info!("Mounted WebSocket: {}", path);
                            }
                        },
                        _ => crate::log_error!("Unknown mount type: {}", type_str),
                    }
                }
            }

            // 3. 处理顶层 root_dir (作为默认静态服务，挂载在 /)
            // 只有当 mounts 没有显式覆盖 / 或者用户希望双重挂载时才会有用。
            // 这里的策略是：追加到链尾。
            let root_dir_val = cfg.get("root_dir").and_then(|v| v.as_str());
            
            if let Some(root) = root_dir_val {
                let root_path = PathBuf::from(root);
                if root_path.exists() && root_path.is_dir() {
                    let plugin = StaticFilePlugin::new("/".to_string(), root_path.clone());
                    
                    // 尝试应用旧的 static_file 配置块 (如果存在)
                    if let Some(static_cfg) = cfg.get("static_file") {
                         if let Err(e) = plugin.on_load(static_cfg) {
                             crate::log_error!("Failed to configure default Static plugin: {}", e);
                         }
                    }
                    
                    plugin_manager.register_plugin(Box::new(plugin));
                    crate::log_info!("Mounted Default Static Root: / -> {:?}", root_path);
                    
                    // 为了 AppHttpServer 的 X-Accel-Redirect 逻辑，我们需要保留一个主要的 root_dir
                    // 但这里的 self.root_dir 更多是给 callback 模式用的。
                } else {
                     crate::log_error!("Warning: root_dir '{}' is invalid or not accessible", root);
                }
            }
        }
        
        // 从配置解析 root_dir (用于 callback 的 X-Accel-Redirect)
        // 即使使用了 mounts，我们可能仍需要知道哪个是“主”root_dir，或者我们干脆废弃 callback 中的文件读取？
        // 原有逻辑：callback 返回 X-Accel-Redirect -> Server 读取 root_dir + path
        // 为了兼容，我们还是尝试解析 root_dir
        let root_dir: Option<PathBuf> = config.as_ref()
            .and_then(|c| c.get("root_dir"))
            .and_then(|v| v.as_str())
            .map(PathBuf::from);

        Self {
            port,
            host,
            root_dir,
            callback,
            plugin_manager: Arc::new(plugin_manager),
            runtime: None,
        }
    }
    
    pub fn start(&mut self) -> u16 {
        let host = self.host.clone();
        let port = self.port;
        let root_dir = self.root_dir.clone();
        let callback = self.callback;
        let plugin_manager = self.plugin_manager.clone();
        
        // 解析 host 字符串为 IP 地址
        let addr = crate::utils::resolve_bind_addr(&host, port);

        // 先创建 Runtime，因为 AddrIncoming::bind 需要在 Tokio Runtime 上下文中运行
        let rt = match Runtime::new() {
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
            if let Err(e) = Self::run_app_server_with_incoming(incoming, root_dir, callback, plugin_manager).await {
                crate::log_error!("App server error: {}", e);
            }
        });
        
        self.runtime = Some(rt);
        actual_port
    }
    
    async fn run_app_server_with_incoming(incoming: hyper::server::conn::AddrIncoming, root_dir: Option<PathBuf>, callback: RequestCallback, plugin_manager: Arc<PluginManager>) -> Result<(), hyper::Error> {
        let addr = incoming.local_addr();
        let make_svc = make_service_fn(move |_conn| {
            let root_dir = root_dir.clone();
            let callback = callback;
            let plugin_manager = plugin_manager.clone();
            async move {
                Ok::<_, hyper::Error>(service_fn(move |req| {
                    handle_app_request(req, root_dir.clone(), callback, plugin_manager.clone())
                }))
            }
        });

        let server = Server::builder(incoming).serve(make_svc);
        crate::log_info!("App HTTP Server running on http://{}", addr);
        server.await
    }
    
    #[allow(dead_code)]
    pub fn stop(&mut self) {
        self.runtime = None;
    }
}

// 处理 App 服务器请求（混合静态文件和回调）
async fn handle_app_request(
    req: Request<Body>,
    root_dir: Option<PathBuf>,
    callback: RequestCallback,
    plugin_manager: Arc<PluginManager>,
) -> Result<Response<Body>, hyper::Error> {
    let origin = req.headers().get(hyper::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    if let Some(resp) = crate::cors::preflight_response(&req) {
        return Ok(resp);
    }

    let mut resp = handle_app_request_inner(req, root_dir, callback, plugin_manager).await?;
    crate::cors::apply_cors(&mut resp, origin.as_deref());
    Ok(resp)
}

async fn handle_app_request_inner(
    req: Request<Body>,
    root_dir: Option<PathBuf>,
    callback: RequestCallback,
    plugin_manager: Arc<PluginManager>,
) -> Result<Response<Body>, hyper::Error> {
    // getStats 的 totalRequests：每个进入处理的请求加一次（含插件/回调两条路）
    crate::global::TOTAL_REQUESTS.fetch_add(1, Ordering::Relaxed);

    // 优先让插件处理请求（包括 StaticFilePlugin）
    let req = match plugin_manager.handle_request(req).await {
        Ok(resp) => return Ok(resp),
        Err(req) => req,
    };
    
    // 插件未处理，交给回调处理（支持 X-Accel-Redirect）
    handle_app_callback_request(req, root_dir, callback).await
}

// 处理 App 服务器的回调请求，支持 X-Accel-Redirect
async fn handle_app_callback_request(
    req: Request<Body>,
    root_dir: Option<PathBuf>,
    callback: RequestCallback,
) -> Result<Response<Body>, hyper::Error> {
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
        MAX_BODY_SIZE.load(Ordering::Relaxed),
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(resp) => return Ok(resp),
    };
    
    // 生成请求 ID
    let request_id = Uuid::new_v4().to_string();
    
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
    let timeout_secs = crate::global::REQUEST_TIMEOUT_SECS.load(Ordering::Relaxed);
    match tokio::time::timeout(Duration::from_secs(timeout_secs), rx).await {
        Ok(Ok(response_data)) => {
            // 检查是否有 X-Accel-Redirect 头
            if let Some(accel_path) = response_data.headers.get("X-Accel-Redirect")
                .or_else(|| response_data.headers.get("x-accel-redirect")) {
                
                // 惰性规范化：canonicalize 是一次 realpath syscall，只有真的带
                // X-Accel-Redirect 时才付这个代价（不能放在函数开头，否则每个回调
                // 请求都会多一次 syscall）。
                // 必须用规范化后的 root：否则 root_dir 含 symlink / ".." 时会把合法
                // 文件误判成越界（或反之放过真正的越界）。
                if let Some(canonical_root) = root_dir.as_ref().and_then(|r| r.canonicalize().ok()) {
                    // 尝试读取指定的文件
                    let sanitized_path = accel_path.trim_start_matches('/');
                    let file_path = canonical_root.join(sanitized_path);

                    // 安全检查：确保请求的文件在根目录内
                    if let Ok(canonical_file) = file_path.canonicalize() {
                        if canonical_file.starts_with(&canonical_root) && canonical_file.is_file() {
                            // 读取并返回文件
                            if let Ok(contents) = tokio::fs::read(&canonical_file).await {
                                let content_type = get_content_type(&canonical_file);

                                // 保留原始响应中的其他头（跳过 X-Accel-Redirect 和 Content-Type）
                                let mut out_headers = HashMap::new();
                                for (key, value) in &response_data.headers {
                                    let key_lower = key.to_lowercase();
                                    if key_lower != "x-accel-redirect" && key_lower != "content-type" {
                                        out_headers.insert(key.clone(), value.clone());
                                    }
                                }
                                // 设置正确的 Content-Type（用文件的 mime）
                                out_headers.insert("Content-Type".to_string(), content_type);

                                crate::log_debug!("X-Accel-Redirect: Serving file {:?} for request {}", canonical_file, request_id);

                                return Ok(crate::utils::build_response(
                                    response_data.status_code,
                                    out_headers,
                                    contents,
                                ));
                            } else {
                                crate::log_error!("X-Accel-Redirect: Failed to read file {:?}", file_path);
                            }
                        } else {
                            crate::log_error!("X-Accel-Redirect: File not found or outside root: {:?}", file_path);
                        }
                    } else {
                        crate::log_error!("X-Accel-Redirect: Failed to canonicalize file path: {:?}", file_path);
                    }
                    
                    // 如果 X-Accel-Redirect 文件不存在或读取失败，返回 404
                    return Ok(Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::from("X-Accel-Redirect: File not found"))
                        .unwrap());
                } else {
                    // root_dir 无效，忽略 X-Accel-Redirect 并打印警告
                    crate::log_warn!("Warning: X-Accel-Redirect header '{}' ignored because root_dir is not configured or invalid", accel_path);
                }
            }
            
            // 没有 X-Accel-Redirect 或 root_dir 无效，正常返回响应
            let status_code = response_data.status_code;

            // 添加响应头（跳过 X-Accel-Redirect）
            let mut out_headers = HashMap::new();
            for (key, value) in response_data.headers {
                if key.to_lowercase() != "x-accel-redirect" {
                    out_headers.insert(key, value);
                }
            }

            Ok(crate::utils::build_response(
                status_code,
                out_headers,
                response_data.body,
            ))
        }
        Ok(Err(_)) => {
            // Channel 被关闭（发送端被 drop）
            crate::log_error!("Response channel closed for request {}", request_id);
            crate::global::ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
            
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
            crate::global::ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
            
            // 清理该请求的全部全局状态（channels + bodies + accumulators）
            crate::global::cleanup_request_state(&request_id);

            Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Request timeout"))
                .unwrap())
        }
    }
}
