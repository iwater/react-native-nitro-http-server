use hyper::{Body, Request, Method};
use serde_json::Value;
use std::any::Any;
use std::path::PathBuf;
use async_trait::async_trait;
use crate::plugins::{Plugin, PluginAction};
use multer::Multipart;
use uuid::Uuid;
use tokio::io::AsyncWriteExt;

// 配置结构
#[derive(Debug, Clone)]
pub struct UploadConfig {
    pub path: String,       // 监听路径 (e.g. /upload)
    pub temp_dir: String,   // 临时文件存储目录
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
        // 配置已在 new 中加载，这里可以处理额外的动态配置
        Ok(())
    }

    fn should_handle(&self, req: &Request<Body>) -> bool {
        let path = req.uri().path();
        // 匹配 mount 点（带路径段边界：挂 /upload 不接管 /uploadfoo）且 方法必须是 POST 或 PUT
        super::path_matches_mount(path, &self.config.path) && 
        (req.method() == Method::POST || req.method() == Method::PUT)
    }

    async fn handle_request(&self, mut req: Request<Body>) -> Result<PluginAction, anyhow::Error> {
        // 检查 Content-Type 是否为 multipart/form-data
        let boundary = req.headers()
            .get("content-type")
            .and_then(|ct| ct.to_str().ok())
            .and_then(|ct| multer::parse_boundary(ct).ok());

        if let Some(boundary) = boundary {
            // 准备临时目录
            let temp_dir = PathBuf::from(&self.config.temp_dir);
            if !temp_dir.exists() {
                tokio::fs::create_dir_all(&temp_dir).await?;
            }

            // 处理请求体
            let _body = req.body_mut();
            
            // 注意：这里一定要拿走 body，因为我們要解析它
            // 然而 Request<Body> 的 body 是 private 字段的封装，我们只能通过 into_body 拿走
            // 但是 trait 传入的是 mut Request<Body> 引用？不对, signature 是 req: Request<Body> (move) 
            // 所以我们可以直接解构。
            
            // 哎呀 wait, plugin signature is async fn handle_request(&self, req: Request<Body>) -> ...
            // 所以我们拥有 req 的所有权。
            
            // 我们需要先把 body 拿出来给 multer 处理
            // 但是最后我们可能还要把 req 传给下一个处理器 (PassThrough)
            // 可是 multipart body 是流式的，一旦消费了，就没了。
            // 这意味着 PassThrough 传给后续的 JS callback 的 request body 将是空的或者已变更的。
            // 我们的目标是将文件保存，然后将文件路径放在 Header 传给 JS。
            // JS 端收到 Request 后，Body 应该是空的 (或者包含非文件字段的 json? Multer 会消费掉所有部分)
            
            // 如果我们消费了 body，我们必须构造一个新的 Request 给 PassThrough
            
            let (parts, body) = req.into_parts();
            let mut multipart = Multipart::new(body, boundary);
            
            // 收集上传的文件信息
            let mut uploaded_files = Vec::new();
            
            // 遍历所有部分
            while let Some(mut field) = multipart.next_field().await? {
                let _name = field.name().map(|s| s.to_owned());
                let file_name = field.file_name().map(|s| s.to_owned());
                
                if let Some(file_name) = file_name {
                    // 这是一个文件
                    // 扩展名必须是 ASCII：它会拼进落盘文件名，进而进 `X-Uploaded-File-Path`。
                    // 实测 `Path::new("文件.报告").extension()` → `Some("报告")` ——
                    // 非 ASCII 扩展名会让该 header 变成 obs-text，下游 `to_str()` 失败
                    // → 路径被静默置空（`integration_c_api.rs:360` 的 JS 侧拿不到它会返回 400）。
                    // 扩展名只是提示性信息，原始文件名另有 `X-Uploaded-Original-Name` 承载，
                    // 所以这里退化成 "tmp" 而不做有损编码。
                    let ext = std::path::Path::new(&file_name).extension()
                        .and_then(|s| s.to_str())
                        .filter(|s| s.is_ascii())
                        .unwrap_or("tmp");
                    let file_uuid = Uuid::new_v4().to_string();
                    let save_filename = format!("{}.{}", file_uuid, ext);
                    let save_path = temp_dir.join(&save_filename);
                    
                    // 保存文件
                    let mut file = tokio::fs::File::create(&save_path).await?;
                    let mut field_size = 0;
                    
                    while let Some(chunk) = field.chunk().await? {
                        file.write_all(&chunk).await?;
                        field_size += chunk.len();
                    }
                    file.flush().await?;
                    
                    crate::log_debug!("File uploaded: {:?} -> {:?}", file_name, save_path);
                    
                    uploaded_files.push((file_name, save_path, field_size));
                } else {
                    // 这是一个普通字段
                    // 我们应该怎么处理？也许收集起来放 header？或者放 body？
                    // 为了简单起见，目前只处理文件。
                }
            }
            
            // 构造新的请求，附加 Header
            let mut new_req = Request::builder()
                .method(parts.method)
                .uri(parts.uri)
                .version(parts.version);
                
            // 复制原有 Header
            for (k, v) in parts.headers.iter() {
                 new_req = new_req.header(k, v);
            }
            
            // 添加自定义 Header
            if let Some((original_name, save_path, size)) = uploaded_files.first() {
                // 目前只支持单个文件上传的 Header 注入，多个文件可能需要 JSON Header
                // 为简单起见，使用 X-Uploaded-File-*
                //
                // 文件名必须先过 encode_filename_for_header：非 ASCII 直接塞进去虽然
                // HeaderValue 会接受，但下游 `to_str().unwrap_or("")` 会把它读成空串
                // （JS 侧拿不到文件名）。编码过就补一个标记头告诉 JS 要 decode。
                let (encoded_name, was_encoded) = super::encode_filename_for_header(original_name);
                new_req = new_req.header("X-Uploaded-File-Path", save_path.to_string_lossy().to_string());
                new_req = new_req.header("X-Uploaded-Original-Name", encoded_name);
                if was_encoded {
                    new_req = new_req.header("X-Uploaded-Filename-Encoding", "percent");
                }
                new_req = new_req.header("X-Uploaded-Size", size.to_string());
            }

            // 构造新的 Body (空的，因为流已被消费)
            // 或者我们可以放一个 JSON body 包含文件信息？
            // 保持简单：空 Body
            let new_req = new_req.body(Body::empty())?;
            
            Ok(PluginAction::PassThrough(new_req))
            
        } else {
            // 非 multipart 请求，直接通过
            Ok(PluginAction::PassThrough(req))
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
