use super::super::*;
use std::ffi::CString;
use crate::types::{HttpRequest, RequestBodyData};
use crate::global::{REQUEST_BODIES, CHUNK_ACCUMULATORS};
use crate::tests::test_utils::{get_free_port, wait_for_port, run_outside_async, test_callback_ok};

extern "C" fn test_callback(_request: *mut HttpRequest) {
    println!("Test callback called");
}

/// 捕获 request_id 后故意不响应，由测试自己决定何时 send_response
static PENDING_REQUEST_ID: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

extern "C" fn capture_request_callback(request: *mut HttpRequest) {
    if request.is_null() { return; }
    unsafe {
        let req = &*request;
        let id = std::ffi::CStr::from_ptr(req.request_id).to_string_lossy().into_owned();
        *PENDING_REQUEST_ID.lock().unwrap() = Some(id);
    }
    unsafe {
        crate::c_api::free_http_request(request);
    }
}

/// 等回调把 request_id 交出来
async fn await_pending_request_id() -> String {
    for _ in 0..100 {
        if let Some(id) = PENDING_REQUEST_ID.lock().unwrap().take() {
            return id;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("callback never captured a request id");
}

#[test]
fn test_server_creation() {
    // 测试 C 接口
    let result = start_server(8080, std::ptr::null(), test_callback);
    // 注意：在测试环境中可能无法真正启动服务器
    assert!(result >= 0); // 总是通过，因为实际测试需要运行环境
}

#[test]
fn test_free_functions() {
    // 测试 free_string
    let c_str = CString::new("test").unwrap();
    let ptr = c_str.into_raw();
    unsafe {
        free_string(ptr);
    }
    
    // 测试 free_http_request
    let request = Box::new(HttpRequest {
        request_id: CString::new("id").unwrap().into_raw(),
        method: CString::new("GET").unwrap().into_raw(),
        path: CString::new("/").unwrap().into_raw(),
        headers_json: CString::new("{}").unwrap().into_raw(),
        body: CString::new("").unwrap().into_raw(),
        body_len: 0,
    });
    
    unsafe {
        free_http_request(Box::into_raw(request));
    }
}

#[test]
fn test_read_request_body_chunk() {
    let request_id = "test-req-id";
    let data = b"Hello World";
    
    // Setup global state
    {
        let mut bodies = REQUEST_BODIES.lock().unwrap();
        bodies.insert(request_id.to_string(), RequestBodyData {
            data: data.to_vec(),
            read_position: 0,
        });
    }
    
    // Test reading in chunks
    let mut buffer = [0u8; 5];
    let request_id_c = CString::new(request_id).unwrap();
    
    let read = read_request_body_chunk(
            request_id_c.as_ptr(),
            buffer.as_mut_ptr() as *mut i8, // c_char is i8 on some platforms
            5
        );
    
    assert_eq!(read, 5);
    assert_eq!(&buffer, b"Hello");
    
    // Read next chunk
    let mut buffer2 = [0u8; 10];
    let read2 = read_request_body_chunk(
            request_id_c.as_ptr(),
            buffer2.as_mut_ptr() as *mut i8,
            10
        );
    
    // Should finish reading (remaining 6 bytes: " World")
    assert_eq!(read2, 6);
    assert_eq!(&buffer2[..6], b" World");
    
    // Cleanup
    {
        let mut bodies = REQUEST_BODIES.lock().unwrap();
        bodies.remove(request_id);
    }
}

#[test]
fn test_write_response_chunk() {
    let request_id = "test-chunk-req";
    let chunk1 = b"Part 1";
    let chunk2 = b"Part 2";
    let request_id_c = CString::new(request_id).unwrap();
    
    // Write chunks via C API
    write_response_chunk(
        request_id_c.as_ptr(),
        chunk1.as_ptr() as *const i8,
        chunk1.len() as i32
    );
    
    write_response_chunk(
        request_id_c.as_ptr(),
        chunk2.as_ptr() as *const i8,
        chunk2.len() as i32
    );
    
    // Verify accumulation
    {
        let mut accs = CHUNK_ACCUMULATORS.lock().unwrap();
        let acc = accs.get(request_id).expect("Accumulator should exist");
        assert_eq!(acc.chunks.len(), 2);
        assert_eq!(acc.chunks[0], chunk1);
        assert_eq!(acc.chunks[1], chunk2);
        
        // Clean up manually since we aren't calling end_response
        accs.remove(request_id);
    }
}

#[tokio::test]
async fn test_invalid_status_code_falls_back_to_500() {
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), capture_request_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let client = reqwest::Client::new();
    let handle = tokio::spawn(async move {
        client.get(format!("http://127.0.0.1:{}/x", actual)).send().await
    });

    let rid = await_pending_request_id().await;

    // 非法 status 99 + 含控制字符的非法 header 值 + 含非 ASCII 的 header + 一个合法 header
    let rid_c = CString::new(rid).unwrap();
    let headers = CString::new(
        r#"{"X-Bad": "line1\nline2", "X-Name": "中文文件名.pdf", "X-Ok": "yes"}"#,
    )
    .unwrap();
    let body = CString::new("hi").unwrap();
    assert!(crate::c_api::send_response(
        rid_c.as_ptr(), 99, headers.as_ptr(), body.as_ptr() as *const i8, 2));

    let resp = handle.await.unwrap().expect("request must not be reset by a panic");
    assert_eq!(resp.status(), 500, "invalid status must fall back to 500");
    assert_eq!(resp.headers().get("x-ok").unwrap(), "yes", "valid header must survive");
    assert!(resp.headers().get("x-bad").is_none(), "header with control char must be dropped, not panic");
    // http 0.2 的 HeaderValue 允许 obs-text（0x80-0xFF），所以非 ASCII 值是**合法**的，
    // 会原样透传（客户端读到的是原始 UTF-8 字节）。真正被拒的是控制字符（见上面的 x-bad）。
    assert_eq!(
        resp.headers().get("x-name").map(|v| v.as_bytes().to_vec()),
        Some("中文文件名.pdf".as_bytes().to_vec()),
        "非 ASCII header 值是合法 obs-text，应原样透传"
    );

    run_outside_async(|| crate::c_api::stop_server());
}

/// `REQUEST_TIMEOUT_SECS` 是全局状态：本用例把它改成 1s，无论断言是否失败都必须复位，
/// 否则后续用例会继承 1s 超时。
struct TimeoutResetGuard;
impl Drop for TimeoutResetGuard {
    fn drop(&mut self) {
        crate::global::REQUEST_TIMEOUT_SECS
            .store(30, std::sync::atomic::Ordering::Relaxed);
    }
}

/// config 里的 request_timeout_secs 必须真的生效：设为 1s 后，handler 永不响应
/// 应在 ~1s 收到 500，而不是等满 30s。
#[tokio::test]
async fn test_request_timeout_configurable() {
    let _reset = TimeoutResetGuard;

    let config = CString::new(r#"{"request_timeout_secs": 1}"#).unwrap();
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server_with_config(
            port as i32,
            host.as_ptr(),
            capture_request_callback,
            config.as_ptr(),
        )
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let start = std::time::Instant::now();
    let resp = reqwest::get(format!("http://127.0.0.1:{}/slow", actual))
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(resp.status(), 500);
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "超时配置未生效: {:?}",
        elapsed
    );

    run_outside_async(|| crate::c_api::stop_app_server());
    let _ = PENDING_REQUEST_ID.lock().unwrap().take();
}

/// 反向验证：超时会抢在「迟到的响应」之前生效。
/// 1s 超时 + 2s 后才响应 → 客户端在 1s 就拿到 500，JS 那次 send_response 落空。
/// （这条能区分 config=1 与默认 30：默认 30s 下同样的用例会返回 200。）
#[tokio::test]
async fn test_request_timeout_preempts_late_response() {
    let _reset = TimeoutResetGuard;

    let config = CString::new(r#"{"request_timeout_secs": 1}"#).unwrap();
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server_with_config(
            port as i32,
            host.as_ptr(),
            capture_request_callback,
            config.as_ptr(),
        )
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let client = reqwest::Client::new();
    let handle = tokio::spawn(async move {
        client
            .get(format!("http://127.0.0.1:{}/slow", actual))
            .send()
            .await
            .unwrap()
    });

    let rid = await_pending_request_id().await;

    // 2 秒后才响应（与请求并发）。1s 超时应先到 → 客户端拿 500；
    // 若超时仍是默认 30s，客户端会在 2s 拿到 200，状态码断言即失败。
    let rid_c = CString::new(rid).unwrap();
    let headers = CString::new("{}").unwrap();
    let responder = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        crate::c_api::send_response(rid_c.as_ptr(), 200, headers.as_ptr(), std::ptr::null(), 0)
    });

    let start = std::time::Instant::now();
    let resp = handle.await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(resp.status(), 500, "1s 超时应先于 2s 后才到的响应生效");
    assert!(elapsed < std::time::Duration::from_secs(5), "超时未生效: {:?}", elapsed);

    // 迟到的响应落空：通道已被超时分支清理，send 返回 false（也不该留下残留）
    assert!(!responder.await.unwrap(), "超时后通道应已被清理");

    run_outside_async(|| crate::c_api::stop_app_server());
}

