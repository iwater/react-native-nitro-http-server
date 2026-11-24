use async_trait::async_trait;
use hyper::{Body, Request, Response, header::HeaderValue};
use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;

pub mod manager;
pub mod webdav;
pub mod zip_mount;
pub mod static_file;
pub mod upload;
pub mod buffer_upload;
pub mod rewrite;
pub mod websocket;

pub enum PluginAction {
    Handled(Response<Body>),
    PassThrough(Request<Body>),
}

#[async_trait]
pub trait Plugin: Send + Sync {
    /// Plugin identifier.
    fn name(&self) -> &str;
    
    /// Initialize or update plugin configuration.
    fn on_load(&self, config: &Value) -> Result<(), anyhow::Error>;
    
    /// Return whether this plugin should handle the request.
    fn should_handle(&self, req: &Request<Body>) -> bool;
    
    /// Process the request, either returning a response or passing it through.
    async fn handle_request(&self, req: Request<Body>) -> Result<PluginAction, anyhow::Error>;

    /// Allow downcasting for plugin-specific functionality.
    fn as_any(&self) -> &dyn Any;
}

/// Encode a filename for HTTP header transport: keep as-is when readable via
/// `HeaderValue::to_str()`, otherwise percent-encode.
///
/// Note: `HeaderValue::from_str` accepts obs-text (0x80-0xFF), but downstream
/// header collection uses `to_str()`, which fails on obs-text and silently drops
/// the value. Returns `(encoded_value, was_encoded)`; callers must set an encoding
/// indicator header when `was_encoded` is `true`.
pub fn encode_filename_for_header(name: &str) -> (String, bool) {
    let is_header_readable = HeaderValue::from_str(name)
        .map(|v| v.to_str().is_ok())
        .unwrap_or(false);

    if is_header_readable {
        (name.to_string(), false)
    } else {
        (urlencoding::encode(name).into_owned(), true)
    }
}

/// Check whether `path` falls under `mount_path` on a path-segment boundary:
/// `/static` matches `/static` and `/static/...`, but not `/staticfoo/...`.
///
/// Trailing slashes on `mount_path` are ignored (`"/static/"` equals `"/static"`).
/// An empty string `""` is treated as root mount `"/"`; callers that treat empty
/// as disabled (`webdav`, `zip_mount`) must check for empty before calling.
pub fn path_matches_mount(path: &str, mount_path: &str) -> bool {
    let mount = mount_path.trim_end_matches('/');
    if mount.is_empty() {
        return true;
    }
    // `starts_with` guarantees the slice offset below lands on a UTF-8 boundary.
    if !path.starts_with(mount) {
        return false;
    }
    let rest = &path[mount.len()..];
    rest.is_empty() || rest.starts_with('/')
}

/// Convert a request path into a percent-decoded path relative to `mount_path`.
///
/// Uses the same trailing-slash normalization as [`path_matches_mount`] and decodes
/// after stripping the mount prefix so encoded slashes do not alter mount matching.
/// Callers remain responsible for path-traversal checks (e.g. `canonicalize`) since
/// decoding can introduce `..`.
pub fn resolve_relative(mount_path: &str, path: &str) -> String {
    let mount = mount_path.trim_end_matches('/');
    let relative = if mount.is_empty() {
        path
    } else {
        path.strip_prefix(mount).unwrap_or("/")
    };

    // Fall back to raw relative path if percent-decoding yields invalid UTF-8.
    let decoded = urlencoding::decode(relative)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| relative.to_string());

    decoded.trim_start_matches('/').to_string()
}

/// Parse custom response headers (`"headers": {"Key": "Value"}`) from a mount config.
pub fn parse_custom_headers(config: &Value) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(obj) = config.get("headers").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                map.insert(k.clone(), s.to_string());
            } else {
                crate::log_warn!("Ignoring non-string custom header: {}", k);
            }
        }
    }
    map
}

/// Inject custom headers onto a response (overwriting existing values; invalid names/values ignored).
pub fn inject_custom_headers(resp: &mut Response<Body>, headers: &HashMap<String, String>) {
    if headers.is_empty() {
        return;
    }
    for (k, v) in headers {
        match (k.parse::<hyper::header::HeaderName>(), HeaderValue::from_str(v)) {
            (Ok(name), Ok(value)) => {
                resp.headers_mut().insert(name, value);
            }
            _ => crate::log_warn!("Ignoring invalid custom header: {}: {}", k, v),
        }
    }
}
