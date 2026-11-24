use crate::server::static_server::StaticHttpServer;
use crate::tests::test_utils::{get_free_port, wait_for_server};
use std::time::Duration;
use std::fs;
use reqwest::StatusCode;

#[tokio::test]
async fn test_static_server_integration() {
    let temp_dir_base = std::env::temp_dir().join("test_static_server");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    let temp_dir = temp_dir_base.canonicalize().unwrap();
    
    fs::write(temp_dir.join("hello.txt"), "Hello Static World").unwrap();
    fs::create_dir(temp_dir.join("subdir")).unwrap();
    fs::write(temp_dir.join("subdir/sub.txt"), "Sub Content").unwrap();
    
    let free_port = get_free_port();
    
    // Clone path for thread
    let root_for_server = temp_dir.clone();
    
    let server_task = tokio::spawn(async move {
        // StaticHttpServer::new(port, host, root_dir)
        let mut server = StaticHttpServer::new(free_port, "127.0.0.1".to_string(), root_for_server);
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    wait_for_server(free_port).await;
    let base_url = format!("http://127.0.0.1:{}", free_port);
    let client = reqwest::Client::new();

    // 1. Root file
    let resp = client.get(format!("{}/hello.txt", base_url)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "Hello Static World");
    
    // 2. Subdir file
    let resp_sub = client.get(format!("{}/subdir/sub.txt", base_url)).send().await.unwrap();
    assert_eq!(resp_sub.status(), StatusCode::OK);
    assert_eq!(resp_sub.text().await.unwrap(), "Sub Content");
    
    // 3. 404
    let resp_404 = client.get(format!("{}/unknown.txt", base_url)).send().await.unwrap();
    assert_eq!(resp_404.status(), StatusCode::NOT_FOUND);

    server_task.abort();
    fs::remove_dir_all(&temp_dir_base).unwrap();
}

// StaticHttpServer delegates to StaticFilePlugin to align behavior with AppHttpServer.

/// Start a StaticHttpServer and return `(port, canonical_root, task_handle)`.
async fn t17_start(tag: &str) -> (u16, std::path::PathBuf, tokio::task::JoinHandle<()>) {
    let base = std::env::temp_dir().join(format!("t17_{}_{}", tag, uuid::Uuid::new_v4()));
    fs::create_dir_all(&base).unwrap();
    let root = base.canonicalize().unwrap();

    let port = get_free_port();
    let root_for_server = root.clone();
    let handle = tokio::spawn(async move {
        let mut server = StaticHttpServer::new(port, "127.0.0.1".to_string(), root_for_server);
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    wait_for_server(port).await;
    (port, root, handle)
}

#[tokio::test]
async fn test_static_server_directory_request_serves_index() {
    let (port, root, task) = t17_start("dirindex").await;
    fs::write(root.join("index.html"), "root index").unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub").join("index.html"), "sub index").unwrap();

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", port);

    let r = client.get(format!("{}/", base)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.text().await.unwrap(), "root index", "GET / should serve root index.html");

    let r = client.get(format!("{}/sub", base)).send().await.unwrap();
    assert_eq!(
        r.status(),
        StatusCode::OK,
        "GET /sub should serve sub/index.html"
    );
    assert_eq!(r.text().await.unwrap(), "sub index");

    let r = client.get(format!("{}/sub/", base)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK, "trailing slash should also serve index.html");

    task.abort();
    fs::remove_dir_all(root.parent().unwrap()).ok();
}

#[tokio::test]
async fn test_static_server_blocks_hidden_files() {
    let (port, root, task) = t17_start("hidden").await;
    fs::write(root.join(".secret"), "TOP SECRET").unwrap();
    fs::write(root.join("normal.txt"), "ok").unwrap();

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", port);

    let r = client.get(format!("{}/.secret", base)).send().await.unwrap();
    assert_eq!(
        r.status(),
        StatusCode::FORBIDDEN,
        "hidden files must return 403"
    );
    let body = r.text().await.unwrap();
    assert!(
        !body.contains("TOP SECRET"),
        "403 body must not leak file contents"
    );

    let r = client.get(format!("{}/normal.txt", base)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    task.abort();
    fs::remove_dir_all(root.parent().unwrap()).ok();
}

#[tokio::test]
async fn test_static_server_url_decodes_path() {
    let (port, root, task) = t17_start("decode").await;
    fs::write(root.join("hello world.txt"), "spaced").unwrap();

    let client = reqwest::Client::new();
    let r = client
        .get(format!("http://127.0.0.1:{}/hello%20world.txt", port))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK, "percent-encoded path should match hello world.txt");
    assert_eq!(r.text().await.unwrap(), "spaced");

    task.abort();
    fs::remove_dir_all(root.parent().unwrap()).ok();
}

#[tokio::test]
async fn test_static_server_head_and_method_scope() {
    let (port, root, task) = t17_start("headmethod").await;
    fs::write(root.join("a.txt"), "hi").unwrap();

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", port);

    let r = client.head(format!("{}/a.txt", base)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        r.headers().get("content-length").map(|v| v.to_str().unwrap()),
        Some("2")
    );
    assert_eq!(r.bytes().await.unwrap().len(), 0, "HEAD must not return a body");

    let r = client.post(format!("{}/a.txt", base)).send().await.unwrap();
    assert_eq!(
        r.status(),
        StatusCode::NOT_FOUND,
        "non-GET/HEAD methods should not be handled by the static plugin"
    );

    task.abort();
    fs::remove_dir_all(root.parent().unwrap()).ok();
}

/// Plugin I/O errors must return 500 rather than 404.
/// Note: requires running as a non-root user so chmod 000 triggers EACCES.
#[tokio::test]
async fn test_static_server_plugin_error_is_500_not_404() {
    use std::os::unix::fs::PermissionsExt;

    let (port, root, task) = t17_start("plugerr").await;
    let locked = root.join("locked.txt");
    fs::write(&locked, "secret").unwrap();
    let mut perms = fs::metadata(&locked).unwrap().permissions();
    perms.set_mode(0o000);
    fs::set_permissions(&locked, perms).unwrap();

    let client = reqwest::Client::new();
    let r = client
        .get(format!("http://127.0.0.1:{}/locked.txt", port))
        .send()
        .await
        .unwrap();

    assert_eq!(
        r.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "plugin I/O failure must return 500, not 404"
    );
    let body = r.text().await.unwrap();
    assert!(
        body.contains("Plugin error"),
        "expected `Plugin error: ...` in body, got {:?}",
        body
    );

    // Restore permissions before cleanup.
    let mut perms = fs::metadata(&locked).unwrap().permissions();
    perms.set_mode(0o644);
    fs::set_permissions(&locked, perms).unwrap();

    task.abort();
    fs::remove_dir_all(root.parent().unwrap()).ok();
}