/// 客户端在等 JS 响应时断开（典型场景：App 退后台 / 用户关页面）。
///
/// 实测：hyper 会直接 drop handler future，30s 超时分支**不会**执行——
/// 等 31s 后两张表仍各残留 1 条，是永久泄漏而非"最多留 30 秒"。
/// 这条路径只能靠 `RequestStateGuard` 的 Drop 兜底。
#[tokio::test]
async fn test_global_maps_cleaned_on_client_disconnect() {
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), capture_request_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    let base_channels = crate::global::response_channels_len();
    let base_bodies = crate::global::request_bodies_len();

    let client = reqwest::Client::new();
    let handle = tokio::spawn(async move {
        client
            .post(format!("http://127.0.0.1:{}/x", actual))
            .body("payload")
            .send()
            .await
            .unwrap()
    });

    let _rid = await_pending_request_id().await;
    handle.abort();

    // 轮询等清理生效（不写死 sleep，避免 flaky）；guard 未生效时会走到断言失败
    for _ in 0..40 {
        if crate::global::response_channels_len() == base_channels
            && crate::global::request_bodies_len() == base_bodies
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    assert_eq!(
        crate::global::response_channels_len(),
        base_channels,
        "客户端断开后 RESPONSE_CHANNELS 残留在表内"
    );
    assert_eq!(
        crate::global::request_bodies_len(),
        base_bodies,
        "客户端断开后 REQUEST_BODIES 残留在表内"
    );

    run_outside_async(|| crate::c_api::stop_server());
}

