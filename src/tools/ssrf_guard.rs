/// SSRF (Server-Side Request Forgery) protection for all HTTP-making tools.
///
/// This module resolves the hostname in a URL and rejects requests targeting:
/// - Loopback addresses (127.x.x.x, ::1)
/// - Private RFC-1918 ranges (10.x, 172.16-31.x, 192.168.x)
/// - Link-local addresses (169.254.x.x — cloud metadata endpoint lives here)
/// - Broadcast / documentation ranges
/// - IPv6 ULA (fc00::/7)
///
/// DNS re-binding protection: ALL resolved addresses are checked, not just the first.
///
/// Philosophy: this protects the host infrastructure, not the agent's capabilities.
/// The agent can still fetch any public URL — only private/internal ranges are blocked.

use anyhow::{bail, Result};
use reqwest::Url;
use std::net::{IpAddr, Ipv4Addr};

/// Validate a URL for SSRF safety.  Returns `Ok(())` if safe, `Err` with a
/// user-readable message if the target resolves to a private/internal address.
///
/// This is an async DNS lookup.
pub async fn check_url(url: &str) -> Result<()> {
    let parsed = Url::parse(url).map_err(|e| anyhow::anyhow!("Invalid URL: {}", e))?;

    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        bail!("Only http:// and https:// URLs are permitted");
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("URL has no host"))?;

    // Reject raw IP addresses that are obviously private without doing DNS
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(ip) {
            bail!(
                "SSRF protection: requests to private/internal IPs are not allowed ({})",
                ip
            );
        }
        return Ok(());
    }

    // Resolve hostname → IP(s) and check each one
    let port = parsed.port().unwrap_or(if scheme == "https" { 443 } else { 80 });
    let addrs = tokio::net::lookup_host(format!("{}:{}", host, port))
        .await
        .map_err(|e| anyhow::anyhow!("DNS resolution failed for '{}': {}", host, e))?;

    for addr in addrs {
        let ip = addr.ip();
        if is_private_ip(ip) {
            bail!(
                "SSRF protection: '{}' resolves to a private/internal IP ({}) — request blocked",
                host,
                ip
            );
        }
    }

    Ok(())
}

/// Return `true` if the IP is loopback, private, link-local, or otherwise
/// an address that should never be reachable from a public request.
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()           // 127.0.0.0/8
                || v4.is_private()     // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()  // 169.254.0.0/16  ← cloud metadata
                || v4.is_broadcast()   // 255.255.255.255
                || v4.is_documentation() // 192.0.2/24 etc.
                || is_shared_address_space(v4) // 100.64.0.0/10
                || is_ietf_protocol_assignments(v4) // 192.0.0.0/24
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()                         // ::1
                || is_ipv6_ula(v6)                   // fc00::/7
                || v6.is_unspecified()               // ::
                // Map ::ffff:0:0/96 (IPv4-mapped) and re-check
                || v6.to_ipv4_mapped().map(|v4| is_private_ip(IpAddr::V4(v4))).unwrap_or(false)
                || v6.to_ipv4().map(|v4| is_private_ip(IpAddr::V4(v4))).unwrap_or(false)
        }
    }
}

/// 100.64.0.0/10 — IANA Shared Address Space (RFC 6598)
fn is_shared_address_space(v4: Ipv4Addr) -> bool {
    let octets = v4.octets();
    octets[0] == 100 && (octets[1] & 0xC0) == 64
}

/// 192.0.0.0/24 — IETF Protocol Assignments (RFC 6890)
fn is_ietf_protocol_assignments(v4: Ipv4Addr) -> bool {
    let octets = v4.octets();
    octets[0] == 192 && octets[1] == 0 && octets[2] == 0
}

/// fc00::/7 — IPv6 Unique Local Addresses (RFC 4193)
fn is_ipv6_ula(v6: std::net::Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xFE00) == 0xFC00
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_loopback_rejected() {
        assert!(check_url("http://127.0.0.1/secret").await.is_err());
        assert!(check_url("http://localhost/secret").await.is_err());
        assert!(check_url("http://[::1]/secret").await.is_err());
    }

    #[tokio::test]
    async fn test_cloud_metadata_rejected() {
        assert!(check_url("http://169.254.169.254/latest/meta-data/").await.is_err());
        assert!(check_url("http://169.254.0.1/foo").await.is_err());
    }

    #[tokio::test]
    async fn test_private_networks_rejected() {
        assert!(check_url("http://10.0.0.1/").await.is_err());
        assert!(check_url("http://172.16.0.1/").await.is_err());
        assert!(check_url("http://192.168.1.1/").await.is_err());
    }

    #[tokio::test]
    async fn test_public_allowed() {
        // This test requires DNS so we just check it doesn't fail with our custom SSRF error
        let result = check_url("https://example.com/").await;
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(!msg.contains("private/internal IPs are not allowed"), "Public IP blocked incorrectly");
        }
    }

    #[tokio::test]
    async fn test_invalid_scheme_rejected() {
        assert!(check_url("file:///etc/passwd").await.is_err());
        assert!(check_url("ftp://internal.host/data").await.is_err());
    }
}
