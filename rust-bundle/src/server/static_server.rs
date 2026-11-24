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

        let addr = crate::utils::resolve_bind_addr(&host, port);

        // Build runtime before binding because AddrIncoming::bind requires an active Tokio reactor.
        let rt = match Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                crate::log_error!("Failed to create runtime: {}", e);
                return 0;
            }
        };

        let _guard = rt.enter();

        let incoming = match hyper::server::conn::AddrIncoming::bind(&addr) {
            Ok(inc) => inc,
            Err(e) => {
                crate::log_error!("Failed to bind to {}: {}", addr, e);
                return 0;
            }
        };

        let actual_port = incoming.local_addr().port();
        self.port = actual_port;

        // Construct the plugin once at startup so the canonicalized root_dir is cached across requests.
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

/// Delegate static file serving to `StaticFilePlugin` (mount = `/`) so index
/// fallback, hidden-file blocking, and percent-decoding match `AppHttpServer`.
async fn handle_static_request_inner(
    req: Request<Body>,
    plugin: Arc<crate::plugins::static_file::StaticFilePlugin>,
) -> Result<Response<Body>, hyper::Error> {
    crate::global::TOTAL_REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    use crate::plugins::Plugin;

    if plugin.should_handle(&req) {
        match plugin.handle_request(req).await {
            Ok(crate::plugins::PluginAction::Handled(resp)) => return Ok(resp),
            Ok(crate::plugins::PluginAction::PassThrough(_)) => {}
            Err(e) => {
                // Return 500 on plugin I/O errors rather than masking them as 404.
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
