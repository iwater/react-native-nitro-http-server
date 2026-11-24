use std::path::Path;
use std::collections::HashMap;
use std::str::FromStr;
use std::os::raw::c_char;

use hyper::{Body, Response, StatusCode};
use hyper::header::{HeaderName, HeaderValue};

use crate::global::CUSTOM_MIME_TYPES;

/// Copy a request body slice for C ownership, returning `(ptr, len)`.
///
/// `free_http_request` reclaims memory via `Vec::from_raw_parts(ptr, len, len)`,
/// which requires `capacity == len`, so excess capacity is stripped via `normalize_capacity`.
/// Returns a null pointer for empty bodies (matching `body_len > 0` in `free_http_request`).
pub fn alloc_request_body(body: &[u8]) -> (*mut c_char, usize) {
    if body.is_empty() {
        return (std::ptr::null_mut(), 0);
    }
    let mut v = normalize_capacity(body.to_vec());
    let ptr = v.as_mut_ptr() as *mut c_char;
    std::mem::forget(v); // Transferred to C; freed by free_http_request
    (ptr, body.len())
}

/// Shrink a Vec's capacity to match its length.
/// Kept as a separate helper so unit tests can verify `capacity == len` before `mem::forget`.
pub(crate) fn normalize_capacity(v: Vec<u8>) -> Vec<u8> {
    v.into_boxed_slice().into_vec()
}

/// Read a request body with a hard byte limit.
///
/// Uses both a `Content-Length` pre-check and a streaming byte counter because
/// hyper 0.14's `to_bytes` is unbounded and chunked/dishonest requests may omit
/// or underreport `Content-Length`. Returns 413 on overflow or 400 on read error.
pub async fn read_body_limited(body: Body, max: u64) -> Result<Vec<u8>, Response<Body>> {
    use hyper::body::HttpBody;

    if let Some(len) = body.size_hint().exact() {
        if len > max {
            return Err(payload_too_large());
        }
    }

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

/// Build an HTTP response without panicking: invalid status codes fall back to 500
/// and invalid header names/values (e.g. control chars) are dropped with a warning.
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
        // Fallback 500 in case builder construction fails for any unexpected reason.
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from("Internal Server Error"))
            .expect("fallback response must always build")
    })
}

pub fn get_content_type(path: &Path) -> String {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        let ext_lower = ext.to_lowercase();
        
        if let Ok(custom_types) = CUSTOM_MIME_TYPES.lock() {
            if let Some(mime_type) = custom_types.get(&ext_lower) {
                return mime_type.clone();
            }
        }
        
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

/// Parse `host` into a bind `SocketAddr`, falling back to `127.0.0.1` only when parsing fails.
/// Valid addresses (including `0.0.0.0`) are preserved as-is.
pub fn resolve_bind_addr(host: &str, port: u16) -> std::net::SocketAddr {
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => (ip, port).into(),
        Err(_) => {
            crate::log_error!("Invalid host address: {}, using 127.0.0.1", host);
            ([127, 0, 0, 1], port).into()
        }
    }
}
