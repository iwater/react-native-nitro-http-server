// WebSocket 插件单元测试
use crate::plugins::websocket::{
    WebSocketPlugin, WebSocketEventType, WebSocketEvent, ConnectionCommand,
    set_global_websocket_plugin, get_global_websocket_plugin, WEBSOCKET_PLUGIN,
};
use crate::plugins::Plugin;
use hyper::{Request, Body, Method, header};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[test]
fn test_websocket_plugin_should_handle() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    
    // 1. 非 WebSocket 升级请求不应处理
    let req = Request::builder()
        .uri("/ws/test")
        .method(Method::GET)
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req), "Should not handle non-upgrade request");
    
    // 2. 路径不匹配不应处理
    let req = Request::builder()
        .uri("/other/path")
        .method(Method::GET)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "Upgrade")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("Sec-WebSocket-Version", "13")
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req), "Should not handle wrong path");
    
    // 3. 正确的 WebSocket 升级请求应该处理
    let req = Request::builder()
        .uri("/ws/chat")
        .method(Method::GET)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "Upgrade")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("Sec-WebSocket-Version", "13")
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&req), "Should handle valid WebSocket upgrade request");
    
    // 4. 精确路径匹配
    let req = Request::builder()
        .uri("/ws")
        .method(Method::GET)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "Upgrade")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("Sec-WebSocket-Version", "13")
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&req), "Should handle exact path match");
}

#[test]
fn test_websocket_plugin_on_load() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    
    // 测试配置加载
    let config = json!({
        "path": "/websocket",
        "max_message_size": 1048576,
        "enabled": true
    });
    
    let result = plugin.on_load(&config);
    assert!(result.is_ok(), "on_load should succeed");
    
    // 验证配置更新后路径匹配
    let req = Request::builder()
        .uri("/websocket/test")
        .method(Method::GET)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "Upgrade")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("Sec-WebSocket-Version", "13")
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&req), "Should handle new path after config update");
    
    // 旧路径不再匹配
    let req_old = Request::builder()
        .uri("/ws/test")
        .method(Method::GET)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "Upgrade")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("Sec-WebSocket-Version", "13")
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req_old), "Should not handle old path after config update");
}

#[test]
fn test_websocket_plugin_disabled() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    
    // 禁用插件
    let config = json!({
        "enabled": false
    });
    plugin.on_load(&config).unwrap();
    
    // 禁用后不应处理任何请求
    let req = Request::builder()
        .uri("/ws/chat")
        .method(Method::GET)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "Upgrade")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("Sec-WebSocket-Version", "13")
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req), "Should not handle when disabled");
}

#[test]
fn test_websocket_event_types() {
    // 验证事件类型值
    assert_eq!(WebSocketEventType::Open as i32, 1);
    assert_eq!(WebSocketEventType::Message as i32, 2);
    assert_eq!(WebSocketEventType::Close as i32, 3);
    assert_eq!(WebSocketEventType::Error as i32, 4);
}

#[test]
fn test_websocket_plugin_name() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    assert_eq!(plugin.name(), "websocket");
}

#[test]
fn test_websocket_event_has_error_message_field() {
    // 构造一个 Error 事件并验证 error_message 独立于 text_data 存在
    let event = WebSocketEvent {
        connection_id: "c1".to_string(),
        event_type: WebSocketEventType::Error,
        path: None,
        query: None,
        headers: None,
        text_data: None,
        binary_data: None,
        close_code: None,
        close_reason: None,
        error_message: Some("boom".to_string()),
    };
    assert_eq!(event.error_message.as_deref(), Some("boom"));
    assert!(event.text_data.is_none());
}

// ---- 整条 C 边界链路的验证 ----

/// C 回调里抓到的字段（C 字符串已拷成 String，避免指针生命周期问题）
#[derive(Debug)]
struct CapturedEvent {
    event_type: i32,
    error_message: Option<String>,
    text_data: Option<String>,
    text_len: i32,
}

static CAPTURED: Mutex<Option<CapturedEvent>> = Mutex::new(None);

