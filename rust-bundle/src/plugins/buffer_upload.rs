use hyper::{Body, Request, Method, Response, StatusCode};
use serde_json::Value;
use std::any::Any;
use async_trait::async_trait;
use crate::plugins::{Plugin, PluginAction};
use multer::Multipart;

// 100MB file size limit
const MAX_FILE_SIZE: usize = 100 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct BufferUploadConfig {
    pub path: String,
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
        let boundary = req.headers()
            .get("content-type")
            .and_then(|ct| ct.to_str().ok())
            .and_then(|ct| multer::parse_boundary(ct).ok());

        if let Some(boundary) = boundary {
            let (parts, body) = req.into_parts();
            let mut multipart = Multipart::new(body, boundary);

            let mut file_data: Option<(String, Vec<u8>, String)> = None; // (filename, data, content_type)

            // Process only the first uploaded file field.
            while let Some(mut field) = multipart.next_field().await? {
                let file_name = field.file_name().map(|s| s.to_owned());

                if let Some(file_name) = file_name {
                    let content_type = field.content_type()
                        .map(|ct| ct.to_string())
                        .unwrap_or_else(|| "application/octet-stream".to_string());

                    let mut file_bytes = Vec::new();

                    while let Some(chunk) = field.chunk().await? {
                        if file_bytes.len() + chunk.len() > MAX_FILE_SIZE {
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

                    file_data = Some((file_name, file_bytes, content_type));
                    break;
                }
            }

            if let Some((filename, data, content_type)) = file_data {
                let mut new_req = Request::builder()
                    .method(parts.method)
                    .uri(parts.uri)
                    .version(parts.version);

                for (k, v) in parts.headers.iter() {
                     new_req = new_req.header(k, v);
                }

                // Percent-encode non-ASCII filenames because downstream `HeaderValue::to_str()` rejects obs-text.
                let (encoded_filename, was_encoded) = super::encode_filename_for_header(&filename);
                new_req = new_req.header("X-Upload-Filename", encoded_filename);
                if was_encoded {
                    new_req = new_req.header("X-Upload-Filename-Encoding", "percent");
                }
                new_req = new_req.header("X-Upload-Size", data.len().to_string());
                new_req = new_req.header("X-Upload-Content-Type", content_type);

                let new_req = new_req.body(Body::from(data))?;

                Ok(PluginAction::PassThrough(new_req))
            } else {
                let error_response = Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"error": "No file found in the request"}"#))?;

                Ok(PluginAction::Handled(error_response))
            }

        } else {
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
        *self.custom_headers.lock().unwrap() = super::parse_custom_headers(config);
        Ok(())
    }

    fn should_handle(&self, req: &Request<Body>) -> bool {
        let path = req.uri().path();
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
