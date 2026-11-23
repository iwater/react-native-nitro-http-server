use super::{Plugin, PluginAction};
use hyper::{Body, Request, Response};
use serde_json::Value;

pub struct PluginManager {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub fn register_plugin(&mut self, plugin: Box<dyn Plugin>) {
        self.plugins.push(plugin);
    }

    pub fn initialize(&self, config: &Value) -> Result<(), anyhow::Error> {
        if let Some(plugins_config) = config.as_object() {
            for plugin in &self.plugins {
                let name = plugin.name();
                if let Some(plugin_config) = plugins_config.get(name) {
                    plugin.on_load(plugin_config)?;
                }
            }
        }
        Ok(())
    }



    pub async fn handle_request(&self, mut req: Request<Body>) -> Result<Response<Body>, Request<Body>> {
        for plugin in &self.plugins {
            if plugin.should_handle(&req) {
                match plugin.handle_request(req).await {
                    Ok(PluginAction::Handled(resp)) => return Ok(resp),
                    Ok(PluginAction::PassThrough(new_req)) => {
                        req = new_req;
                        continue;
                    },
                    Err(e) => {
                        return Ok(Response::builder()
                            .status(500)
                            .body(Body::from(format!("Plugin error: {}", e)))
                            .unwrap());
                    }
                }
            }
        }
        Err(req)
    }
}
