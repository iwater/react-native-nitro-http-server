// WebSocket plugin supporting protocol upgrade and bidirectional messaging
use super::{Plugin, PluginAction};
use async_trait::async_trait;
use dashmap::DashMap;
use futures::stream::StreamExt;
use futures::SinkExt;
use hyper::{Body, Request};
use hyper_tungstenite::{is_upgrade_request, upgrade, HyperWebsocket};
use hyper_tungstenite::tungstenite::Message;
use serde_json::Value;
use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use uuid::Uuid;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WebSocketEventType {
    Open = 1,
    Message = 2,
    Close = 3,
    Error = 4,
}

#[derive(Debug, Clone)]
pub struct WebSocketEvent {
    pub connection_id: String,
    pub event_type: WebSocketEventType,
    pub path: Option<String>,
    pub query: Option<String>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub text_data: Option<String>,
    pub binary_data: Option<Vec<u8>>,
    pub close_code: Option<u16>,
    pub close_reason: Option<String>,
    /// Error message (Error event only; never put error text into `text_data`).
    pub error_message: Option<String>,
}

pub struct ConnectionHandle {
    sender: mpsc::Sender<ConnectionCommand>,
}

/// Outbound queue capacity per connection.
/// Sized at 256 so brief bursts do not prematurely trigger synchronous `try_send` failures.
const CONNECTION_QUEUE_CAPACITY: usize = 256;

/// Outbound connection command (`pub(crate)` for unit tests asserting queue ordering).
pub(crate) enum ConnectionCommand {
    SendText(String),
    SendBinary(Vec<u8>),
    Close(u16, String),
}

impl ConnectionHandle {
    pub async fn send_text(&self, message: String) -> Result<(), anyhow::Error> {
        self.sender
            .send(ConnectionCommand::SendText(message))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send text: {}", e))
    }

    pub async fn send_binary(&self, data: Vec<u8>) -> Result<(), anyhow::Error> {
        self.sender
            .send(ConnectionCommand::SendBinary(data))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send binary: {}", e))
    }

    pub async fn close(&self, code: u16, reason: String) -> Result<(), anyhow::Error> {
        self.sender
            .send(ConnectionCommand::Close(code, reason))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to close: {}", e))
    }
}

pub type WebSocketEventCallback = Box<dyn Fn(WebSocketEvent) + Send + Sync + 'static>;

#[derive(Debug, Clone)]
struct WebSocketConfig {
    enabled: bool,
    mount_path: String,
    max_message_size: Option<usize>,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mount_path: "/ws".to_string(),
            max_message_size: Some(64 * 1024 * 1024), // 64MB default
        }
    }
}

pub struct WebSocketPlugin {
    config: Arc<Mutex<WebSocketConfig>>,
    connections: Arc<DashMap<String, ConnectionHandle>>,
    event_callback: Arc<Mutex<Option<WebSocketEventCallback>>>,
}

