use super::Plugin;
use async_trait::async_trait;
use hyper::{Body, Request, Response, StatusCode};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::any::Any;

// 从 lib.rs 中访问公共 API
use crate::utils::get_content_type;

// 目录列表配置
#[derive(Debug, Clone)]
pub struct DirListConfig {
    pub enabled: bool,
    pub show_hidden: bool,
}

impl Default for DirListConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            show_hidden: false,
        }
    }
}

// 目录条目信息
struct DirEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: String,
    icon: &'static str,
}

// 静态文件服务插件配置
#[derive(Debug, Clone)]
struct StaticFileConfig {
    enabled: bool,
    mount_path: String,
    root_dir: PathBuf,
    /// `root_dir.canonicalize()` 的缓存（每请求一次 canonicalize 是纯浪费）。
    ///
    /// ⚠️ **只在成功时缓存**，见 [`StaticFilePlugin::canonical_root_cached`]。
    canonical_root: Option<PathBuf>,
    dir_list: DirListConfig,
    default_index: Vec<String>,
    custom_headers: std::collections::HashMap<String, String>,
}

impl Default for StaticFileConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mount_path: "/".to_string(),
            root_dir: PathBuf::from("."),
            canonical_root: None,
            dir_list: DirListConfig::default(),
            default_index: vec!["index.html".to_string(), "index.htm".to_string()],
            custom_headers: std::collections::HashMap::new(),
        }
    }
}

pub struct StaticFilePlugin {
    config: Arc<Mutex<StaticFileConfig>>,
}

impl StaticFilePlugin {
    pub fn new(mount_path: String, root_dir: PathBuf) -> Self {
        let mut config = StaticFileConfig::default();
        config.mount_path = mount_path;
        config.root_dir = root_dir;
        // 构造时就把 canonical root 算出来（生产路径上 root 此时一定已存在：
        // app_server 的 static mount 分支会先检查 exists/is_dir）
        Self::canonical_root_cached(&mut config);
        Self {
            config: Arc::new(Mutex::new(config)),
        }
    }

    /// 取 `root_dir` 的 canonical 形式（必要时重算）。
    ///
    /// 为什么**不把失败也缓存成 `None`**：`canonicalize()` 对不存在的目录会失败，
    /// 而 root 目录有可能是服务器启动**之后**才创建的（例如 App 首次运行才建文档目录）。
    /// 若把失败缓存住，那条路径会**永久静默失效** —— 改之前每请求重算时它是能自愈的。
    /// 所以：成功才写回，`None` 时下次再试。
    ///
    /// 注意是关联函数（不接 `&self`）：调用点通常已经持有 `self.config` 的锁，
    /// 再 `lock()` 一次会**死锁**（std 的 Mutex 不可重入）。
    fn canonical_root_cached(cfg: &mut StaticFileConfig) -> Option<PathBuf> {
        if cfg.canonical_root.is_none() {
            cfg.canonical_root = cfg.root_dir.canonicalize().ok();
        }
        cfg.canonical_root.clone()
    }

    /// 构造文件响应：GET 走**流式**（不再整文件读进内存），HEAD 只回头。
    ///
    /// 显式带 `Content-Length`：流式 body 的 size hint 不是精确值，hyper 会退化成
    /// `Transfer-Encoding: chunked`；显式给出长度后 hyper 用 `Encoder::length`，
    /// 线上格式与改动前一致（客户端不感知），HEAD 也才有意义。
    async fn file_response(path: &Path, is_head: bool) -> Result<Response<Body>, anyhow::Error> {
        let metadata = tokio::fs::metadata(path).await?;
        let content_type = get_content_type(path);

        let body = if is_head {
            Body::empty()
        } else {
            let file = tokio::fs::File::open(path).await?;
            Body::wrap_stream(tokio_util::io::ReaderStream::new(file))
        };

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", content_type)
            .header("Content-Length", metadata.len())
            .body(body)?)
    }
}

#[async_trait]
impl Plugin for StaticFilePlugin {
    fn name(&self) -> &str {
        "static_file"
    }

