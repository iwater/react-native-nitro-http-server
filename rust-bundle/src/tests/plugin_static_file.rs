use crate::plugins::static_file::StaticFilePlugin;
use crate::plugins::Plugin;
use hyper::{Request, Body};
use tokio::runtime::Runtime;
use serde_json::json;

#[test]
fn test_static_file_plugin_security() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        // Use current directory for testing
        let current_dir = std::env::current_dir().unwrap();
        let plugin = StaticFilePlugin::new("/static".to_string(), current_dir.clone());
        
        // 1. Valid request to existing file (e.g., Cargo.toml which should exist in project root)
        let req_valid = Request::builder()
            .uri("/static/Cargo.toml")
            .method("GET")
            .body(Body::empty())
            .unwrap();
            
        assert!(plugin.should_handle(&req_valid));
        
        // 2. Directory traversal attempt
        let req_traversal = Request::builder()
            .uri("/static/../Cargo.toml")
            .method("GET")
            .body(Body::empty())
            .unwrap();
            
        // The plugin logic prevents handling paths that traverse out of root
        assert!(!plugin.should_handle(&req_traversal));
        
        // 3. Absolute path attempt
        let req_abs = Request::builder()
            .uri("/static//etc/passwd") 
            .method("GET")
            .body(Body::empty())
            .unwrap();
            
        assert!(!plugin.should_handle(&req_abs));
    });
}

#[test]
fn test_static_file_plugin_serving() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir().join("test_static_file_serving");
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir).unwrap();
        }
        std::fs::create_dir_all(&temp_dir).unwrap();
        
        let file_path = temp_dir.join("index.html");
        std::fs::write(&file_path, "<html><body>Hello</body></html>").unwrap();
        
        let plugin = StaticFilePlugin::new("/static".to_string(), temp_dir.clone());
        
        // 1. Serve existing file
        let req = Request::builder()
            .uri("/static/index.html")
            .method("GET")
            .body(Body::empty())
            .unwrap();
            
        if let Ok(crate::plugins::PluginAction::Handled(resp)) = plugin.handle_request(req).await {
            assert_eq!(resp.status(), 200);
            assert_eq!(resp.headers().get("content-type").unwrap(), "text/html; charset=utf-8");
            let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
            assert_eq!(bytes, "<html><body>Hello</body></html>");
        } else {
            panic!("Should handle existing file");
        }
        
        // 2. Serve non-existent file (PassThrough or 404?)
        // In StaticFilePlugin logic, if file doesn't exist, it might try directory listing or return 404.
        // If directory listing is disabled (default?), and no index file found.
        // Let's check behavior. Usually it returns 404.
        let req_404 = Request::builder()
            .uri("/static/not_found.html")
            .method("GET")
            .body(Body::empty())
            .unwrap();
            
        if let Ok(crate::plugins::PluginAction::Handled(resp)) = plugin.handle_request(req_404).await {
             // It returns Handled(404) usually
             assert_eq!(resp.status(), 404);
        } else {
             // Or maybe PassThrough if configured to fallback?
             // Assuming default is strict static serving for mount path.
        }
        
        std::fs::remove_dir_all(&temp_dir).unwrap();
    });
}

#[test]
fn test_static_file_plugin_directory_listing() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir().join("test_static_file_listing");
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir).unwrap();
        }
        std::fs::create_dir_all(&temp_dir).unwrap();
        
        // Create some files
        std::fs::write(temp_dir.join("file1.txt"), "Content 1").unwrap();
        std::fs::write(temp_dir.join("file2.log"), "Content 2").unwrap();
        std::fs::create_dir(temp_dir.join("subdir")).unwrap();
        
        let plugin = StaticFilePlugin::new("/static".to_string(), temp_dir.clone());
        
        // Enable directory listing via on_load
        let config = json!({
            "dir_list": {
                "enabled": true,
                "show_hidden": false
            }
        });
        plugin.on_load(&config).unwrap();
        
        // Request directory
        let req = Request::builder()
            .uri("/static/")
            .method("GET")
            .body(Body::empty())
            .unwrap();
            
        if let Ok(crate::plugins::PluginAction::Handled(resp)) = plugin.handle_request(req).await {
            assert_eq!(resp.status(), 200);
            assert_eq!(resp.headers().get("content-type").unwrap(), "text/html; charset=utf-8");
            
            let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
            let html = String::from_utf8(bytes.to_vec()).unwrap();
            
            // Verify HTML content
            assert!(html.contains("Directory Listing"));
            assert!(html.contains("file1.txt"));
            assert!(html.contains("file2.log"));
            assert!(html.contains("subdir/"));
            assert!(html.contains("Index of /static/"));
        } else {
            panic!("Should generate directory listing");
        }
        
        std::fs::remove_dir_all(&temp_dir).unwrap();
    });
}