impl WebSocketPlugin {
    pub fn new(mount_path: String) -> Self {
        let mut config = WebSocketConfig::default();
        config.mount_path = mount_path;
        
        Self {
            config: Arc::new(Mutex::new(config)),
            connections: Arc::new(DashMap::new()),
            event_callback: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_event_callback<F>(&self, callback: F)
    where
        F: Fn(WebSocketEvent) + Send + Sync + 'static,
    {
        let mut cb = self.event_callback.lock().unwrap();
        *cb = Some(Box::new(callback));
    }

    /// Enqueue a command synchronously via `try_send`.
    ///
    /// Never falls back to `tokio::spawn` because spawned tasks do not preserve
    /// FIFO ordering (e.g. `close` overtaking `send`), report success before
    /// enqueueing, and panic when invoked from synchronous C API callers.
    ///
    /// Returns `true` if queued, or `false` if the connection is missing, closed,
    /// or the queue is full.
    fn enqueue(&self, connection_id: &str, cmd: ConnectionCommand) -> bool {
        // Clone sender before try_send to avoid holding the DashMap shard lock.
        let sender = match self.connections.get(connection_id) {
            Some(handle) => handle.sender.clone(),
            None => {
                crate::log_error!("ws send failed: connection not found: {}", connection_id);
                return false;
            }
        };

        match sender.try_send(cmd) {
            Ok(()) => true,
            Err(e) => {
                crate::log_error!(
                    "ws send failed (connection={}): {} —— 消息未入队，调用方需重试或视为连接异常",
                    connection_id,
                    e
                );
                false
            }
        }
    }

    /// Enqueue a text message (`true` = queued, `false` = missing/full/closed).
    pub fn send_text(&self, connection_id: &str, message: &str) -> bool {
        self.enqueue(connection_id, ConnectionCommand::SendText(message.to_string()))
    }

    /// Enqueue a binary message (same return semantics as [`Self::send_text`]).
    pub fn send_binary(&self, connection_id: &str, data: &[u8]) -> bool {
        self.enqueue(connection_id, ConnectionCommand::SendBinary(data.to_vec()))
    }

    /// Enqueue a close frame after all previously queued messages on the same queue.
    pub fn close_connection(&self, connection_id: &str, code: u16, reason: &str) -> bool {
        self.enqueue(connection_id, ConnectionCommand::Close(code, reason.to_string()))
    }

    /// Test-only: inject a connection handle with a controlled queue capacity to
    /// deterministically test full-queue and closed-receiver states.
    #[cfg(test)]
    pub(crate) fn new_test_connection(
        &self,
        connection_id: &str,
        capacity: usize,
    ) -> mpsc::Receiver<ConnectionCommand> {
        let (tx, rx) = mpsc::channel(capacity);
        self.connections
            .insert(connection_id.to_string(), ConnectionHandle { sender: tx });
        rx
    }

    pub(crate) fn emit_event(&self, event: WebSocketEvent) {
        if let Ok(cb) = self.event_callback.lock() {
            if let Some(ref callback) = *cb {
                callback(event);
            }
        }
    }

    async fn handle_websocket(
        websocket: HyperWebsocket,
        connection_id: String,
        path: String,
        query: Option<String>,
        headers: Option<std::collections::HashMap<String, String>>,
        connections: Arc<DashMap<String, ConnectionHandle>>,
        event_callback: Arc<Mutex<Option<WebSocketEventCallback>>>,
    ) {
        let ws = match websocket.await {
            Ok(ws) => ws,
            Err(e) => {
                crate::log_error!("WebSocket upgrade failed: {}", e);
                if let Ok(cb) = event_callback.lock() {
                    if let Some(ref callback) = *cb {
                        callback(WebSocketEvent {
                            connection_id: connection_id.clone(),
                            event_type: WebSocketEventType::Error,
                            path: None,
                            query: None,
                            headers: None,
                            text_data: None,
                            binary_data: None,
                            close_code: None,
                            close_reason: None,
                            error_message: Some(format!("Upgrade failed: {}", e)),
                        });
                    }
                }
                return;
            }
        };

        let (mut ws_sender, mut ws_receiver) = ws.split();
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<ConnectionCommand>(CONNECTION_QUEUE_CAPACITY);

        connections.insert(
            connection_id.clone(),
            ConnectionHandle { sender: cmd_tx },
        );

        if let Ok(cb) = event_callback.lock() {
            if let Some(ref callback) = *cb {
                callback(WebSocketEvent {
                    connection_id: connection_id.clone(),
                    event_type: WebSocketEventType::Open,
                    path: Some(path.clone()),
                    query: query.clone(),
                    headers: headers.clone(),
                    text_data: None,
                    binary_data: None,
                    close_code: None,
                    close_reason: None,
                    error_message: None,
                });
            }
        }

        crate::log_info!("WebSocket connection opened: {}", connection_id);

        let conn_id_for_send = connection_id.clone();
        let event_cb_for_send = event_callback.clone();
        let close_info: Arc<Mutex<Option<(u16, String)>>> = Arc::new(Mutex::new(None));
        let close_info_for_send = close_info.clone();
        let close_emitted = Arc::new(AtomicBool::new(false));
        let close_emitted_for_recv = close_emitted.clone();

        let send_task = tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                let result = match cmd {
                    ConnectionCommand::SendText(text) => {
                        ws_sender.send(Message::Text(text)).await
                    }
                    ConnectionCommand::SendBinary(data) => {
                        ws_sender.send(Message::Binary(data)).await
                    }
                    ConnectionCommand::Close(code, reason) => {
                        *close_info_for_send.lock().unwrap() = Some((code, reason.clone()));
                        let close_frame = hyper_tungstenite::tungstenite::protocol::CloseFrame {
                            code: hyper_tungstenite::tungstenite::protocol::frame::coding::CloseCode::from(code),
                            reason: reason.into(),
                        };
                        let _ = ws_sender.send(Message::Close(Some(close_frame))).await;
                        break;
                    }
                };

                if let Err(e) = result {
                    crate::log_error!("WebSocket send error: {}", e);
                    if let Ok(cb) = event_cb_for_send.lock() {
                        if let Some(ref callback) = *cb {
                            callback(WebSocketEvent {
                                connection_id: conn_id_for_send.clone(),
                                event_type: WebSocketEventType::Error,
                                path: None,
                                query: None,
                                headers: None,
                                text_data: None,
                                binary_data: None,
                                close_code: None,
                                close_reason: None,
                                error_message: Some(format!("Send error: {}", e)),
                            });
                        }
                    }
                    break;
                }
            }
        });

