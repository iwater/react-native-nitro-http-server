use crate::c_api::set_cors_config;
use crate::cors::{cors_config_from_json, CorsConfig, CORS_CONFIG};
use crate::plugins::static_file::StaticFilePlugin;
use crate::plugins::Plugin;
use crate::tests::test_utils::{get_free_port, test_callback_ok, wait_for_server};
use hyper::{Request, Body};
use serde_json::json;
use std::ffi::CString;
use tokio::runtime::Runtime;

// 串行化锁：任何触碰全局 CORS_CONFIG 的测试都必须持有此锁，
// 否则默认并行 cargo test 下会互相干扰产生偶发失败。
static CORS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 获取 CORS 测试串行锁（毒化容忍）。
///
/// 这里**刻意**不用 `lock().unwrap()`：某个用例 panic 时该锁会被毒化，
/// 后续每个用例的 `unwrap()` 都会再 panic 一次并报 `PoisonError` ——
/// 于是**一个真实失败被放大成一串假失败**，失败清单完全失真。
/// 实测过：并行跑时 13 个失败里有 3 个只是 PoisonError 级联。
/// 锁的作用只是「串行化」，不保护任何数据不变量，所以毒化后继续用是安全的。
fn cors_test_lock() -> std::sync::MutexGuard<'static, ()> {
    CORS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// Drop guard：测试结束（包括断言失败 panic）时自动重置全局 CORS 配置，避免泄漏到下一个测试
struct ConfigReset;
impl Drop for ConfigReset {
    fn drop(&mut self) {
        crate::cors::set_cors_config_json(None);
    }
}

// start_*/stop_* 会 drop 全局句柄中残留的服务器 Runtime（例如 c_api 测试启动后
// 从未停止的服务器），而 tokio 禁止在 async 上下文中 drop Runtime（panic 并 abort
// 整个测试进程），因此这些调用统一放到独立线程中执行。
fn run_outside_async<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(f).join().unwrap()
}

#[test]
fn test_cors_config_true_gives_default() {
    let cfg = cors_config_from_json("true").unwrap();
    assert_eq!(cfg, CorsConfig::default());
    assert_eq!(cfg.origin, "*");
    assert!(!cfg.credentials);
    assert!(cfg.methods.contains(&"GET".to_string()));
    assert!(cfg.methods.contains(&"PROPFIND".to_string())); // WebDAV 方法
}

#[test]
fn test_cors_config_false_and_null_disable() {
    assert!(cors_config_from_json("false").is_none());
    assert!(cors_config_from_json("null").is_none());
    assert!(cors_config_from_json("not json").is_none());
    assert!(cors_config_from_json("123").is_none());
}

