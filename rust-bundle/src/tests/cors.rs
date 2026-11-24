use crate::c_api::set_cors_config;
use crate::cors::{cors_config_from_json, CorsConfig, CORS_CONFIG};
use crate::plugins::static_file::StaticFilePlugin;
use crate::plugins::Plugin;
use crate::tests::test_utils::{get_free_port, test_callback_ok, wait_for_server};
use hyper::{Request, Body};
use serde_json::json;
use std::ffi::CString;
use tokio::runtime::Runtime;

// Serialize tests that mutate global CORS_CONFIG.
static CORS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquire the CORS test lock, recovering from poisoning so one test panic does
/// not cause cascading PoisonError failures across subsequent tests.
fn cors_test_lock() -> std::sync::MutexGuard<'static, ()> {
    CORS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// Drop guard to reset global CORS_CONFIG even if a test panics.
struct ConfigReset;
impl Drop for ConfigReset {
    fn drop(&mut self) {
        crate::cors::set_cors_config_json(None);
    }
}

// Run start_*/stop_* on a dedicated OS thread because dropping a tokio Runtime
// inside an async test context panics.
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
    assert!(cfg.methods.contains(&"PROPFIND".to_string())); // WebDAV method
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
    // Omitted fields keep defaults.
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

    // Enable
    let json = CString::new("true").unwrap();
    set_cors_config(json.as_ptr());
    assert!(CORS_CONFIG.lock().unwrap().is_some());

    // Custom origin
    let json = CString::new(r#"{"origin": "https://x.com"}"#).unwrap();
    set_cors_config(json.as_ptr());
    assert_eq!(CORS_CONFIG.lock().unwrap().as_ref().unwrap().origin, "https://x.com");

    // NULL disables CORS
    set_cors_config(std::ptr::null());
    assert!(CORS_CONFIG.lock().unwrap().is_none());

    // "false" disables CORS
    let json = CString::new("false").unwrap();
    set_cors_config(json.as_ptr());
    assert!(CORS_CONFIG.lock().unwrap().is_none());
}

#[test]
fn test_apply_cors_credentials_without_origin_adds_no_header() {
    let _guard = cors_test_lock();
    let _reset = ConfigReset;

    // Verify apply_cors omits CORS headers in credentials mode when Origin is absent.
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
    // Echo Access-Control-Request-Headers
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
    set_cors_config(std::ptr::null());

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
        // Omit Access-Control-Request-Method so this stays a WebDAV OPTIONS request.
        .send().await.unwrap();

    // Must reach the WebDAV handler instead of short-circuiting as 204 preflight.
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
        // Custom headers may override plugin defaults (including Content-Type).
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
