//! Relay endpoint validation (defense against SSRF via a hostile
//! `relay_url` setting). Rules:
//!
//! * http/https schemes only (https recommended; http permitted for
//!   the self-hosted LAN/dev relay);
//! * no embedded credentials (`user:pass@` — the URL would leak the
//!   bearer token path to a surprising place and creds into logs);
//! * no fragment/query on the base URL;
//! * host must be explicit; cloud instance-metadata endpoints are
//!   rejected outright (169.254.169.254, 100.100.100.200, GCE metadata
//!   hostname, IPv4-mapped IPv6 equivalents, link-local IPv6);
//! * loopback + RFC 1918 LAN + ULA stay allowed (the default relay is
//!   `http://127.0.0.1:8080` and LAN self-host is a supported layout).
//!
//! Residual TOCTOU: the name is validated at save/use time but resolved
//! at connect time, so a hostile DNS (rebinding) can still steer the
//! connection. Impact is capped by design — the relay is a blind blob
//! store (E2EE via `aead::seal` with `table:record` AAD), so a rogue
//! endpoint sees only routing headers + ciphertext, never plaintext.
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
    let host = url.host_str().unwrap_or_default().to_string();
    if host.is_empty() {
        return Err("relay URL has no host".into());
    }
    // GCE metadata hostname (exact + subdomains).
    let host_lc = host.to_ascii_lowercase();
    if host_lc == "metadata.google.internal" || host_lc.ends_with(".metadata.google.internal") {
        return Err("cloud metadata hosts are not allowed as relays".into());
    }
    // Cloud instance-metadata IPs, in every representable form
    // (bare v4, IPv4-mapped IPv6, link-local v6).
    match url.host() {
        Some(url::Host::Ipv4(v4)) => {
            if is_metadata_ipv4(&v4) {
                return Err("cloud metadata hosts are not allowed as relays".into());
            }
        }
        Some(url::Host::Ipv6(v6)) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                if is_metadata_ipv4(&mapped) {
                    return Err("cloud metadata hosts are not allowed as relays".into());
                }
            } else if v6.is_unspecified() || v6.segments()[0] & 0xffc0 == 0xfe80 {
                return Err("link-local hosts are not allowed as relays".into());
            }
        }
        _ => {}
    }
    // Normalize: strip trailing slashes so `{base}{path}` formatting
    // is stable.
    let s = trimmed.trim_end_matches('/').to_string();
    Ok(s)
}

/// Cloud metadata / link-local IPv4: 169.254.0.0/16, 100.100.100.200
/// (Alibaba), 0.0.0.0 (unspecified = any-interface rebinding risk).
fn is_metadata_ipv4(v4: &std::net::Ipv4Addr) -> bool {
    v4.is_link_local() || v4.is_unspecified() || *v4 == std::net::Ipv4Addr::new(100, 100, 100, 200)
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
        assert!(validate_relay_url("http://100.100.100.200/").is_err());
        assert!(validate_relay_url("http://0.0.0.0:8080/").is_err());
        assert!(validate_relay_url("http://[::ffff:169.254.169.254]/").is_err());
        assert!(validate_relay_url("http://[::ffff:100.100.100.200]/").is_err());
        assert!(validate_relay_url("http://[fe80::1]/").is_err());
        assert!(validate_relay_url("http://metadata.google.internal/").is_err());
    }

    #[test]
    fn allows_loopback_lan_and_ula() {
        // Default local relay + LAN/ULA self-host must keep working.
        assert!(validate_relay_url("http://127.0.0.1:8080").is_ok());
        assert!(validate_relay_url("http://[::1]:8080").is_ok());
        assert!(validate_relay_url("http://192.168.1.10:8080").is_ok());
        assert!(validate_relay_url("http://10.0.0.5:8080").is_ok());
        assert!(validate_relay_url("http://[fd00::5]:8080").is_ok());
    }

    #[test]
    fn rejects_garbage() {
        assert!(validate_relay_url("").is_err());
        assert!(validate_relay_url("not a url").is_err());
        assert!(validate_relay_url("https://").is_err());
    }
}