extern "C" fn capture_c_event(ev: *const crate::c_api::CWebSocketEvent) {
    if ev.is_null() {
        return;
    }
    unsafe {
        let e = &*ev;
        let to_string = |p: *const std::os::raw::c_char| {
            if p.is_null() {
                None
            } else {
                Some(std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
            }
        };
        *CAPTURED.lock().unwrap() = Some(CapturedEvent {
            event_type: e.event_type,
            error_message: to_string(e.error_message),
            text_data: to_string(e.text_data),
            text_len: e.text_len,
        });
    }
}

/// 全局 WS 插件是全局状态：用完必须精确还原（包括 None），否则污染其它用例。
struct GlobalWsGuard {
    prev: Option<Arc<WebSocketPlugin>>,
}

impl GlobalWsGuard {
    fn install(plugin: Arc<WebSocketPlugin>) -> Self {
        let prev = get_global_websocket_plugin();
        set_global_websocket_plugin(plugin);
        Self { prev }
    }
}

impl Drop for GlobalWsGuard {
    fn drop(&mut self) {
        let prev = self.prev.take();
        *WEBSOCKET_PLUGIN.lock().unwrap() = prev.clone();
        let mut list = crate::plugins::websocket::WEBSOCKET_PLUGINS.lock().unwrap();
        list.clear();
        if let Some(p) = prev {
            list.push(p);
        }
    }
}

/// 这是本 Task 真正要保证的事：Error 事件的文本必须出现在 C 结构体的
/// `error_message` 字段里，而**不是** `text_data`。
/// 修复前 text_data 有值、error_message 字段根本不存在 → TS 侧永远 "Unknown error"。
#[test]
fn test_error_message_reaches_c_callback() {
    let plugin = Arc::new(WebSocketPlugin::new("/ws".to_string()));
    let _guard = GlobalWsGuard::install(plugin.clone());

    crate::c_api::set_websocket_callback(capture_c_event);
    *CAPTURED.lock().unwrap() = None;

    plugin.emit_event(WebSocketEvent {
        connection_id: "c1".to_string(),
        event_type: WebSocketEventType::Error,
        path: None,
        query: None,
        headers: None,
        text_data: None,
        binary_data: None,
        close_code: None,
        close_reason: None,
        error_message: Some("Receive error: boom".to_string()),
    });

    let captured = CAPTURED
        .lock()
        .unwrap()
        .take()
        .expect("C 回调没有被调用：set_websocket_callback 没挂上");

    assert_eq!(captured.event_type, 4, "应该是 Error 事件");
    assert_eq!(
        captured.error_message.as_deref(),
        Some("Receive error: boom"),
        "error_message 没传到 C 边界"
    );
    assert!(
        captured.text_data.is_none(),
        "Error 事件的文本不应再塞进 text_data，实际为 {:?}",
        captured.text_data
    );
    assert_eq!(captured.text_len, 0, "text_len 应与 text_data 一致为 0");
}

/// 非 Error 事件不应带 error_message（避免把正常事件也标成错误）
#[test]
fn test_non_error_event_has_no_error_message() {
    let plugin = Arc::new(WebSocketPlugin::new("/ws".to_string()));
    let _guard = GlobalWsGuard::install(plugin.clone());

    crate::c_api::set_websocket_callback(capture_c_event);
    *CAPTURED.lock().unwrap() = None;

    plugin.emit_event(WebSocketEvent {
        connection_id: "c1".to_string(),
        event_type: WebSocketEventType::Message,
        path: None,
        query: None,
        headers: None,
        text_data: Some("hello".to_string()),
        binary_data: None,
        close_code: None,
        close_reason: None,
        error_message: None,
    });

    let captured = CAPTURED.lock().unwrap().take().expect("C 回调没有被调用");
    assert_eq!(captured.event_type, 2);
    assert!(captured.error_message.is_none());
    assert_eq!(captured.text_data.as_deref(), Some("hello"));
    assert_eq!(captured.text_len, 5);
}

// ---- Task 7: 发送保序 + 返回值诚实 ----

/// 把队列里现有的命令按出队顺序读成种类标记。`try_recv` 是同步的，
/// 不会让出调度 —— 因此它能区分「同步入队」与「丢给后台任务稍后入队」。
fn drain_kinds(rx: &mut tokio::sync::mpsc::Receiver<ConnectionCommand>) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    while let Ok(cmd) = rx.try_recv() {
        kinds.push(match cmd {
            ConnectionCommand::SendText(_) => "text",
            ConnectionCommand::SendBinary(_) => "binary",
            ConnectionCommand::Close(_, _) => "close",
        });
    }
    kinds
}

/// 未知连接：三个方法都必须返回 false。
/// 这是**防回归守卫** —— 修复前后都应通过，钉住的是「连接不存在」这条分支。
#[test]
fn test_ws_send_returns_false_for_unknown_connection() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    assert!(!plugin.send_text("no-such-conn", "hello"));
    assert!(!plugin.send_binary("no-such-conn", b"x"));
    assert!(!plugin.close_connection("no-such-conn", 1000, ""));
}

