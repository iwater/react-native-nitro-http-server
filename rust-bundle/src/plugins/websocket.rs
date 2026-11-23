// WebSocket 插件 - 支持 WebSocket 协议升级和双向通信
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

/// WebSocket 事件类型
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WebSocketEventType {
    Open = 1,
    Message = 2,
    Close = 3,
    Error = 4,
}

/// WebSocket 事件数据
#[derive(Debug, Clone)]
pub struct WebSocketEvent {
    pub connection_id: String,
    pub event_type: WebSocketEventType,
    pub path: Option<String>,           // 连接路径
    pub query: Option<String>,          // 查询字符串 (如 ?token=abc)
    pub headers: Option<std::collections::HashMap<String, String>>,  // 握手时的 HTTP 头
    pub text_data: Option<String>,
    pub binary_data: Option<Vec<u8>>,
    pub close_code: Option<u16>,
    pub close_reason: Option<String>,
    /// 错误信息（仅 Error 事件）。**不要**再往 text_data 里塞错误文本：
    /// 下游 TS 侧只认 errorMessage，塞 text_data 会被当成普通消息且错误文本丢失。
    pub error_message: Option<String>,
}

/// WebSocket 连接句柄 - 用于发送消息和关闭连接
pub struct ConnectionHandle {
    sender: mpsc::Sender<ConnectionCommand>,
}

/// 每条连接的发送队列容量。
///
/// 因为入队失败是**同步返回 false**（不再悄悄 spawn 到后台），容量太小会让
/// 短暂突发就产生 false —— 调用方拿到 false 通常意味着「连接异常」，误报代价高。
/// 所以从 32 提到 256：256 条积压已经足以说明对端真的消费不动了。
const CONNECTION_QUEUE_CAPACITY: usize = 256;

/// 连接命令
///
/// `pub(crate)` 是为了让 `src/tests/` 下的用例能断言入队顺序。
/// 它不对外暴露（crate-type 是 staticlib/cdylib，无 Rust 消费者）。
pub(crate) enum ConnectionCommand {
    SendText(String),
    SendBinary(Vec<u8>),
    Close(u16, String),
}

impl ConnectionHandle {
    /// 发送文本消息
    pub async fn send_text(&self, message: String) -> Result<(), anyhow::Error> {
        self.sender
            .send(ConnectionCommand::SendText(message))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send text: {}", e))
    }

    /// 发送二进制消息
    pub async fn send_binary(&self, data: Vec<u8>) -> Result<(), anyhow::Error> {
        self.sender
            .send(ConnectionCommand::SendBinary(data))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send binary: {}", e))
    }

    /// 关闭连接
    pub async fn close(&self, code: u16, reason: String) -> Result<(), anyhow::Error> {
        self.sender
            .send(ConnectionCommand::Close(code, reason))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to close: {}", e))
    }
}

/// WebSocket 事件回调类型
pub type WebSocketEventCallback = Box<dyn Fn(WebSocketEvent) + Send + Sync + 'static>;

/// WebSocket 插件配置
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

/// WebSocket 插件
pub struct WebSocketPlugin {
    config: Arc<Mutex<WebSocketConfig>>,
    connections: Arc<DashMap<String, ConnectionHandle>>,
    event_callback: Arc<Mutex<Option<WebSocketEventCallback>>>,
}

impl WebSocketPlugin {
    /// 创建新的 WebSocket 插件
    pub fn new(mount_path: String) -> Self {
        let mut config = WebSocketConfig::default();
        config.mount_path = mount_path;
        
        Self {
            config: Arc::new(Mutex::new(config)),
            connections: Arc::new(DashMap::new()),
            event_callback: Arc::new(Mutex::new(None)),
        }
    }

    /// 设置事件回调
    pub fn set_event_callback<F>(&self, callback: F)
    where
        F: Fn(WebSocketEvent) + Send + Sync + 'static,
    {
        let mut cb = self.event_callback.lock().unwrap();
        *cb = Some(Box::new(callback));
    }

