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

/// Task 6：非 ASCII 文件名必须先 percent-encode 才能放进 header。
///
/// 机制（本机实测，非推断）：
/// - `hyper 0.14` 的 `HeaderValue::from_str("报告.pdf")` 返回 **Ok** —— obs-text
///   （0x80-0xFF）是合法 header 值，**不会 500**（计划里最初那个说法已被证伪）。
/// - 但下游 `http_server.rs` / `app_server.rs` 收集 header 时用的是
///   `value.to_str().unwrap_or("")`，而 `to_str()` 对 obs-text 返回 **Err**
///   → **文件名被静默置空字符串传给 JS**。
///
/// 所以这条用例的核心判据不是「header 能不能设进去」，而是
/// 「设进去的值能不能通过 `to_str()`」—— 那正是下游的读取方式。
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

                // 核心判据：值必须能 to_str()，否则下游拿到的是空串
                let raw = headers
                    .get("X-Uploaded-Original-Name")
                    .expect("应存在 X-Uploaded-Original-Name");
                let name = raw.to_str().expect(
                    "header 值必须是 ASCII 可读 —— obs-text 会被下游 to_str().unwrap_or(\"\") 静默置空",
                );
                assert_eq!(name, urlencoding::encode("报告.pdf").to_string());
                assert_eq!(name, "%E6%8A%A5%E5%91%8A.pdf");

                // 标记头：告诉 JS 侧需要 decode
                assert_eq!(
                    headers
                        .get("X-Uploaded-Filename-Encoding")
                        .expect("非 ASCII 时应带 encoding 标记头")
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

/// 向后兼容：纯 ASCII 文件名必须**原样**传递，且**不带** encoding 标记头。
///
/// 这条用例防的是「修 bug 时把所有文件名都编码一遍」——
/// 那会让既有 JS 侧（按原样读 `X-Uploaded-Original-Name`）拿到 `a%20b.txt`。
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
        // 故意带一个空格，确认「ASCII 但需要转义」的名字也不被编码
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
                    "纯 ASCII 文件名必须原样传递（向后兼容）"
                );
                assert!(
                    headers.get("X-Uploaded-Filename-Encoding").is_none(),
                    "纯 ASCII 时不应出现 encoding 标记头"
                );
                fs::remove_dir_all(&temp_dir).ok();
            }
            _ => panic!("expected PassThrough"),
        }
    });
}

/// 同一 bug 的第二个实例：**扩展名本身非 ASCII** 时，`X-Uploaded-File-Path` 也会被置空。
///
/// 实测：`Path::new("文件.报告").extension()` → `Some("报告")`，
/// 而 `save_filename = format!("{}.{}", uuid, ext)` → 落盘名与 `X-Uploaded-File-Path`
/// 都含 obs-text → 下游 `to_str()` 失败 → 路径变空串。
/// 后果不止「JS 看不到路径」：`integration_c_api.rs:360` 的 JS 侧拿不到该头会直接返回 **400**。
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
                    .expect("应存在 X-Uploaded-File-Path");
                let path_str = path_raw.to_str().expect(
                    "路径头必须是 ASCII 可读 —— 否则下游 to_str() 失败、JS 侧拿到空串（并返回 400）",
                );
                assert!(
                    std::path::Path::new(path_str).exists(),
                    "路径头指向的文件应真实存在: {}",
                    path_str
                );

                // 原始文件名仍应可读（percent 编码）
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

/// `encode_filename_for_header` 的契约测试 —— 钉住「什么该编码」。
///
/// 重点是第 4 条：`\x01` 是**纯 ASCII**，但 `HeaderValue::from_str` 会拒绝它。
/// 所以判定不能写成「`name.is_ascii()` 就原样放行」（计划初版正是这么写的）——
/// 那样控制字符会走进 `.header()` 并静默失败。这里改成直接用
/// 「`from_str` 能接受 **且** `to_str` 能读回」做判据，即**下游的读取方式本身**。
#[test]
fn test_encode_filename_for_header_contract() {
    // 1) 普通 ASCII → 原样，且不要求标记头
    assert_eq!(
        encode_filename_for_header("report.pdf"),
        ("report.pdf".to_string(), false)
    );

    // 2) 含空格的 ASCII → 原样（header 值允许空格，编码了反而破坏向后兼容）
    assert_eq!(
        encode_filename_for_header("my report.txt"),
        ("my report.txt".to_string(), false)
    );

    // 3) 非 ASCII → percent 编码 + 需要标记头
    let (enc, did_encode) = encode_filename_for_header("报告.pdf");
    assert!(did_encode);
    assert_eq!(enc, "%E6%8A%A5%E5%91%8A.pdf");

    // 4) 控制字符：纯 ASCII，但 header 非法 → 必须编码
    let (enc_ctrl, did_ctrl) = encode_filename_for_header("a\x01b.txt");
    assert!(did_ctrl, "控制字符是 ASCII，但 HeaderValue 不接受，必须编码");
    assert_eq!(enc_ctrl, "a%01b.txt");

    // 5) CR/LF（header 注入）→ 必须编码，且结果里不能残留换行
    let (enc_crlf, did_crlf) = encode_filename_for_header("a\r\nX-Evil: 1");
    assert!(did_crlf);
    assert!(!enc_crlf.contains('\r') && !enc_crlf.contains('\n'));

    // 6) 汇总判据：任何输入的输出都必须「能被下游读回」—— 这正是函数存在的理由
    for name in ["report.pdf", "my report.txt", "报告.pdf", "a\x01b.txt", "a\r\nX-Evil: 1"] {
        let (encoded, _) = encode_filename_for_header(name);
        let hv = hyper::header::HeaderValue::from_str(&encoded)
            .unwrap_or_else(|_| panic!("编码结果应是合法 header 值: {:?}", encoded));
        assert!(
            hv.to_str().is_ok(),
            "编码结果必须能被 to_str() 读回（否则下游 unwrap_or(\"\") 会置空）: {:?}",
            encoded
        );
    }
}


/// Task 8：挂载 `/upload` 不该接管 `/uploadfoo`（方法检查保留）。
#[test]
fn test_upload_plugin_rejects_prefix_collision() {
    let plugin = UploadPlugin::new("/upload".to_string(), std::env::temp_dir().to_string_lossy().to_string());

    let collided = Request::builder()
        .uri("/uploadfoo")
        .method(Method::POST)
        .body(Body::empty())
        .unwrap();
    assert!(!plugin.should_handle(&collided), "挂载 /upload 不该接管 /uploadfoo");

    // 防回归：/upload 本身与子路径仍要接管，且仍要求 POST/PUT
    for uri in ["/upload", "/upload/avatar"] {
        let req = Request::builder().uri(uri).method(Method::POST).body(Body::empty()).unwrap();
        assert!(plugin.should_handle(&req), "{} 仍应被接管", uri);

        let get = Request::builder().uri(uri).method(Method::GET).body(Body::empty()).unwrap();
        assert!(!plugin.should_handle(&get), "{} 用 GET 不该被接管", uri);
    }
}