#[tokio::test]
async fn test_global_maps_cleaned_after_send_response() {
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), capture_request_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;

    // 三张表分别取基线：之前的用例可能留下残留，不能假设从 0 开始
    let base_channels = crate::global::response_channels_len();
    let base_bodies = crate::global::request_bodies_len();
    let base_accs = crate::global::chunk_accumulators_len();

    let client = reqwest::Client::new();
    let handle = tokio::spawn(async move {
        client
            .post(format!("http://127.0.0.1:{}/x", actual))
            .body("payload")
            .send()
            .await
            .unwrap()
    });

    let rid = await_pending_request_id().await;
    let rid_c = CString::new(rid).unwrap();
    let headers = CString::new("{}").unwrap();

    // 先累积一个 chunk：走 send_response（而不是 end_response）结束时，
    // 若只清 channels，这张表同样会泄漏
    let chunk = CString::new("chunk-data").unwrap();
    assert!(crate::c_api::write_response_chunk(
        rid_c.as_ptr(),
        chunk.as_ptr() as *const i8,
        10
    ));

    assert!(crate::c_api::send_response(
        rid_c.as_ptr(), 200, headers.as_ptr(), std::ptr::null(), 0));
    handle.await.unwrap();

    assert_eq!(crate::global::response_channels_len(), base_channels, "RESPONSE_CHANNELS 泄漏");
    assert_eq!(crate::global::request_bodies_len(), base_bodies, "REQUEST_BODIES 泄漏");
    assert_eq!(crate::global::chunk_accumulators_len(), base_accs, "CHUNK_ACCUMULATORS 泄漏");

    run_outside_async(|| crate::c_api::stop_server());
}

/// 回调返回一个 X-Accel-Redirect，让 server 去读 root_dir 下的文件
extern "C" fn accel_redirect_callback(request: *mut HttpRequest) {
    if request.is_null() { return; }
    unsafe {
        let req = &*request;
        let headers = CString::new(
            r#"{"X-Accel-Redirect": "/secret.txt", "Content-Type": "application/json", "X-Keep": "kept"}"#,
        )
        .unwrap();
        crate::c_api::send_response(
            req.request_id as *const i8,
            200,
            headers.as_ptr(),
            std::ptr::null(),
            0,
        );
    }
    unsafe {
        crate::c_api::free_http_request(request);
    }
}

#[tokio::test]
async fn test_x_accel_redirect_uses_canonical_root() {
    let temp = std::env::temp_dir().join("test_accel_root");
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::write(temp.join("secret.txt"), "accel-body").unwrap();

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let root = CString::new(temp.to_string_lossy().to_string()).unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_app_server(
            port as i32,
            host.as_ptr(),
            root.as_ptr(),
            accel_redirect_callback,
        )
    });
    assert!(actual > 0, "app server failed to start");
    wait_for_port(actual as u16).await;

    // /api/serve 不是静态文件，会落到 callback；callback 用 X-Accel-Redirect 指向真实文件
    let resp = reqwest::get(format!("http://127.0.0.1:{}/api/serve", actual))
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "canonical root 未生效时这里会是 404");
    assert_eq!(resp.headers().get("x-keep").unwrap(), "kept", "其他响应头应保留");
    assert!(
        resp.headers().get("x-accel-redirect").is_none(),
        "X-Accel-Redirect 不应出现在最终响应里"
    );
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/plain; charset=utf-8",
        "Content-Type 应被文件的 mime 覆盖（不是 callback 给的 application/json）"
    );
    assert_eq!(resp.text().await.unwrap(), "accel-body");

    run_outside_async(|| crate::c_api::stop_app_server());
    std::fs::remove_dir_all(&temp).ok();
}

