use super::Plugin;
use async_trait::async_trait;
use async_zip::base::read::mem::ZipFileReader as MemZipFileReader;
use hyper::{Body, Request, Response};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::any::Any;
use std::path::PathBuf;

// 从 lib.rs 中访问全局自定义 MIME types
use crate::global::CUSTOM_MIME_TYPES;

// 为 zip 文件中的内容获取 Content-Type
fn get_content_type_for_zip(path: &str) -> String {
    // 获取文件扩展名
    if let Some(ext) = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str()) 
    {
        let ext_lower = ext.to_lowercase();
        
        // 优先查找自定义 MIME types
        if let Ok(custom_types) = CUSTOM_MIME_TYPES.lock() {
            if let Some(mime_type) = custom_types.get(&ext_lower) {
                return mime_type.clone();
            }
        }
    }
    
    // 回退到 mime_guess
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .as_ref()
        .to_string()
}

// 注意：这里**没有** `Debug` —— `async_zip` 的 `mem::ZipFileReader` 只实现了 `Clone`，
// 带上 `Debug` 就编不过。本结构体只在内部使用，日志按字段打（见 on_load）。
#[derive(Clone)]
struct ZipMount {
    mount_path: String,
    zip_file_path: PathBuf,
    custom_headers: std::collections::HashMap<String, String>,
    /// 惰性解析好的**内存** zip reader —— 解析一次，之后请求零文件 IO。
    ///
    /// 为什么是**惰性**而不是在 `new()` / `on_load()` 里加载：`ZipFileReader::new` 是
    /// **async**（`async_zip-0.0.18/src/base/read/mem.rs:96`），而 `new()` / `on_load()`
    /// 是同步上下文。惰性初始化既绕开同步/异步鸿沟，也天然兼容「构造后不调 on_load」
    /// 的既有调用方式（`plugin_zip.rs` 的用例就是 `new` 完直接 `handle_request`）。
    ///
    /// 失败**不缓存**（`get_or_try_init` 失败后 cell 仍为空）→ zip 稍后才出现、
    /// 或被换掉时能自愈。换 zip 时由 `on_load` 重置这个 cell。
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

        // 与 should_handle 用同一个判据，避免两处对「什么算匹配」的理解漂移
        if !super::path_matches_mount(&uri_path, &mount.mount_path) {
            return Ok(super::PluginAction::Handled(Response::builder()
                .status(404)
                .body(Body::from("Mount point not found"))?));
        }

        // 计算相对路径 + percent-decode：复用 static_file 那个 helper（与
        // path_matches_mount 同一套归一化）。
        //
        // ⚠️ 不要自己按 `mount_path.len()` 切片：`path_matches_mount` 认「尾斜杠等价」，
        // 所以 mount="/zip/" + 请求 "/zip" 会进到这里，而按字节切片会**越界 panic**
        // （实测：`byte index 5 is out of bounds of /zip`，一个 GET 就能打崩）。
        let entry_name = super::resolve_relative(&mount.mount_path, &uri_path);

        // 惰性解析一次，之后请求零文件 IO
        let zip = mount
            .zip_reader
            .get_or_try_init(|| async {
                let data = tokio::fs::read(&mount.zip_file_path).await?;
                Ok::<_, anyhow::Error>(Arc::new(MemZipFileReader::new(data).await?))
            })
            .await?;

        // 查找文件
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
        
        // 支持更新配置
        if let Some(path) = config.get("mount_path").and_then(|v| v.as_str()) {
            mount.mount_path = path.to_string();
        }
        
        if let Some(file) = config.get("zip_file").and_then(|v| v.as_str()) {
            let new_path = PathBuf::from(file);
            if new_path != mount.zip_file_path {
                // 换了 zip 必须丢掉已解析的结果，否则会继续服务**旧 zip 的内容**
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
        
        // 空 mount_path = 未挂载（该语义保留在调用方，helper 把 "" 当根挂载处理）
        if mount.mount_path.is_empty() {
            return false;
        }

        // 与其它插件统一：带路径段边界的 mount 匹配
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
