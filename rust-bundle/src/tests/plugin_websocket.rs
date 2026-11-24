// WebSocket plugin unit tests
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
    
    // 1. Non-upgrade requests must be ignored.
    let req = Request::builder()
        .uri("/ws/test")
        .method(Method::GET)
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req), "Should not handle non-upgrade request");
    
    // 2. Mismatched path must be ignored.
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
    
    // 3. Valid WebSocket upgrade under mount path should be handled.
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
    
    // 4. Exact mount path match.
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
    
    let config = json!({
        "path": "/websocket",
        "max_message_size": 1048576,
        "enabled": true
    });
    
    let result = plugin.on_load(&config);
    assert!(result.is_ok(), "on_load should succeed");
    
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
    
    let config = json!({
        "enabled": false
    });
    plugin.on_load(&config).unwrap();
    
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

// ---- C-boundary event dispatch verification ----

/// Captured event fields (copied from C pointers into owned Strings).
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

/// Restore the global WebSocket plugin state on drop to avoid leaking into other tests.
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
        .expect("C callback was not invoked");

    assert_eq!(captured.event_type, 4, "expected Error event");
    assert_eq!(
        captured.error_message.as_deref(),
        Some("Receive error: boom"),
        "error_message must reach C callback"
    );
    assert!(
        captured.text_data.is_none(),
        "error message must not be placed in text_data, got {:?}",
        captured.text_data
    );
    assert_eq!(captured.text_len, 0, "text_len should be 0");
}

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

    let captured = CAPTURED.lock().unwrap().take().expect("C callback was not invoked");
    assert_eq!(captured.event_type, 2);
    assert!(captured.error_message.is_none());
    assert_eq!(captured.text_data.as_deref(), Some("hello"));
    assert_eq!(captured.text_len, 5);
}

// ---- Outbound queue ordering and synchronous return semantics ----

/// Drain currently queued commands synchronously via `try_recv`.
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

#[test]
fn test_ws_send_returns_false_for_unknown_connection() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    assert!(!plugin.send_text("no-such-conn", "hello"));
    assert!(!plugin.send_binary("no-such-conn", b"x"));
    assert!(!plugin.close_connection("no-such-conn", 1000, ""));
}

#[tokio::test]
async fn test_ws_send_text_returns_false_when_channel_full() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    let mut rx = plugin.new_test_connection("c1", 2);

    assert!(plugin.send_text("c1", "m1"));
    assert!(plugin.send_text("c1", "m2"));
    assert!(!plugin.send_text("c1", "m3"), "must return false when queue is full");

    assert_eq!(
        drain_kinds(&mut rx),
        vec!["text", "text"],
        "no extra command should be queued when full"
    );
}

#[tokio::test]
async fn test_ws_send_binary_and_close_return_false_when_channel_full() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    let mut rx = plugin.new_test_connection("c1", 1);

    assert!(plugin.send_binary("c1", b"b1"));
    assert!(!plugin.send_binary("c1", b"b2"), "send_binary must return false when full");
    assert!(!plugin.close_connection("c1", 1000, "bye"), "close_connection must return false when full");

    assert_eq!(drain_kinds(&mut rx), vec!["binary"]);
}

#[tokio::test]
async fn test_ws_send_returns_false_when_channel_closed() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    let rx = plugin.new_test_connection("c1", 8);
    drop(rx);

    assert!(!plugin.send_text("c1", "m"));
    assert!(!plugin.send_binary("c1", b"m"));
    assert!(!plugin.close_connection("c1", 1000, ""));
}

/// All three methods must enqueue synchronously in call order without yielding.
#[tokio::test]
async fn test_ws_commands_are_queued_in_order() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    let mut rx = plugin.new_test_connection("c1", 8);

    assert!(plugin.send_text("c1", "t1"));
    assert!(plugin.send_binary("c1", b"b1"));
    assert!(plugin.close_connection("c1", 1000, "bye"));

    assert_eq!(
        drain_kinds(&mut rx),
        vec!["text", "binary", "close"],
        "commands must enqueue synchronously in call order"
    );
}

/// All three methods must work outside a Tokio runtime (as called from C API).
#[test]
fn test_ws_send_does_not_require_tokio_runtime() {
    let plugin = WebSocketPlugin::new("/ws".to_string());
    let mut rx = plugin.new_test_connection("c1", 8);

    assert!(plugin.send_text("c1", "t"));
    assert!(plugin.send_binary("c1", b"b"));
    assert!(plugin.close_connection("c1", 1000, ""));

    assert_eq!(drain_kinds(&mut rx), vec!["text", "binary", "close"]);
}

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
    assert!(!plugin.should_handle(&collided), "/ws mount must not match /wsfoo/chat");

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
        assert!(plugin.should_handle(&req), "{} should still match", uri);
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

    let captured = CAPTURED.lock().unwrap().take().expect("C callback was not invoked");
    assert_eq!(captured.text_len, 3, "text_len must not be truncated at NUL byte");
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
        "0-byte binary message should enqueue"
    );
    assert_eq!(drain_kinds(&mut rx), vec!["binary"]);
}

#[test]
fn test_set_websocket_callback_before_plugin_registration_and_multi_mount() {
    use crate::plugins::websocket::{
        clear_global_websocket_plugins, register_global_websocket_plugin,
    };

    clear_global_websocket_plugins();
    // Register callback first, then register multiple plugins.
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
    assert!(CAPTURED.lock().unwrap().take().is_some(), "first mount should receive pre-registered callback");

    // Verify ws_send_text routes to both p1 and p2.
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