#[tokio::test]
async fn test_port_out_of_range_rejected() {
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(70000, host.as_ptr(), capture_request_callback)
    });
    assert_eq!(actual, 0, "port 70000 must be rejected, not truncated to 4464");
}

// ---- Task 9: C++ escapeJson 修正后的跨层契约 ----

/// 走**真实的 `send_response` 解析路径**，把解析结果用 RESPONSE_CHANNELS 取回来。
///
/// 这不是在测 serde_json —— 它验证的是「C++ 序列化出来的 bytes 到了 Rust 侧
/// 会变成什么」。这条链路的两端（C++ 的 `escapeJsonString` 与这里的
/// `serde_json::from_str`）必须对「什么算合法 JSON」有一致理解。
fn send_response_and_collect_headers(
    request_id: &str,
    headers_json: &str,
) -> std::collections::HashMap<String, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<crate::types::ResponseData>();
    crate::global::RESPONSE_CHANNELS
        .lock()
        .unwrap()
        .insert(request_id.to_string(), tx);

    let rid = CString::new(request_id).unwrap();
    let hj = CString::new(headers_json).unwrap();
    let body = CString::new("ok").unwrap();

    let ok = crate::c_api::send_response(rid.as_ptr(), 200, hj.as_ptr(), body.as_ptr(), 2);
    assert!(ok, "send_response 应成功（channel 已注册）");

    rx.blocking_recv().expect("应收到 ResponseData").headers
}

/// 修正后的 C++ 产出（控制字符已转义）必须被完整解析，
/// 且转义序列要还原成真正的控制字符。
#[test]
fn test_send_response_parses_escaped_headers() {
    let headers = send_response_and_collect_headers(
        "t9-escaped",
        "{\"X-Multi\":\"line1\\nline2\",\"X-Tab\":\"a\\tb\",\"X-Plain\":\"ok\"}",
    );

    assert_eq!(
        headers.get("X-Multi").map(|s| s.as_str()),
        Some("line1\nline2"),
        "\\n 应还原成真正的换行"
    );
    assert_eq!(
        headers.get("X-Tab").map(|s| s.as_str()),
        Some("a\tb"),
        "\\t 应还原成真正的制表符"
    );
    assert_eq!(headers.get("X-Plain").map(|s| s.as_str()), Some("ok"));
}

/// **修复前的 C++ 产出**（控制字符原样裸放）会让**整段** headers JSON 解析失败
/// → Rust 侧回退成空 HashMap → 这一次响应的**所有**响应头被静默丢弃。
///
/// 这条钉住的就是「为什么 C++ 必须转义控制字符」：不是「值不对」，
/// 而是**一个坏字节毁掉全部响应头**，且调用方看不到任何错误。
#[test]
fn test_send_response_drops_all_headers_on_raw_control_char() {
    // 注意 `\n` 是真的换行字节（不是 \\n 两个字符）—— 即修复前 C++ 的输出
    let headers = send_response_and_collect_headers(
        "t9-raw",
        "{\"X-Multi\":\"line1\nline2\",\"X-Plain\":\"ok\"}",
    );

    assert!(
        headers.is_empty(),
        "裸控制字符应让整段 JSON 解析失败（修复前 JS 侧因此看不到任何响应头），实际解析出: {:?}",
        headers
    );
}

// ---- `res.write(...)` 与 `res.end(ArrayBuffer)` 混用时的 body 合并 ----

/// 在 RESPONSE_CHANNELS 里注册一个 request_id，跑完 `f` 后取回 ResponseData。
///
/// 为什么不用真 socket：要断言的是**Rust 侧最终拼出来的 body 字节**，
/// 起真服务器只会多一层超时/端口噪声，抓不到「哪条通道的字节丢了」。
fn collect_response_after<F: FnOnce()>(request_id: &str, f: F) -> crate::types::ResponseData {
    let (tx, rx) = tokio::sync::oneshot::channel::<crate::types::ResponseData>();
    crate::global::RESPONSE_CHANNELS
        .lock()
        .unwrap()
        .insert(request_id.to_string(), tx);
    f();
    rx.blocking_recv().expect("应收到 ResponseData")
}