#[test]
fn test_cors_config_object_partial_fields() {
    let cfg = cors_config_from_json(r#"{"origin": "https://example.com", "credentials": true}"#).unwrap();
    assert_eq!(cfg.origin, "https://example.com");
    assert!(cfg.credentials);
    // 未提供的字段保持默认
    assert_eq!(cfg.headers, vec!["*".to_string()]);
    assert!(cfg.max_age.is_none());
}

#[test]
fn test_cors_config_object_full_fields() {
    let cfg = cors_config_from_json(r#"{
        "origin": "https://a.com",
        "methods": ["GET", "POST"],
        "headers": ["Content-Type", "Authorization"],
        "credentials": true,
        "max_age": 600
    }"#).unwrap();
    assert_eq!(cfg.methods, vec!["GET".to_string(), "POST".to_string()]);
    assert_eq!(cfg.headers, vec!["Content-Type".to_string(), "Authorization".to_string()]);
    assert_eq!(cfg.max_age, Some(600));
}

#[test]
fn test_set_cors_config_c_api() {
    let _guard = cors_test_lock();
    let _reset = ConfigReset;

    // 开启
    let json = CString::new("true").unwrap();
    set_cors_config(json.as_ptr());
    assert!(CORS_CONFIG.lock().unwrap().is_some());

    // 自定义 origin
    let json = CString::new(r#"{"origin": "https://x.com"}"#).unwrap();
    set_cors_config(json.as_ptr());
    assert_eq!(CORS_CONFIG.lock().unwrap().as_ref().unwrap().origin, "https://x.com");

    // NULL 关闭
    set_cors_config(std::ptr::null());
    assert!(CORS_CONFIG.lock().unwrap().is_none());

    // "false" 关闭
    let json = CString::new("false").unwrap();
    set_cors_config(json.as_ptr());
    assert!(CORS_CONFIG.lock().unwrap().is_none());
}

#[test]
fn test_apply_cors_credentials_without_origin_adds_no_header() {
    let _guard = cors_test_lock();
    let _reset = ConfigReset;

    // 构造 preflight 但 Origin 无法提取的边界情况难以直接构造（is_preflight 要求 Origin 存在），
    // 这里验证 apply_cors 在 credentials 模式无 origin 时不加头
    crate::cors::set_cors_config_json(Some(r#"{"credentials": true}"#));
    let mut resp = hyper::Response::builder().status(200).body(hyper::Body::empty()).unwrap();
    crate::cors::apply_cors(&mut resp, None);
    assert!(resp.headers().get("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn test_cors_headers_on_callback_response() {
    let _guard = cors_test_lock();
    let _reset = ConfigReset;
    let json = CString::new("true").unwrap();
    set_cors_config(json.as_ptr());

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || crate::c_api::start_server(port as i32, host.as_ptr(), test_callback_ok));
    assert!(actual > 0);
    wait_for_server(actual as u16).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/hello", actual)).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("access-control-allow-origin").unwrap(), "*");

    run_outside_async(|| crate::c_api::stop_server());
}

#[tokio::test]
async fn test_preflight_returns_204_without_callback() {
    let _guard = cors_test_lock();
    let _reset = ConfigReset;
    let json = CString::new("true").unwrap();
    set_cors_config(json.as_ptr());

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || crate::c_api::start_server(port as i32, host.as_ptr(), test_callback_ok));
    assert!(actual > 0);
    wait_for_server(actual as u16).await;

    let client = reqwest::Client::new();
    let resp = client.request(reqwest::Method::OPTIONS, format!("http://127.0.0.1:{}/api", actual))
        .header("Origin", "https://example.com")
        .header("Access-Control-Request-Method", "POST")
        .header("Access-Control-Request-Headers", "Content-Type, X-Custom")
        .send().await.unwrap();

    assert_eq!(resp.status(), 204);
    assert_eq!(resp.headers().get("access-control-allow-origin").unwrap(), "*");
    // 回显 Access-Control-Request-Headers
    assert_eq!(resp.headers().get("access-control-allow-headers").unwrap(), "Content-Type, X-Custom");
    assert!(resp.headers().get("access-control-allow-methods").unwrap().to_str().unwrap().contains("POST"));

    run_outside_async(|| crate::c_api::stop_server());
}

#[tokio::test]
async fn test_cors_credentials_echoes_origin() {
    let _guard = cors_test_lock();
    let _reset = ConfigReset;
    let json = CString::new(r#"{"credentials": true}"#).unwrap();
    set_cors_config(json.as_ptr());

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || crate::c_api::start_server(port as i32, host.as_ptr(), test_callback_ok));
    assert!(actual > 0);
    wait_for_server(actual as u16).await;

    let client = reqwest::Client::new();
    let resp = client.get(format!("http://127.0.0.1:{}/hello", actual))
        .header("Origin", "https://app.example.com")
        .send().await.unwrap();

    assert_eq!(resp.headers().get("access-control-allow-origin").unwrap(), "https://app.example.com");
    assert_eq!(resp.headers().get("access-control-allow-credentials").unwrap(), "true");

    run_outside_async(|| crate::c_api::stop_server());
}

#[tokio::test]
async fn test_no_cors_headers_when_disabled() {
    let _guard = cors_test_lock();
    let _reset = ConfigReset;
    set_cors_config(std::ptr::null()); // 确保关闭

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || crate::c_api::start_server(port as i32, host.as_ptr(), test_callback_ok));
    assert!(actual > 0);
    wait_for_server(actual as u16).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/hello", actual)).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("access-control-allow-origin").is_none());

    run_outside_async(|| crate::c_api::stop_server());
}

#[tokio::test]
async fn test_cors_headers_on_static_server() {
    let _guard = cors_test_lock();
    let _reset = ConfigReset;
    let json = CString::new("true").unwrap();
    set_cors_config(json.as_ptr());

    let temp_dir = std::env::temp_dir().join("test_cors_static");
    std::fs::create_dir_all(&temp_dir).unwrap();
    std::fs::write(temp_dir.join("a.txt"), "hello").unwrap();

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let root = CString::new(temp_dir.to_string_lossy().to_string()).unwrap();
    let actual = run_outside_async(move || crate::c_api::start_static_server(port as i32, host.as_ptr(), root.as_ptr()));
    assert!(actual > 0);
    wait_for_server(actual as u16).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/a.txt", actual)).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("access-control-allow-origin").unwrap(), "*");

    run_outside_async(|| crate::c_api::stop_static_server());
    std::fs::remove_dir_all(&temp_dir).ok();
}

#[tokio::test]
async fn test_webdav_options_not_intercepted_by_preflight() {
    let _guard = cors_test_lock();
    let _reset = ConfigReset;
    let json = CString::new("true").unwrap();
    set_cors_config(json.as_ptr());

    let temp_dir = std::env::temp_dir().join("test_webdav_options");
    std::fs::create_dir_all(&temp_dir).unwrap();

    let config = CString::new(format!(
        r#"{{"mounts": [{{"type": "webdav", "path": "/dav", "root": "{}"}}]}}"#,
        temp_dir.to_string_lossy()
    )).unwrap();

    let port = get_free_port();
    let host = CString::new("127.0.0.1").unwrap();
    let actual = run_outside_async(move || crate::c_api::start_server_with_config(
        port as i32, host.as_ptr(), crate::tests::test_utils::test_callback_404, config.as_ptr()));
    assert!(actual > 0);
    wait_for_server(actual as u16).await;

    let client = reqwest::Client::new();
    let resp = client.request(reqwest::Method::OPTIONS, format!("http://127.0.0.1:{}/dav/", actual))
        .header("Origin", "https://example.com")
        // 故意不带 Access-Control-Request-Method —— 这是 WebDAV 协议自身的 OPTIONS
        .send().await.unwrap();

    // 应到达 webdav handler，而非被 preflight 短路（204）
    assert_ne!(resp.status(), 204);

    run_outside_async(|| crate::c_api::stop_app_server());
    std::fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn test_static_mount_custom_headers() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir().join("test_mount_headers");
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(temp_dir.join("a.txt"), "hello").unwrap();

        let plugin = StaticFilePlugin::new("/s".to_string(), temp_dir.clone());
        plugin.on_load(&json!({
            "headers": { "Cache-Control": "no-cache", "X-Frame-Options": "DENY" }
        })).unwrap();

        let req = Request::builder().uri("/s/a.txt").method("GET")
            .body(Body::empty()).unwrap();
        match plugin.handle_request(req).await.unwrap() {
            crate::plugins::PluginAction::Handled(resp) => {
                assert_eq!(resp.headers().get("cache-control").unwrap(), "no-cache");
                assert_eq!(resp.headers().get("x-frame-options").unwrap(), "DENY");
            }
            _ => panic!("expected Handled"),
        }
        std::fs::remove_dir_all(&temp_dir).unwrap();
    });
}

#[test]
fn test_static_mount_custom_headers_override_default() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir().join("test_mount_headers_override");
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(temp_dir.join("a.txt"), "hello").unwrap();

        let plugin = StaticFilePlugin::new("/s".to_string(), temp_dir.clone());
        // 自定义头允许覆盖插件默认头（含 Content-Type）
        plugin.on_load(&json!({ "headers": { "Content-Type": "text/custom" } })).unwrap();

        let req = Request::builder().uri("/s/a.txt").method("GET")
            .body(Body::empty()).unwrap();
        match plugin.handle_request(req).await.unwrap() {
            crate::plugins::PluginAction::Handled(resp) => {
                assert_eq!(resp.headers().get("content-type").unwrap(), "text/custom");
            }
            _ => panic!("expected Handled"),
        }
        std::fs::remove_dir_all(&temp_dir).unwrap();
    });
}
