use anyhow::{anyhow, Result};
use std::net::{IpAddr, ToSocketAddrs};

/// Extract the hostname from an HTTP(S) URL, stripping port, path, query, fragment.
pub fn extract_host(url: &str) -> Option<String> {
    // Manual parsing to avoid url crate dependency
    let (scheme, rest) = if let Some(idx) = url.find("://") {
        (&url[..idx], &url[idx + 3..])
    } else {
        return None;
    };

    if scheme != "http" && scheme != "https" {
        return None;
    }

    let end_of_authority = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..end_of_authority];

    // Strip userinfo
    let host_port = if let Some(at_idx) = authority.rfind('@') {
        &authority[at_idx + 1..]
    } else {
        authority
    };

    // Strip port
    let host = if host_port.starts_with('[') {
        // IPv6 literal
        if let Some(close_bracket) = host_port.find(']') {
            if close_bracket + 1 < host_port.len() && host_port.as_bytes()[close_bracket + 1] == b':' {
                // [ipv6]:port
                &host_port[..close_bracket + 1]
            } else {
                host_port // [ipv6] or [ipv6]/path
            }
        } else {
            return None; // Invalid IPv6
        }
    } else if let Some(colon_idx) = host_port.find(':') {
        &host_port[..colon_idx]
    } else {
        host_port
    };

    if host.is_empty() {
        return None;
    }

    Some(host.to_string())
}

/// Check if a host matches the allowlist.
/// Supports exact match and wildcard subdomain patterns ("*.example.com").
pub fn host_matches_allowlist(host: &str, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true; // empty allowlist = allow all
    }
    for pattern in allowed {
        // Exact match
        if host == pattern {
            return true;
        }
        // Wildcard subdomain: "*.example.com" matches "api.example.com"
        if let Some(domain) = pattern.strip_prefix("*.")
            && host.ends_with(domain) {
                let prefix_len = host.len().saturating_sub(domain.len());
                if prefix_len > 0 && host.as_bytes()[prefix_len - 1] == b'.' {
                    return true;
                }
            }
        // Implicit subdomain match (like browser_open does)
        if host.len() > pattern.len() {
            let offset = host.len() - pattern.len();
            if &host[offset..] == pattern && host.as_bytes()[offset - 1] == b'.' {
                return true;
            }
        }
    }
    false
}

/// SSRF: check if host is localhost or a private/reserved IP.
pub fn is_local_host(host: &str) -> bool {
    // Strip brackets from IPv6 addresses like [::1]
    let bare = if host.starts_with('[') && host.ends_with(']') {
        &host[1..host.len() - 1]
    } else {
        host
    };

    // Drop IPv6 zone id suffix
    let unscoped = bare.split('%').next().unwrap_or(bare);
    if unscoped.is_empty() {
        return true;
    }

    if unscoped == "localhost" || unscoped.ends_with(".localhost") || unscoped.ends_with(".local") {
        return true;
    }

    if let Ok(ip) = unscoped.parse::<IpAddr>() {
        return is_non_global_ip(ip);
    }

    false
}

fn is_non_global_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            ipv4.is_loopback()
                || ipv4.is_private()
                || ipv4.is_link_local()
                || ipv4.is_broadcast()
                || ipv4.is_documentation()
                || ipv4.octets()[0] == 0 // 0.0.0.0/8
        }
        IpAddr::V6(ipv6) => {
            ipv6.is_loopback() || (ipv6.segments()[0] & 0xfe00) == 0xfc00 // Unique local
                || (ipv6.segments()[0] & 0xffc0) == 0xfe80 // Link local
                || ipv6.is_unspecified()
        }
    }
}

/// Resolve host and return a concrete connect target (IP literal) that is
/// guaranteed to be globally routable. If any resolved address is local/private,
/// reject to prevent mixed-record SSRF bypasses.
pub fn resolve_connect_host(host: &str, port: u16) -> Result<String> {
    let addrs = (host, port).to_socket_addrs()?;
    let mut selected_ip: Option<IpAddr> = None;

    for addr in addrs {
        let ip = addr.ip();
        if is_non_global_ip(ip) {
            return Err(anyhow!("Local address blocked: {}", ip));
        }
        if selected_ip.is_none() {
            selected_ip = Some(ip);
        }
    }

    if let Some(ip) = selected_ip {
        Ok(ip.to_string())
    } else {
        Err(anyhow!("Host resolution failed"))
    }
}