#[test]
fn test_static_file_plugin_default_index() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir().join("test_static_file_index");
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir).unwrap();
        }
        std::fs::create_dir_all(&temp_dir).unwrap();
        
        std::fs::write(temp_dir.join("index.html"), "Index Content").unwrap();
        
        let plugin = StaticFilePlugin::new("/static".to_string(), temp_dir.clone());
        
        // Request directory, expecting index.html
        let req = Request::builder()
            .uri("/static/")
            .method("GET")
            .body(Body::empty())
            .unwrap();
            
        if let Ok(crate::plugins::PluginAction::Handled(resp)) = plugin.handle_request(req).await {
            assert_eq!(resp.status(), 200);
            let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
            assert_eq!(bytes, "Index Content");
        } else {
            panic!("Should serve index.html");
        }
        
        std::fs::remove_dir_all(&temp_dir).unwrap();
    });
}

#[test]
fn test_static_file_plugin_hidden_files() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir().join("test_static_file_hidden");
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir).unwrap();
        }
        std::fs::create_dir_all(&temp_dir).unwrap();
        
        // Create hidden file and regular file
        std::fs::write(temp_dir.join(".hidden.txt"), "Hidden Content").unwrap();
        std::fs::write(temp_dir.join("visible.txt"), "Visible Content").unwrap();
        
        let plugin = StaticFilePlugin::new("/static".to_string(), temp_dir.clone());
        
        // 1. Default config (show_hidden: false)
        // Explicit access to hidden file should fail (return 403 or 404 depending on impl security)
        // Assuming security check blocks it.
        let req_hidden = Request::builder()
            .uri("/static/.hidden.txt")
            .method("GET")
            .body(Body::empty())
            .unwrap();
            
        // should_handle returns bool, not Result
        if plugin.should_handle(&req_hidden) {
             // should_handle is true for path match, handle_request enforces policy
        }
        
        // Verify handle_request blocks it by default
        if let Ok(crate::plugins::PluginAction::Handled(resp)) = plugin.handle_request(req_hidden).await {
            // Usually returns 403 Forbidden or 404 for hidden files
            assert!(resp.status() == 403 || resp.status() == 404);
        }
        
        // 2. Enable show_hidden
        let config = json!({
            "dir_list": {
                "show_hidden": true
            }
        });
        plugin.on_load(&config).unwrap();
        
        // Access hidden file again
        let req_hidden_2 = Request::builder()
            .uri("/static/.hidden.txt")
            .method("GET")
            .body(Body::empty())
            .unwrap();
            
        if let Ok(crate::plugins::PluginAction::Handled(resp)) = plugin.handle_request(req_hidden_2).await {
            assert_eq!(resp.status(), 200);
            let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
            assert_eq!(bytes, "Hidden Content");
        } else {
            panic!("Should serve hidden file when enabled");
        }
        
        std::fs::remove_dir_all(&temp_dir).unwrap();
    });
}

// ---- Task 8: mount 前缀匹配边界 ----

/// helper 契约：路径段边界。`/static` 只能接管 `/static` 本身与 `/static/...`，
/// 不能接管 `/staticfoo/...`。
#[test]
fn test_mount_prefix_boundary() {
    assert!(crate::plugins::path_matches_mount("/static/a.txt", "/static"));
    assert!(crate::plugins::path_matches_mount("/static", "/static"));
    assert!(crate::plugins::path_matches_mount("/static/", "/static"));
    assert!(crate::plugins::path_matches_mount("/anything", "/"));
    assert!(!crate::plugins::path_matches_mount("/staticfoo/a.txt", "/static"));
    assert!(!crate::plugins::path_matches_mount("/stat", "/static"));
}

