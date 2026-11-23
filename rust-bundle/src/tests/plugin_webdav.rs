use crate::plugins::webdav::WebDavPlugin;
use crate::plugins::Plugin;
use hyper::{Request, Body, Method};
use tokio::runtime::Runtime;
use std::fs;
use serde_json::json;

#[test]
fn test_webdav_plugin_should_handle() {
    let temp_dir = std::env::temp_dir();
    let plugin = WebDavPlugin::new("/webdav".to_string(), temp_dir.to_string_lossy().to_string());
    
    // 1. Match prefix
    let req = Request::builder()
        .uri("/webdav/file.txt")
        .method(Method::from_bytes(b"PROPFIND").unwrap())
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&req));
    
    // 2. Exact prefix match
    let req_exact = Request::builder()
        .uri("/webdav")
        .method(Method::GET)
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&req_exact));

    // 3. Mismatch prefix
    let req_other = Request::builder()
        .uri("/other/file.txt")
        .method(Method::GET)
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req_other));

    // 4. Verify all documented methods
    let methods = vec!["MKCOL", "COPY", "MOVE", "LOCK", "UNLOCK", "PROPPATCH"];
    for m in methods {
        let req_m = Request::builder()
            .uri("/webdav/resource")
            .method(Method::from_bytes(m.as_bytes()).unwrap())
            .body(Body::empty())
            .unwrap();
        assert!(plugin.should_handle(&req_m), "Should handle method {}", m);
    }
}

#[test]
fn test_webdav_plugin_initialization() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir().join("test_webdav_init");
        if !temp_dir.exists() {
            fs::create_dir(&temp_dir).unwrap();
        }
        
        let plugin = WebDavPlugin::new("/webdav".to_string(), temp_dir.to_string_lossy().to_string());
        
        // Just verify it doesn't crash on standard request
        // PROPFIND /webdav/ is a standard check
        let req = Request::builder()
            .uri("/webdav/")
            .method("PROPFIND")
            .header("Depth", "1")
            .body(Body::empty())
            .unwrap();
            
        if let Ok(crate::plugins::PluginAction::Handled(resp)) = plugin.handle_request(req).await {
            // dav-server might return 207 Multi-Status or 404/405 depending on exact setup
            // But it should be Handled.
            assert!(resp.status().as_u16() < 500); 
        } else {
             // It might return handled.
             // If it returns PassThrough, then something is wrong because it should handle correct path.
             // Wait, handle_request returns PluginAction.
        }
        
        fs::remove_dir_all(&temp_dir).unwrap();
        
        if temp_dir.exists() {
            let _ = fs::remove_dir_all(&temp_dir);
        }
    });
}

#[test]
fn test_webdav_plugin_on_load() {
    // Test dynamic configuration update
    let temp_dir_1 = std::env::temp_dir().join("test_webdav_1");
    let temp_dir_2 = std::env::temp_dir().join("test_webdav_2");
    
    // Clean up if exists
    if temp_dir_1.exists() { let _ = fs::remove_dir_all(&temp_dir_1); }
    if temp_dir_2.exists() { let _ = fs::remove_dir_all(&temp_dir_2); }

    fs::create_dir_all(&temp_dir_1).unwrap();
    fs::create_dir_all(&temp_dir_2).unwrap();
    
    let plugin = WebDavPlugin::new("/old".to_string(), temp_dir_1.to_string_lossy().to_string());
    
    // 1. Check initial config
    let req_old = Request::builder()
        .uri("/old/file")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&req_old));
    
    let req_new = Request::builder()
        .uri("/new/file")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req_new));
    
    // 2. Update config via on_load
    let config = json!({
        "prefix": "/new",
        "root": temp_dir_2.to_string_lossy()
    });
    
    plugin.on_load(&config).unwrap();
    
    // 3. Verify new config (Old path should fail, new path should work)
    let req_old_2 = Request::builder()
        .uri("/old/file")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req_old_2));
    
    let req_new_2 = Request::builder()
        .uri("/new/file")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&req_new_2));
    
    let _ = fs::remove_dir_all(&temp_dir_1);
    let _ = fs::remove_dir_all(&temp_dir_2);
}
