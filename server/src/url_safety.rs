//! SSRF guards for server-initiated HTTP fetches.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use url::Url;

pub async fn validate_remote_http_url(url_str: &str) -> Result<Url, String> {
    let url = Url::parse(url_str).map_err(|_| "invalid url".to_string())?;
    match url.scheme() {
        "http" | "https" => {}
        _ => return Err("only http and https URLs are allowed".to_string()),
    }

    let host = url
        .host_str()
        .ok_or_else(|| "missing host".to_string())?
        .trim()
        .trim_matches(|c| c == '[' || c == ']')
        .to_lowercase();

    if host.is_empty() {
        return Err("missing host".to_string());
    }

    if host == "localhost" || host.ends_with(".localhost") {
        return Err("localhost is not allowed".to_string());
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            return Err("private or reserved addresses are not allowed".to_string());
        }
        return Ok(url);
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let mut resolved_any = false;
    match tokio::net::lookup_host((host.as_str(), port)).await {
        Ok(addrs) => {
            for addr in addrs {
                resolved_any = true;
                if is_blocked_ip(addr.ip()) {
                    return Err(
                        "hostname resolves to a private or reserved address".to_string(),
                    );
                }
            }
        }
        Err(_) => return Err("hostname could not be resolved".to_string()),
    }

    if !resolved_any {
        return Err("hostname could not be resolved".to_string());
    }

    Ok(url)
}

/// Validate resolved connect target before issuing a request (defense in depth).
pub fn validate_socket_addr(addr: SocketAddr) -> Result<(), String> {
    if is_blocked_ip(addr.ip()) {
        Err("private or reserved addresses are not allowed".to_string())
    } else {
        Ok(())
    }
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] == 0
                || matches!(v4.octets(), [100, 64..=127, _, _])
                || matches!(v4.octets(), [169, 254, _, _])
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_blocked_ip;
    use std::net::Ipv4Addr;

    #[test]
    fn blocks_loopback() {
        assert!(is_blocked_ip(Ipv4Addr::LOCALHOST.into()));
    }
}