/// 队列满时 `send_text` 必须返回 false。
/// 修复前它回退 `tokio::spawn` 并**照样返回 true** —— 消息事实上没进队列，
/// 调用方却以为发成功了。
#[tokio::test]
async fn test_ws_send_text_returns_false_when_channel_full() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    // 容量 2 且从不消费 → 第 3 条起必然 Full
    let mut rx = plugin.new_test_connection("c1", 2);

    assert!(plugin.send_text("c1", "m1"), "第 1 条应成功入队");
    assert!(plugin.send_text("c1", "m2"), "第 2 条应成功入队");
    assert!(
        !plugin.send_text("c1", "m3"),
        "队列满时必须返回 false：修复前会回退 spawn 并返回 true"
    );

    // 队列里只应有前两条，m3 没有被偷偷塞进去
    assert_eq!(
        drain_kinds(&mut rx),
        vec!["text", "text"],
        "满队列时不应有额外的命令入队"
    );
}

/// 队列满时 `send_binary` / `close_connection` 也必须返回 false。
/// 修复前这两个方法**总是** spawn 并立即返回 true，完全不看通道状态。
#[tokio::test]
async fn test_ws_send_binary_and_close_return_false_when_channel_full() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    let mut rx = plugin.new_test_connection("c1", 1);

    assert!(plugin.send_binary("c1", b"b1"), "第 1 条应成功入队");
    assert!(
        !plugin.send_binary("c1", b"b2"),
        "队列满时 send_binary 必须返回 false"
    );
    assert!(
        !plugin.close_connection("c1", 1000, "bye"),
        "队列满时 close_connection 必须返回 false"
    );

    assert_eq!(drain_kinds(&mut rx), vec!["binary"]);
}

/// 接收端已断开（连接已关闭）时必须返回 false。
/// 修复前 `send_text` 回退 spawn、另两个总是 spawn，三者都返回 true。
#[tokio::test]
async fn test_ws_send_returns_false_when_channel_closed() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    let rx = plugin.new_test_connection("c1", 8);
    drop(rx); // 模拟对端任务已退出（send_task 结束 → 接收端被 drop）

    assert!(!plugin.send_text("c1", "m"), "已关闭时 send_text 应返回 false");
    assert!(!plugin.send_binary("c1", b"m"), "已关闭时 send_binary 应返回 false");
    assert!(
        !plugin.close_connection("c1", 1000, ""),
        "已关闭时 close_connection 应返回 false"
    );
}

/// **保序**：三个方法必须按调用顺序进入同一条队列，且是**同步**入队。
///
/// 这条在修复前会失败：`send_text` 走 try_send 立刻入队，而 `send_binary` /
/// `close_connection` 是 `tokio::spawn` —— 在 current_thread runtime 下，
/// 不让出调度它们根本不会执行，队列里此时只有 text。
/// 而一旦让出调度，多个 spawn 任务拿到 permit 的先后就不受调用顺序约束了
/// （这就是「close 插到已排队的 send 前面」的来源）。
#[tokio::test]
async fn test_ws_commands_are_queued_in_order() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    let mut rx = plugin.new_test_connection("c1", 8);

    assert!(plugin.send_text("c1", "t1"));
    assert!(plugin.send_binary("c1", b"b1"));
    assert!(plugin.close_connection("c1", 1000, "bye"));

    // 不 await、不 yield —— 三条必须已经全部在队列里
    assert_eq!(
        drain_kinds(&mut rx),
        vec!["text", "binary", "close"],
        "三个方法必须同步入队且保持调用顺序"
    );
}

/// 三个方法都必须在**没有 Tokio runtime** 的上下文里可用 —— C API
/// （`ws_send_text` / `ws_send_binary` / `ws_close`）就是这种同步上下文。
/// 修复前 `send_binary` / `close_connection` 直接 `tokio::spawn`，
/// 在这里会 panic。
#[test]
fn test_ws_send_does_not_require_tokio_runtime() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    let mut rx = plugin.new_test_connection("c1", 8);

    assert!(plugin.send_text("c1", "t"));
    assert!(plugin.send_binary("c1", b"b"));
    assert!(plugin.close_connection("c1", 1000, ""));

    assert_eq!(drain_kinds(&mut rx), vec!["text", "binary", "close"]);
}

