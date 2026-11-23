use std::path::Path;
use std::collections::HashMap;
use std::str::FromStr;
use std::os::raw::c_char;

use hyper::{Body, Response, StatusCode};
use hyper::header::{HeaderName, HeaderValue};

use crate::global::CUSTOM_MIME_TYPES;

/// 把请求体拷成一份交给 C 侧的内存，返回 `(ptr, len)`。
///
/// 契约：`free_http_request` 用 `Vec::from_raw_parts(ptr, len, len)` 回收，也就是
/// **假定 capacity == len**。所以这里必须用 `into_boxed_slice().into_vec()` 把多余
/// capacity 挤掉——依赖 `to_vec()` / `with_capacity` 恰好不多分配是 UB，那只是分配器
/// 的当前行为而不是契约（实测本机各尺寸下 to_vec 确实 cap==len，但换个分配器就可能不是）。
///
/// 空 body 返回 null 指针，与 `free_http_request` 里 `body_len > 0` 的判断对应。
pub fn alloc_request_body(body: &[u8]) -> (*mut c_char, usize) {
    if body.is_empty() {
        return (std::ptr::null_mut(), 0);
    }
    let mut v = normalize_capacity(body.to_vec());
    let ptr = v.as_mut_ptr() as *mut c_char;
    std::mem::forget(v); // 交给 C 侧，由 free_http_request 回收
    (ptr, body.len())
}

/// 把 Vec 的 capacity 压到等于 len。
///
/// 单独拆出来是因为：`Vec::from_raw_parts(ptr, len, len)` 会把 capacity **声明**成 len，
/// 所以从重建后的 Vec 上永远看不到真实 capacity —— 这个契约只能在 forget 之前测。
pub(crate) fn normalize_capacity(v: Vec<u8>) -> Vec<u8> {
    v.into_boxed_slice().into_vec()
}

/// 读取请求体，带**硬上限**。
///
/// 为什么不能用 `hyper::body::to_bytes`：hyper 0.14 的它**没有内建上限**
/// （`hyper-0.14.32/src/body/to_bytes.rs` 把 chunk 一路拼进一个 buffer），
/// 一个超大 body 就能把进程内存撑爆。
///
/// 两道闸，缺一不可：
/// 1. `content-length` 预检 —— 能提前拒绝，连 body 都不用读；
/// 2. **流式累积检查** —— 真正的兜底。客户端可以不给 content-length（chunked），
///    也可以谎报一个小的值，所以只有边读边数才靠得住。
///
/// 返回 `Err(Response)` 的语义是「直接用这个响应回掉请求」：超限 413，读取出错 400。
pub async fn read_body_limited(body: Body, max: u64) -> Result<Vec<u8>, Response<Body>> {
    use hyper::body::HttpBody;

    // 闸 1：content-length 预检
    if let Some(len) = body.size_hint().exact() {
        if len > max {
            return Err(payload_too_large());
        }
    }

    // 闸 2：边读边数
    let mut body = body;
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.map_err(|_| {
            Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from("Bad request body"))
                .unwrap()
        })?;
        if buf.len() as u64 + chunk.len() as u64 > max {
            return Err(payload_too_large());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

fn payload_too_large() -> Response<Body> {
    Response::builder()
        .status(StatusCode::PAYLOAD_TOO_LARGE)
        .body(Body::from("Payload Too Large"))
        .unwrap()
}

/// 安全构建 HTTP 响应：非法 status 回退 500，非法 header 跳过并告警，**绝不 panic**。
///
/// 背景：`Response::builder()` 的每一步都只是把错误记进 builder，真正 `.body()` 时才返回
/// Err。原先调用点直接 `.unwrap()`，JS 传入非法 statusCode（如 99）就会 panic，
/// hyper 丢连接 → 客户端收到 IncompleteMessage。
/// 至于 header：http 0.2 的 HeaderValue 允许 obs-text（0x80-0xFF，即非 ASCII），
/// 但**拒绝控制字符**（\0 \r \n）以及非法 header name，所以校验仍然必要。
pub fn build_response(
    status_code: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
) -> Response<Body> {
    let status = StatusCode::from_u16(status_code).unwrap_or_else(|_| {
        crate::log_warn!("Invalid status code {}, falling back to 500", status_code);
        StatusCode::INTERNAL_SERVER_ERROR
    });

    let mut builder = Response::builder().status(status);
    for (key, value) in &headers {
        match (HeaderName::from_str(key), HeaderValue::from_str(value)) {
            (Ok(name), Ok(value)) => {
                builder = builder.header(name, value);
            }
            _ => {
                crate::log_warn!("Dropping invalid response header: {}: {:?}", key, value);
            }
        }
    }

    builder.body(Body::from(body)).unwrap_or_else(|_| {
        // 走到这里只可能是 status/header 之外的构建错误（理论上不可达）；
        // 兜底返回一个一定能构建成功的 500。
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from("Internal Server Error"))
            .expect("fallback response must always build")
    })
}

