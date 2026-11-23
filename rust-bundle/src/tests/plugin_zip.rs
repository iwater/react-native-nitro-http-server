use crate::plugins::zip_mount::ZipMountPlugin;
use crate::plugins::Plugin;
use hyper::{Request, Body, Method};
use tokio::runtime::Runtime;
use std::fs::File;

use async_zip::base::write::ZipFileWriter;
use async_zip::Compression;
// use std::sync::Arc;

#[test]
fn test_zip_mount_plugin_should_handle() {
    let temp_dir = std::env::temp_dir();
    let zip_path = temp_dir.join("test.zip");
    // Create empty file just to satisfy path existence check if any (ZipMountPlugin checks file existence on load)
    File::create(&zip_path).unwrap();

    let plugin = ZipMountPlugin::new("/zip".to_string(), zip_path.to_string_lossy().to_string());
    
    // 1. Match path
    let req = Request::builder()
        .uri("/zip/file.txt")
        .method(Method::GET)
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&req));
    
    // 2. Mismatch path
    let req_path = Request::builder()
        .uri("/other/file.txt")
        .method(Method::GET)
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req_path));
    
    // 3. Mismatch method (only GET supported for static files usually)
    // Actually ZipMountPlugin only supports GET? Let's assume so.
    let req_method = Request::builder()
        .uri("/zip/file.txt")
        .method(Method::POST)
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req_method));
}

#[test]
fn test_zip_mount_plugin_content_reading() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir();
        let zip_path = temp_dir.join("test_content.zip");
        
        // Create a real zip file
        let file = tokio::fs::File::create(&zip_path).await.unwrap();
        let mut writer = ZipFileWriter::with_tokio(file);
        
        let builder = async_zip::ZipEntryBuilder::new("hello.txt".to_string().into(), Compression::Deflate);
        writer.write_entry_whole(builder, b"Hello Zip World").await.unwrap();
        
        let builder_dir = async_zip::ZipEntryBuilder::new("dir/nested.txt".to_string().into(), Compression::Deflate);
        writer.write_entry_whole(builder_dir, b"Nested Content").await.unwrap();
        
        writer.close().await.unwrap();
        
        // Initialize plugin
        // Note: ZipMountPlugin::new doesn't return Result, but it might try to open file?
        // Actually it probably just stores config. The file is opened on demand or during on_load?
        // Let's check implementation if needed. Assuming lazy opening or on_load.
        // Actually ZipMountPlugin likely maps file in memory or keeps handle.
        // Let's assume standard behavior.
        
        let plugin = ZipMountPlugin::new("/zip".to_string(), zip_path.to_string_lossy().to_string());
        
        // Setup mock config if on_load does anything critical?
        // Typically new() is enough for basic config.
        
        // 1. Read root file
        let req = Request::builder()
            .uri("/zip/hello.txt")
            .method(Method::GET)
            .body(Body::empty())
            .unwrap();
            
        if let Ok(crate::plugins::PluginAction::Handled(resp)) = plugin.handle_request(req).await {
            assert_eq!(resp.status(), 200);
            let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
            assert_eq!(bytes, "Hello Zip World");
        } else {
             panic!("Should Handle root file");
        }
        
        // 2. Read nested file
        let req_nested = Request::builder()
            .uri("/zip/dir/nested.txt")
            .method(Method::GET)
            .body(Body::empty())
            .unwrap();
            
        if let Ok(crate::plugins::PluginAction::Handled(resp)) = plugin.handle_request(req_nested).await {
            assert_eq!(resp.status(), 200);
            let headers = resp.headers();
            // Verify MIME type for .txt is text/plain
            assert!(headers.get("content-type").unwrap().to_str().unwrap().contains("text/plain"));
            
            let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
            assert_eq!(bytes, "Nested Content");
        } else {
             panic!("Should Handle nested file");
        }
        
        // 3. Not found
        let req_404 = Request::builder()
            .uri("/zip/not_exist.txt")
            .method(Method::GET)
            .body(Body::empty())
            .unwrap();
            
        if let Ok(crate::plugins::PluginAction::Handled(resp)) = plugin.handle_request(req_404).await {
            assert_eq!(resp.status(), 404);
        } else {
            // It might pass through? Or handle with 404? 
            // Usually static handlers return 404 if path matches mount but file missing.
            // Let's see implementation behavior.
             panic!("Should Handle missing file (as 404 or similar) or PassThrough depending on impl");
        }
    });
}

// ===========================================================================
// Task 14：zip 只在加载时解析一次（请求时零文件 IO）+ 挂载点切片的 panic
// ===========================================================================

/// 造一个带 uuid 的 zip（避免用例之间用同一个文件名互相踩）
async fn t14_make_zip(tag: &str, entries: &[(&str, &str)]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("t14_{}_{}.zip", tag, uuid::Uuid::new_v4()));
    let file = tokio::fs::File::create(&path).await.unwrap();
    let mut writer = ZipFileWriter::with_tokio(file);
    for (name, body) in entries {
        let builder = async_zip::ZipEntryBuilder::new((*name).to_string().into(), Compression::Deflate);
        writer.write_entry_whole(builder, body.as_bytes()).await.unwrap();
    }
    writer.close().await.unwrap();
    path
}

fn t14_get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).method(Method::GET).body(Body::empty()).unwrap()
}

