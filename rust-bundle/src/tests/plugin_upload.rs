use crate::plugins::encode_filename_for_header;
use crate::plugins::upload::UploadPlugin;
use crate::plugins::Plugin;
use hyper::{Request, Body, Method};
use tokio::runtime::Runtime;
use std::fs;

#[test]
fn test_upload_plugin_should_handle() {
    let temp_dir = std::env::temp_dir().join("test_upload_plugin");
    let plugin = UploadPlugin::new("/upload".to_string(), temp_dir.to_string_lossy().to_string());
    
    // 1. Match path and method
    let req = Request::builder()
        .uri("/upload")
        .method(Method::POST)
        .body(Body::empty())
        .unwrap();
    assert!(plugin.should_handle(&req));
    
    // 2. Mismatch path
    let req_path = Request::builder()
        .uri("/other")
        .method(Method::POST)
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&req_path));
}

#[test]
fn test_upload_plugin_handle_multipart() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir().join("test_upload_plugin_exec");
        if temp_dir.exists() {
            fs::remove_dir_all(&temp_dir).unwrap();
        }
        
        let plugin = UploadPlugin::new("/upload".to_string(), temp_dir.to_string_lossy().to_string());
        
        let boundary = "------------------------BoundaryUpload";
        let body_content = format!(
            "--{boundary}\r\n\
            Content-Disposition: form-data; name=\"file\"; filename=\"test_file.txt\"\r\n\
            Content-Type: text/plain\r\n\
            \r\n\
            File Content For Disk Upload\r\n\
            --{boundary}--\r\n",
            boundary = boundary
        );
        
        let req = Request::builder()
            .uri("/upload")
            .method(Method::POST)
            .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
            .body(Body::from(body_content))
            .unwrap();
            
        // Should return PassThrough with modified request (new headers)
        if let Ok(crate::plugins::PluginAction::PassThrough(new_req)) = plugin.handle_request(req).await {
            // Verify new headers
            let headers = new_req.headers();
            assert_eq!(headers.get("X-Uploaded-Original-Name").unwrap(), "test_file.txt");
            let size = headers.get("X-Uploaded-Size").unwrap().to_str().unwrap();
            assert_eq!(size, "28"); // "File Content For Disk Upload".len()
            
            // Verify file exists on disk
            let file_path_str = headers.get("X-Uploaded-File-Path").unwrap().to_str().unwrap();
            let file_path = std::path::Path::new(file_path_str);
            assert!(file_path.exists());
            
            let content = fs::read_to_string(file_path).unwrap();
            assert_eq!(content, "File Content For Disk Upload");
            
            // Cleanup
            fs::remove_dir_all(&temp_dir).unwrap();
        } else {
            panic!("Should PassThrough");
        }
    });
}

/// Non-ASCII filenames must be percent-encoded so downstream `HeaderValue::to_str()` succeeds.
#[test]
fn test_upload_non_ascii_filename() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir().join("test_upload_non_ascii");
        if temp_dir.exists() {
            fs::remove_dir_all(&temp_dir).unwrap();
        }

        let plugin = UploadPlugin::new("/upload".to_string(), temp_dir.to_string_lossy().to_string());

        let boundary = "------------------------BoundaryNonAscii";
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
            .uri("/upload")
            .method(Method::POST)
            .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
            .body(Body::from(body_content))
            .unwrap();

        match plugin.handle_request(req).await.unwrap() {
            crate::plugins::PluginAction::PassThrough(new_req) => {
                let headers = new_req.headers();

                let raw = headers
                    .get("X-Uploaded-Original-Name")
                    .expect("X-Uploaded-Original-Name should be present");
                let name = raw.to_str().expect(
                    "header value must be ASCII-readable for downstream to_str()",
                );
                assert_eq!(name, urlencoding::encode("报告.pdf").to_string());
                assert_eq!(name, "%E6%8A%A5%E5%91%8A.pdf");

                assert_eq!(
                    headers
                        .get("X-Uploaded-Filename-Encoding")
                        .expect("encoding header must be present for non-ASCII filenames")
                        .to_str()
                        .unwrap(),
                    "percent"
                );

                fs::remove_dir_all(&temp_dir).ok();
            }
            _ => panic!("expected PassThrough"),
        }
    });
}

/// Plain ASCII filenames pass through unchanged without an encoding header.
#[test]
fn test_upload_ascii_filename_unchanged() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir().join("test_upload_ascii_unchanged");
        if temp_dir.exists() {
            fs::remove_dir_all(&temp_dir).unwrap();
        }

        let plugin = UploadPlugin::new("/upload".to_string(), temp_dir.to_string_lossy().to_string());

        let boundary = "------------------------BoundaryAscii";
        // Include a space to verify ASCII filenames with spaces are not percent-encoded.
        let body_content = format!(
            "--{boundary}\r\n\
            Content-Disposition: form-data; name=\"file\"; filename=\"my report.txt\"\r\n\
            Content-Type: text/plain\r\n\
            \r\n\
            hello\r\n\
            --{boundary}--\r\n",
            boundary = boundary
        );

        let req = Request::builder()
            .uri("/upload")
            .method(Method::POST)
            .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
            .body(Body::from(body_content))
            .unwrap();

        match plugin.handle_request(req).await.unwrap() {
            crate::plugins::PluginAction::PassThrough(new_req) => {
                let headers = new_req.headers();
                assert_eq!(
                    headers.get("X-Uploaded-Original-Name").unwrap(),
                    "my report.txt",
                    "plain ASCII filename must pass through unchanged"
                );
                assert!(
                    headers.get("X-Uploaded-Filename-Encoding").is_none(),
                    "encoding header must not be added for plain ASCII"
                );
                fs::remove_dir_all(&temp_dir).ok();
            }
            _ => panic!("expected PassThrough"),
        }
    });
}

