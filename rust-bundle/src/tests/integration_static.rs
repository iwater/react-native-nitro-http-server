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

// ===========================================================================
// Task 17：StaticHttpServer 委托给 StaticFilePlugin（行为与 app server 对齐）
// ===========================================================================

/// 起一个 StaticHttpServer，返回 (port, 沙盒根目录, 任务句柄)。调用方负责清理。
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

/// ⚠️ 目录请求必须回 index.html。
///
/// 旧实现把 index 回退写在「`file_path.canonicalize()` **失败**」的分支里，
/// 而目录是**能** canonicalize 成功的 → 于是直接去 `tokio::fs::read(目录)` → EISDIR → 404。
/// 也就是「有 index.html 也回不去」。
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
    assert_eq!(r.text().await.unwrap(), "root index", "GET / 应回根目录的 index.html");

    let r = client.get(format!("{}/sub", base)).send().await.unwrap();
    assert_eq!(
        r.status(),
        StatusCode::OK,
        "GET /sub 应回 sub/index.html（旧实现这里 404：目录能 canonicalize，于是走了 read(目录)）"
    );
    assert_eq!(r.text().await.unwrap(), "sub index");

    let r = client.get(format!("{}/sub/", base)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK, "带尾斜杠也要回 index");

    task.abort();
    fs::remove_dir_all(root.parent().unwrap()).ok();
}

/// ⚠️ 隐藏文件必须 403。
///
/// 旧实现**完全不挡** —— `/.secret` 会原样 200 服务出去（把点文件当普通文件发）。
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
        "隐藏文件必须 403（旧实现会 200 把内容发出去）"
    );
    let body = r.text().await.unwrap();
    assert!(
        !body.contains("TOP SECRET"),
        "403 的响应体里也不能带出文件内容"
    );

    // 防回归：普通文件照常
    let r = client.get(format!("{}/normal.txt", base)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    task.abort();
    fs::remove_dir_all(root.parent().unwrap()).ok();
}

/// percent 编码路径要能命中带空格的文件名（旧实现不 decode → 404）。
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
    assert_eq!(r.status(), StatusCode::OK, "percent 编码路径应命中 hello world.txt");
    assert_eq!(r.text().await.unwrap(), "spaced");

    task.abort();
    fs::remove_dir_all(root.parent().unwrap()).ok();
}

/// HEAD 只回头；以及**行为变更**：委托后非 GET/HEAD 不再被静态服务接管（404）。
///
/// 旧实现不检查请求方法 —— POST /a.txt 也会把文件内容发出去。
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
    assert_eq!(r.bytes().await.unwrap().len(), 0, "HEAD 不能带 body");

    let r = client.post(format!("{}/a.txt", base)).send().await.unwrap();
    assert_eq!(
        r.status(),
        StatusCode::NOT_FOUND,
        "委托给插件后非 GET/HEAD 不再被接管（插件只处理 GET/HEAD）—— 这是本任务注明的行为变更"
    );

    task.abort();
    fs::remove_dir_all(root.parent().unwrap()).ok();
}

/// 插件报错 → **500 `Plugin error: ...`**，与 `PluginManager`（app server 走的那条）一致；
/// **不是** 404。
///
/// 计划给的片段是 `_ => {}` 落到 404。那会把「文件在 canonicalize 与 open 之间消失」
/// 「权限不足」这类**真实 IO 错误**伪装成「文件不存在」，查错方向会被带偏 ——
/// 而本任务的标题就是「对齐 static_file 插件行为」。
///
/// 触发方式：去掉文件的读权限。`canonicalize()`/`metadata()` 仍成功（只需要父目录的
/// x 位），但 `File::open` 会 EACCES → 插件返回 Err。
/// ⚠️ 以 root 跑时 open 仍会成功，这条用例会失败 —— 本仓库的测试一直以普通用户跑。
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
        "插件 IO 失败必须是 500（与 PluginManager 的约定一致），不是 404"
    );
    let body = r.text().await.unwrap();
    assert!(
        body.contains("Plugin error"),
        "响应体应带 `Plugin error: ...`，实际 {:?}",
        body
    );

    // 恢复权限再清理（unlink 只看父目录的写位，但保持沙盒干净）
    let mut perms = fs::metadata(&locked).unwrap().permissions();
    perms.set_mode(0o644);
    fs::set_permissions(&locked, perms).unwrap();

    task.abort();
    fs::remove_dir_all(root.parent().unwrap()).ok();
}