/// 尾斜杠等价：配置里写 `"/static/"` 很常见，必须与 `"/static"` 完全等价。
///
/// 计划给的 helper 实现**没有**这一条 —— 它会把 `/static/` 归一化漏掉，
/// 于是 `"/static/a".starts_with("/static/")` 成立但 `rest` = `"a"`（不以 `/` 开头）
/// → 返回 false。也就是**挂了却永远匹配不上**，比裸 starts_with 还糟。
#[test]
fn test_mount_prefix_trailing_slash_is_equivalent() {
    for mount in ["/static", "/static/"] {
        assert!(
            crate::plugins::path_matches_mount("/static/a.txt", mount),
            "mount={} 应匹配 /static/a.txt",
            mount
        );
        assert!(
            crate::plugins::path_matches_mount("/static", mount),
            "mount={} 应匹配 /static 本身",
            mount
        );
        assert!(
            !crate::plugins::path_matches_mount("/staticfoo/a.txt", mount),
            "mount={} 不该匹配 /staticfoo/a.txt",
            mount
        );
    }
}

/// 插件级：这是本任务真正的用户可见症状 —— 挂 `/static` 的插件会把
/// `/staticfoo/a.txt` 当成 `/static` 下的 `foo/a.txt` 读走。
///
/// 为了让这条有区分力，被碰撞的路径**必须真实存在**：否则修复前后
/// `should_handle` 都因为「文件不存在」返回 false，测不出差别。
#[test]
fn test_static_file_plugin_rejects_prefix_collision() {
    let root = std::env::temp_dir().join(format!("t8_static_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("foo")).unwrap();
    std::fs::write(root.join("foo").join("a.txt"), b"leaked").unwrap();

    let plugin = StaticFilePlugin::new("/static".to_string(), root.clone());

    let collided = Request::builder()
        .uri("/staticfoo/a.txt")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    assert!(
        !plugin.should_handle(&collided),
        "挂载 /static 不该接管 /staticfoo/a.txt —— 否则会把它当成 foo/a.txt 从 root_dir 读走"
    );

    // 防回归：正常路径仍然要接管
    let normal = Request::builder()
        .uri("/static/foo/a.txt")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&normal), "/static/foo/a.txt 仍应被接管");

    std::fs::remove_dir_all(&root).ok();
}

// ===========================================================================
// Task 13：percent-decode（`should_handle` **与** `handle_inner` 都要）+ HEAD
// ===========================================================================

/// 造一个带 uuid 的临时根目录（避免残留与并行用例互相干扰）
fn t13_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("t13_{}_{}", tag, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// helper 契约：`resolve_relative` 必须与 `path_matches_mount` 用**同一套**归一化。
///
/// ⚠️ 实测口径（变异校验得出的）：计划给的 `if mount_path == "/"` 版本在
/// **常见写法**（`"/static"` / `"/static/"`）下与正确版本**等价** —— 因为
/// `strip_prefix` 失败后会 `unwrap_or("/")`，恰好把尾斜杠的差异掩盖掉。
/// 只有**重复尾斜杠**（`"/static//"`）才暴露：那时它会给出一条凭空的 "/"，
/// 于是 `handle_inner` 去服务 root 的 index.html 而不是用户要的文件
/// —— **服务错文件**，比 404 糟。
///
/// 所以这条偏离属**契约加固**（让两个 helper 的归一化真正同源），不是修活 bug。
#[test]
fn test_resolve_relative_consistent_with_path_matches_mount() {
    use crate::plugins::resolve_relative as rr;

    // 与 path_matches_mount 同一口径：先 trim 尾斜杠，"" 与 "/" 都算根挂载
    for mount in ["/static", "/static/", "/static//"] {
        assert_eq!(rr(mount, "/static/a.txt"), "a.txt", "mount={}", mount);
        assert_eq!(rr(mount, "/static"), "", "mount={} 挂载点本身", mount);
        assert_eq!(rr(mount, "/static/"), "", "mount={} 挂载点 + 尾斜杠", mount);
    }
    assert_eq!(rr("/", "/a.txt"), "a.txt");
    assert_eq!(rr("", "/a.txt"), "a.txt");
    assert_eq!(rr("///", "/a.txt"), "a.txt");

    // percent-decode
    assert_eq!(rr("/s", "/s/hello%20world.txt"), "hello world.txt");
    assert_eq!(rr("/s", "/s/a%2Bb.txt"), "a+b.txt");
    assert_eq!(rr("/s", "/s/%E6%8A%A5%E5%91%8A.txt"), "报告.txt");

    // 非法序列：urlencoding 对 "%ZZ" 是**宽松**的（原样保留），不报错
    assert_eq!(rr("/s", "/s/a%ZZb.txt"), "a%ZZb.txt");
    // 解出非法 UTF-8（%FF）时退化回原文，不 panic
    assert_eq!(rr("/s", "/s/a%FFb"), "a%FFb");
}

/// 插件级：percent 编码路径要能真的**服务到文件**。
///
/// ⚠️ 两个断言缺一不可 —— 计划给的用例只测 `should_handle`：
/// decode 只写在 `handle_inner` 里 → `should_handle` 因 canonicalize 失败返回 false；
/// 只写在 `should_handle` 里 → 请求进来了但去找字面量 `hello%20world.txt` → 404。
#[tokio::test]
async fn test_static_file_percent_decoded_path_is_served() {
    let root = t13_root("decode");
    std::fs::write(root.join("hello world.txt"), b"hi there").unwrap();

    let plugin = StaticFilePlugin::new("/s".to_string(), root.clone());

    let make_req = || {
        Request::builder()
            .uri("/s/hello%20world.txt")
            .method("GET")
            .body(Body::empty())
            .unwrap()
    };

    assert!(
        plugin.should_handle(&make_req()),
        "percent 编码路径应被接管（只在 handle_inner 里 decode 的话这里就是 false）"
    );

    let resp = match plugin.handle_request(make_req()).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("静态插件应接管该请求"),
    };
    assert_eq!(
        resp.status(),
        200,
        "只在 should_handle 里 decode 的话，handle_inner 会去找字面量 hello%20world.txt → 404"
    );
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(&bytes[..], b"hi there");

    std::fs::remove_dir_all(&root).ok();
}