    fn on_load(&self, config: &Value) -> Result<(), anyhow::Error> {
        let mut cfg = self.config.lock().unwrap();
        
        // 解析其他配置
        if let Some(enabled) = config.get("enabled").and_then(|v| v.as_bool()) {
            cfg.enabled = enabled;
        }
        
        // mount_path 和 root_dir 通常在构造时传入，但也可以覆盖 (如果 mounts配置对象包含这些)
        // 在新架构下，AppHttpServer 负责提取 path 和 root，所以这里主要是 dir_list 等
        
        // 解析目录列表配置
        if let Some(dir_list) = config.get("dir_list") {
            cfg.dir_list.enabled = dir_list.get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            cfg.dir_list.show_hidden = dir_list.get("show_hidden")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        }
        
        // 解析默认索引文件
        if let Some(index_files) = config.get("default_index").and_then(|v| v.as_array()) {
            cfg.default_index.clear();
            for file in index_files {
                if let Some(filename) = file.as_str() {
                    cfg.default_index.push(filename.to_string());
                }
            }
        }
        
        // 解析自定义响应头
        cfg.custom_headers = super::parse_custom_headers(config);

        // root_dir 可能是在 new() 之后才创建的 —— 这里再算一次（成功才写回）
        Self::canonical_root_cached(&mut cfg);

        crate::log_info!("Static file plugin loaded: mount={}, root={:?}, enabled={}", 
            cfg.mount_path, cfg.root_dir, cfg.enabled);
        Ok(())
    }