        let conn_id_for_recv = connection_id.clone();
        let event_cb_for_recv = event_callback.clone();

        let recv_task = tokio::spawn(async move {
            while let Some(msg_result) = ws_receiver.next().await {
                match msg_result {
                    Ok(msg) => {
                        match msg {
                            Message::Text(text) => {
                                if let Ok(cb) = event_cb_for_recv.lock() {
                                    if let Some(ref callback) = *cb {
                                        callback(WebSocketEvent {
                                            connection_id: conn_id_for_recv.clone(),
                                            event_type: WebSocketEventType::Message,
                                            path: None,
                                            query: None,
                                            headers: None,
                                            text_data: Some(text),
                                            binary_data: None,
                                            close_code: None,
                                            close_reason: None,
                                            error_message: None,
                                        });
                                    } else {
                                        crate::log_error!("WebSocket callback is None!");
                                    }
                                } else {
                                    crate::log_error!("Failed to lock WebSocket callback mutex!");
                                }
                            }
                            Message::Binary(data) => {
                                if let Ok(cb) = event_cb_for_recv.lock() {
                                    if let Some(ref callback) = *cb {
                                        callback(WebSocketEvent {
                                            connection_id: conn_id_for_recv.clone(),
                                            event_type: WebSocketEventType::Message,
                                            path: None,
                                            query: None,
                                            headers: None,
                                            text_data: None,
                                            binary_data: Some(data),
                                            close_code: None,
                                            close_reason: None,
                                            error_message: None,
                                        });
                                    }
                                }
                            }
                            Message::Close(frame) => {
                                let (code, reason) = frame
                                    .map(|f| (u16::from(f.code), f.reason.to_string()))
                                    .unwrap_or((1000, String::new()));

                                close_emitted_for_recv.store(true, Ordering::SeqCst);
                                if let Ok(cb) = event_cb_for_recv.lock() {
                                    if let Some(ref callback) = *cb {
                                        callback(WebSocketEvent {
                                            connection_id: conn_id_for_recv.clone(),
                                            event_type: WebSocketEventType::Close,
                                            path: None,
                                            query: None,
                                            headers: None,
                                            text_data: None,
                                            binary_data: None,
                                            close_code: Some(code),
                                            close_reason: Some(reason),
                                            error_message: None,
                                        });
                                    }
                                }
                                break;
                            }
                            Message::Ping(_) | Message::Pong(_) => {}
                            Message::Frame(_) => {}
                        }
                    }
                    Err(e) => {
                        crate::log_error!("WebSocket receive error: {}", e);
                        if let Ok(cb) = event_cb_for_recv.lock() {
                            if let Some(ref callback) = *cb {
                                callback(WebSocketEvent {
                                    connection_id: conn_id_for_recv.clone(),
                                    event_type: WebSocketEventType::Error,
                                    path: None,
                                    query: None,
                                    headers: None,
                                    text_data: None,
                                    binary_data: None,
                                    close_code: None,
                                    close_reason: None,
                                    error_message: Some(format!("Receive error: {}", e)),
                                });
                            }
                        }
                        break;
                    }
                }
            }
        });

        // Abort the peer task as soon as either side finishes to avoid leaked background tasks.
        let mut send_task = send_task;
        let mut recv_task = recv_task;
        tokio::select! {
            _ = &mut send_task => {}
            _ = &mut recv_task => {}
        }
        send_task.abort();
        recv_task.abort();

        connections.remove(&connection_id);

        // Ensure exactly one Close event is emitted even on abrupt disconnects.
        if !close_emitted.swap(true, Ordering::SeqCst) {
            let (code, reason) = close_info
                .lock()
                .unwrap()
                .take()
                .unwrap_or((1006, String::new()));
            if let Ok(cb) = event_callback.lock() {
                if let Some(ref callback) = *cb {
                    callback(WebSocketEvent {
                        connection_id: connection_id.clone(),
                        event_type: WebSocketEventType::Close,
                        path: None,
                        query: None,
                        headers: None,
                        text_data: None,
                        binary_data: None,
                        close_code: Some(code),
                        close_reason: Some(reason),
                        error_message: None,
                    });
                }
            }
        }
        crate::log_info!("WebSocket connection closed: {}", connection_id);
    }
}