    /// 把一条命令放进连接的发送队列。
    ///
    /// **只用 `try_send`，绝不回退 `tokio::spawn`。** spawn 有两个致命问题：
    /// 1. 不保序 —— 多个 spawn 出去的任务何时拿到队列 permit 不受调用顺序约束，
    ///    典型症状是 `close` 插到已排队的 `send` 前面，连接被提前关掉。
    /// 2. 返回值撒谎 —— `tokio::spawn` 立即返回，于是调用方拿到 true，
    ///    但消息此刻根本还没进队列（甚至可能永远进不去）。
    ///
    /// 另外 spawn 需要 Tokio runtime，而 C API（`ws_send_text` / `ws_send_binary` /
    /// `ws_close`）是同步上下文 —— 在那里 spawn 会直接 panic。
    /// `try_send` 是同步的，两个问题一并解决。
    ///
    /// 返回值语义：**true = 已入队（保序）**；false = 连接不存在 / 队列已满 /
    /// 连接已关闭。false 时调用方应重试或视为连接异常，消息**不会**被悄悄丢弃。
    fn enqueue(&self, connection_id: &str, cmd: ConnectionCommand) -> bool {
        // 先取出 sender 再入队，避免持着 DashMap 的分片锁做可能耗时的操作
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

    /// 发送文本消息到指定连接
    ///
    /// 返回 true 表示已入队；false 表示未入队（连接不存在 / 队列满 / 已关闭），
    /// 消息不会被丢弃也不会被延迟到后台发送。
    pub fn send_text(&self, connection_id: &str, message: &str) -> bool {
        self.enqueue(connection_id, ConnectionCommand::SendText(message.to_string()))
    }

    /// 发送二进制消息到指定连接（返回值语义同 [`Self::send_text`]）
    pub fn send_binary(&self, connection_id: &str, data: &[u8]) -> bool {
        self.enqueue(connection_id, ConnectionCommand::SendBinary(data.to_vec()))
    }

    /// 关闭指定连接（返回值语义同 [`Self::send_text`]）
    ///
    /// 与 `send_*` 共用同一条队列，因此 close 一定排在它之前入队的消息之后执行。
    pub fn close_connection(&self, connection_id: &str, code: u16, reason: &str) -> bool {
        self.enqueue(connection_id, ConnectionCommand::Close(code, reason.to_string()))
    }

    /// **仅测试用**：注入一个连接句柄，并返回它的接收端。
    ///
    /// 为什么需要它：生产路径的句柄由 `handle_websocket` 在升级成功后插入，
    /// 接收端被 send_task 持续消费 —— 想让队列积压到容量上限需要 socket 背压，
    /// 端到端测试里既不稳定又慢。注入后可以由单测直接控制容量与消费时机，
    /// 从而确定性地构造「队列满」和「接收端已关闭」这两种状态。
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

    /// 触发事件回调
    pub(crate) fn emit_event(&self, event: WebSocketEvent) {
        if let Ok(cb) = self.event_callback.lock() {
            if let Some(ref callback) = *cb {
                callback(event);
            }
        }
    }

    /// 处理 WebSocket 连接
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
                // 触发错误事件
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

        // 存储连接句柄
        connections.insert(
            connection_id.clone(),
            ConnectionHandle { sender: cmd_tx },
        );

        // 触发 open 事件
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

        // 发送任务
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

        // 接收任务
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
                            Message::Ping(_) | Message::Pong(_) => {
                                // 自动处理 ping/pong
                            }
                            Message::Frame(_) => {
                                // 忽略原始帧
                            }
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

        // 等待任意一个任务完成，并中止另一侧任务避免后台泄漏
        let mut send_task = send_task;
        let mut recv_task = recv_task;
        tokio::select! {
            _ = &mut send_task => {}
            _ = &mut recv_task => {}
        }
        send_task.abort();
        recv_task.abort();

        // 清理连接
        connections.remove(&connection_id);

        // 确保异常断连或主动 close 时总能触发一次 Close 事件
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

        // 检查 mount 点（带路径段边界：挂 /ws 不接管 /wsfoo/...）
        if !super::path_matches_mount(path, &config.mount_path) {
            return false;
        }

        // 必须是 WebSocket 升级请求
        is_upgrade_request(req)
    }

    async fn handle_request(&self, req: Request<Body>) -> Result<PluginAction, anyhow::Error> {
        // 生成连接 ID
        let connection_id = Uuid::new_v4().to_string();

        crate::log_info!(
            "Upgrading WebSocket connection: {} (path: {})",
            connection_id,
            req.uri().path()
        );

        // 执行 WebSocket 升级
        let path = req.uri().path().to_string();
        let query = req.uri().query().map(|q| q.to_string());
        
        // 提取 HTTP headers
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

        // 启动后台任务处理 WebSocket 连接
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

/// 全局 WebSocket 插件实例（用于 C API 访问，兼容旧接口）
pub static WEBSOCKET_PLUGIN: once_cell::sync::Lazy<Mutex<Option<Arc<WebSocketPlugin>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));

/// 全局 WebSocket 插件实例列表（支持多挂载点）
pub static WEBSOCKET_PLUGINS: once_cell::sync::Lazy<Mutex<Vec<Arc<WebSocketPlugin>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Vec::new()));

/// 清空全局 WebSocket 插件列表（新服务器启动时调用）
pub fn clear_global_websocket_plugins() {
    WEBSOCKET_PLUGINS.lock().unwrap().clear();
    *WEBSOCKET_PLUGIN.lock().unwrap() = None;
}

/// 注册全局 WebSocket 插件实例（追加，支持多挂载点；若已设置 C 回调则立即挂载）
pub fn register_global_websocket_plugin(plugin: Arc<WebSocketPlugin>) {
    *WEBSOCKET_PLUGIN.lock().unwrap() = Some(plugin.clone());
    WEBSOCKET_PLUGINS.lock().unwrap().push(plugin.clone());
    crate::c_api::apply_saved_websocket_callback(&plugin);
}

/// 设置全局 WebSocket 插件实例（替换）
pub fn set_global_websocket_plugin(plugin: Arc<WebSocketPlugin>) {
    *WEBSOCKET_PLUGIN.lock().unwrap() = Some(plugin.clone());
    {
        let mut plugins = WEBSOCKET_PLUGINS.lock().unwrap();
        plugins.clear();
        plugins.push(plugin.clone());
    }
    crate::c_api::apply_saved_websocket_callback(&plugin);
}

/// 获取全局 WebSocket 插件实例（返回最后一个注册的实例）
pub fn get_global_websocket_plugin() -> Option<Arc<WebSocketPlugin>> {
    WEBSOCKET_PLUGIN.lock().unwrap().clone()
}

/// 获取所有全局 WebSocket 插件实例
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
