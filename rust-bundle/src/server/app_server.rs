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
        
        let config: Option<serde_json::Value> = if !config_json.is_empty() {
            serde_json::from_str(config_json).ok()
        } else {
            None
        };
        
        if let Some(ref cfg) = config {
            if let Some(v_val) = cfg.get("verbose") {
                let level = if let Some(b) = v_val.as_bool() {
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
                
                if level >= LOG_INFO {
                    crate::log_info!("Logging enabled at level: {}", level);
                }
            }

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

            // Warn if `host` was placed inside the config object instead of the 4th start argument.
            if let Some(v) = cfg.get("host") {
                crate::log_warn!(
                    "`host` inside the config object is IGNORED (got {:?}). Pass it as the 4th \
                     argument instead, e.g. start(port, handler, config, \"0.0.0.0\").",
                    v
                );
            }

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

            // Custom MIME types (global)
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

            if let Some(mounts) = cfg.get("mounts").and_then(|v| v.as_array()) {
                for mount in mounts {
                    let type_str = mount.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let path = mount.get("path").and_then(|v| v.as_str()).unwrap_or("/");
                    
                    match type_str {
                        "webdav" => {
                            if let Some(root) = mount.get("root").and_then(|v| v.as_str()) {
                                let plugin = WebDavPlugin::new(path.to_string(), root.to_string());
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
                                // Register globally for C API access across multiple mount points.
                                register_global_websocket_plugin(plugin.clone());
                                
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

            // Append top-level root_dir as fallback static mount at `/` after explicit mounts.
            let root_dir_val = cfg.get("root_dir").and_then(|v| v.as_str());
            
            if let Some(root) = root_dir_val {
                let root_path = PathBuf::from(root);
                if root_path.exists() && root_path.is_dir() {
                    let plugin = StaticFilePlugin::new("/".to_string(), root_path.clone());
                    
                    if let Some(static_cfg) = cfg.get("static_file") {
                         if let Err(e) = plugin.on_load(static_cfg) {
                             crate::log_error!("Failed to configure default Static plugin: {}", e);
                         }
                    }
                    
                    plugin_manager.register_plugin(Box::new(plugin));
                    crate::log_info!("Mounted Default Static Root: / -> {:?}", root_path);
                } else {
                     crate::log_error!("Warning: root_dir '{}' is invalid or not accessible", root);
                }
            }
        }
        
        // Retain root_dir for X-Accel-Redirect resolution in callback responses.
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
        
        let addr = crate::utils::resolve_bind_addr(&host, port);

        // Build runtime before binding because AddrIncoming::bind requires an active Tokio reactor.
        let rt = match Runtime::new() {
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
    crate::global::TOTAL_REQUESTS.fetch_add(1, Ordering::Relaxed);

    // Let mounted plugins handle the request first.
    let req = match plugin_manager.handle_request(req).await {
        Ok(resp) => return Ok(resp),
        Err(req) => req,
    };
    
    handle_app_callback_request(req, root_dir, callback).await
}

async fn handle_app_callback_request(
    req: Request<Body>,
    root_dir: Option<PathBuf>,
    callback: RequestCallback,
) -> Result<Response<Body>, hyper::Error> {
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
        MAX_BODY_SIZE.load(Ordering::Relaxed),
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(resp) => return Ok(resp),
    };
    
    let request_id = Uuid::new_v4().to_string();
    
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
    
    let timeout_secs = crate::global::REQUEST_TIMEOUT_SECS.load(Ordering::Relaxed);
    match tokio::time::timeout(Duration::from_secs(timeout_secs), rx).await {
        Ok(Ok(response_data)) => {
            if let Some(accel_path) = response_data.headers.get("X-Accel-Redirect")
                .or_else(|| response_data.headers.get("x-accel-redirect")) {
                
                // Lazily canonicalize root_dir only when X-Accel-Redirect is present,
                // ensuring symlinked/".." roots are accurately checked for path traversal.
                if let Some(canonical_root) = root_dir.as_ref().and_then(|r| r.canonicalize().ok()) {
                    let sanitized_path = accel_path.trim_start_matches('/');
                    let file_path = canonical_root.join(sanitized_path);

                    if let Ok(canonical_file) = file_path.canonicalize() {
                        if canonical_file.starts_with(&canonical_root) && canonical_file.is_file() {
                            if let Ok(contents) = tokio::fs::read(&canonical_file).await {
                                let content_type = get_content_type(&canonical_file);

                                let mut out_headers = HashMap::new();
                                for (key, value) in &response_data.headers {
                                    let key_lower = key.to_lowercase();
                                    if key_lower != "x-accel-redirect" && key_lower != "content-type" {
                                        out_headers.insert(key.clone(), value.clone());
                                    }
                                }
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
                    
                    return Ok(Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::from("X-Accel-Redirect: File not found"))
                        .unwrap());
                } else {
                    crate::log_warn!("Warning: X-Accel-Redirect header '{}' ignored because root_dir is not configured or invalid", accel_path);
                }
            }
            
            let status_code = response_data.status_code;

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
            crate::log_error!("Response channel closed for request {}", request_id);
            crate::global::ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
            
            crate::global::cleanup_request_state(&request_id);

            Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Response channel closed"))
                .unwrap())
        }
        Err(_) => {
            crate::log_error!("Response timeout for request {}", request_id);
            crate::global::ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
            
            crate::global::cleanup_request_state(&request_id);

            Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Request timeout"))
                .unwrap())
        }
    }
}