#[async_trait]
impl Plugin for WebSocketPlugin {
    fn name(&self) -> &str {
        "websocket"
    }

    fn on_load(&self, config: &Value) -> Result<(), anyhow::Error> {
        let mut cfg = self.config.lock().unwrap();

        if let Some(enabled) = config.get("enabled").and_then(|v| v.as_bool()) {
            cfg.enabled = enabled;
        }

        if let Some(path) = config.get("path").and_then(|v| v.as_str()) {
            cfg.mount_path = path.to_string();
        }

        if let Some(max_size) = config.get("max_message_size").and_then(|v| v.as_u64()) {
            cfg.max_message_size = Some(max_size as usize);
        }

        crate::log_info!(
            "WebSocket plugin loaded: mount={}, enabled={}",
            cfg.mount_path,
            cfg.enabled
        );
        Ok(())
    }

    fn should_handle(&self, req: &Request<Body>) -> bool {
        let config = self.config.lock().unwrap();

        if !config.enabled {
            return false;
        }

        let path = req.uri().path();

        if !super::path_matches_mount(path, &config.mount_path) {
            return false;
        }

        is_upgrade_request(req)
    }

    async fn handle_request(&self, req: Request<Body>) -> Result<PluginAction, anyhow::Error> {
        let connection_id = Uuid::new_v4().to_string();

        crate::log_info!(
            "Upgrading WebSocket connection: {} (path: {})",
            connection_id,
            req.uri().path()
        );

        let path = req.uri().path().to_string();
        let query = req.uri().query().map(|q| q.to_string());
        
        let headers: std::collections::HashMap<String, String> = req.headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str().ok().map(|val| (k.to_string(), val.to_string()))
            })
            .collect();
        
        let max_message_size = self.config.lock().unwrap().max_message_size;
        let mut ws_config = hyper_tungstenite::tungstenite::protocol::WebSocketConfig::default();
        ws_config.max_message_size = max_message_size;
        ws_config.max_frame_size = max_message_size;

        let (response, websocket) = upgrade(req, Some(ws_config))?;

        let connections = self.connections.clone();
        let event_callback = self.event_callback.clone();
        let conn_id = connection_id.clone();

        tokio::spawn(async move {
            Self::handle_websocket(websocket, conn_id, path, query, Some(headers), connections, event_callback).await;
        });

        Ok(PluginAction::Handled(response))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Global WebSocket plugin instance (legacy single-mount accessor for C API).
pub static WEBSOCKET_PLUGIN: once_cell::sync::Lazy<Mutex<Option<Arc<WebSocketPlugin>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));

/// Global WebSocket plugin instances across all mounts.
pub static WEBSOCKET_PLUGINS: once_cell::sync::Lazy<Mutex<Vec<Arc<WebSocketPlugin>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Vec::new()));

/// Clear all global WebSocket plugin registrations (called when starting a new server).
pub fn clear_global_websocket_plugins() {
    WEBSOCKET_PLUGINS.lock().unwrap().clear();
    *WEBSOCKET_PLUGIN.lock().unwrap() = None;
}

/// Register a global WebSocket plugin and attach any saved C callback.
pub fn register_global_websocket_plugin(plugin: Arc<WebSocketPlugin>) {
    *WEBSOCKET_PLUGIN.lock().unwrap() = Some(plugin.clone());
    WEBSOCKET_PLUGINS.lock().unwrap().push(plugin.clone());
    crate::c_api::apply_saved_websocket_callback(&plugin);
}

/// Replace global WebSocket plugin registration with a single instance.
pub fn set_global_websocket_plugin(plugin: Arc<WebSocketPlugin>) {
    *WEBSOCKET_PLUGIN.lock().unwrap() = Some(plugin.clone());
    {
        let mut plugins = WEBSOCKET_PLUGINS.lock().unwrap();
        plugins.clear();
        plugins.push(plugin.clone());
    }
    crate::c_api::apply_saved_websocket_callback(&plugin);
}

/// Return the most recently registered global WebSocket plugin instance.
pub fn get_global_websocket_plugin() -> Option<Arc<WebSocketPlugin>> {
    WEBSOCKET_PLUGIN.lock().unwrap().clone()
}

/// Return all registered global WebSocket plugin instances.
pub fn get_all_global_websocket_plugins() -> Vec<Arc<WebSocketPlugin>> {
    let list = WEBSOCKET_PLUGINS.lock().unwrap().clone();
    if !list.is_empty() {
        return list;
    }
    if let Some(p) = WEBSOCKET_PLUGIN.lock().unwrap().clone() {
        return vec![p];
    }
    Vec::new()
}
