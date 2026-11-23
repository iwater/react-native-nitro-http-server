use crate::plugins::buffer_upload::BufferUploadPlugin;
use crate::plugins::Plugin;
use hyper::{Request, Body, Method};
use tokio::runtime::Runtime;

#[test]
fn test_buffer_upload_plugin_should_handle() {
    let plugin = BufferUploadPlugin::new("/buffer-upload".to_string());
    
    // 1. Match path and method
    let req = Request::builder()
        .uri("/buffer-upload")
        .method(Method::POST)
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&req));
    
    let req_put = Request::builder()
        .uri("/buffer-upload")
        .method(Method::PUT)
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&req_put));
    
    // 2. Mismatch path
    let req_path = Request::builder()
        .uri("/other")
        .method(Method::POST)
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req_path));
    
    // 3. Mismatch method
    let req_method = Request::builder()
        .uri("/buffer-upload")
        .method(Method::GET)
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req_method));
}

#[test]
fn test_buffer_upload_plugin_handle_multipart() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let plugin = BufferUploadPlugin::new("/buffer-upload".to_string());
        
        let boundary = "------------------------Boundary123";
        let body_content = format!(
            "--{boundary}\r\n\
            Content-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n\
            Content-Type: text/plain\r\n\
            \r\n\
            Hello World Content\r\n\
            --{boundary}--\r\n",
            boundary = boundary
        );
        
        let req = Request::builder()
            .uri("/buffer-upload")
            .method(Method::POST)
            .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
            .body(Body::from(body_content))
            .unwrap();
            
        // Should return PassThrough with modified request (new headers)
        if let Ok(crate::plugins::PluginAction::PassThrough(new_req)) = plugin.handle_request(req).await {
            // Verify new headers
            let headers = new_req.headers();
            assert_eq!(headers.get("X-Upload-Filename").unwrap(), "test.txt");
            assert_eq!(headers.get("X-Upload-Content-Type").unwrap(), "text/plain");
            
            let size = headers.get("X-Upload-Size").unwrap().to_str().unwrap();
            assert_eq!(size, "19"); // "Hello World Content".len()
            
            // Verify body content (need to read it)
            // Note: reading body consumes it.
            let bytes = hyper::body::to_bytes(new_req.into_body()).await.unwrap();
            assert_eq!(bytes, "Hello World Content");
        } else {
            panic!("Should PassThrough");
        }
    });
}

/// Task 6：非 ASCII 文件名必须 percent-encode（机制同 `plugin_upload.rs` 里的详注：
/// `HeaderValue::from_str` 接受 obs-text 不会 500，但下游 `to_str().unwrap_or("")`
/// 会把它静默置空）。核心判据 = 值能通过 `to_str()`。
#[test]
fn test_buffer_upload_non_ascii_filename() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let plugin = BufferUploadPlugin::new("/buffer-upload".to_string());

        let boundary = "------------------------BoundaryNonAsciiBuf";
        let body_content = format!(
            "--{boundary}\r\n\
            Content-Disposition: form-data; name=\"file\"; filename=\"报告.pdf\"\r\n\
            Content-Type: application/pdf\r\n\
            \r\n\
            hello\r\n\
            --{boundary}--\r\n",
            boundary = boundary
        );

        let req = Request::builder()
            .uri("/buffer-upload")
            .method(Method::POST)
            .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
            .body(Body::from(body_content))
            .unwrap();

        match plugin.handle_request(req).await.unwrap() {
            crate::plugins::PluginAction::PassThrough(new_req) => {
                let headers = new_req.headers();

                let raw = headers
                    .get("X-Upload-Filename")
                    .expect("应存在 X-Upload-Filename");
                let name = raw.to_str().expect(
                    "header 值必须是 ASCII 可读 —— obs-text 会被下游 to_str().unwrap_or(\"\") 静默置空",
                );
                assert_eq!(name, urlencoding::encode("报告.pdf").to_string());
                assert_eq!(name, "%E6%8A%A5%E5%91%8A.pdf");

                assert_eq!(
                    headers
                        .get("X-Upload-Filename-Encoding")
                        .expect("非 ASCII 时应带 encoding 标记头")
                        .to_str()
                        .unwrap(),
                    "percent"
                );
            }
            _ => panic!("expected PassThrough"),
        }
    });
}

/// 向后兼容：纯 ASCII 文件名原样传递、不带 encoding 标记头。
///
/// 这条尤其要紧：`HybridHttpServer.cpp:315` 是靠 `x-upload-filename` 的**存在**
/// 来判定「这是 buffer 上传、body 要当 ArrayBuffer 处理」的，
/// 所以该头既不能改名、也不能在 ASCII 情况下消失。
#[test]
fn test_buffer_upload_ascii_filename_unchanged() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let plugin = BufferUploadPlugin::new("/buffer-upload".to_string());

        let boundary = "------------------------BoundaryAsciiBuf";
        let body_content = format!(
            "--{boundary}\r\n\
            Content-Disposition: form-data; name=\"file\"; filename=\"my report.bin\"\r\n\
            Content-Type: application/octet-stream\r\n\
            \r\n\
            hello\r\n\
            --{boundary}--\r\n",
            boundary = boundary
        );

        let req = Request::builder()
            .uri("/buffer-upload")
            .method(Method::POST)
            .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
            .body(Body::from(body_content))
            .unwrap();

        match plugin.handle_request(req).await.unwrap() {
            crate::plugins::PluginAction::PassThrough(new_req) => {
                let headers = new_req.headers();
                assert_eq!(
                    headers.get("X-Upload-Filename").unwrap(),
                    "my report.bin",
                    "纯 ASCII 文件名必须原样传递（向后兼容）"
                );
                assert!(
                    headers.get("X-Upload-Filename-Encoding").is_none(),
                    "纯 ASCII 时不应出现 encoding 标记头"
                );
            }
            _ => panic!("expected PassThrough"),
        }
    });
}

/// Task 8：挂载 `/buffer-upload` 不该接管 `/buffer-uploadfoo`。
#[test]
fn test_buffer_upload_plugin_rejects_prefix_collision() {
    let plugin = BufferUploadPlugin::new("/buffer-upload".to_string());

    let collided = Request::builder()
        .uri("/buffer-uploadfoo")
        .method(Method::POST)
        .body(Body::empty())
        .unwrap();
    assert!(
        !plugin.should_handle(&collided),
        "挂载 /buffer-upload 不该接管 /buffer-uploadfoo"
    );

    // 防回归
    for uri in ["/buffer-upload", "/buffer-upload/image"] {
        let req = Request::builder().uri(uri).method(Method::POST).body(Body::empty()).unwrap();
        assert!(plugin.should_handle(&req), "{} 仍应被接管", uri);
    }
}