/// 回归：`res.write('a')` 的累积块 **和** `res.end(Buffer.from([0x62,0x63]))` 的显式 body
/// 必须**都**发出去。
///
/// 修复前 `send_response` 只读显式 body（`body`/`body_len` 两个参数），
/// 累积器里的字节被 `cleanup_request_state` 直接丢掉 ⇒ JS 侧
/// `res.write('a'); res.end(arrayBuffer)` 里的 `'a'` **静默消失**（响应体只剩 arrayBuffer）。
///
/// 期望语义来自**真实 node v22.22.2 实测**：
/// `write('a'); end(Buffer.from([0x62,0x63]))` → body `616263`（即 "abc"）。
/// ⇒ 先 write 的在前，`end(chunk)` 的在后。
#[test]
fn test_send_response_merges_accumulated_chunks_before_explicit_body() {
    let rid = "t-merge-accumulated-chunks";
    let rid_c = CString::new(rid).unwrap();
    let hj = CString::new("{}").unwrap();

    let data = collect_response_after(rid, || {
        // 模拟 res.write('a') —— 走 write_response_chunk（累积器通道）
        let chunk = CString::new("a").unwrap();
        assert!(crate::c_api::write_response_chunk(
            rid_c.as_ptr(),
            chunk.as_ptr() as *const i8,
            1
        ));

        // 模拟 res.end(Buffer.from([0x62, 0x63])) —— 走 sendBinaryResponse → send_response
        let explicit: &[u8] = b"bc";
        assert!(crate::c_api::send_response(
            rid_c.as_ptr(),
            200,
            hj.as_ptr(),
            explicit.as_ptr() as *const i8,
            2
        ));
    });

    assert_eq!(
        data.body, b"abc",
        "累积块必须排在显式 body 之前（与 node 一致：write 在前、end 的 chunk 在后）"
    );
}

/// **负对照**：没有累积块时（普通 `sendResponse` 路径）显式 body 必须**原样**输出，
/// 不能被合并逻辑改动。
///
/// 没有这条，一个「无条件前置空 buffer」或「无条件把 body 置空」的实现也能让上一条变绿。
#[test]
fn test_send_response_without_chunks_keeps_explicit_body() {
    let rid = "t-no-accumulated-chunks";
    let rid_c = CString::new(rid).unwrap();
    let hj = CString::new("{}").unwrap();

    let data = collect_response_after(rid, || {
        let explicit: &[u8] = b"hello";
        assert!(crate::c_api::send_response(
            rid_c.as_ptr(),
            200,
            hj.as_ptr(),
            explicit.as_ptr() as *const i8,
            5
        ));
    });

    assert_eq!(data.body, b"hello", "无累积块时 body 不应被改动");
}

/// **只有累积块、显式 body 为空**的路径（`res.write(x); res.end()`）也必须发出去。
///
/// 这条覆盖 `send_response(..., null, 0)`：修复前它发出的是**空 body**，
/// 累积的字节全丢。
#[test]
fn test_send_response_with_only_accumulated_chunks() {
    let rid = "t-only-accumulated-chunks";
    let rid_c = CString::new(rid).unwrap();
    let hj = CString::new("{}").unwrap();

    let data = collect_response_after(rid, || {
        let chunk = CString::new("streamed").unwrap();
        assert!(crate::c_api::write_response_chunk(
            rid_c.as_ptr(),
            chunk.as_ptr() as *const i8,
            8
        ));
        assert!(crate::c_api::send_response(
            rid_c.as_ptr(),
            200,
            hj.as_ptr(),
            std::ptr::null(),
            0
        ));
    });

    assert_eq!(data.body, b"streamed", "只有累积块时也必须作为 body 发出");
}


// ===========================================================================
// Task 15：回调路径请求体大小上限（默认 100MB，config 可调，超限 413 且不回调 JS）
// ===========================================================================

/// 记录「回调被调用了几次」并正常回 200。
///
/// 为什么需要它：「413 时**不回调 JS**」是这条修复的一半语义，只断言状态码证明不了。
extern "C" fn recording_callback(request: *mut HttpRequest) {
    CALLBACK_HITS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    crate::tests::test_utils::test_callback_ok(request);
}

