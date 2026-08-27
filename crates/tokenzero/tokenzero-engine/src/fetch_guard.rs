//! SSRF guard for `tz_fetch`: URL/host validation, allow/deny lists, and
//! post-DNS IP checks with connection pinning. Without this, any MCP-capable
//! agent could fetch loopback, RFC1918, link-local, and cloud-metadata
//! endpoints and cache the bodies behind refs.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

/// A validated fetch destination for one redirect hop.
#[derive(Debug)]
pub(crate) struct FetchTarget {
    pub(crate) host: String,
    pub(crate) port: u16,
    /// IP to pin curl's connection to via `--resolve`, closing the
    /// resolve-then-connect (DNS rebinding) window. None when the host is
    /// allowlisted (explicitly trusted as configured) or is an IP literal
    /// that already passed the checks.
    pub(crate) pinned_ip: Option<IpAddr>,
}

#[derive(Debug)]
pub(crate) struct FetchBlocked {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) repair: Option<String>,
}

impl FetchBlocked {
    fn new(code: &'static str, message: String) -> Self {
        Self {
            code,
            message,
            repair: None,
        }
    }

    fn with_repair(code: &'static str, message: String, repair: &str) -> Self {
        Self {
            code,
            message,
            repair: Some(repair.to_string()),
        }
    }
}

/// Validate one fetch URL against the configured policy. Allowlisted hosts
/// skip resolution and IP checks: the allowlist is the explicit escape hatch
/// for intentionally reachable private hosts.
pub(crate) fn validate_fetch_target(
    url: &str,
    allow_hosts: &[String],
    deny_hosts: &[String],
) -> Result<FetchTarget, FetchBlocked> {
    let (host, port) = parse_host_port(url)?;
    if host_is_listed(&host, deny_hosts) {
        return Err(FetchBlocked::new(
            "fetch_blocked",
            format!("host {host} is denied by TOKENZERO_FETCH_DENY"),
        ));
    }
    if host_is_listed(&host, allow_hosts) {
        return Ok(FetchTarget {
            host,
            port,
            pinned_ip: None,
        });
    }
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        if let Some(reason) = blocked_ip_reason(ip) {
            return Err(blocked_ip_error(&host, ip, reason));
        }
        return Ok(FetchTarget {
            host,
            port,
            pinned_ip: None,
        });
    }
    let addrs: Vec<SocketAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|err| {
            FetchBlocked::new(
                "fetch_failed",
                format!("could not resolve host {host}: {err}"),
            )
        })?
        .collect();
    if addrs.is_empty() {
        return Err(FetchBlocked::new(
            "fetch_failed",
            format!("could not resolve host {host}"),
        ));
    }
    // Reject if ANY resolved address is blocked: a mixed answer is exactly
    // what a rebinding attack looks like.
    for addr in &addrs {
        if let Some(reason) = blocked_ip_reason(addr.ip()) {
            return Err(blocked_ip_error(&host, addr.ip(), reason));
        }
    }
    Ok(FetchTarget {
        host,
        port,
        pinned_ip: Some(addrs[0].ip()),
    })
}

fn blocked_ip_error(host: &str, ip: IpAddr, reason: &str) -> FetchBlocked {
    FetchBlocked::with_repair(
        "fetch_blocked",
        format!("host {host} resolves to {ip} ({reason}); refusing to fetch"),
        "add the host to TOKENZERO_FETCH_ALLOW to explicitly trust it",
    )
}

/// Extract (host, port) from an http(s) URL. Rejects userinfo to rule out
/// `http://trusted.com@evil.com/` confusion.
fn parse_host_port(url: &str) -> Result<(String, u16), FetchBlocked> {
    let (authority, default_port) = fetch_authority(url)?;
    reject_userinfo(authority)?;
    if authority.starts_with('[') {
        return parse_bracketed_ipv6_authority(authority, default_port, url);
    }
    parse_hostname_authority(authority, default_port, url)
}

