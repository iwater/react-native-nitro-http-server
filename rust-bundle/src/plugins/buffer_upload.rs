use hyper::{Body, Request, Method, Response, StatusCode};
use serde_json::Value;
use std::any::Any;
use async_trait::async_trait;
use crate::plugins::{Plugin, PluginAction};
use multer::Multipart;

// 100MB 文件大小限制
const MAX_FILE_SIZE: usize = 100 * 1024 * 1024;

// 配置结构
#[derive(Debug, Clone)]
pub struct BufferUploadConfig {
    pub path: String,       // 监听路径 (e.g. /buffer-upload)
}

pub struct BufferUploadPlugin {
    config: BufferUploadConfig,
    custom_headers: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl BufferUploadPlugin {
    pub fn new(path: String) -> Self {
        Self {
            config: BufferUploadConfig { path },
            custom_headers: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    async fn handle_inner(&self, req: Request<Body>) -> Result<PluginAction, anyhow::Error> {
        // 检查 Content-Type 是否为 multipart/form-data
        let boundary = req.headers()
            .get("content-type")
            .and_then(|ct| ct.to_str().ok())
            .and_then(|ct| multer::parse_boundary(ct).ok());

        if let Some(boundary) = boundary {
            // 解构请求,获取 body
            let (parts, body) = req.into_parts();
            let mut multipart = Multipart::new(body, boundary);

            // 用于存储第一个文件的信息
            let mut file_data: Option<(String, Vec<u8>, String)> = None; // (filename, data, content_type)

            // 遍历所有部分,只处理第一个文件
            while let Some(mut field) = multipart.next_field().await? {
                let file_name = field.file_name().map(|s| s.to_owned());

                if let Some(file_name) = file_name {
                    // 获取 content-type (如果有)
                    let content_type = field.content_type()
                        .map(|ct| ct.to_string())
                        .unwrap_or_else(|| "application/octet-stream".to_string());

                    // 收集文件内容
                    let mut file_bytes = Vec::new();

                    while let Some(chunk) = field.chunk().await? {
                        // 检查文件大小限制
                        if file_bytes.len() + chunk.len() > MAX_FILE_SIZE {
                            // 超过大小限制,返回错误响应
                            let error_response = Response::builder()
                                .status(StatusCode::PAYLOAD_TOO_LARGE)
                                .header("Content-Type", "application/json")
                                .body(Body::from(
                                    format!(r#"{{"error": "File too large. Maximum size is {} MB"}}"#, MAX_FILE_SIZE / 1024 / 1024)
                                ))?;

                            return Ok(PluginAction::Handled(error_response));
                        }

                        file_bytes.extend_from_slice(&chunk);
                    }

                    crate::log_debug!("BufferUpload: Received file '{}' ({} bytes, {})",
                             file_name, file_bytes.len(), content_type);

                    // 保存第一个文件信息并跳出循环
                    file_data = Some((file_name, file_bytes, content_type));
                    break;
                }
            }

            // 如果找到了文件,构造新的请求传递给 JS
            if let Some((filename, data, content_type)) = file_data {
                // 构造新的请求
                let mut new_req = Request::builder()
                    .method(parts.method)
                    .uri(parts.uri)
                    .version(parts.version);

                // 复制原有 Header
                for (k, v) in parts.headers.iter() {
                     new_req = new_req.header(k, v);
                }

                // 添加文件元数据 Header
                // 文件名非 ASCII 时 percent-encode 并补标记头 —— 理由同 upload.rs：
                // obs-text 能被 `HeaderValue` 接受（不会 500），但下游
                // `to_str().unwrap_or("")` 会把它读成空串，JS 侧拿不到文件名。
                let (encoded_filename, was_encoded) = super::encode_filename_for_header(&filename);
                new_req = new_req.header("X-Upload-Filename", encoded_filename);
                if was_encoded {
                    new_req = new_req.header("X-Upload-Filename-Encoding", "percent");
                }
                new_req = new_req.header("X-Upload-Size", data.len().to_string());
                new_req = new_req.header("X-Upload-Content-Type", content_type);

                // 将文件内容作为 body (binary data)
                let new_req = new_req.body(Body::from(data))?;

                Ok(PluginAction::PassThrough(new_req))
            } else {
                // 没有找到文件,返回错误响应
                let error_response = Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"error": "No file found in the request"}"#))?;

                Ok(PluginAction::Handled(error_response))
            }

        } else {
            // 非 multipart 请求,返回错误
            let error_response = Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"error": "Content-Type must be multipart/form-data"}"#))?;

            Ok(PluginAction::Handled(error_response))
        }
    }
}

#[async_trait]
impl Plugin for BufferUploadPlugin {
    fn name(&self) -> &str {
        "buffer_upload"
    }

    fn on_load(&self, config: &Value) -> Result<(), anyhow::Error> {
        // 配置已在 new 中加载，这里可以处理额外的动态配置
        *self.custom_headers.lock().unwrap() = super::parse_custom_headers(config);
        Ok(())
    }

    fn should_handle(&self, req: &Request<Body>) -> bool {
        let path = req.uri().path();
        // 匹配 mount 点（带路径段边界：挂 /upload 不接管 /uploadfoo）且 方法必须是 POST 或 PUT
        super::path_matches_mount(path, &self.config.path) && 
        (req.method() == Method::POST || req.method() == Method::PUT)
    }

    async fn handle_request(&self, req: Request<Body>) -> Result<PluginAction, anyhow::Error> {
        let headers = self.custom_headers.lock().unwrap().clone();
        match self.handle_inner(req).await? {
            PluginAction::Handled(mut resp) => {
                super::inject_custom_headers(&mut resp, &headers);
                Ok(PluginAction::Handled(resp))
            }
            other => Ok(other),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