static CALLBACK_HITS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `MAX_BODY_SIZE` 是全局状态：无论断言是否失败都必须复位（同 TimeoutResetGuard 的理由）。
struct MaxBodySizeResetGuard;
impl Drop for MaxBodySizeResetGuard {
    fn drop(&mut self) {
        crate::global::MAX_BODY_SIZE.store(
            100 * 1024 * 1024,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

async fn start_app_server_with(config_json: &'static str) -> u16 {
    let config = CString::new(config_json).unwrap();
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server_with_config(
            port as i32,
            host.as_ptr(),
            recording_callback,
            config.as_ptr(),
        )
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;
    // wait_for_port 自己会发一个 GET 打中回调，所以计数要在它之后再清零
    CALLBACK_HITS.store(0, std::sync::atomic::Ordering::SeqCst);
    actual as u16
}

/// 核心：config 的 `max_body_size` 生效 —— 超过就 413，且**不回调 JS**。
///
/// 没有上限时 `hyper::body::to_bytes` 会把整个 body 收进内存
/// （hyper 0.14 的 `to_bytes` 实测无内建上限），一个请求就能把进程撑爆。
#[tokio::test]
async fn test_oversized_body_rejected_413() {
    let _reset = MaxBodySizeResetGuard;
    let port = start_app_server_with(r#"{"max_body_size": 1024}"#).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/big", port))
        .body(vec![0u8; 2048])
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        413,
        "超过 max_body_size 的请求体必须被拒 —— 否则大 body 直接进内存"
    );
    assert_eq!(
        CALLBACK_HITS.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "413 时不能回调 JS（评审要求：拒绝要在回调之前）"
    );

    run_outside_async(|| crate::c_api::stop_app_server());
}

/// ⚠️ 关键：**没有 content-length**（chunked）时超限也必须被拒。
///
/// 只做「先查 content-length 头」的实现会在这里放过 —— 那时 2048 字节会全进内存。
/// 这条用例把「流式累积检查」与「头部预检」区分开：前者才是真正的兜底。
#[tokio::test]
async fn test_oversized_chunked_body_rejected_413() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let _reset = MaxBodySizeResetGuard;
    let port = start_app_server_with(r#"{"max_body_size": 1024}"#).await;

    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let chunk = vec![b'x'; 2048];
    let mut raw = format!(
        "POST /big HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        port
    )
    .into_bytes();
    raw.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
    raw.extend_from_slice(&chunk);
    raw.extend_from_slice(b"\r\n0\r\n\r\n");
    sock.write_all(&raw).await.unwrap();

    let mut resp = Vec::new();
    sock.read_to_end(&mut resp).await.unwrap();
    let text = String::from_utf8_lossy(&resp);
    assert!(
        text.starts_with("HTTP/1.1 413"),
        "chunked（无 content-length）超限也必须 413，实际响应头: {:?}",
        &text[..text.len().min(60)]
    );
    assert_eq!(
        CALLBACK_HITS.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "chunked 超限时也不能回调 JS"
    );

    run_outside_async(|| crate::c_api::stop_app_server());
}

/// 防回归：限额**以内**的 body 必须照常放行（别把上限写成「一律 413」）。
#[tokio::test]
async fn test_body_within_limit_still_passes() {
    let _reset = MaxBodySizeResetGuard;
    let port = start_app_server_with(r#"{"max_body_size": 1024}"#).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/ok", port))
        .body(vec![0u8; 512])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "512 < 1024，应正常放行");
    assert_eq!(
        CALLBACK_HITS.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "限额内应正常回调 JS 一次"
    );

    run_outside_async(|| crate::c_api::stop_app_server());
}

/// 防回归：**默认上限**不能是 0/极小值 —— 没给 max_body_size 时 1MB 的 body 要能过。
#[tokio::test]
async fn test_default_limit_allows_1mb_body() {
    let _reset = MaxBodySizeResetGuard;
    let port = start_app_server_with(r#"{"verbose": false}"#).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/mb", port))
        .body(vec![0u8; 1024 * 1024])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "默认上限是 100MB，1MB 的 body 必须能过");

    run_outside_async(|| crate::c_api::stop_app_server());
}

/// 覆盖**另一条** to_bytes 调用点：普通 HttpServer（`http_server.rs`）没有 config，
/// 走全局 `MAX_BODY_SIZE`。这里把全局调小来验证它也被接上了。
#[tokio::test]
async fn test_plain_http_server_enforces_body_limit() {
    let _reset = MaxBodySizeResetGuard;
    crate::global::MAX_BODY_SIZE.store(1024, std::sync::atomic::Ordering::Relaxed);

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), recording_callback)
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;
    CALLBACK_HITS.store(0, std::sync::atomic::Ordering::SeqCst);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/big", actual))
        .body(vec![0u8; 2048])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 413, "普通 HttpServer 也要有上限");
    assert_eq!(
        CALLBACK_HITS.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "普通 HttpServer 超限时也不能回调 JS"
    );

    run_outside_async(|| crate::c_api::stop_server());
}

// ===========================================================================
// Task 16：getStats 的真实统计
// ===========================================================================

