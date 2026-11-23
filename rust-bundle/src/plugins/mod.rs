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
    /// 插件名称
    fn name(&self) -> &str;
    
    /// 初始化插件 (从配置中读取选项)
    fn on_load(&self, config: &Value) -> Result<(), anyhow::Error>;
    
    /// 检查是否应该处理该请求
    fn should_handle(&self, req: &Request<Body>) -> bool;
    
    /// 处理请求
    async fn handle_request(&self, req: Request<Body>) -> Result<PluginAction, anyhow::Error>;

    /// 允许向下转型 (如果需要特定插件的特有功能)
    fn as_any(&self) -> &dyn Any;
}

/// 把文件名放进 HTTP header 前的编码：能安全往返就原样返回，否则 percent-encode。
///
/// **为什么需要它**（本机实测，不是推断）：
/// - `HeaderValue::from_str("报告.pdf")` 返回 **Ok** —— obs-text（0x80-0xFF）是合法
///   header 值，所以「非 ASCII 文件名会让请求 500」这个说法**是错的**。
/// - 真正的坑在下游：`http_server.rs` / `app_server.rs` 收集 header 时用的是
///   `value.to_str().unwrap_or("")`，而 `to_str()` 对 obs-text 返回 **Err**
///   → **值被静默置空**，JS 侧只看到空字符串。
///
/// 所以判定条件直接写成**下游的读取方式**（`from_str` 能接受 **且** `to_str` 能读回），
/// 而不是自己列一份「哪些字符要编码」的清单 —— 后者会漏掉控制字符
/// （如 `\x01`：它是 ASCII，但 `from_str` 会拒绝，`.header()` 静默失败）。
///
/// 返回值第二项表示「是否做了编码」；为 `true` 时调用方**必须**同时设置
/// `X-*-Encoding: percent` 标记头，否则接收方无从判断要不要 decode。
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

/// 检查请求路径是否落在 mount 点下，**带路径段边界**：
/// `/static` 匹配 `/static` 与 `/static/...`，但**不**匹配 `/staticfoo/...`、也不匹配 `/stat`。
///
/// 为什么需要它：各插件原本各自写 `path.starts_with(mount)`，于是挂 `/static`
/// 会把 `/staticfoo/a.txt` 也抢走（当成 `/static` 下的 `foo/a.txt`）。
/// `zip_mount` / `webdav` 自己手写了边界检查，`static_file` / `websocket` /
/// `upload` / `buffer_upload` 没有 —— 同一个配置在不同插件下行为不一致。
///
/// **尾斜杠等价**：配置里写 `"/static/"` 与 `"/static"` 等价。
/// 不归一化的话，`"/static/a"` 相对 `"/static/"` 的 rest 是 `"a"`（不以 `/` 开头）
/// → 永远返回 false，即「挂了却匹配不上」，比裸 `starts_with` 更糟。
///
/// **空串按根挂载处理**（`""` 与 `"/"` 等价）。需要「空 = 未挂载 / 禁用」语义的
/// 调用方（`webdav`、`zip_mount`）在自己那里先判断，不要指望这里返回 false。
pub fn path_matches_mount(path: &str, mount_path: &str) -> bool {
    let mount = mount_path.trim_end_matches('/');
    if mount.is_empty() {
        // "" 与 "/" 都表示根挂载
        return true;
    }
    // starts_with 保证了下面的切片落在 UTF-8 字符边界上
    if !path.starts_with(mount) {
        return false;
    }
    let rest = &path[mount.len()..];
    rest.is_empty() || rest.starts_with('/')
}

/// 把请求路径变成「相对挂载点」的路径，并做 percent-decode。
///
/// ⚠️ **必须与 [`path_matches_mount`] 用同一套归一化**（尾斜杠先 `trim_end_matches`），
/// 否则会出现「`should_handle` 说接管、这里却算不出相对路径」的不一致：
/// `mount="/static/"` + `path="/static"` 在旧的 `strip_prefix(&mount_path)` 下会 strip 失败，
/// 落进 `unwrap_or("/")`；而 `path_matches_mount` 认为它匹配。两边必须同源。
///
/// decode 放在剥离挂载点**之后**：挂载点是配置里写的字面量（未编码），
/// 先 decode 会把 `/s%2Fetc` 这类路径的边界算错。
///
/// ⚠️ decode 会引入 `..`（`%2e%2e`），所以调用方**必须**继续用
/// `canonicalize` + `starts_with(root)` 兜住路径遍历 —— 本函数不做安全判断。
pub fn resolve_relative(mount_path: &str, path: &str) -> String {
    let mount = mount_path.trim_end_matches('/');
    let relative = if mount.is_empty() {
        // "" 与 "/" 都表示根挂载（同 path_matches_mount）
        path
    } else {
        path.strip_prefix(mount).unwrap_or("/")
    };

    // decode 失败（例如 `%FF` 不是合法 UTF-8）时退化成原文：让它走「文件不存在」，
    // 而不是 panic 或把请求变成别的东西。
    let decoded = urlencoding::decode(relative)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| relative.to_string());

    decoded.trim_start_matches('/').to_string()
}

/// 从 mount 配置解析自定义响应头 ("headers": {"Key": "Value"})
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

/// 将自定义头注入响应（允许覆盖已有头；忽略非法名称/值）
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
