use crate::server::app_server::AppHttpServer;
use crate::tests::test_utils::{get_free_port, test_callback_404, wait_for_server};
use std::time::Duration;
use std::fs;
use reqwest::StatusCode;

#[tokio::test]
async fn test_app_server_basic_rewrite() {
    let temp_dir_base = std::env::temp_dir().join("test_app_server_basic");
    if temp_dir_base.exists() {
        fs::remove_dir_all(&temp_dir_base).unwrap();
    }
    fs::create_dir_all(&temp_dir_base).unwrap();
    let temp_dir = temp_dir_base.canonicalize().unwrap();
    
    let idx_path = temp_dir.join("index.html");
    fs::write(&idx_path, "<html>App Server Test</html>").unwrap();
    
    let config_json = format!(r#"{{
        "verbose": true,
        "mounts": [
            {{
                "type": "rewrite",
                "rules": [
                    {{ "pattern": "^/api/(.*)", "replacement": "/static/$1" }}
                ]
            }},
            {{
                "type": "static",
                "path": "/static",
                "root": "{}"
            }}
        ]
    }}"#, temp_dir.to_string_lossy().replace("\\", "\\\\")); 

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

    // 1. Static File
    let resp = client.get(format!("{}/static/index.html", base_url)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "<html>App Server Test</html>");
    
    // 2. Rewrite
    let resp_rewrite = client.get(format!("{}/api/index.html", base_url)).send().await.unwrap();
    assert_eq!(resp_rewrite.status(), StatusCode::OK);
    assert_eq!(resp_rewrite.text().await.unwrap(), "<html>App Server Test</html>");

    server_task.abort();
    fs::remove_dir_all(&temp_dir_base).unwrap();
}

/// Task 13 端到端：真实 socket 上验证 percent-decode 与 HEAD 的**线上分帧**。
///
/// 为什么插件级用例不够：插件返回 `Response<Body>` 只说明「头写对了」，
/// 而线上到底是 `Content-Length` 还是 `Transfer-Encoding: chunked`、
/// HEAD 到底有没有 body —— 这些由 **hyper 的 encoder** 决定
/// （`proto/h1/role.rs`：有显式 content-length 时用 `Encoder::length`；
/// HEAD 走 `can_have_body == false` 强制 `Encoder::length(0)`）。
/// 只有真 socket 能证明这两条。
#[tokio::test]
async fn test_app_server_static_percent_decode_and_head_on_wire() {
    let base = std::env::temp_dir().join(format!("t13_e2e_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&base).unwrap();
    let root = base.canonicalize().unwrap();
    fs::write(root.join("hello world.txt"), "0123456789").unwrap();

    let config_json = format!(r#"{{
        "mounts": [ {{ "type": "static", "path": "/s", "root": "{}" }} ]
    }}"#, root.to_string_lossy().replace("\\", "\\\\"));

    let port = get_free_port();
    let server_task = tokio::spawn(async move {
        let mut server = AppHttpServer::new(port, "127.0.0.1".to_string(), test_callback_404, &config_json);
        server.start();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    wait_for_server(port).await;
    let base_url = format!("http://127.0.0.1:{}", port);
    let client = reqwest::Client::new();

    // 1. GET：percent 编码路径命中带空格的文件名，且**不是 chunked**
    let resp = client
        .get(format!("{}/s/hello%20world.txt", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "percent 编码路径应命中 hello world.txt");
    assert_eq!(
        resp.headers().get("content-length").map(|v| v.to_str().unwrap()),
        Some("10"),
        "流式响应必须显式带 Content-Length"
    );
    assert!(
        resp.headers().get("transfer-encoding").is_none(),
        "不该退化成 chunked（线上格式与改动前保持一致）"
    );
    assert_eq!(resp.text().await.unwrap(), "0123456789");

    // 2. HEAD：同样的头，但**没有 body**
    let resp = client
        .head(format!("{}/s/hello%20world.txt", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "HEAD 必须由静态插件接管（否则会落到 callback 的 404）");
    assert_eq!(
        resp.headers().get("content-length").map(|v| v.to_str().unwrap()),
        Some("10")
    );
    assert_eq!(
        resp.headers().get("content-type").map(|v| v.to_str().unwrap()),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(resp.bytes().await.unwrap().len(), 0, "HEAD 不能带 body");

    // 3. 防回归：普通 GET 仍然正常
    fs::write(root.join("plain.txt"), "plain").unwrap();
    let resp = client.get(format!("{}/s/plain.txt", base_url)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "plain");

    server_task.abort();
    fs::remove_dir_all(&base).ok();
}
