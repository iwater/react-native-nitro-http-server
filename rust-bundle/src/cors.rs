// src/cors.rs - 全局 CORS 支持
use std::sync::Mutex;
use once_cell::sync::Lazy;
use hyper::{Body, Request, Response, StatusCode};
use hyper::header::HeaderValue;

#[derive(Debug, Clone, PartialEq)]
pub struct CorsConfig {
    pub origin: String,            // 默认 "*"
    pub methods: Vec<String>,      // 默认常见方法 + WebDAV 方法
    pub headers: Vec<String>,      // 默认 ["*"]
    pub credentials: bool,         // 默认 false
    pub max_age: Option<u32>,      // 默认 None
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            origin: "*".to_string(),
            methods: [
                "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS",
                // WebDAV 方法
                "PROPFIND", "PROPPATCH", "MKCOL", "COPY", "MOVE", "LOCK", "UNLOCK",
            ].iter().map(|s| s.to_string()).collect(),
            headers: vec!["*".to_string()],
            credentials: false,
            max_age: None,
        }
    }
}

// 全局 CORS 配置，None 表示未启用
pub static CORS_CONFIG: Lazy<Mutex<Option<CorsConfig>>> = Lazy::new(|| Mutex::new(None));

/// 解析 CORS 配置 JSON："true" → 默认配置；对象 → 字段填充缺省值；其他 → None（关闭）
pub fn cors_config_from_json(json: &str) -> Option<CorsConfig> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    match &value {
        serde_json::Value::Bool(true) => Some(CorsConfig::default()),
        serde_json::Value::Object(_) => {
            let mut cfg = CorsConfig::default();
            if let Some(origin) = value.get("origin").and_then(|v| v.as_str()) {
                cfg.origin = origin.to_string();
            }
            if let Some(methods) = value.get("methods").and_then(|v| v.as_array()) {
                let list: Vec<String> = methods.iter()
                    .filter_map(|m| m.as_str().map(String::from)).collect();
                if !list.is_empty() { cfg.methods = list; }
            }
            if let Some(headers) = value.get("headers").and_then(|v| v.as_array()) {
                let list: Vec<String> = headers.iter()
                    .filter_map(|h| h.as_str().map(String::from)).collect();
                if !list.is_empty() { cfg.headers = list; }
            }
            if let Some(credentials) = value.get("credentials").and_then(|v| v.as_bool()) {
                cfg.credentials = credentials;
            }
            if let Some(max_age) = value.get("max_age").and_then(|v| v.as_u64()) {
                cfg.max_age = u32::try_from(max_age).ok();
            }
            Some(cfg)
        }
        _ => None,
    }
}

/// 设置全局 CORS 配置（None / 解析失败 → 关闭）
pub fn set_cors_config_json(json: Option<&str>) {
    let parsed = json.and_then(cors_config_from_json);
    let mut guard = CORS_CONFIG.lock().unwrap();
    *guard = parsed;
    match guard.as_ref() {
        Some(cfg) => crate::log_info!("CORS enabled: origin={}, credentials={}", cfg.origin, cfg.credentials),
        None => crate::log_info!("CORS disabled"),
    }
}

/// 提取请求的 Origin 头
fn request_origin(req: &Request<Body>) -> Option<String> {
    req.headers().get(hyper::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
}

/// 判断是否为 CORS preflight 请求
/// WebDAV 协议自身的 OPTIONS 不带 Access-Control-Request-Method，不会被误判
pub fn is_preflight(req: &Request<Body>) -> bool {
    req.method() == hyper::Method::OPTIONS
        && req.headers().contains_key(hyper::header::ORIGIN)
        && req.headers().contains_key("access-control-request-method")
}

/// 如果是 preflight 且 CORS 已启用，返回 204 应答；否则返回 None
pub fn preflight_response(req: &Request<Body>) -> Option<Response<Body>> {
    if !is_preflight(req) {
        return None;
    }
    let cfg = CORS_CONFIG.lock().unwrap().clone()?;

    // 回显请求的 Access-Control-Request-Headers（旧 WebView 不接受 "*"）；
    // 请求未带时回退为配置的 headers
    let allow_headers = req.headers().get("access-control-request-headers")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .unwrap_or_else(|| cfg.headers.join(", "));

    let allow_origin = if cfg.credentials {
        match request_origin(req) {
            Some(o) => o,
            None => return None,
        }
    } else {
        cfg.origin.clone()
    };

    let mut builder = Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header("access-control-allow-origin", allow_origin)
        .header("access-control-allow-methods", cfg.methods.join(", "))
        .header("access-control-allow-headers", allow_headers);
    if cfg.credentials {
        builder = builder.header("access-control-allow-credentials", "true");
    }
    if let Some(max_age) = cfg.max_age {
        builder = builder.header("access-control-max-age", max_age.to_string());
    }
    builder.body(Body::empty()).ok()
}

/// 在响应出口处注入 CORS 头。已存在 access-control-allow-origin 时不覆盖（JS 逃生口）。
pub fn apply_cors(resp: &mut Response<Body>, request_origin: Option<&str>) {
    let cfg = match CORS_CONFIG.lock().unwrap().clone() {
        Some(c) => c,
        None => return,
    };
    if resp.headers().contains_key("access-control-allow-origin") {
        return;
    }
    let allow_origin = if cfg.credentials {
        match request_origin {
            Some(o) => o.to_string(),
            None => return, // credentials 模式下无 Origin 不加头
        }
    } else {
        cfg.origin.clone()
    };
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&allow_origin) {
        headers.insert("access-control-allow-origin", v);
    }
    if cfg.credentials {
        headers.insert("access-control-allow-credentials", HeaderValue::from_static("true"));
    }
}
