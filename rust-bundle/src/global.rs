use std::collections::HashMap;
use std::sync::{Mutex};
use std::sync::atomic::{AtomicU8, AtomicU64};
use once_cell::sync::Lazy;
use tokio::sync::oneshot;

use crate::types::{ResponseData, RequestBodyData, StreamingResponseData, ResponseChunkAccumulator, RequestAbortedCallback};

// 全局 Logging Level
// 0 = Off
// 1 = Error
// 2 = Warn
// 3 = Info
// 4 = Debug
pub static VERBOSE: AtomicU8 = AtomicU8::new(0);

// 全局响应超时（秒）：等 JS 回调返回响应的最长时间，默认 30。
// 长轮询 / SSE 需要调大。只有 config 里显式给 `request_timeout_secs` 才会改；
// 与 VERBOSE / CUSTOM_MIME_TYPES 一样是全局状态（server 本身也是全局单例），
// 重启 config server 时会重新应用。
pub static REQUEST_TIMEOUT_SECS: AtomicU64 = AtomicU64::new(30);

// 全局请求体上限（字节），默认 100MB（与 buffer_upload 的上限一致）。
//
// 为什么必须有：`hyper::body::to_bytes` 在 hyper 0.14 **没有内建上限**
// （`body/to_bytes.rs` 把 chunk 一路拼进一个 buffer），一个超大 body 就能把进程撑爆。
//
// 只有 config 里显式给 `max_body_size` 才会改；与 VERBOSE / CUSTOM_MIME_TYPES /
// REQUEST_TIMEOUT_SECS 一样是全局状态（server 本身也是全局单例），
// 重启 config server 时会重新应用。
pub static MAX_BODY_SIZE: AtomicU64 = AtomicU64::new(100 * 1024 * 1024);

// ---- getStats 用的计数器 ---------------------------------------------------
//
// ⚠️ 这里**只放能 100% 覆盖统计路径**的计数器。`ServerStats` 一共 6 个字段，
// 另外三个恒为 0，原因写在 `c_api.rs::get_server_stats` 里 —— 与其报一个看起来
// 真实、实际只覆盖了一半路径的数字，不如留 0 并写清为什么。
//
// 与其它全局状态同构：进程级、不随 server 重启清零（重启会重新应用 start 时间）。

/// 收到并进入请求处理的请求数（三个 server 的 handle 入口各加一次）。
pub static TOTAL_REQUESTS: AtomicU64 = AtomicU64::new(0);

/// **服务器自身**没能按请求产出响应的次数：
/// 等 JS 回调超时、响应通道被关闭、静态服务的 root 不可用或插件 IO 失败（→ 500）。
///
/// 口径：JS / 插件**主动**返回的 4xx/5xx **不计入** —— 那是业务结果，不是服务器故障。
pub static ERROR_COUNT: AtomicU64 = AtomicU64::new(0);

/// 最近一次 `start_*` 的 Unix 秒时间戳（0 = 从未启动）。`uptime` 由它算。
pub static SERVER_START_UNIX: AtomicU64 = AtomicU64::new(0);

/// 标记服务器已启动（记录起始时间，供 `uptime` 用）
pub fn mark_server_started() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    SERVER_START_UNIX.store(now, std::sync::atomic::Ordering::Relaxed);
}

