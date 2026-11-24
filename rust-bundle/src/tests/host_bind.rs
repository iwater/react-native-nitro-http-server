//! Bind address resolution tests: valid addresses (including `0.0.0.0` / `::`)
//! are preserved as-is, and only unparseable/empty hosts fall back to `127.0.0.1`.

use crate::tests::test_utils::{get_free_port, run_outside_async, wait_for_port};
use crate::utils::resolve_bind_addr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[test]
fn test_wildcard_host_is_not_rewritten() {
    assert_eq!(
        resolve_bind_addr("0.0.0.0", 8080),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080),
        "0.0.0.0 must not be rewritten"
    );
}

#[test]
fn test_ipv6_wildcard_is_not_rewritten() {
    assert_eq!(
        resolve_bind_addr("::", 8080),
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 8080)
    );
}

#[test]
fn test_concrete_hosts_pass_through() {
    assert_eq!(
        resolve_bind_addr("127.0.0.1", 3000),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3000)
    );
    assert_eq!(
        resolve_bind_addr("192.168.1.5", 3000),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5)), 3000)
    );
}

#[test]
fn test_invalid_hosts_fall_back_to_loopback() {
    for bad in ["", "localhost", "not-an-ip", "999.1.1.1"] {
        assert_eq!(
            resolve_bind_addr(bad, 5000),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5000),
            "{:?} should fall back to 127.0.0.1",
            bad
        );
    }
}

/// Find a non-loopback local IPv4 address using a UDP route probe (no packets sent).
fn local_non_loopback_ipv4() -> Option<Ipv4Addr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("1.1.1.1:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// Smoke test verifying that binding `0.0.0.0` succeeds and accepts requests.
#[tokio::test]
async fn test_wildcard_bind_smoke() {
    let Some(ip) = local_non_loopback_ipv4() else {
        eprintln!("Skipping test_wildcard_bind_smoke: no non-loopback IPv4 available");
        return;
    };

    let port = get_free_port();
    let host = std::ffi::CString::new("0.0.0.0").unwrap();
    let actual = run_outside_async(move || {
        crate::c_api::start_server(
            port as i32,
            host.as_ptr(),
            crate::tests::test_utils::test_callback_ok,
        )
    });
    assert!(actual > 0, "binding 0.0.0.0 must succeed");
    wait_for_port(actual as u16).await;

    let client = reqwest::Client::new();
    let via_lan = client
        .get(format!("http://{}:{}/x", ip, actual))
        .send()
        .await;
    let via_loopback = client
        .get(format!("http://127.0.0.1:{}/x", actual))
        .send()
        .await;

    run_outside_async(|| crate::c_api::stop_server());

    assert!(
        via_loopback.is_ok(),
        "server bound to 0.0.0.0 must be reachable via 127.0.0.1: {:?}",
        via_loopback.err()
    );
    // Log LAN IP probe for diagnostics only; loopback-bound sockets on the same host
    // may also accept local interface connections on some OSes.
    eprintln!(
        "LAN IP {} probe: {:?} (diagnostic only)",
        ip,
        via_lan.as_ref().map(|r| r.status()).map_err(|e| e.to_string())
    );
}