    fn should_handle(&self, req: &Request<Body>) -> bool {
        let mut config = self.config.lock().unwrap();

        // 只处理 GET 与 HEAD。
        // HEAD 必须接管 —— 否则 HEAD /s/a.txt 会落到用户 callback，拿不到文件的
        // Content-Type / Content-Length（客户端做条件请求或探测文件时全靠它）。
        if req.method() != hyper::Method::GET && req.method() != hyper::Method::HEAD {
            return false;
        }
        
        // 必须启用
        if !config.enabled {
            return false;
        }
        
        let path = req.uri().path();
        
        // 检查 mount 点（带路径段边界：挂 /static 不接管 /staticfoo/...）
        if !super::path_matches_mount(path, &config.mount_path) {
            return false;
        }
        
        // 解析文件路径（percent-decode 必须在这里就做 —— 下面要做存在性检查，
        // 用未解码的 "hello%20world.txt" 去找文件必然 canonicalize 失败 → 返回 false，
        // 请求根本走不到 handle_inner）
        let sanitized_path = super::resolve_relative(&config.mount_path, path);
        let file_path = config.root_dir.join(&sanitized_path);
        
        // 检验文件是否存在且在 root_dir 内（canonical_root 走缓存，不再每请求一次系统调用）
        if let Some(canonical_root) = Self::canonical_root_cached(&mut config) {
            if let Ok(canonical_file) = file_path.canonicalize() {
                if canonical_file.starts_with(&canonical_root) {
                    return canonical_file.is_file() || canonical_file.is_dir();
                }
            }
            
            // 特殊情况：如果是根路径 "/" (或 mount_path 本身)，且 sanitized_path 为空，
            // 且 root_dir 存在，则由 handle_request 进一步决定（如检查 index.html）
            // 这里仅仅为了性能，可以简单判断 root 是否存在
            if sanitized_path.is_empty() {
                return true;
            }
        }
        
        false
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

impl StaticFilePlugin {
    async fn handle_inner(&self, req: Request<Body>) -> Result<super::PluginAction, anyhow::Error> {
        let mut config = self.config.lock().unwrap().clone();
        let path = req.uri().path().to_string();
        let is_head = req.method() == hyper::Method::HEAD;

        // 剥离 mount_path 并 percent-decode（与 should_handle 同一套逻辑，见 resolve_relative）
        let sanitized_path = super::resolve_relative(&config.mount_path, &path);
        let file_path = config.root_dir.join(&sanitized_path);

        let canonical_root = Self::canonical_root_cached(&mut config)
            .ok_or_else(|| anyhow::anyhow!("static root 不可用: {:?}", config.root_dir))?;

        // 尝试规范化文件路径
        if let Ok(canonical_file) = file_path.canonicalize() {
            // 确保在根目录内
            // ⚠️ 这是 decode 之后唯一的边界：`%2e%2e` 会解成 `..`，只有这一步能挡住它
            if !canonical_file.starts_with(&canonical_root) {
                return Ok(super::PluginAction::Handled(Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .body(Body::from("403 Forbidden"))
                    .unwrap()));
            }

            // 隐藏文件/隐藏目录检查：任意相对路径段以 '.' 开头且未开启 show_hidden 时一律拒绝
            if !config.dir_list.show_hidden {
                if let Ok(rel) = canonical_file.strip_prefix(&canonical_root) {
                    let has_hidden_component = rel.components().any(|c| {
                        matches!(c, std::path::Component::Normal(os) if os.to_string_lossy().starts_with('.'))
                    });
                    if has_hidden_component {
                        return Ok(super::PluginAction::Handled(Response::builder()
                            .status(StatusCode::FORBIDDEN)
                            .body(Body::from("403 Forbidden (Hidden File)"))
                            .unwrap()));
                    }
                }
            }

            if canonical_file.is_file() {
                // 直接返回文件（GET 流式 / HEAD 只回头）
                return Ok(super::PluginAction::Handled(
                    Self::file_response(&canonical_file, is_head).await?
                ));
            } else if canonical_file.is_dir() {
                // 这是一个目录，尝试查找索引文件
                for index_name in &config.default_index {
                    let index_path = canonical_file.join(index_name);
                    if index_path.is_file() {
                        return Ok(super::PluginAction::Handled(
                            Self::file_response(&index_path, is_head).await?
                        ));
                    }
                }

                // 没有索引文件，检查是否启用目录列表
                if config.dir_list.enabled {
                    let resp = self.generate_directory_listing(&path, &canonical_file, &config.dir_list).await?;
                    return Ok(super::PluginAction::Handled(resp));
                }
            }
        } else {
            // 文件不存在，可能是请求根目录 (例如 mount_path/ -> stripped is "", or mount_path -> stripped is "")
            if sanitized_path.is_empty() {
                // 检查根目录下的索引文件
                for index_name in &config.default_index {
                    let index_path = canonical_root.join(index_name);
                    if index_path.is_file() {
                        return Ok(super::PluginAction::Handled(
                            Self::file_response(&index_path, is_head).await?
                        ));
                    }
                }

                // 没有索引文件，检查是否启用目录列表
                if config.dir_list.enabled {
                    // 对于根目录列表，使用请求路径作为显示路径
                    let display_path = if path.ends_with('/') { path.clone() } else { format!("{}/", path) };
                    let resp = self.generate_directory_listing(&display_path, &canonical_root, &config.dir_list).await?;
                    return Ok(super::PluginAction::Handled(resp));
                }
            }
        }

        Ok(super::PluginAction::Handled(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("404 Not Found"))
            .unwrap()))
    }

    async fn generate_directory_listing(
        &self,
        request_path: &str,
        dir_path: &Path,
        dir_config: &DirListConfig,
    ) -> Result<Response<Body>, anyhow::Error> {
        let mut entries = tokio::fs::read_dir(dir_path).await?;
        let mut dir_entries = Vec::new();
        
        while let Some(entry) = entries.next_entry().await? {
            let entry_name = entry.file_name().to_string_lossy().to_string();
            
            // 检查是否隐藏文件
            if !dir_config.show_hidden && entry_name.starts_with('.') {
                continue;
            }
            
            let entry_path = entry.path();
            let is_dir = entry_path.is_dir();
            let metadata = entry.metadata().await.ok();
            
            let size = metadata.as_ref()
                .map(|m| m.len())
                .unwrap_or(0);
            
            let modified = metadata.as_ref()
                .and_then(|m| m.modified().ok())
                .map(|t| {
                    let datetime: chrono::DateTime<chrono::Local> = t.into();
                    datetime.format("%Y-%m-%d %H:%M").to_string()
                })
                .unwrap_or_else(|| "-".to_string());
            
            let icon = get_file_icon(&entry_path, is_dir);
            
            dir_entries.push(DirEntry {
                name: entry_name,
                is_dir,
                size,
                modified,
                icon,
            });
        }
        
        let html = generate_directory_listing_html(request_path, dir_entries);
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(Body::from(html))
            .unwrap())
    }
}