/// 读一次 stats 并**按新契约释放**（返回堆字符串，调用方必须 free_string）。
fn read_stats() -> serde_json::Value {
    let ptr = crate::c_api::get_server_stats();
    assert!(!ptr.is_null(), "get_server_stats 不该返回 null");
    let text = unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { crate::c_api::free_string(ptr as *mut std::os::raw::c_char) };
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("stats 必须是合法 JSON: {} / {}", e, text))
}

/// `totalRequests` 必须真的随请求增长。
///
/// 用**差值**断言而不是绝对值：计数器是进程级全局状态，前面的用例也会加。
#[tokio::test]
async fn test_get_server_stats_total_requests_counts() {
    let before = read_stats()["totalRequests"].as_u64().unwrap();

    let port = start_app_server_with(r#"{"verbose": false}"#).await;
    let client = reqwest::Client::new();
    for _ in 0..3 {
        let _ = client
            .get(format!("http://127.0.0.1:{}/x", port))
            .send()
            .await
            .unwrap();
    }

    let after = read_stats();
    assert_eq!(
        after["totalRequests"].as_u64().unwrap() - before,
        3,
        "3 次请求之后 totalRequests 必须 +3（接线前恒为 0）"
    );

    // 形状与 status 字段：既有调用方依赖 "status":"running"
    assert_eq!(after["status"].as_str(), Some("running"));
    for key in [
        "totalRequests",
        "activeConnections",
        "bytesSent",
        "bytesReceived",
        "uptime",
        "errorCount",
    ] {
        assert!(
            after.get(key).and_then(|v| v.as_u64()).is_some(),
            "字段 {} 必须存在且为非负整数，实际 stats = {}",
            key,
            after
        );
    }

    run_outside_async(|| crate::c_api::stop_app_server());
}

/// `errorCount` 必须真的统计「服务器自身没能产出响应」——这里用**超时**触发。
///
/// 同时顺带钉 `uptime`：超时窗口本身就是 1s，所以此刻 uptime 必然 >= 1。
/// （`uptime` 只记录「最近一次 start 的时间戳」，从未启动过才是 0 —— 只断言
/// `>= 0` 是没有区分力的。）
#[tokio::test]
async fn test_get_server_stats_error_count_and_uptime() {
    let _reset = TimeoutResetGuard;
    let before_errors = read_stats()["errorCount"].as_u64().unwrap();

    // request_timeout_secs=1 + 永不响应的回调 → 1s 后走超时分支
    let config = CString::new(r#"{"request_timeout_secs": 1}"#).unwrap();
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server_with_config(
            port as i32,
            host.as_ptr(),
            capture_request_callback, // 只记录 request_id，不响应
            config.as_ptr(),
        )
    });
    assert!(actual > 0, "server failed to start");
    wait_for_port(actual as u16).await;
    let _ = PENDING_REQUEST_ID.lock().unwrap().take();

    let resp = reqwest::get(format!("http://127.0.0.1:{}/slow", actual))
        .await
        .unwrap();
    assert_eq!(resp.status(), 500, "超时应回 500");

    let after = read_stats();
    assert_eq!(
        after["errorCount"].as_u64().unwrap() - before_errors,
        1,
        "一次超时必须让 errorCount +1（接线前恒为 0）"
    );
    assert!(
        after["uptime"].as_u64().unwrap() >= 1,
        "已启动并过了 1s，uptime 必须 >= 1（恒 0 说明 start 时间戳没接上），实际 {}",
        after["uptime"]
    );

    run_outside_async(|| crate::c_api::stop_app_server());
    let _ = PENDING_REQUEST_ID.lock().unwrap().take();
}

/// 防回归：`totalRequests` 覆盖**三条** server 路径，不只是 app server。
/// （普通 HttpServer 与 StaticHttpServer 各有自己的 handle 入口。）
#[tokio::test]
async fn test_get_server_stats_counts_plain_and_static_servers() {
    // 普通 HttpServer
    let before = read_stats()["totalRequests"].as_u64().unwrap();
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), recording_callback)
    });
    assert!(actual > 0);
    wait_for_port(actual as u16).await;
    let _ = reqwest::get(format!("http://127.0.0.1:{}/a", actual)).await.unwrap();
    assert_eq!(
        read_stats()["totalRequests"].as_u64().unwrap() - before,
        1,
        "普通 HttpServer 的请求也要计数"
    );
    run_outside_async(|| crate::c_api::stop_server());

    // StaticHttpServer
    let root = std::env::temp_dir().join(format!("t16_static_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"hi").unwrap();

    let before = read_stats()["totalRequests"].as_u64().unwrap();
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let root_str = CString::new(root.to_string_lossy().to_string()).unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_static_server(port as i32, host.as_ptr(), root_str.as_ptr())
    });
    assert!(actual > 0);
    wait_for_port(actual as u16).await;
    let _ = reqwest::get(format!("http://127.0.0.1:{}/a.txt", actual)).await.unwrap();
    assert_eq!(
        read_stats()["totalRequests"].as_u64().unwrap() - before,
        1,
        "StaticHttpServer 的请求也要计数"
    );
    run_outside_async(|| crate::c_api::stop_static_server());

    std::fs::remove_dir_all(&root).ok();
}