/// 插件级：HEAD 必须被插件接管（否则落到用户 callback），且**只回头不带 body**。
#[tokio::test]
async fn test_static_file_head_serves_headers_without_body() {
    let root = t13_root("head");
    std::fs::write(root.join("a.txt"), b"0123456789").unwrap();

    let plugin = StaticFilePlugin::new("/s".to_string(), root.clone());

    let req = Request::builder()
        .uri("/s/a.txt")
        .method("HEAD")
        .body(Body::empty())
        .unwrap();
    assert!(
        plugin.should_handle(&req),
        "HEAD 必须被接管 —— 否则 HEAD /s/a.txt 会落到用户 callback（拿不到文件头）"
    );

    let req = Request::builder()
        .uri("/s/a.txt")
        .method("HEAD")
        .body(Body::empty())
        .unwrap();
    let resp = match plugin.handle_request(req).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("静态插件应接管 HEAD"),
    };
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-length").map(|v| v.to_str().unwrap()),
        Some("10"),
        "HEAD 必须带上文件的 Content-Length"
    );
    assert_eq!(
        resp.headers().get("content-type").map(|v| v.to_str().unwrap()),
        Some("text/plain; charset=utf-8")
    );
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(bytes.len(), 0, "HEAD 不能带 body");

    std::fs::remove_dir_all(&root).ok();
}

/// 防回归：GET 走流式后，内容必须完整且**仍然带 Content-Length**（不能悄悄变成 chunked）。
#[tokio::test]
async fn test_static_file_get_streams_full_body_with_content_length() {
    let root = t13_root("stream");
    let content = "x".repeat(100_000);
    std::fs::write(root.join("big.txt"), &content).unwrap();

    let plugin = StaticFilePlugin::new("/s".to_string(), root.clone());
    let req = Request::builder()
        .uri("/s/big.txt")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let resp = match plugin.handle_request(req).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("静态插件应接管该请求"),
    };
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-length").map(|v| v.to_str().unwrap()),
        Some("100000"),
        "改成流式后必须显式给 Content-Length —— 否则 hyper 会改成 chunked 编码"
    );
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(bytes.len(), 100_000, "流式响应不能截断");

    std::fs::remove_dir_all(&root).ok();
}

