use std::collections::HashMap;
use std::os::raw::{c_char, c_int};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct ResponseData {
    pub status_code: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// Reserved for future streaming response support.
#[allow(dead_code)]
pub struct StreamingResponseData {
    pub status_code: u16,
    pub headers: HashMap<String, String>,
    pub body_receiver: mpsc::Receiver<Vec<u8>>,
}

/// Request body buffer used for chunked reads.
pub struct RequestBodyData {
    pub data: Vec<u8>,
    pub read_position: usize,
}

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

pub type RequestCallback = extern "C" fn(*mut HttpRequest);

/// Callback invoked when a client aborts a request before a response is sent.
///
/// Receives `request_id` (valid only for the duration of the call).
pub type RequestAbortedCallback = extern "C" fn(*const std::os::raw::c_char);

/// Accumulator for chunked response writes before finalization.
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
