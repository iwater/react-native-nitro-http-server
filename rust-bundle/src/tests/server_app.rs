use crate::server::app_server::AppHttpServer;
use crate::types::HttpRequest;
use crate::global::CUSTOM_MIME_TYPES;
use std::path::PathBuf;

extern "C" fn test_callback(_request: *mut HttpRequest) {
    println!("Test callback called");
}

#[test]
fn test_app_server_config_parsing() {
    let config_json = r#"{
        "verbose": true,
        "root_dir": "/tmp",
        "mounts": [
            {
                "type": "static",
                "path": "/static",
                "root": "/var/www"
            }
        ],
        "mime_types": {
            ".custom": "text/custom"
        }
    }"#;

    // Initialize server with config
    let server = AppHttpServer::new(8081, "127.0.0.1".to_string(), test_callback, config_json);
    
    assert_eq!(server.port, 8081);
    assert_eq!(server.host, "127.0.0.1");
    // root_dir should be parsed
    assert_eq!(server.root_dir, Some(PathBuf::from("/tmp")));
    
    // verify CUSTOM_MIME_TYPES contains .custom
    {
        let custom_types = CUSTOM_MIME_TYPES.lock().unwrap();
        assert_eq!(custom_types.get("custom"), Some(&"text/custom".to_string()));
    }
}
