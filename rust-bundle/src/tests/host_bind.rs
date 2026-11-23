//! `host` → 绑定地址的解析与真实绑定。
//!
//! 为什么单独钉这一组：GitHub issue #7 报过「所有 server 类型都不支持
//! `host: '0.0.0.0'`」。当时的核实结论是**环境问题**（Windows Subsystem for Android
//! 的共享网络），不是代码问题 —— 但那时**本地没有任何测试**覆盖这条路径，
//! 结论只能靠读代码得出。
//!
//! 这组用例把它钉住：**合法地址一律原样绑定**，只有解析失败（含空串）才回落 127.0.0.1。
//! 以后谁加「0.0.0.0 → 127.0.0.1」这种"优化"，第一条用例会立刻红。

use crate::tests::test_utils::{get_free_port, run_outside_async, wait_for_port};
use crate::utils::resolve_bind_addr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// ⚠️ 核心用例：`0.0.0.0` 必须原样绑定（绑全网卡）。
#[test]
fn test_wildcard_host_is_not_rewritten() {
    assert_eq!(
        resolve_bind_addr("0.0.0.0", 8080),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080),
        "0.0.0.0 被改写了 —— 那正是 issue #7 报告的症状"
    );
}

/// IPv6 的 `::`（也是「全网卡」）同理。
#[test]
fn test_ipv6_wildcard_is_not_rewritten() {
    assert_eq!(
        resolve_bind_addr("::", 8080),
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 8080)
    );
}

/// 具体地址（回环 / LAN）原样透传。
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

/// 只有解析失败（含空串）才回落 127.0.0.1。
#[test]
fn test_invalid_hosts_fall_back_to_loopback() {
    for bad in ["", "localhost", "not-an-ip", "999.1.1.1"] {
        assert_eq!(
            resolve_bind_addr(bad, 5000),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5000),
            "{:?} 应回落 127.0.0.1",
            bad
        );
    }
}

/// 找一个**非环回**的本机 IPv4；没有就返回 None（用例跳过）。
///
/// 用 UDP `connect` 让内核选路由（不真的发包，离线也安全）；
/// 拿不到路由就说明这个环境没有可用的 LAN 地址 → 跳过。
fn local_non_loopback_ipv4() -> Option<Ipv4Addr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("1.1.1.1:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// 真绑一次 `0.0.0.0` 的**冒烟**用例：证明「绑全网卡」这条路真的能起服务并被访问。
///
/// ⚠️ **它没有区分力，不是守卫** —— 2026-09-22 变异检验实测：
/// 把 `0.0.0.0` 改成回环（`resolve_bind_addr` 里注入 `is_unspecified() → 127.0.0.1`），
/// 这条**仍然通过**（本机那个「非环回 IPv4」在回环绑定下也可达）。
/// 真正的守卫是上面那两条 `*_is_not_rewritten` 单元用例（变异下确实变红）。
///
/// 保留它的理由：它能抓到「绑 0.0.0.0 直接起不来」（返回 0）这类更粗的问题。
#[tokio::test]
async fn test_wildcard_bind_smoke() {
    let Some(ip) = local_non_loopback_ipv4() else {
        eprintln!("跳过 test_wildcard_bind_smoke：本机没有非环回 IPv4（离线/容器环境）");
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
    assert!(actual > 0, "绑定 0.0.0.0 必须成功（返回 0 = 绑定失败）");
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
        "绑 0.0.0.0 之后经 127.0.0.1 必须能访问：{:?}",
        via_loopback.err()
    );
    // 经 LAN IP 只作**信息**输出，不断言：实测它在回环绑定下也可达（见上面的说明），
    // 断言它会给出一条永远绿的假守卫。
    eprintln!(
        "经 LAN IP {} 访问：{:?}（仅信息，不作断言）",
        ip,
        via_lan.as_ref().map(|r| r.status()).map_err(|e| e.to_string())
    );
}
