use std::sync::{Arc, Mutex};
use std::path::{PathBuf};

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, StatusCode};
use tokio::runtime::Runtime;
use once_cell::sync::Lazy;

pub struct StaticHttpServer {
    pub port: u16,
    pub host: String,
    pub root_dir: PathBuf,
    pub runtime: Option<Runtime>,
}

pub static STATIC_SERVER_HANDLE: Lazy<Mutex<Option<Arc<StaticHttpServer>>>> = Lazy::new(|| Mutex::new(None));

impl StaticHttpServer {
    pub fn new(port: u16, host: String, root_dir: PathBuf) -> Self {
        Self {
            port,
            host,
            root_dir,
            runtime: None,
        }
    }
    
    pub fn start(&mut self) -> u16 {
        let host = self.host.clone();
        let port = self.port;
        let root_dir = self.root_dir.clone();

        // 解析 host 字符串为 IP 地址
        let addr = crate::utils::resolve_bind_addr(&host, port);

        // 先创建 Runtime，因为 AddrIncoming::bind 需要在 Tokio Runtime 上下文中运行
        let rt = match Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                crate::log_error!("Failed to create runtime: {}", e);
                return 0;
            }
        };

        // 进入 Runtime 上下文，确保 AddrIncoming::bind 可以找到 reactor
        let _guard = rt.enter();

        // 使用 try_bind 风格的 AddrIncoming::bind 避免 panic
        let incoming = match hyper::server::conn::AddrIncoming::bind(&addr) {
            Ok(inc) => inc,
            Err(e) => {
                crate::log_error!("Failed to bind to {}: {}", addr, e);
                return 0;
            }
        };

        let actual_port = incoming.local_addr().port();
        self.port = actual_port;

        // 插件**只构造一次**（mount="/"）：构造时会把 root 目录 canonicalize 一次，
        // 而 Task 13 的整条优化就是为了消掉「每请求一次 canonicalize」——
        // 放在请求路径里构造会把它加回来。
        let plugin = Arc::new(crate::plugins::static_file::StaticFilePlugin::new(
            "/".to_string(),
            root_dir,
        ));

        rt.spawn(async move {
            if let Err(e) = Self::run_static_server_with_incoming(incoming, plugin).await {
                crate::log_error!("Static server error: {}", e);
            }
        });

        self.runtime = Some(rt);
        actual_port
    }
    
    async fn run_static_server_with_incoming(
        incoming: hyper::server::conn::AddrIncoming,
        plugin: Arc<crate::plugins::static_file::StaticFilePlugin>,
    ) -> Result<(), hyper::Error> {
        let addr = incoming.local_addr();
        let make_svc = make_service_fn(move |_conn| {
            let plugin = plugin.clone();
            async move {
                Ok::<_, hyper::Error>(service_fn(move |req| {
                    handle_static_request(req, plugin.clone())
                }))
            }
        });

        let server = Server::builder(incoming).serve(make_svc);
        crate::log_info!("Static HTTP Server running on http://{}", addr);
        server.await
    }
    
    #[allow(dead_code)]
    pub fn stop(&mut self) {
        self.runtime = None;
    }
}

// 处理静态文件请求
async fn handle_static_request(
    req: Request<Body>,
    plugin: Arc<crate::plugins::static_file::StaticFilePlugin>,
) -> Result<Response<Body>, hyper::Error> {
    let origin = req.headers().get(hyper::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    if let Some(resp) = crate::cors::preflight_response(&req) {
        return Ok(resp);
    }

    let mut resp = handle_static_request_inner(req, plugin).await?;
    crate::cors::apply_cors(&mut resp, origin.as_deref());
    Ok(resp)
}

/// 静态服务本体：**全部委托给 `StaticFilePlugin`**（mount = `/`）。
///
/// 为什么不再手写第二套路径处理：委托前这里是另一份实现，与 app server 的
/// static mount 行为不一致，实测有三处差异（都已写成回归用例）：
/// 1. 目录请求 404 —— index 回退被写在「`canonicalize()` 失败」的分支里，
///    而目录**能** canonicalize 成功，于是去 `read(目录)` → EISDIR → 404；
/// 2. `/.hidden` 照常 200 服务出去（不挡隐藏文件）；
/// 3. 不做 percent-decode —— 含空格的文件名 404。
///
/// 委托后这些行为与插件（以及 app server 的 static mount）自动一致。
async fn handle_static_request_inner(
    req: Request<Body>,
    plugin: Arc<crate::plugins::static_file::StaticFilePlugin>,
) -> Result<Response<Body>, hyper::Error> {
    // getStats 的 totalRequests
    crate::global::TOTAL_REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    use crate::plugins::Plugin;

    if plugin.should_handle(&req) {
        match plugin.handle_request(req).await {
            Ok(crate::plugins::PluginAction::Handled(resp)) => return Ok(resp),
            Ok(crate::plugins::PluginAction::PassThrough(_)) => {
                // StaticFilePlugin 目前**不会**返回 PassThrough（已核实源码里没有该分支）；
                // 真出现了也没有别的 handler 可以交，按 404 处理。
            }
            Err(e) => {
                // 与 `PluginManager` 的约定保持一致：插件报错 → 500 `Plugin error: {e}`
                // （app server 走的就是这条）。**不能**沿用旧的「一律 404」——
                // 那会把「文件在 canonicalize 与 open 之间消失」「权限不足」这类
                // 真实 IO 错误伪装成「文件不存在」。
                crate::global::ERROR_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from(format!("Plugin error: {}", e)))
                    .unwrap());
            }
        }
    }

    Ok(Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("404 Not Found"))
        .unwrap())
}
