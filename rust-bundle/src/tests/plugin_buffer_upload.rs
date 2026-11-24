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

/// Non-ASCII filenames must be percent-encoded so downstream `HeaderValue::to_str()` succeeds.
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
                    .expect("X-Upload-Filename should be present");
                let name = raw.to_str().expect(
                    "header value must be ASCII-readable for downstream to_str()",
                );
                assert_eq!(name, urlencoding::encode("报告.pdf").to_string());
                assert_eq!(name, "%E6%8A%A5%E5%91%8A.pdf");

                assert_eq!(
                    headers
                        .get("X-Upload-Filename-Encoding")
                        .expect("encoding header must be present for non-ASCII filenames")
                        .to_str()
                        .unwrap(),
                    "percent"
                );
            }
            _ => panic!("expected PassThrough"),
        }
    });
}

/// Backwards compatibility: plain ASCII filenames pass through unchanged without encoding header.
/// Note: C++ checks for `x-upload-filename` presence to detect buffer uploads.
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
                    "plain ASCII filename must pass through unchanged"
                );
                assert!(
                    headers.get("X-Upload-Filename-Encoding").is_none(),
                    "encoding header must not be added for plain ASCII"
                );
            }
            _ => panic!("expected PassThrough"),
        }
    });
}

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
        "/buffer-upload mount must not match /buffer-uploadfoo"
    );

    for uri in ["/buffer-upload", "/buffer-upload/image"] {
        let req = Request::builder().uri(uri).method(Method::POST).body(Body::empty()).unwrap();
        assert!(plugin.should_handle(&req), "{} should still match", uri);
    }
}
