//! 「客户端断开 → 通知 JS」的回调链路（Rust 层）
//!
//! 链路：客户端断开 → hyper drop handler future → `RequestStateGuard::drop`
//! 判定「响应从未发出」→ `notify_request_aborted()` → 注册进来的 C 回调。
//!
//! ⚠️ 两个用例**必须成对看**：只测「断开时会触发」的话，一个「**永远**都触发」的
//! 实现同样能过 —— 那正是最危险的错法（正常回完也报中断，JS 侧会把好请求当死的）。
//! `..._not_fired_on_normal_response` 就是这条负对照。
//!
//! 测试串行由 `.cargo/config.toml` 的 `RUST_TEST_THREADS = "1"` 保证（原生 server
//! 是进程级单例）。

use crate::tests::test_utils::{get_free_port, run_outside_async, wait_for_port};
use crate::types::{HttpRequest, ResponseData};
use std::ffi::CString;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// 回调 / 捕获器
// ---------------------------------------------------------------------------

/// 被判定为「客户端断开」的 request_id 列表
static ABORTED_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// 交给 `set_request_aborted_callback` 的那个回调
extern "C" fn capture_abort_callback(request_id: *const std::os::raw::c_char) {
    if request_id.is_null() {
        return;
    }
    let id = unsafe { std::ffi::CStr::from_ptr(request_id) }
        .to_string_lossy()
        .into_owned();
    ABORTED_IDS.lock().unwrap().push(id);
}

/// 只捕获 request_id、**不响应**（模拟「handler 还在跑，客户端就走了」）
static PENDING_ID: Mutex<Option<String>> = Mutex::new(None);

extern "C" fn capture_only_callback(request: *mut HttpRequest) {
    if request.is_null() {
        return;
    }
    let id = unsafe {
        let req = &*request;
        std::ffi::CStr::from_ptr(req.request_id)
            .to_string_lossy()
            .into_owned()
    };
    *PENDING_ID.lock().unwrap() = Some(id);
    unsafe {
        crate::c_api::free_http_request(request);
    }
}

/// 捕获 request_id 并**立即回 200**（正常路径的负对照）
static RESPONDED_ID: Mutex<Option<String>> = Mutex::new(None);

extern "C" fn capture_and_respond_callback(request: *mut HttpRequest) {
    if request.is_null() {
        return;
    }
    let id = unsafe {
        let req = &*request;
        std::ffi::CStr::from_ptr(req.request_id)
            .to_string_lossy()
            .into_owned()
    };
    *RESPONDED_ID.lock().unwrap() = Some(id.clone());

    if let Some(tx) = crate::global::RESPONSE_CHANNELS.lock().unwrap().remove(&id) {
        let _ = tx.send(ResponseData {
            status_code: 200,
            headers: std::collections::HashMap::new(),
            body: b"ok".to_vec(),
        });
    }

    unsafe {
        crate::c_api::free_http_request(request);
    }
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

fn register_abort_callback() {
    ABORTED_IDS.lock().unwrap().clear();
    assert!(
        crate::c_api::set_request_aborted_callback(Some(capture_abort_callback)),
        "注册 abort 回调应当返回 true"
    );
}

fn unregister_abort_callback() {
    let _ = crate::c_api::set_request_aborted_callback(None);
}

async fn await_pending_id() -> String {
    for _ in 0..100 {
        if let Some(id) = PENDING_ID.lock().unwrap().take() {
            return id;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("handler 从未被调用（PENDING_ID 一直是 None）");
}

/// 等 `expected` 出现在 ABORTED_IDS 里；超时返回 false（不 panic，交给断言给出可读信息）
async fn await_aborted(expected: &str) -> bool {
    for _ in 0..100 {
        if ABORTED_IDS.lock().unwrap().iter().any(|id| id == expected) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    false
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

/// 裸 TcpStream：连上 → 发一个完整请求 → 等 handler 被调用 → **直接关掉 socket**。
/// 回调必须带着**这个** request_id 触发。
#[tokio::test]
async fn test_request_aborted_callback_fires_on_client_disconnect() {
    use tokio::io::AsyncWriteExt;

    register_abort_callback();

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), capture_only_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", actual as u16))
        .await
        .expect("connect failed");
    sock.write_all(b"POST /abort HTTP/1.1\r\nHost: localhost\r\nContent-Length: 7\r\n\r\npayload")
        .await
        .expect("write failed");
    sock.flush().await.expect("flush failed");

    // 先确认请求真的进了 handler，再断开 —— 否则「回调没触发」可能只是请求还没到
    let rid = await_pending_id().await;
    drop(sock);

    assert!(
        await_aborted(&rid).await,
        "客户端断开后 abort 回调未触发（request_id={rid}，已收到 {:?}）",
        ABORTED_IDS.lock().unwrap()
    );

    // 顺带确认没有留下残留（guard 的清理与通知走的是同一条路径）
    assert_eq!(
        crate::global::response_channels_len(),
        0,
        "断开后 RESPONSE_CHANNELS 应已清空"
    );

    run_outside_async(|| crate::c_api::stop_server());
    unregister_abort_callback();
}

/// **负对照**：handler 正常回完 200 的请求**不得**触发 abort 回调。
///
/// 这条在「判据写错成『只要 guard drop 就通知』」的实现下必红 —— 而那正是把
/// 正常请求误报成中断的错法。
#[tokio::test]
async fn test_request_aborted_callback_not_fired_on_normal_response() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    register_abort_callback();
    *RESPONDED_ID.lock().unwrap() = None;

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), capture_and_respond_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", actual as u16))
        .await
        .expect("connect failed");
    // Connection: close —— 读完响应即 EOF，不必自己判 Content-Length
    sock.write_all(b"GET /ok HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("write failed");
    sock.flush().await.expect("flush failed");

    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.expect("read failed");
    assert!(
        buf.starts_with(b"HTTP/1.1 200"),
        "响应应当成功（否则这条负对照没有意义）：{:?}",
        String::from_utf8_lossy(&buf[..buf.len().min(40)])
    );

    let rid = RESPONDED_ID
        .lock()
        .unwrap()
        .clone()
        .expect("handler 从未被调用");

    // 给 guard 足够时间落定（它在 handler future 结束时 drop）
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    assert!(
        !ABORTED_IDS.lock().unwrap().iter().any(|id| id == &rid),
        "正常回完的请求不该触发 abort 回调（request_id={rid}）—— \
         说明判据不是「响应是否已发出」"
    );

    run_outside_async(|| crate::c_api::stop_server());
    unregister_abort_callback();
}

/// 没注册回调时**不能** panic / 崩 —— 这是「纯增量且默认惰性」的守卫。
/// 没有它的话，一个「拿到 None 就 unwrap」的实现会在默认配置下把服务打崩。
#[tokio::test]
async fn test_no_abort_callback_registered_is_safe() {
    use tokio::io::AsyncWriteExt;

    unregister_abort_callback();

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), capture_only_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", actual as u16))
        .await
        .expect("connect failed");
    sock.write_all(b"POST /abort HTTP/1.1\r\nHost: localhost\r\nContent-Length: 7\r\n\r\npayload")
        .await
        .expect("write failed");
    sock.flush().await.expect("flush failed");

    let _rid = await_pending_id().await;
    drop(sock);

    // 没注册 → 通知是 no-op；这里只要「没崩 + 状态被清理」即可
    for _ in 0..40 {
        if crate::global::response_channels_len() == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        crate::global::response_channels_len(),
        0,
        "未注册回调时也应正常清理（通知与清理是同一条路径，通知失败不该影响清理）"
    );

    run_outside_async(|| crate::c_api::stop_server());
}