fn fetch_authority(url: &str) -> Result<(&str, u16), FetchBlocked> {
    let (rest, default_port) = if let Some(rest) = url.strip_prefix("https://") {
        (rest, 443u16)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (rest, 80u16)
    } else {
        return Err(FetchBlocked::new(
            "invalid_url",
            format!("fetch requires an http(s) URL, got {url}"),
        ));
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    if authority.is_empty() {
        return Err(FetchBlocked::new(
            "invalid_url",
            format!("URL has no host: {url}"),
        ));
    }
    Ok((authority, default_port))
}

fn reject_userinfo(authority: &str) -> Result<(), FetchBlocked> {
    if authority.contains('@') {
        Err(FetchBlocked::new(
            "invalid_url",
            "userinfo in fetch URLs is not supported".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn parse_bracketed_ipv6_authority(
    authority: &str,
    default_port: u16,
    url: &str,
) -> Result<(String, u16), FetchBlocked> {
    let rest = authority.strip_prefix('[').expect("checked by caller");
    let Some((host, after)) = rest.split_once(']') else {
        return Err(FetchBlocked::new(
            "invalid_url",
            format!("unterminated IPv6 literal in {url}"),
        ));
    };
    let port = match after.strip_prefix(':') {
        None if after.is_empty() => default_port,
        Some(port_text) => parse_port(port_text, url)?,
        None => {
            return Err(FetchBlocked::new(
                "invalid_url",
                format!("malformed authority in {url}"),
            ));
        }
    };
    Ok((format!("[{}]", host.to_ascii_lowercase()), port))
}

fn parse_hostname_authority(
    authority: &str,
    default_port: u16,
    url: &str,
) -> Result<(String, u16), FetchBlocked> {
    match authority.split_once(':') {
        Some((host, port_text)) if !port_text.contains(':') => {
            Ok((host.to_ascii_lowercase(), parse_port(port_text, url)?))
        }
        Some(_) => Err(FetchBlocked::new(
            "invalid_url",
            format!("IPv6 hosts must be bracketed in {url}"),
        )),
        None => Ok((authority.to_ascii_lowercase(), default_port)),
    }
}

fn parse_port(text: &str, url: &str) -> Result<u16, FetchBlocked> {
    text.parse::<u16>()
        .map_err(|_| FetchBlocked::new("invalid_url", format!("invalid port in {url}")))
}

fn host_is_listed(host: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| host_matches(host, pattern))
}

/// Suffix match: `example.com` matches `example.com` and `api.example.com`.
fn host_matches(host: &str, pattern: &str) -> bool {
    let pattern = pattern.trim().to_ascii_lowercase();
    if pattern.is_empty() {
        return false;
    }
    host == pattern || host.ends_with(&format!(".{pattern}"))
}

/// Why an IP must not be fetched, or None when it is publicly routable.
fn blocked_ip_reason(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => blocked_ipv4_reason(v4),
        IpAddr::V6(v6) => blocked_ipv6_reason(v6),
    }
}

fn blocked_ipv4_reason(ip: std::net::Ipv4Addr) -> Option<&'static str> {
    let octets = ip.octets();
    if ip.is_unspecified() || octets[0] == 0 {
        Some("unspecified")
    } else if ip.is_loopback() {
        Some("loopback")
    } else if ip.is_private() {
        Some("private (RFC1918)")
    } else if ip.is_link_local() {
        // includes the 169.254.169.254 cloud metadata endpoint
        Some("link-local")
    } else if ip.is_broadcast() {
        Some("broadcast")
    } else if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
        Some("carrier-grade NAT (100.64/10)")
    } else if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
        Some("IETF protocol assignments (192.0.0/24)")
    } else if octets[0] >= 224 && octets[0] < 240 {
        Some("multicast (224.0.0.0/4)")
    } else if octets[0] >= 240 {
        Some("reserved (240.0.0.0/4)")
    } else if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) {
        Some("benchmark/documentation (198.18.0.0/15)")
    } else {
        None
    }
}

fn blocked_ipv6_reason(ip: std::net::Ipv6Addr) -> Option<&'static str> {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return blocked_ipv4_reason(mapped);
    }
    let segments = ip.segments();
    if ip.is_unspecified() {
        Some("unspecified")
    } else if ip.is_loopback() {
        Some("loopback")
    } else if (segments[0] & 0xffc0) == 0xfe80 {
        Some("link-local")
    } else if (segments[0] & 0xfe00) == 0xfc00 {
        Some("unique-local (fc00::/7)")
    } else {
        None
    }
}

/// Marker appended to curl stdout via `-w` so one invocation yields body,
/// status code, and redirect target without following redirects itself.
pub(crate) const FETCH_META_MARKER: &str = "__TOKENZERO_FETCH_META__";

/// Split curl output into (body, http_code, redirect_url). Output from a
/// curl that ignored `-w` (or a truncated capture) parses as a plain body.
pub(crate) fn split_fetch_meta(stdout: &str) -> (String, Option<u16>, Option<String>) {
    let marker = format!("\n{FETCH_META_MARKER} ");
    let Some((body, meta)) = stdout.rsplit_once(&marker) else {
        return (stdout.to_string(), None, None);
    };
    let meta = meta.trim();
    let (code_text, redirect) = meta.split_once(' ').unwrap_or((meta, ""));
    let redirect = redirect.trim();
    (
        body.to_string(),
        code_text.parse().ok(),
        (!redirect.is_empty()).then(|| redirect.to_string()),
    )
}
