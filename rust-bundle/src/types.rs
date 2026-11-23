use std::collections::HashMap;
use std::os::raw::{c_char, c_int};
use tokio::sync::mpsc;
// use std::ffi::{CString};

// 响应数据结构
#[derive(Debug, Clone)]
pub struct ResponseData {
    pub status_code: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

// 流式响应数据结构（预留用于未来流式响应增强）
#[allow(dead_code)]
pub struct StreamingResponseData {
    pub status_code: u16,
    pub headers: HashMap<String, String>,
    pub body_receiver: mpsc::Receiver<Vec<u8>>,
}

// 请求体数据结构（用于分块读取）
pub struct RequestBodyData {
    pub data: Vec<u8>,
    pub read_position: usize,
}

// 请求数据结构
#[repr(C)]
#[derive(Debug)]
pub struct HttpRequest {
    pub request_id: *mut c_char,
    pub method: *mut c_char,
    pub path: *mut c_char,
    pub headers_json: *mut c_char,
    pub body: *mut c_char,
    pub body_len: c_int,
}

// 使用更简单的回调类型
pub type RequestCallback = extern "C" fn(*mut HttpRequest);

/// 请求被中断（客户端断开连接）时通知 JS 的回调。
///
/// 入参是 `request_id`（C 字符串，调用期间有效）。
/// **为什么需要它**：客户端断开时 hyper 会直接 drop handler future
/// （见 `RequestStateGuard` 的注释），JS 侧的 handler 此前拿不到任何通知 ——
/// 只能在发送响应失败或等超时时才间接察觉。
pub type RequestAbortedCallback = extern "C" fn(*const std::os::raw::c_char);

// 流式响应块累积器（用于累积分块写入的数据）
pub struct ResponseChunkAccumulator {
    pub chunks: Vec<Vec<u8>>,
    pub status_code: u16,
    pub headers: HashMap<String, String>,
}

impl ResponseChunkAccumulator {
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            status_code: 200,
            headers: HashMap::new(),
        }
    }
    
    pub fn add_chunk(&mut self, chunk: Vec<u8>) {
        self.chunks.push(chunk);
    }
    
    pub fn into_response_data(self) -> ResponseData {
        let body: Vec<u8> = self.chunks.into_iter().flatten().collect();
        ResponseData {
            status_code: self.status_code,
            headers: self.headers,
            body,
        }
    }
}
