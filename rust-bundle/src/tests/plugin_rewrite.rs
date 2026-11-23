use crate::plugins::rewrite::RewritePlugin;
use crate::plugins::Plugin;
use hyper::{Request, Body};
use serde_json::json;
use tokio::runtime::Runtime;

#[test]
fn test_rewrite_plugin_logic() {
    // Prepare Runtime for async test
    let rt = Runtime::new().unwrap();
    
    rt.block_on(async {
        let plugin = RewritePlugin::new();
        
        // Config with rules
        let config = json!({
            "rules": [
                { "pattern": "^/api/v1/(.*)", "replacement": "/api/v2/$1" },
                { "pattern": "^/old/(.*)", "replacement": "/new/$1" }
            ]
        });
        
        // Load config
        assert!(plugin.on_load(&config).is_ok());
        
        // Test Case 1: Match first rule
        let req = Request::builder()
            .uri("/api/v1/users?id=1")
            .body(Body::empty())
            .unwrap();
            
        assert!(plugin.should_handle(&req));
        
        if let Ok(crate::plugins::PluginAction::PassThrough(new_req)) = plugin.handle_request(req).await {
            assert_eq!(new_req.uri().path(), "/api/v2/users");
            assert_eq!(new_req.uri().query(), Some("id=1"));
        } else {
            panic!("Rewrite plugin should return PassThrough");
        }
        
        // Test Case 2: No match
        let req = Request::builder()
            .uri("/other/path")
            .body(Body::empty())
            .unwrap();
            
        assert!(!plugin.should_handle(&req));
    });
}

#[test]
fn test_rewrite_plugin_precedence() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let plugin = RewritePlugin::new();
        
        // Config with overlapping rules
        // Rule 1: specific match
        // Rule 2: general match (catch-all)
        // If precedence works (sequential), Rule 1 should win for specific path.
        let config = json!({
            "rules": [
                { "pattern": "^/api/specific", "replacement": "/target/specific" },
                { "pattern": "^/api/(.*)", "replacement": "/target/catchall/$1" }
            ]
        });
        
        plugin.on_load(&config).unwrap();
        
        // 1. Test specific path - should match Rule 1
        let req_1 = Request::builder()
            .uri("/api/specific")
            .body(Body::empty())
            .unwrap();
            
        if let Ok(crate::plugins::PluginAction::PassThrough(new_req)) = plugin.handle_request(req_1).await {
            assert_eq!(new_req.uri().path(), "/target/specific");
        } else {
            panic!("Should rewrite specific path");
        }
        
        // 2. Test general path - should match Rule 2
        let req_2 = Request::builder()
            .uri("/api/other")
            .body(Body::empty())
            .unwrap();
            
        if let Ok(crate::plugins::PluginAction::PassThrough(new_req)) = plugin.handle_request(req_2).await {
            assert_eq!(new_req.uri().path(), "/target/catchall/other");
        } else {
            panic!("Should rewrite other path");
        }
    });
}

#[test]
fn test_rewrite_plugin_appends_query_with_ampersand_when_replacement_has_query() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let plugin = RewritePlugin::new();
        let config = json!({
            "rules": [
                { "pattern": "^/app/(user|admin)/(.*)", "replacement": "/internal/$1/handler?path=$2" }
            ]
        });
        plugin.on_load(&config).unwrap();

        let req = Request::builder()
            .uri("/app/user/profile?tab=1")
            .body(Body::empty())
            .unwrap();

        if let Ok(crate::plugins::PluginAction::PassThrough(new_req)) = plugin.handle_request(req).await {
            assert_eq!(new_req.uri().to_string(), "/internal/user/handler?path=profile&tab=1");
        } else {
            panic!("Rewrite plugin should return PassThrough");
        }
    });
}
