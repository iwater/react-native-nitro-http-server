use std::path::Path;
use crate::utils::get_content_type;

#[test]
fn test_get_content_type() {
    assert_eq!(get_content_type(Path::new("test.html")), "text/html; charset=utf-8");
    assert_eq!(get_content_type(Path::new("test.css")), "text/css; charset=utf-8");
    assert_eq!(get_content_type(Path::new("test.js")), "application/javascript; charset=utf-8");
    assert_eq!(get_content_type(Path::new("test.json")), "application/json; charset=utf-8");
    assert_eq!(get_content_type(Path::new("test.png")), "image/png");
    assert_eq!(get_content_type(Path::new("unknown.xyz")), "application/octet-stream");
    assert_eq!(get_content_type(Path::new("ignore_case.HTML")), "text/html; charset=utf-8");
    // New edge cases
    assert_eq!(get_content_type(Path::new("file.tar.gz")), "application/octet-stream"); // extension() only returns 'gz'
    assert_eq!(get_content_type(Path::new(".gitignore")), "application/octet-stream"); // no extension or known type
    assert_eq!(get_content_type(Path::new("APP.JS")), "application/javascript; charset=utf-8"); // Uppercase check
}

/// 归一化这一步必须有牙齿：给一个 capacity 远大于 len 的 Vec，压完必须 cap == len。
/// （`Vec::from_raw_parts(ptr, len, len)` 会把 capacity 声明成 len，从重建后的 Vec
///  上看不出问题，所以只能在 forget 之前测这一层。）
#[test]
fn test_normalize_capacity_shrinks_to_len() {
    let mut padded = Vec::with_capacity(1024);
    padded.extend_from_slice(b"hello");
    assert!(padded.capacity() > padded.len(), "前提：确实构造出了 capacity > len 的 Vec");

    let normalized = crate::utils::normalize_capacity(padded);
    assert_eq!(normalized.len(), 5);
    assert_eq!(normalized.capacity(), 5, "capacity 未压到 len → free_http_request 是 UB");
    assert_eq!(&normalized[..], b"hello");
}

/// `alloc_request_body` 与 `free_http_request` 之间的配套：len、内容、空 body 的 null。
/// capacity 那一层由上面的 test_normalize_capacity_shrinks_to_len 覆盖。
#[test]
fn test_alloc_request_body_capacity_equals_len() {
    for data in [
        &b"a"[..],
        &b"hello"[..],
        &vec![0x41u8; 1000][..],
        &vec![0u8; 65536][..], // 含 \0，二进制安全
    ] {
        let (ptr, len) = crate::utils::alloc_request_body(data);
        assert_eq!(len, data.len());
        assert!(!ptr.is_null());
        unsafe {
            // 完全照 free_http_request 的方式回收
            let v = Vec::from_raw_parts(ptr as *mut u8, len, len);
            assert_eq!(v.capacity(), v.len(), "capacity != len 会让 free_http_request 变 UB");
            assert_eq!(&v[..], data);
        } // v 在这里 drop，正常释放，不泄漏
    }
}

/// 空 body 必须是 null：`free_http_request` 只在 `body_len > 0` 时才回收。
#[test]
fn test_alloc_request_body_empty_is_null() {
    let (ptr, len) = crate::utils::alloc_request_body(b"");
    assert!(ptr.is_null());
    assert_eq!(len, 0);
}

#[test]
fn test_uuid_generation() {
    use uuid::Uuid;
    let uuid = Uuid::new_v4();
    assert!(!uuid.to_string().is_empty());
}
