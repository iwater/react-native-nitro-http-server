use super::Plugin;
use async_trait::async_trait;
use dav_server::{localfs::LocalFs, DavHandler};
use hyper::{Body, Request, Response};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::any::Any;

pub struct WebDavPlugin {
    handler: Arc<Mutex<Option<DavHandler>>>,
    prefix: Arc<Mutex<String>>,
    custom_headers: Arc<Mutex<std::collections::HashMap<String, String>>>,
}

impl WebDavPlugin {
    pub fn new(prefix: String, root: String) -> Self {
        let handler_builder = DavHandler::builder()
            .filesystem(LocalFs::new(&root, false, false, false))
            .strip_prefix(prefix.clone());
            
        let handler = handler_builder.build_handler();

        Self {
            handler: Arc::new(Mutex::new(Some(handler))),
            prefix: Arc::new(Mutex::new(prefix)),
            custom_headers: Arc::new(Mutex::new(Default::default())),
        }
    }

    async fn handle_inner(&self, req: Request<Body>) -> Result<super::PluginAction, anyhow::Error> {
        let handler_option = self.handler.lock().unwrap().clone();
        if let Some(h) = handler_option {
            // The dav-server handler expects the request path to be relative to its configured prefix.
            // Since we configured `strip_prefix` during handler creation, we can pass the original request.
            let resp = h.handle(req).await;
            let (parts, body) = resp.into_parts();
            Ok(super::PluginAction::Handled(Response::from_parts(parts, Body::wrap_stream(body))))
        } else {
            // This case should ideally not be reached if the plugin is properly initialized.
            // If it is, we return a 500 error.
            Ok(super::PluginAction::Handled(Response::builder()
                .status(500)
                .body(Body::from("WebDAV handler not initialized"))?))
        }
    }
}

#[async_trait]
impl Plugin for WebDavPlugin {
    fn name(&self) -> &str {
        "webdav"
    }

    fn on_load(&self, config: &Value) -> Result<(), anyhow::Error> {
        if let (Some(prefix), Some(root)) = (
            config.get("prefix").and_then(|v| v.as_str()),
            config.get("root").and_then(|v| v.as_str())
        ) {
            let handler = DavHandler::builder()
                .filesystem(LocalFs::new(root, false, false, false))
                .strip_prefix(prefix.to_string())
                .build_handler();

            *self.handler.lock().unwrap() = Some(handler);
            *self.prefix.lock().unwrap() = prefix.to_string();
            
            crate::log_info!("WebDAV plugin updated: prefix={}, root={}", prefix, root);
        }
        *self.custom_headers.lock().unwrap() = super::parse_custom_headers(config);
        Ok(())
    }

    fn should_handle(&self, req: &Request<Body>) -> bool {
        let path = req.uri().path();
        let prefix = self.prefix.lock().unwrap();
        
        // Empty prefix means unmounted (path_matches_mount treats "" as "/").
        if prefix.is_empty() {
            return false;
        }

        super::path_matches_mount(path, prefix.as_str())
    }

    async fn handle_request(&self, req: Request<Body>) -> Result<super::PluginAction, anyhow::Error> {
        let headers = self.custom_headers.lock().unwrap().clone();
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