// 根据文件扩展名获取图标
fn get_file_icon(path: &Path, is_dir: bool) -> &'static str {
    if is_dir {
        return "📁";
    }
    
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        match ext.to_lowercase().as_str() {
            // 图片
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "ico" | "bmp" => "🖼️",
            // 视频
            "mp4" | "webm" | "avi" | "mov" | "mkv" | "ogv" | "flv" => "🎬",
            // 音频
            "mp3" | "wav" | "ogg" | "m4a" | "flac" | "aac" => "🎵",
            // 文档
            "pdf" => "📕",
            "doc" | "docx" => "📘",
            "xls" | "xlsx" => "📗",
            "ppt" | "pptx" => "📙",
            // 代码
            "html" | "htm" => "🌐",
            "css" => "🎨",
            "js" | "ts" | "jsx" | "tsx" => "📜",
            "json" => "📋",
            "rs" | "go" | "py" | "rb" | "java" | "c" | "cpp" | "h" | "hpp" => "💻",
            // 压缩
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" => "📦",
            // 文本
            "txt" | "md" | "log" => "📝",
            // 配置
            "yaml" | "yml" | "toml" | "ini" | "cfg" | "conf" => "⚙️",
            _ => "📄"
        }
    } else {
        "📄"
    }
}

// 格式化文件大小
fn format_file_size(size: u64) -> String {
    if size < 1024 {
        format!("{} B", size)
    } else if size < 1024 * 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else if size < 1024 * 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", size as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

fn encode_uri_component(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

// 生成目录列表 HTML
fn generate_directory_listing_html(request_path: &str, entries: Vec<DirEntry>) -> String {
    let mut html = String::new();

    // 规范化路径段（先 decode 再统一编码 href，避免二次编码，同时防止 XSS）
    let raw_parts: Vec<&str> = request_path.split('/').filter(|s| !s.is_empty()).collect();
    let decoded_parts: Vec<String> = raw_parts
        .iter()
        .map(|p| {
            urlencoding::decode(p)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| (*p).to_string())
        })
        .collect();

    // 面包屑导航
    let mut breadcrumb = String::from(r#"<a href="/">🏠 Root</a>"#);
    let mut accumulated_href = String::new();
    for part in &decoded_parts {
        accumulated_href.push('/');
        accumulated_href.push_str(&encode_uri_component(part));
        breadcrumb.push_str(&format!(
            r#" / <a href="{}">{}</a>"#,
            escape_html(&accumulated_href),
            escape_html(part)
        ));
    }

    let base_href = if decoded_parts.is_empty() {
        "/".to_string()
    } else {
        let encoded: Vec<String> = decoded_parts.iter().map(|p| encode_uri_component(p)).collect();
        format!("/{}/", encoded.join("/"))
    };
    
    // 分离目录和文件，目录在前
    let mut dirs: Vec<&DirEntry> = entries.iter().filter(|e| e.is_dir).collect();
    let mut files: Vec<&DirEntry> = entries.iter().filter(|e| !e.is_dir).collect();
    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    
    // 生成表格行
    let mut rows = String::new();
    
    // 添加返回上级目录
    if !decoded_parts.is_empty() {
        let parent_path = if decoded_parts.len() == 1 {
            "/".to_string()
        } else {
            let encoded_parent: Vec<String> = decoded_parts[..decoded_parts.len() - 1]
                .iter()
                .map(|p| encode_uri_component(p))
                .collect();
            format!("/{}", encoded_parent.join("/"))
        };
        rows.push_str(&format!(r#"
            <tr class="item dir">
                <td class="icon">📂</td>
                <td class="name"><a href="{}">..</a></td>
                <td class="size">-</td>
                <td class="modified">-</td>
            </tr>"#, escape_html(&parent_path)));
    }
    
    // 目录
    for entry in dirs {
        let href = format!("{}{}/", base_href, encode_uri_component(&entry.name));
        rows.push_str(&format!(r#"
            <tr class="item dir">
                <td class="icon">{}</td>
                <td class="name"><a href="{}">{}/</a></td>
                <td class="size">-</td>
                <td class="modified">{}</td>
            </tr>"#, entry.icon, escape_html(&href), escape_html(&entry.name), escape_html(&entry.modified)));
    }
    
    // 文件
    for entry in files {
        let href = format!("{}{}", base_href, encode_uri_component(&entry.name));
        rows.push_str(&format!(r#"
            <tr class="item file">
                <td class="icon">{}</td>
                <td class="name"><a href="{}">{}</a></td>
                <td class="size">{}</td>
                <td class="modified">{}</td>
            </tr>"#, entry.icon, escape_html(&href), escape_html(&entry.name), format_file_size(entry.size), escape_html(&entry.modified)));
    }
    
    let title = if decoded_parts.is_empty() {
        "Index of /".to_string()
    } else if request_path.ends_with('/') {
        format!("Index of /{}/", decoded_parts.join("/"))
    } else {
        format!("Index of /{}", decoded_parts.join("/"))
    };
    let escaped_title = escape_html(&title);
    
    html.push_str(&format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{}</title>
    <style>
        :root {{
            --bg: #f8fafc;
            --card-bg: #ffffff;
            --text: #1e293b;
            --text-secondary: #64748b;
            --border: #e2e8f0;
            --hover: #f1f5f9;
            --accent: #3b82f6;
            --accent-hover: #2563eb;
        }}
        @media (prefers-color-scheme: dark) {{
            :root {{
                --bg: #0f172a;
                --card-bg: #1e293b;
                --text: #f1f5f9;
                --text-secondary: #94a3b8;
                --border: #334155;
                --hover: #334155;
                --accent: #60a5fa;
                --accent-hover: #93c5fd;
            }}
        }}
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', sans-serif;
            background: var(--bg);
            color: var(--text);
            line-height: 1.6;
            min-height: 100vh;
        }}
        .container {{
            max-width: 1000px;
            margin: 0 auto;
            padding: 2rem 1rem;
        }}
        header {{
            margin-bottom: 2rem;
        }}
        h1 {{
            font-size: 1.5rem;
            font-weight: 600;
            margin-bottom: 0.5rem;
            color: var(--accent);
        }}
        .breadcrumb {{
            font-size: 0.9rem;
            color: var(--text-secondary);
        }}
        .breadcrumb a {{
            color: var(--accent);
            text-decoration: none;
        }}
        .breadcrumb a:hover {{
            text-decoration: underline;
        }}
        .card {{
            background: var(--card-bg);
            border-radius: 12px;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
            overflow: hidden;
            border: 1px solid var(--border);
        }}
        table {{
            width: 100%;
            border-collapse: collapse;
        }}
        th {{
            text-align: left;
            padding: 0.75rem 1rem;
            font-weight: 600;
            font-size: 0.75rem;
            text-transform: uppercase;
            letter-spacing: 0.05em;
            color: var(--text-secondary);
            background: var(--bg);
            border-bottom: 1px solid var(--border);
        }}
        td {{
            padding: 0.75rem 1rem;
            border-bottom: 1px solid var(--border);
        }}
        tr:last-child td {{
            border-bottom: none;
        }}
        tr.item:hover {{
            background: var(--hover);
        }}
        .icon {{
            width: 40px;
            text-align: center;
            font-size: 1.25rem;
        }}
        .name {{
            font-weight: 500;
        }}
        .name a {{
            color: var(--text);
            text-decoration: none;
        }}
        .name a:hover {{
            color: var(--accent);
        }}
        .dir .name a {{
            color: var(--accent);
        }}
        .size, .modified {{
            color: var(--text-secondary);
            font-size: 0.9rem;
            white-space: nowrap;
        }}
        .size {{
            width: 100px;
            text-align: right;
        }}
        .modified {{
            width: 180px;
        }}
        @media (max-width: 640px) {{
            .modified {{
                display: none;
            }}
            .size {{
                width: auto;
            }}
        }}
        footer {{
            margin-top: 2rem;
            text-align: center;
            font-size: 0.8rem;
            color: var(--text-secondary);
        }}
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1>📂 Directory Listing</h1>
            <div class="breadcrumb">{}</div>
        </header>
        <div class="card">
            <table>
                <thead>
                    <tr>
                        <th></th>
                        <th>Name</th>
                        <th class="size">Size</th>
                        <th class="modified">Modified</th>
                    </tr>
                </thead>
                <tbody>{}</tbody>
            </table>
        </div>
        <footer>
            Powered by React Native HTTP Server
        </footer>
    </div>
</body>
</html>"#, escaped_title, breadcrumb, rows));
    
    html
}