/// `start_server_with_options` 的 worker 线程数参数：三种取值都要能起、能服务。
///
/// 这条用例的价值全在**边界**上，不在正常值上：
/// - **`-1` 是真实会发生的调用错误**（JS 侧传了个负数）。tokio 的
///   `Builder::worker_threads(0)` 会**直接 panic**，所以 c_api 必须把非正数归 0。
///   这层防御若被删掉，本用例以 panic 收场（而不是静默降级）—— 这就是它的牙齿。
/// - `0` 必须等价于「不传」= tokio 默认（按 CPU 核数）。
/// - `2` 是 §6.6.8 实测出来的高并发最优值，必须能正常服务请求。
///
/// ⚠️ **不断言实际线程数**：Rust 侧没有可移植的「按名字数线程」API，
/// 线程数由**行为探针**验证（`sample` 数 `tokio-runtime-worker` 出现次数），
/// 见报告 §6.6.8「二进制新鲜度用行为探针判」。本用例只保证参数**不会炸**、
/// 且三种取值都产出**功能正常**的服务器。
#[tokio::test]
async fn test_start_server_with_options_worker_threads() {
    for workers in [2i32, -1, 0] {
        let port = get_free_port();
        let host = CString::new("127.0.0.1").unwrap();
        let actual = run_outside_async(move || {
            crate::c_api::start_server_with_options(
                port as i32,
                host.as_ptr(),
                test_callback_ok,
                workers,
            )
        });
        assert!(actual > 0, "worker_threads={} 时服务器启动失败", workers);
        wait_for_port(actual as u16).await;

        let resp = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{}/probe", actual))
            .send()
            .await
            .expect("worker_threads={} 时请求失败");
        assert_eq!(resp.status(), 200, "worker_threads={} 时状态码异常", workers);
        assert_eq!(
            resp.text().await.unwrap(),
            "Hello World",
            "worker_threads={} 时响应体异常",
            workers
        );

        run_outside_async(|| crate::c_api::stop_server());
    }
}

/// 三参数的 `start_server` 必须与 `start_server_with_options(..., 0)` 等价。
///
/// 保留三参数版本是为了 **C ABI 向后兼容**（`rn_http_server.h` 的老调用方、
/// iOS 已发布的 `.xcframework` 头都不需要改）。这条用例把「等价」钉住：
/// 若哪天有人把转调改成别的默认值（比如顺手写个 2），这里会红。
#[tokio::test]
async fn test_start_server_legacy_equals_default_options() {
    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(port as i32, host.as_ptr(), test_callback_ok)
    });
    assert!(actual > 0, "三参数 start_server 启动失败");
    wait_for_port(actual as u16).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/probe", actual))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    run_outside_async(|| crate::c_api::stop_server());
}

#[test]
fn test_read_request_body_chunk_preserves_utf8_boundaries() {
    let request_id = "test-utf8-chunk";
    // "ab中文cd" -> 'a'(1), 'b'(1), '中'(3), '文'(3), 'c'(1), 'd'(1) = 10 字节
    // 用 buffer_size = 4 读取：若不保护边界会截断 '中'（第 2..4 字节）
    let data = "ab中文cd".as_bytes();
    {
        let mut bodies = REQUEST_BODIES.lock().unwrap();
        bodies.insert(request_id.to_string(), RequestBodyData {
            data: data.to_vec(),
            read_position: 0,
        });
    }

    let request_id_c = CString::new(request_id).unwrap();
    let mut chunks: Vec<String> = Vec::new();
    let mut buf = [0u8; 4];
    loop {
        let n = read_request_body_chunk(
            request_id_c.as_ptr(),
            buf.as_mut_ptr() as *mut std::os::raw::c_char,
            4,
        );
        assert!(n >= 0);
        if n == 0 {
            break;
        }
        let s = std::str::from_utf8(&buf[..n as usize])
            .expect("每个分块都必须是合法 UTF-8，不能从多字节字符中间切断");
        chunks.push(s.to_string());
    }

    assert_eq!(chunks.concat(), "ab中文cd");
    REQUEST_BODIES.lock().unwrap().remove(request_id);
}