// 根据文件扩展名获取 Content-Type
pub fn get_content_type(path: &Path) -> String {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        let ext_lower = ext.to_lowercase();
        
        // 优先查找自定义 MIME types
        if let Ok(custom_types) = CUSTOM_MIME_TYPES.lock() {
            if let Some(mime_type) = custom_types.get(&ext_lower) {
                return mime_type.clone();
            }
        }
        
        // 回退到内置 MIME types
        match ext_lower.as_str() {
            "html" | "htm" => return "text/html; charset=utf-8".to_string(),
            "css" => return "text/css; charset=utf-8".to_string(),
            "js" => return "application/javascript; charset=utf-8".to_string(),
            "json" => return "application/json; charset=utf-8".to_string(),
            "png" => return "image/png".to_string(),
            "jpg" | "jpeg" => return "image/jpeg".to_string(),
            "gif" => return "image/gif".to_string(),
            "svg" => return "image/svg+xml".to_string(),
            "ico" => return "image/x-icon".to_string(),
            "woff" => return "font/woff".to_string(),
            "woff2" => return "font/woff2".to_string(),
            "ttf" => return "font/ttf".to_string(),
            "txt" => return "text/plain; charset=utf-8".to_string(),
            "xml" => return "application/xml; charset=utf-8".to_string(),
            "pdf" => return "application/pdf".to_string(),
            "zip" => return "application/zip".to_string(),
            "wasm" => return "application/wasm".to_string(),
            "webp" => return "image/webp".to_string(),
            "mp3" => return "audio/mpeg".to_string(),
            "m4a" => return "audio/mp4".to_string(),
            "mp4" => return "video/mp4".to_string(),
            "webm" => return "video/webm".to_string(),
            "ogg" => return "audio/ogg".to_string(),
            "ogv" => return "video/ogg".to_string(),
            "wav" => return "audio/wav".to_string(),
            "flac" => return "audio/flac".to_string(),
            "aac" => return "audio/aac".to_string(),
            "mp4a" => return "audio/mp4a-latm".to_string(),
            "mp4v" => return "video/mp4v-es".to_string(),
            "mp4s" => return "video/mp4s".to_string(),
             _ => {}
        }
    }
    "application/octet-stream".to_string()
}

/// 把 host 字符串解析成**绑定地址**。
///
/// **只有解析失败（含空串）才回落 127.0.0.1 —— 合法地址一律原样返回。**
///
/// 为什么单独抽出来并有专门的用例：GitHub issue #7 报过
/// 「所有 server 类型都不支持 `host: '0.0.0.0'`」。当时的核实结论是**环境问题**
/// （Windows Subsystem for Android 的共享网络），不是代码问题 —— 但那时**本地没有
/// 任何测试**覆盖这条路径，结论只能靠读代码得出。
/// 抽出来 + 钉住之后，以后谁加「0.0.0.0 → 127.0.0.1」这种"优化"，测试会立刻红。
///
/// 三条服务路径（callback / app / static）都调它，顺带消掉了三份重复实现。
pub fn resolve_bind_addr(host: &str, port: u16) -> std::net::SocketAddr {
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => (ip, port).into(),
        Err(_) => {
            crate::log_error!("Invalid host address: {}, using 127.0.0.1", host);
            ([127, 0, 0, 1], port).into()
        }
    }
}