/// 已运行秒数（从未启动过则为 0）
pub fn uptime_secs() -> u64 {
    let start = SERVER_START_UNIX.load(std::sync::atomic::Ordering::Relaxed);
    if start == 0 {
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(start)
}

// 全局响应通道映射表
pub static RESPONSE_CHANNELS: Lazy<Mutex<HashMap<String, oneshot::Sender<ResponseData>>>> = 
    Lazy::new(|| Mutex::new(HashMap::new()));

// 全局请求体存储（用于分块读取）
pub static REQUEST_BODIES: Lazy<Mutex<HashMap<String, RequestBodyData>>> = 
    Lazy::new(|| Mutex::new(HashMap::new()));

// 全局流式响应通道映射表（预留用于未来流式响应增强）
#[allow(dead_code)]
pub static STREAMING_RESPONSE_CHANNELS: Lazy<Mutex<HashMap<String, oneshot::Sender<StreamingResponseData>>>> = 
    Lazy::new(|| Mutex::new(HashMap::new()));

// 全局流式响应块累积器映射表
pub static CHUNK_ACCUMULATORS: Lazy<Mutex<HashMap<String, ResponseChunkAccumulator>>> = 
    Lazy::new(|| Mutex::new(HashMap::new()));

// 全局自定义 MIME types 映射表 (扩展名 -> MIME type)
pub static CUSTOM_MIME_TYPES: Lazy<Mutex<HashMap<String, String>>> = 
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 清理一个请求的全部全局状态（三张表），幂等。
///
/// 只清 `RESPONSE_CHANNELS` 会留下 `REQUEST_BODIES`（含完整请求体拷贝）与
/// `CHUNK_ACCUMULATORS`（未 end_response 的块），每请求泄漏一条。
///
/// 逐个加锁、不嵌套，避免锁顺序问题。
pub(crate) fn cleanup_request_state(request_id: &str) {
    RESPONSE_CHANNELS.lock().unwrap().remove(request_id);
    REQUEST_BODIES.lock().unwrap().remove(request_id);
    CHUNK_ACCUMULATORS.lock().unwrap().remove(request_id);
}

/// 「请求被中断」的回调（由 C++ 在 start 时注册，见 `set_request_aborted_callback`）。
///
/// 只有一个（原生 server 是单例）。
pub static ABORT_CALLBACK: Lazy<Mutex<Option<RequestAbortedCallback>>> = Lazy::new(|| Mutex::new(None));

/// 通知 JS：这个请求的客户端已经走了。
///
/// 静默失败是刻意的：这是「尽力通知」，不该因为回调缺失/panic 影响请求收尾。
fn notify_request_aborted(request_id: &str) {
    let cb = *ABORT_CALLBACK.lock().unwrap();
    let Some(cb) = cb else { return };
    match std::ffi::CString::new(request_id) {
        Ok(c) => cb(c.as_ptr()),
        Err(_) => crate::log_warn!("request_id 含 NUL，无法通知 abort: {}", request_id),
    }
}

/// 与 [`cleanup_request_state`] 相同，但**返回「响应是否从未发出」**。
///
/// 判据：`RESPONSE_CHANNELS` 里是否还留着该 request 的 tx ——
/// `send_response` 是**先把 tx 取走再 send** 的，所以「还留着」= 从没响应过。
pub(crate) fn cleanup_request_state_checking_response(request_id: &str) -> bool {
    let never_responded = RESPONSE_CHANNELS.lock().unwrap().remove(request_id).is_some();
    REQUEST_BODIES.lock().unwrap().remove(request_id);
    CHUNK_ACCUMULATORS.lock().unwrap().remove(request_id);
    never_responded
}

/// RAII guard：请求条目在**任何**退出路径下都会被清理。
///
/// 为什么必须存在（实测）：客户端断开连接时 hyper 会直接 drop handler future，
/// 30s 超时分支根本不会执行——实测 abort 后等 31s，RESPONSE_CHANNELS 与
/// REQUEST_BODIES 仍各残留 1 条，即**永久泄漏**，不是"最多留 30 秒"。
/// 显式调用只能覆盖"handler 自己走完"的路径，覆盖不到 future 被丢弃的情况。
///
/// 成功路径上它是 no-op：`send_response` 已经把 tx 从表里取走再 send。
pub(crate) struct RequestStateGuard {
    request_id: String,
}

impl RequestStateGuard {
    pub(crate) fn new(request_id: String) -> Self {
        Self { request_id }
    }
}

impl Drop for RequestStateGuard {
    fn drop(&mut self) {
        // 「响应从未发出」= 客户端断开（hyper 把 future 丢了），而不是 handler 正常回完。
        // 正常路径上 send_response 已经取走了 tx，这里是 false，不会有任何通知。
        if cleanup_request_state_checking_response(&self.request_id) {
            crate::log_info!("Request {} aborted by client (future dropped before response)", self.request_id);
            notify_request_aborted(&self.request_id);
        }
    }
}

// ---- 仅测试用：观察三张全局表的长度，用于泄漏断言 ----
#[cfg(test)]
pub fn response_channels_len() -> usize {
    RESPONSE_CHANNELS.lock().unwrap().len()
}

#[cfg(test)]
pub fn request_bodies_len() -> usize {
    REQUEST_BODIES.lock().unwrap().len()
}

#[cfg(test)]
pub fn chunk_accumulators_len() -> usize {
    CHUNK_ACCUMULATORS.lock().unwrap().len()
}
