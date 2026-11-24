use hyper::{Body, Request, Method};
use serde_json::Value;
use std::any::Any;
use std::path::PathBuf;
use async_trait::async_trait;
use crate::plugins::{Plugin, PluginAction};
use multer::Multipart;
use uuid::Uuid;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone)]
pub struct UploadConfig {
    pub path: String,
    pub temp_dir: String,
}

pub struct UploadPlugin {
    config: UploadConfig,
}

impl UploadPlugin {
    pub fn new(path: String, temp_dir: String) -> Self {
        Self {
            config: UploadConfig { path, temp_dir },
        }
    }
}

#[async_trait]
impl Plugin for UploadPlugin {
    fn name(&self) -> &str {
        "upload"
    }

    fn on_load(&self, _config: &Value) -> Result<(), anyhow::Error> {
        Ok(())
    }

    fn should_handle(&self, req: &Request<Body>) -> bool {
        let path = req.uri().path();
        super::path_matches_mount(path, &self.config.path) && 
        (req.method() == Method::POST || req.method() == Method::PUT)
    }

    async fn handle_request(&self, mut req: Request<Body>) -> Result<PluginAction, anyhow::Error> {
        let boundary = req.headers()
            .get("content-type")
            .and_then(|ct| ct.to_str().ok())
            .and_then(|ct| multer::parse_boundary(ct).ok());

        if let Some(boundary) = boundary {
            let temp_dir = PathBuf::from(&self.config.temp_dir);
            if !temp_dir.exists() {
                tokio::fs::create_dir_all(&temp_dir).await?;
            }

            let _body = req.body_mut();

            // Consume the multipart stream, write uploaded files to temp_dir, and pass
            // metadata headers forward on a new empty-body request.
            let (parts, body) = req.into_parts();
            let mut multipart = Multipart::new(body, boundary);
            
            let mut uploaded_files = Vec::new();
            
            while let Some(mut field) = multipart.next_field().await? {
                let _name = field.name().map(|s| s.to_owned());
                let file_name = field.file_name().map(|s| s.to_owned());
                
                if let Some(file_name) = file_name {
                    // Keep only ASCII extensions for the on-disk filename placed in
                    // `X-Uploaded-File-Path` so `HeaderValue::to_str()` succeeds downstream.
                    let ext = std::path::Path::new(&file_name).extension()
                        .and_then(|s| s.to_str())
                        .filter(|s| s.is_ascii())
                        .unwrap_or("tmp");
                    let file_uuid = Uuid::new_v4().to_string();
                    let save_filename = format!("{}.{}", file_uuid, ext);
                    let save_path = temp_dir.join(&save_filename);
                    
                    let mut file = tokio::fs::File::create(&save_path).await?;
                    let mut field_size = 0;
                    
                    while let Some(chunk) = field.chunk().await? {
                        file.write_all(&chunk).await?;
                        field_size += chunk.len();
                    }
                    file.flush().await?;
                    
                    crate::log_debug!("File uploaded: {:?} -> {:?}", file_name, save_path);
                    
                    uploaded_files.push((file_name, save_path, field_size));
                }
            }
            
            let mut new_req = Request::builder()
                .method(parts.method)
                .uri(parts.uri)
                .version(parts.version);
                
            for (k, v) in parts.headers.iter() {
                 new_req = new_req.header(k, v);
            }
            
            if let Some((original_name, save_path, size)) = uploaded_files.first() {
                // Inject metadata headers for the first uploaded file.
                let (encoded_name, was_encoded) = super::encode_filename_for_header(original_name);
                new_req = new_req.header("X-Uploaded-File-Path", save_path.to_string_lossy().to_string());
                new_req = new_req.header("X-Uploaded-Original-Name", encoded_name);
                if was_encoded {
                    new_req = new_req.header("X-Uploaded-Filename-Encoding", "percent");
                }
                new_req = new_req.header("X-Uploaded-Size", size.to_string());
            }

            let new_req = new_req.body(Body::empty())?;
            
            Ok(PluginAction::PassThrough(new_req))
            
        } else {
            Ok(PluginAction::PassThrough(req))
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
