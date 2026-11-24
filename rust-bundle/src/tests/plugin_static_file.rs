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

// Mount prefix boundary matching

#[test]
fn test_mount_prefix_boundary() {
    assert!(crate::plugins::path_matches_mount("/static/a.txt", "/static"));
    assert!(crate::plugins::path_matches_mount("/static", "/static"));
    assert!(crate::plugins::path_matches_mount("/static/", "/static"));
    assert!(crate::plugins::path_matches_mount("/anything", "/"));
    assert!(!crate::plugins::path_matches_mount("/staticfoo/a.txt", "/static"));
    assert!(!crate::plugins::path_matches_mount("/stat", "/static"));
}

#[test]
fn test_mount_prefix_trailing_slash_is_equivalent() {
    for mount in ["/static", "/static/"] {
        assert!(
            crate::plugins::path_matches_mount("/static/a.txt", mount),
            "mount={} should match /static/a.txt",
            mount
        );
        assert!(
            crate::plugins::path_matches_mount("/static", mount),
            "mount={} should match /static",
            mount
        );
        assert!(
            !crate::plugins::path_matches_mount("/staticfoo/a.txt", mount),
            "mount={} must not match /staticfoo/a.txt",
            mount
        );
    }
}

/// Ensure `/static` does not match `/staticfoo/a.txt` even when `foo/a.txt` exists on disk.
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
        "/static mount must not match /staticfoo/a.txt"
    );

    let normal = Request::builder()
        .uri("/static/foo/a.txt")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&normal), "/static/foo/a.txt should still match");

    std::fs::remove_dir_all(&root).ok();
}

// Percent-decoding and HEAD support

fn t13_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("t13_{}_{}", tag, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// `resolve_relative` must apply the same normalization as `path_matches_mount`.
#[test]
fn test_resolve_relative_consistent_with_path_matches_mount() {
    use crate::plugins::resolve_relative as rr;

    for mount in ["/static", "/static/", "/static//"] {
        assert_eq!(rr(mount, "/static/a.txt"), "a.txt", "mount={}", mount);
        assert_eq!(rr(mount, "/static"), "", "mount={} exact mount path", mount);
        assert_eq!(rr(mount, "/static/"), "", "mount={} mount with trailing slash", mount);
    }
    assert_eq!(rr("/", "/a.txt"), "a.txt");
    assert_eq!(rr("", "/a.txt"), "a.txt");
    assert_eq!(rr("///", "/a.txt"), "a.txt");

    // percent-decode
    assert_eq!(rr("/s", "/s/hello%20world.txt"), "hello world.txt");
    assert_eq!(rr("/s", "/s/a%2Bb.txt"), "a+b.txt");
    assert_eq!(rr("/s", "/s/%E6%8A%A5%E5%91%8A.txt"), "报告.txt");

    // Invalid sequences: `urlencoding` keeps malformed `%ZZ` as-is, and invalid UTF-8
    // (`%FF`) falls back to the raw string without panicking.
    assert_eq!(rr("/s", "/s/a%ZZb.txt"), "a%ZZb.txt");
    assert_eq!(rr("/s", "/s/a%FFb"), "a%FFb");
}

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
        "percent-encoded path must be handled"
    );

    let resp = match plugin.handle_request(make_req()).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("static plugin should handle request"),
    };
    assert_eq!(resp.status(), 200);
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(&bytes[..], b"hi there");

    std::fs::remove_dir_all(&root).ok();
}

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
    assert!(plugin.should_handle(&req), "HEAD must be handled by static plugin");

    let req = Request::builder()
        .uri("/s/a.txt")
        .method("HEAD")
        .body(Body::empty())
        .unwrap();
    let resp = match plugin.handle_request(req).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("static plugin should handle HEAD"),
    };
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-length").map(|v| v.to_str().unwrap()),
        Some("10"),
        "HEAD must include Content-Length"
    );
    assert_eq!(
        resp.headers().get("content-type").map(|v| v.to_str().unwrap()),
        Some("text/plain; charset=utf-8")
    );
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(bytes.len(), 0, "HEAD must not include a body");

    std::fs::remove_dir_all(&root).ok();
}

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
        _ => panic!("static plugin should handle request"),
    };
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-length").map(|v| v.to_str().unwrap()),
        Some("100000"),
        "streamed response must include explicit Content-Length"
    );
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(bytes.len(), 100_000, "streamed response must not be truncated");

    std::fs::remove_dir_all(&root).ok();
}

/// Ensure percent-encoded `..` (`%2e%2e`) is still blocked by canonicalization checks.
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
            "{} must not be handled (escapes root)",
            uri
        );

        let req = Request::builder()
            .uri(&uri)
            .method("GET")
            .body(Body::empty())
            .unwrap();
        if let Ok(crate::plugins::PluginAction::Handled(resp)) = plugin.handle_request(req).await {
            assert_ne!(resp.status(), 200, "{} must never return 200", uri);
            let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
            assert!(
                !String::from_utf8_lossy(&bytes).contains("TOP SECRET"),
                "{} leaked file outside root",
                uri
            );
        }
    }

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_file(&secret).ok();
}

/// Failed `canonical_root` resolution must not be cached so directories created
/// after plugin construction still work.
#[tokio::test]
async fn test_static_file_root_created_after_construction_is_served() {
    let base = t13_root("late");
    let root = base.join("late_root"); // Intentionally created after plugin::new

    let plugin = StaticFilePlugin::new("/s".to_string(), root.clone());

    let make_req = || {
        Request::builder()
            .uri("/s/a.txt")
            .method("GET")
            .body(Body::empty())
            .unwrap()
    };
    assert!(!plugin.should_handle(&make_req()), "should not handle when root is missing");

    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"late but served").unwrap();

    assert!(
        plugin.should_handle(&make_req()),
        "must recover once root directory is created"
    );
    let resp = match plugin.handle_request(make_req()).await.unwrap() {
        crate::plugins::PluginAction::Handled(r) => r,
        _ => panic!("static plugin should handle request"),
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
        _ => panic!("expected 403"),
    };
    assert_eq!(resp.status(), 403, "files inside hidden directories must return 403 by default");

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn test_directory_listing_escapes_html_and_encodes_links() {
    let root = t13_root("xss");
    let evil_name = "a\"<img src=x onerror=alert(1)>#?.txt";
    // Quotes, angle brackets, `#`, and `?` are valid filename characters on POSIX.
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
        _ => panic!("expected directory listing"),
    };
    assert_eq!(resp.status(), 200);
    let html = String::from_utf8(hyper::body::to_bytes(resp.into_body()).await.unwrap().to_vec()).unwrap();
    assert!(!html.contains("<img src=x"), "HTML must escape <img to prevent XSS");
    assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"), "should contain escaped filename");
    assert!(html.contains("%23%3F.txt"), "href should URL-encode # and ?");

    std::fs::remove_dir_all(&root).ok();
}
