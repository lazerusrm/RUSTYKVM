//! SSRF guards for server-initiated HTTP fetches.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use url::Url;

pub fn validate_remote_http_url(url_str: &str) -> Result<Url, String> {
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

    // Block obvious private IPv4 literals embedded in hostnames.
    if host.parse::<Ipv4Addr>().is_ok() || host.parse::<Ipv6Addr>().is_ok() {
        return Err("private or reserved addresses are not allowed".to_string());
    }

    Ok(url)
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
                || matches!(v4.octets(), [100, 64..=127, _, _]) // CGNAT 100.64.0.0/10
                || matches!(v4.octets(), [169, 254, _, _])
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique local
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local
        }
    }
}