/// ⚠️ **panic 回归**：`mount_path` 带尾斜杠 + 请求恰好是挂载点本身。
///
/// 旧实现按**字节长度**切片：`uri_path[mount.mount_path.len()..]`。
/// `mount="/zip/"`（5 字节）而 `uri_path="/zip"`（4 字节）→ 越界 → **panic**。
/// 而 `path_matches_mount` 认为它匹配（尾斜杠等价），所以 `should_handle` 会放它进来。
///
/// 触发条件很普通：配置里写 `"path": "/zip/"`，客户端请求 `/zip`。
#[tokio::test]
async fn test_zip_mount_slash_mount_root_request_does_not_panic() {
    let zip = t14_make_zip("slash", &[("hello.txt", "Hi")]).await;
    let plugin = ZipMountPlugin::new("/zip/".to_string(), zip.to_string_lossy().to_string());

    let req = t14_get("/zip");
    assert!(
        plugin.should_handle(&req),
        "尾斜杠等价：mount=/zip/ 应接管 /zip 本身"
    );

    // 不 panic 即通过；返回 404 是合理结果（zip 里没有名为 "" 的 entry）
    let action = plugin.handle_request(t14_get("/zip")).await;
    match action {
        Ok(crate::plugins::PluginAction::Handled(resp)) => {
            assert_eq!(resp.status(), 404, "挂载点本身在 zip 里没有对应 entry → 404");
        }
        Ok(_) => panic!("应返回 Handled"),
        Err(e) => panic!("不该是错误（旧实现在这里 panic）：{}", e),
    }

    std::fs::remove_file(&zip).ok();
}

/// 核心：解析过的 zip 常驻内存 —— **把磁盘上的 zip 删掉之后，后续请求仍要能服务**。
///
/// 这是「请求时零文件 IO」唯一可观测的判据。旧实现每个请求都 `File::open`，
/// 删掉文件后必然失败。
#[tokio::test]
async fn test_zip_mount_serves_after_zip_file_removed() {
    let zip = t14_make_zip("removed", &[("hello.txt", "Hello Zip World")]).await;
    let plugin = ZipMountPlugin::new("/zip".to_string(), zip.to_string_lossy().to_string());

    // 第一次请求（惰性加载会发生在这里）
    let resp = match plugin.handle_request(t14_get("/zip/hello.txt")).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("应被处理"),
    };
    assert_eq!(resp.status(), 200);
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(bytes, "Hello Zip World");

    // 磁盘上的 zip 消失
    std::fs::remove_file(&zip).unwrap();

    // 后续请求必须仍然成功 —— 证明没有每请求重开文件
    let resp = match plugin.handle_request(t14_get("/zip/hello.txt")).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("应被处理"),
    };
    assert_eq!(
        resp.status(),
        200,
        "zip 已从磁盘删除，但内容在内存里 —— 仍应服务（旧实现每请求 File::open，这里会失败）"
    );
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(bytes, "Hello Zip World");
}

/// 防回归：`on_load` 换了 zip 路径之后，必须**重新加载**而不是继续用旧缓存。
#[tokio::test]
async fn test_zip_mount_on_load_switching_zip_path_reloads() {
    let zip_a = t14_make_zip("a", &[("same.txt", "AAA")]).await;
    let zip_b = t14_make_zip("b", &[("same.txt", "BBB")]).await;

    let plugin = ZipMountPlugin::new("/zip".to_string(), zip_a.to_string_lossy().to_string());

    let resp = match plugin.handle_request(t14_get("/zip/same.txt")).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("应被处理"),
    };
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(bytes, "AAA");

    // 换 zip
    plugin
        .on_load(&serde_json::json!({ "zip_file": zip_b.to_string_lossy() }))
        .unwrap();

    let resp = match plugin.handle_request(t14_get("/zip/same.txt")).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("应被处理"),
    };
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(bytes, "BBB", "on_load 换了 zip 路径后必须重新加载，不能继续用旧缓存");

    std::fs::remove_file(&zip_a).ok();
    std::fs::remove_file(&zip_b).ok();
}

/// 防回归：percent 编码的 entry 名仍要能命中（decode 不能被这次改动弄丢）。
#[tokio::test]
async fn test_zip_mount_percent_decoded_entry_name() {
    let zip = t14_make_zip("decode", &[("hello world.txt", "spaced")]).await;
    let plugin = ZipMountPlugin::new("/zip".to_string(), zip.to_string_lossy().to_string());

    let resp = match plugin.handle_request(t14_get("/zip/hello%20world.txt")).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("应被处理"),
    };
    assert_eq!(resp.status(), 200, "percent 编码的 entry 名应能命中");
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(bytes, "spaced");

    std::fs::remove_file(&zip).ok();
}

/// 防回归：zip 不存在时返回**错误**（不 panic、不 200），且之后 zip 出现能自愈。
#[tokio::test]
async fn test_zip_mount_missing_zip_errors_then_self_heals() {
    let missing = std::env::temp_dir().join(format!("t14_missing_{}.zip", uuid::Uuid::new_v4()));
    let plugin = ZipMountPlugin::new("/zip".to_string(), missing.to_string_lossy().to_string());

    let first = plugin.handle_request(t14_get("/zip/hello.txt")).await;
    assert!(first.is_err(), "zip 不存在时应返回错误");

    // zip 稍后出现 → 必须能自愈（失败的加载不能被永久缓存）
    let file = tokio::fs::File::create(&missing).await.unwrap();
    let mut writer = ZipFileWriter::with_tokio(file);
    let builder = async_zip::ZipEntryBuilder::new("hello.txt".to_string().into(), Compression::Deflate);
    writer.write_entry_whole(builder, b"late zip").await.unwrap();
    writer.close().await.unwrap();

    let resp = match plugin.handle_request(t14_get("/zip/hello.txt")).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("应被处理"),
    };
    assert_eq!(resp.status(), 200, "zip 稍后创建后必须能自愈");
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(bytes, "late zip");

    std::fs::remove_file(&missing).ok();
}
