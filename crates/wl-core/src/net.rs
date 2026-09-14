//! Relay endpoint validation (defense against SSRF via a hostile
//! `relay_url` setting). Rules:
//!
//! * http/https schemes only (https recommended; http permitted for
//!   the self-hosted LAN/dev relay);
//! * no embedded credentials (`user:pass@` — the URL would leak the
//!   bearer token path to a surprising place and creds into logs);
//! * no fragment/query on the base URL;
//! * host must be explicit; bare-IP metadata endpoints
//!   (169.254.0.0/16 link-local) are rejected outright.
//!
//! The check lives in wl-core (platform-clean) so the shell and any
//! future mobile client share one policy.

/// Validates a user-supplied relay base URL. Returns the normalized
/// base (no trailing slash) on success.
pub fn validate_relay_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("relay URL is empty".into());
    }
    let url = url::Url::parse(trimmed).map_err(|e| format!("invalid relay URL: {e}"))?;
    match url.scheme() {
        "https" => {}
        "http" => {}
        other => {
            return Err(format!(
                "relay URL scheme must be http or https (got {other:?})"
            ))
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("relay URL must not embed credentials".into());
    }
    if url.fragment().is_some() || url.query().is_some() {
        return Err("relay URL must be a bare base (no query/fragment)".into());
    }
    if url.host_str().unwrap_or_default().is_empty() {
        return Err("relay URL has no host".into());
    }
    // Link-local metadata endpoints (cloud instance metadata, routers).
    match url.host() {
        Some(url::Host::Ipv4(v4)) if v4.is_link_local() => {
            return Err("link-local hosts are not allowed as relays".into());
        }
        Some(url::Host::Ipv6(v6)) if v6.segments()[0] & 0xffc0 == 0xfe80 => {
            return Err("link-local hosts are not allowed as relays".into());
        }
        _ => {}
    }
    // Normalize: strip trailing slashes so `{base}{path}` formatting
    // is stable.
    let s = trimmed.trim_end_matches('/').to_string();
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https_and_lan_http() {
        assert_eq!(
            validate_relay_url("https://relay.example.com/").unwrap(),
            "https://relay.example.com"
        );
        assert_eq!(
            validate_relay_url("http://127.0.0.1:8080").unwrap(),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(validate_relay_url("file:///etc/passwd").is_err());
        assert!(validate_relay_url("gopher://relay.example.com").is_err());
        assert!(validate_relay_url("ftp://relay.example.com").is_err());
    }

    #[test]
    fn rejects_credentials_query_fragment_and_metadata() {
        assert!(validate_relay_url("https://user:pass@relay.example.com").is_err());
        assert!(validate_relay_url("https://relay.example.com/push?x=1").is_err());
        assert!(validate_relay_url("https://relay.example.com/#frag").is_err());
        assert!(validate_relay_url("http://169.254.169.254/latest/meta").is_err());
        assert!(validate_relay_url("http://169.254.0.1:8080").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(validate_relay_url("").is_err());
        assert!(validate_relay_url("not a url").is_err());
        assert!(validate_relay_url("https://").is_err());
    }
}