/// Non-ASCII file extensions must not leak into `X-Uploaded-File-Path` as obs-text.
#[test]
fn test_upload_non_ascii_extension_keeps_path_header_readable() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir().join("test_upload_non_ascii_ext");
        if temp_dir.exists() {
            fs::remove_dir_all(&temp_dir).unwrap();
        }

        let plugin = UploadPlugin::new("/upload".to_string(), temp_dir.to_string_lossy().to_string());

        let boundary = "------------------------BoundaryNonAsciiExt";
        let body_content = format!(
            "--{boundary}\r\n\
            Content-Disposition: form-data; name=\"file\"; filename=\"文件.报告\"\r\n\
            Content-Type: application/octet-stream\r\n\
            \r\n\
            hello\r\n\
            --{boundary}--\r\n",
            boundary = boundary
        );

        let req = Request::builder()
            .uri("/upload")
            .method(Method::POST)
            .header("Content-Type", format!("multipart/form-data; boundary={}", boundary))
            .body(Body::from(body_content))
            .unwrap();

        match plugin.handle_request(req).await.unwrap() {
            crate::plugins::PluginAction::PassThrough(new_req) => {
                let headers = new_req.headers();

                let path_raw = headers
                    .get("X-Uploaded-File-Path")
                    .expect("X-Uploaded-File-Path should be present");
                let path_str = path_raw.to_str().expect(
                    "path header must be ASCII-readable for downstream to_str()",
                );
                assert!(
                    std::path::Path::new(path_str).exists(),
                    "uploaded file must exist at path: {}",
                    path_str
                );

                assert_eq!(
                    headers
                        .get("X-Uploaded-Original-Name")
                        .unwrap()
                        .to_str()
                        .unwrap(),
                    urlencoding::encode("文件.报告").to_string()
                );

                fs::remove_dir_all(&temp_dir).ok();
            }
            _ => panic!("expected PassThrough"),
        }
    });
}

#[test]
fn test_encode_filename_for_header_contract() {
    // 1) Plain ASCII passes through unchanged.
    assert_eq!(
        encode_filename_for_header("report.pdf"),
        ("report.pdf".to_string(), false)
    );

    // 2) ASCII with spaces passes through unchanged.
    assert_eq!(
        encode_filename_for_header("my report.txt"),
        ("my report.txt".to_string(), false)
    );

    // 3) Non-ASCII is percent-encoded.
    let (enc, did_encode) = encode_filename_for_header("报告.pdf");
    assert!(did_encode);
    assert_eq!(enc, "%E6%8A%A5%E5%91%8A.pdf");

    // 4) Control characters are ASCII but invalid in HeaderValue and must be encoded.
    let (enc_ctrl, did_ctrl) = encode_filename_for_header("a\x01b.txt");
    assert!(did_ctrl, "control characters must be encoded");
    assert_eq!(enc_ctrl, "a%01b.txt");

    // 5) CR/LF must be encoded to prevent header injection.
    let (enc_crlf, did_crlf) = encode_filename_for_header("a\r\nX-Evil: 1");
    assert!(did_crlf);
    assert!(!enc_crlf.contains('\r') && !enc_crlf.contains('\n'));

    // 6) Every encoded output must be accepted by HeaderValue::from_str and readable via to_str().
    for name in ["report.pdf", "my report.txt", "报告.pdf", "a\x01b.txt", "a\r\nX-Evil: 1"] {
        let (encoded, _) = encode_filename_for_header(name);
        let hv = hyper::header::HeaderValue::from_str(&encoded)
            .unwrap_or_else(|_| panic!("encoded value must be a valid HeaderValue: {:?}", encoded));
        assert!(
            hv.to_str().is_ok(),
            "encoded value must be readable via to_str(): {:?}",
            encoded
        );
    }
}

#[test]
fn test_upload_plugin_rejects_prefix_collision() {
    let plugin = UploadPlugin::new("/upload".to_string(), std::env::temp_dir().to_string_lossy().to_string());

    let collided = Request::builder()
        .uri("/uploadfoo")
        .method(Method::POST)
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&collided), "/upload mount must not match /uploadfoo");

    for uri in ["/upload", "/upload/avatar"] {
        let req = Request::builder().uri(uri).method(Method::POST).body(Body::empty()).unwrap();
        assert!(plugin.should_handle(&req), "{} should still match", uri);

        let get = Request::builder().uri(uri).method(Method::GET).body(Body::empty()).unwrap();
        assert!(!plugin.should_handle(&get), "GET {} must not be handled", uri);
    }
}