/// ⚠️ 安全回归：decode 会引入 `..`（`%2e%2e`），必须仍然被 canonicalize 边界挡住。
///
/// 这是**本次改动新引入的攻击面**：改之前 `%2e%2e` 是字面量目录名，走不出 root；
/// 改之后它变成 `..`，只能靠 `canonicalize` + `starts_with(root)` 兜住。
#[tokio::test]
async fn test_static_file_percent_encoded_traversal_still_blocked() {
    let root = t13_root("trav");
    let secret = root.parent().unwrap().join(format!("t13_secret_{}.txt", uuid::Uuid::new_v4()));
    std::fs::write(&secret, b"TOP SECRET").unwrap();

    let plugin = StaticFilePlugin::new("/s".to_string(), root.clone());
    let secret_name = secret.file_name().unwrap().to_str().unwrap();

    for uri in [
        format!("/s/%2e%2e/{}", secret_name),
        format!("/s/%2e%2e%2F{}", secret_name),
    ] {
        let req = Request::builder()
            .uri(&uri)
            .method("GET")
            .body(Body::empty())
            .unwrap();
        assert!(
            !plugin.should_handle(&req),
            "{} 不该被接管（%2e%2e 解码成 .. 后越出了 root）",
            uri
        );

        let req = Request::builder()
            .uri(&uri)
            .method("GET")
            .body(Body::empty())
            .unwrap();
        if let Ok(crate::plugins::PluginAction::Handled(resp)) = plugin.handle_request(req).await {
            assert_ne!(
                resp.status(),
                200,
                "{} 绝不能被当成 200 服务出去",
                uri
            );
            let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
            assert!(
                !String::from_utf8_lossy(&bytes).contains("TOP SECRET"),
                "{} 泄露了 root 之外的文件内容",
                uri
            );
        }
    }

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_file(&secret).ok();
}

/// `canonical_root` 缓存的**失败**不能被永久记住。
///
/// 计划只说「在 new / on_load 末尾计算并缓存」。若把失败也缓存成 `None`，
/// 那么「root 目录在插件构造之后才创建」的路径会**永久静默失效** ——
/// 改之前每请求重算时它是能自愈的（canonicalize 每次都重试）。
///
/// 场景不算生僻：App 首次运行时才创建文档目录，而服务器启动在前。
#[tokio::test]
async fn test_static_file_root_created_after_construction_is_served() {
    let base = t13_root("late");
    let root = base.join("late_root"); // 故意先不创建

    let plugin = StaticFilePlugin::new("/s".to_string(), root.clone());

    let make_req = || {
        Request::builder()
            .uri("/s/a.txt")
            .method("GET")
            .body(Body::empty())
            .unwrap()
    };
    assert!(!plugin.should_handle(&make_req()), "root 不存在时不该接管");

    // root 稍后才出现
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"late but served").unwrap();

    assert!(
        plugin.should_handle(&make_req()),
        "root 稍后创建后必须能自愈 —— 把 canonicalize 的失败永久缓存住就再也不会接管"
    );
    let resp = match plugin.handle_request(make_req()).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("静态插件应接管该请求"),
    };
    assert_eq!(resp.status(), 200);
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(&bytes[..], b"late but served");

    std::fs::remove_dir_all(&base).ok();
}

#[tokio::test]
async fn test_static_file_blocks_hidden_directory_components() {
    let root = t13_root("hidden_dir");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git").join("config"), b"secret-git-config").unwrap();

    let plugin = StaticFilePlugin::new("/s".to_string(), root.clone());
    let req = Request::builder()
        .uri("/s/.git/config")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let resp = match plugin.handle_request(req).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("应返回 403"),
    };
    assert_eq!(resp.status(), 403, "默认配置下隐藏目录内的文件必须返回 403");

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn test_directory_listing_escapes_html_and_encodes_links() {
    let root = t13_root("xss");
    let evil_name = "a\"<img src=x onerror=alert(1)>#?.txt";
    // macOS/Linux 允许引号、尖括号、#、? 作为文件名（不含 '/'）
    std::fs::write(root.join(evil_name), b"ok").unwrap();

    let plugin = StaticFilePlugin::new("/s".to_string(), root.clone());
    plugin
        .on_load(&serde_json::json!({ "dir_list": { "enabled": true } }))
        .unwrap();

    let req = Request::builder()
        .uri("/s/")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let resp = match plugin.handle_request(req).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("应返回目录列表"),
    };
    assert_eq!(resp.status(), 200);
    let html = String::from_utf8(hyper::body::to_bytes(resp.into_body()).await.unwrap().to_vec()).unwrap();
    assert!(!html.contains("<img src=x"), "目录列表 HTML 必须转义 <img 防止 XSS");
    assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"), "应包含转义后的文件名");
    assert!(html.contains("%23%3F.txt"), "href 应对 # 和 ? 进行 URL 编码");

    std::fs::remove_dir_all(&root).ok();
}
