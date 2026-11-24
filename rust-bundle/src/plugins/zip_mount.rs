use super::Plugin;
use async_trait::async_trait;
use async_zip::base::read::mem::ZipFileReader as MemZipFileReader;
use hyper::{Body, Request, Response};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::any::Any;
use std::path::PathBuf;

use crate::global::CUSTOM_MIME_TYPES;

fn get_content_type_for_zip(path: &str) -> String {
    if let Some(ext) = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str()) 
    {
        let ext_lower = ext.to_lowercase();
        
        if let Ok(custom_types) = CUSTOM_MIME_TYPES.lock() {
            if let Some(mime_type) = custom_types.get(&ext_lower) {
                return mime_type.clone();
            }
        }
    }
    
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .as_ref()
        .to_string()
}

// `MemZipFileReader` implements `Clone` but not `Debug`.
#[derive(Clone)]
struct ZipMount {
    mount_path: String,
    zip_file_path: PathBuf,
    custom_headers: std::collections::HashMap<String, String>,
    /// Lazily initialized in-memory zip reader (parsed once on first request; retries on failure).
    /// Lazy initialization avoids doing async file I/O inside synchronous `new()` / `on_load()`.
    zip_reader: Arc<tokio::sync::OnceCell<Arc<MemZipFileReader>>>,
}

pub struct ZipMountPlugin {
    config: Arc<Mutex<ZipMount>>,
}

impl ZipMountPlugin {
    pub fn new(mount_path: String, zip_file: String) -> Self {
        Self {
            config: Arc::new(Mutex::new(ZipMount {
                mount_path,
                zip_file_path: PathBuf::from(zip_file),
                custom_headers: std::collections::HashMap::new(),
                zip_reader: Arc::new(tokio::sync::OnceCell::new()),
            })),
        }
    }

    async fn handle_inner(&self, req: Request<Body>) -> Result<super::PluginAction, anyhow::Error> {
        let uri_path = req.uri().path().to_string();

        // Clone the config to avoid holding the lock across await points
        let mount = self.config.lock().unwrap().clone();

        if !super::path_matches_mount(&uri_path, &mount.mount_path) {
            return Ok(super::PluginAction::Handled(Response::builder()
                .status(404)
                .body(Body::from("Mount point not found"))?));
        }

        // Use resolve_relative (same trailing-slash normalization as path_matches_mount)
        // rather than slicing by mount_path.len(), which would panic on "/zip/" + "/zip".
        let entry_name = super::resolve_relative(&mount.mount_path, &uri_path);

        let zip = mount
            .zip_reader
            .get_or_try_init(|| async {
                let data = tokio::fs::read(&mount.zip_file_path).await?;
                Ok::<_, anyhow::Error>(Arc::new(MemZipFileReader::new(data).await?))
            })
            .await?;

        if let Some(index) = zip.file().entries().iter().position(|e| e.filename().as_str().ok() == Some(&entry_name)) {
            let mut reader = zip.reader_with_entry(index).await?;
            let mut content = Vec::new();
            reader.read_to_end_checked(&mut content).await?;

            let mime_type = get_content_type_for_zip(&entry_name);

            Ok(super::PluginAction::Handled(Response::builder()
                .status(200)
                .header("Content-Type", mime_type)
                .body(Body::from(content))?))
        } else {
             Ok(super::PluginAction::Handled(Response::builder()
                .status(404)
                .body(Body::from("Not Found in Zip"))?))
        }
    }
}

#[async_trait]
impl Plugin for ZipMountPlugin {
    fn name(&self) -> &str {
        "zip_mount"
    }

    fn on_load(&self, config: &Value) -> Result<(), anyhow::Error> {
        let mut mount = self.config.lock().unwrap();
        
        if let Some(path) = config.get("mount_path").and_then(|v| v.as_str()) {
            mount.mount_path = path.to_string();
        }
        
        if let Some(file) = config.get("zip_file").and_then(|v| v.as_str()) {
            let new_path = PathBuf::from(file);
            if new_path != mount.zip_file_path {
                // Reset cached zip reader when switching to a different zip file.
                mount.zip_reader = Arc::new(tokio::sync::OnceCell::new());
            }
            mount.zip_file_path = new_path;
        }

        mount.custom_headers = super::parse_custom_headers(config);
        
        crate::log_info!("Zip mount plugin updated: path={}, file={:?}", mount.mount_path, mount.zip_file_path);
        Ok(())
    }

    fn should_handle(&self, req: &Request<Body>) -> bool {
        let path = req.uri().path();
        if req.method() != hyper::Method::GET {
            return false;
        }

        let mount = self.config.lock().unwrap();
        
        // Empty mount_path means unmounted (path_matches_mount treats "" as "/").
        if mount.mount_path.is_empty() {
            return false;
        }

        super::path_matches_mount(path, &mount.mount_path)
    }

    async fn handle_request(&self, req: Request<Body>) -> Result<super::PluginAction, anyhow::Error> {
        let headers = self.config.lock().unwrap().custom_headers.clone();
        match self.handle_inner(req).await? {
            super::PluginAction::Handled(mut resp) => {
                super::inject_custom_headers(&mut resp, &headers);
                Ok(super::PluginAction::Handled(resp))
            }
            other => Ok(other),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
