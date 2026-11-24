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

/// Ensure `normalize_capacity` shrinks capacity down to `len` before `mem::forget`.
#[test]
fn test_normalize_capacity_shrinks_to_len() {
    let mut padded = Vec::with_capacity(1024);
    padded.extend_from_slice(b"hello");
    assert!(padded.capacity() > padded.len(), "expected capacity > len before normalize");

    let normalized = crate::utils::normalize_capacity(padded);
    assert_eq!(normalized.len(), 5);
    assert_eq!(normalized.capacity(), 5, "capacity must match len for free_http_request");
    assert_eq!(&normalized[..], b"hello");
}

#[test]
fn test_alloc_request_body_capacity_equals_len() {
    for data in [
        &b"a"[..],
        &b"hello"[..],
        &vec![0x41u8; 1000][..],
        &vec![0u8; 65536][..], // Binary-safe with NUL bytes
    ] {
        let (ptr, len) = crate::utils::alloc_request_body(data);
        assert_eq!(len, data.len());
        assert!(!ptr.is_null());
        unsafe {
            // Reclaim using the same pattern as free_http_request.
            let v = Vec::from_raw_parts(ptr as *mut u8, len, len);
            assert_eq!(v.capacity(), v.len(), "capacity != len makes free_http_request UB");
            assert_eq!(&v[..], data);
        }
    }
}

/// Empty body must return null because `free_http_request` only frees when `body_len > 0`.
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