/// Task 8：挂载 `/ws` 不该接管 `/wsfoo/...`。
/// helper 对了不等于调用点用了它 —— 这条钉的是 `websocket.rs` 的 `should_handle`。
#[test]
fn test_websocket_plugin_rejects_prefix_collision() {
    let plugin = WebSocketPlugin::new("/ws".to_string());

    let collided = Request::builder()
        .uri("/wsfoo/chat")
        .method(Method::GET)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "Upgrade")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("Sec-WebSocket-Version", "13")
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&collided), "挂载 /ws 不该接管 /wsfoo/chat");

    // 防回归：/ws 本身与其子路径仍要接管
    for uri in ["/ws", "/ws/chat"] {
        let req = Request::builder()
            .uri(uri)
            .method(Method::GET)
            .header(header::UPGRADE, "websocket")
            .header(header::CONNECTION, "Upgrade")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("Sec-WebSocket-Version", "13")
            .body(Body::empty())
            .unwrap();
        assert!(plugin.should_handle(&req), "{} 仍应被接管", uri);
    }
}

#[test]
fn test_text_message_with_nul_byte_reaches_c_callback_intact() {
    let plugin = Arc::new(WebSocketPlugin::new("/ws".to_string()));
    let _guard = GlobalWsGuard::install(plugin.clone());

    crate::c_api::set_websocket_callback(capture_c_event);
    *CAPTURED.lock().unwrap() = None;

    plugin.emit_event(WebSocketEvent {
        connection_id: "c-nul".to_string(),
        event_type: WebSocketEventType::Message,
        path: None,
        query: None,
        headers: None,
        text_data: Some("a\0b".to_string()),
        binary_data: None,
        close_code: None,
        close_reason: None,
        error_message: None,
    });

    let captured = CAPTURED.lock().unwrap().take().expect("C 回调没有被调用");
    assert_eq!(captured.text_len, 3, "text_len 应为 3，不因含 NUL 被截断");
}

#[tokio::test]
async fn test_ws_send_binary_empty_slice_succeeds() {
    let plugin = Arc::new(WebSocketPlugin::new("/ws".to_string()));
    let _guard = GlobalWsGuard::install(plugin.clone());
    let mut rx = plugin.new_test_connection("c-empty-bin", 4);

    let conn_c = std::ffi::CString::new("c-empty-bin").unwrap();
    let empty_c = std::ffi::CString::new("").unwrap();
    assert!(
        crate::c_api::ws_send_binary(conn_c.as_ptr(), empty_c.as_ptr(), 0),
        "0 字节二进制消息应允许入队"
    );
    assert_eq!(drain_kinds(&mut rx), vec!["binary"]);
}

#[test]
fn test_set_websocket_callback_before_plugin_registration_and_multi_mount() {
    use crate::plugins::websocket::{
        clear_global_websocket_plugins, register_global_websocket_plugin,
    };

    clear_global_websocket_plugins();
    // 先设置回调，再注册多个插件
    crate::c_api::set_websocket_callback(capture_c_event);

    let p1 = Arc::new(WebSocketPlugin::new("/ws1".to_string()));
    let p2 = Arc::new(WebSocketPlugin::new("/ws2".to_string()));
    register_global_websocket_plugin(p1.clone());
    register_global_websocket_plugin(p2.clone());

    *CAPTURED.lock().unwrap() = None;
    p1.emit_event(WebSocketEvent {
        connection_id: "c1".to_string(),
        event_type: WebSocketEventType::Open,
        path: Some("/ws1".to_string()),
        query: None,
        headers: None,
        text_data: None,
        binary_data: None,
        close_code: None,
        close_reason: None,
        error_message: None,
    });
    assert!(CAPTURED.lock().unwrap().take().is_some(), "第一个挂载点应自动挂上先前注册的回调");

    // 验证 ws_send_text 能路由到 p1（即使 p2 在后）
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut rx1 = p1.new_test_connection("c-p1", 4);
    let mut rx2 = p2.new_test_connection("c-p2", 4);
    let c1_id = std::ffi::CString::new("c-p1").unwrap();
    let c2_id = std::ffi::CString::new("c-p2").unwrap();
    let msg = std::ffi::CString::new("hi").unwrap();
    assert!(crate::c_api::ws_send_text(c1_id.as_ptr(), msg.as_ptr()));
    assert!(crate::c_api::ws_send_text(c2_id.as_ptr(), msg.as_ptr()));
    assert_eq!(drain_kinds(&mut rx1), vec!["text"]);
    assert_eq!(drain_kinds(&mut rx2), vec!["text"]);

    clear_global_websocket_plugins();
}
