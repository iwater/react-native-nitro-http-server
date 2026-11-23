use crate::server::app_server::AppHttpServer;
use crate::tests::test_utils::{get_free_port, test_callback_404, wait_for_server};
use std::time::Duration;
use std::fs;
use reqwest::StatusCode;
use async_zip::tokio::write::ZipFileWriter;
use async_zip::Compression;
use tokio::fs::File;

// Helper to create valid zip file
async fn create_test_zip(path: &std::path::Path) {
    let file = File::create(path).await.unwrap();
    let mut writer = ZipFileWriter::with_tokio(file);
    
    let builder = async_zip::ZipEntryBuilder::new("hello.txt".to_string().into(), Compression::Deflate);
    writer.write_entry_whole(builder, b"Hello Zip").await.unwrap();
    
    let builder_dir = async_zip::ZipEntryBuilder::new("dir/world.txt".to_string().into(), Compression::Deflate);
    writer.write_entry_whole(builder_dir, b"World Zip").await.unwrap();
    
    writer.close().await.unwrap();
}

#[tokio::test]
async fn test_zip_plugin_integration() {
    let temp_dir_base = std::env::temp_dir().join("test_zip_plugin");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    let temp_dir = temp_dir_base.canonicalize().unwrap();
    
    let zip_path = temp_dir.join("test.zip");
    create_test_zip(&zip_path).await;
    
    let config_json = format!(r#"{{
        "verbose": true,
        "mounts": [
            {{
                "type": "zip",
                "path": "/zip",
                "zip_file": "{}"
            }}
        ]
    }}"#, zip_path.to_string_lossy().replace("\\", "\\\\")); 

    let free_port = get_free_port();
    
    let server_task = tokio::spawn(async move {
        let mut server = AppHttpServer::new(free_port, "127.0.0.1".to_string(), test_callback_404, &config_json);
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    wait_for_server(free_port).await;
    let base_url = format!("http://127.0.0.1:{}", free_port);
    let client = reqwest::Client::new();

    // 1. Root file
    let resp = client.get(format!("{}/zip/hello.txt", base_url)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "Hello Zip");
    
    // 2. Subdir file
    let resp_sub = client.get(format!("{}/zip/dir/world.txt", base_url)).send().await.unwrap();
    assert_eq!(resp_sub.status(), StatusCode::OK);
    assert_eq!(resp_sub.text().await.unwrap(), "World Zip");
    
    // 3. 404
    let resp_404 = client.get(format!("{}/zip/unknown.txt", base_url)).send().await.unwrap();
    assert_eq!(resp_404.status(), StatusCode::NOT_FOUND);

    server_task.abort();
    fs::remove_dir_all(&temp_dir_base).unwrap();
}
