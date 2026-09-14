//! Outbound proxy selection for iroh's relay and discovery traffic.
//!
//! Reads `[network] proxy` or `ALL_PROXY`/`HTTP(S)_PROXY`. iroh speaks HTTP
//! `CONNECT`, so SOCKS URLs are normalized to `http://` on the same host:port.

use anyhow::{Context, Result};
use url::Url;

/// Environment variables checked for an outbound proxy, in priority order.
/// `ALL_PROXY`/`all_proxy` first (the conventional catch-all), then the
/// HTTP-specific ones, matching what most tooling expects.
const PROXY_ENV_KEYS: &[&str] = &[
    "ALL_PROXY",
    "all_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
];

/// Resolve the outbound proxy for iroh.
///
/// An explicit value (from `[network] proxy` in the config) wins; otherwise the
/// proxy environment variables are consulted. Returns `None` when unset.
pub fn resolve(explicit: Option<&str>) -> Result<Option<Url>> {
    let raw = explicit
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            PROXY_ENV_KEYS
                .iter()
                .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
        });

    match raw {
        Some(raw) => Ok(Some(parse(&raw)?)),
        None => Ok(None),
    }
}

/// Parse and normalize a proxy URL.
///
/// iroh's relay client only speaks HTTP `CONNECT` tunnels. Mixed inbounds
/// (xray/sing-box/clash) accept `CONNECT` on the same port as SOCKS, so a
/// `socks5://host:port` URL is normalized to `http://host:port`; a pure SOCKS
/// proxy will then fail at connect time (with a warning logged here).
pub fn parse(raw: &str) -> Result<Url> {
    let raw = raw.trim();
    let url = Url::parse(raw).with_context(|| format!("invalid proxy URL: {raw}"))?;

    match url.scheme() {
        "http" | "https" => Ok(url),
        "socks5" | "socks5h" | "socks" | "socks4" | "socks4a" => {
            tracing::warn!(
                proxy = %redacted(&url),
                "proxy uses a SOCKS scheme, but iroh only speaks HTTP CONNECT; \
                 trying the same host/port as an HTTP proxy (works for mixed inbounds)"
            );
            // Replace just the scheme text (preserves credentials/host/port).
            let normalized = format!("http{}", &raw[url.scheme().len()..]);
            Url::parse(&normalized).with_context(|| format!("invalid proxy URL: {raw}"))
        }
        other => anyhow::bail!("unsupported proxy scheme '{other}' in {raw}"),
    }
}

/// Render a proxy URL for logs, hiding any embedded credentials.
pub fn redacted(url: &Url) -> String {
    if url.username().is_empty() && url.password().is_none() {
        return url.to_string();
    }
    let mut masked = url.clone();
    let _ = masked.set_username("***");
    let _ = masked.set_password(Some("***"));
    masked.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https() {
        assert_eq!(parse("http://127.0.0.1:10808").unwrap().scheme(), "http");
        assert_eq!(parse("https://proxy:8443").unwrap().scheme(), "https");
    }

    #[test]
    fn normalizes_socks_to_http() {
        let url = parse("socks5://127.0.0.1:10808").unwrap();
        assert_eq!(url.scheme(), "http");
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert_eq!(url.port(), Some(10808));

        assert_eq!(parse("socks5h://127.0.0.1:1080").unwrap().scheme(), "http");
    }

    #[test]
    fn rejects_unknown_scheme() {
        assert!(parse("ftp://127.0.0.1:21").is_err());
    }

    #[test]
    fn redacts_credentials() {
        let url = parse("http://user:secret@127.0.0.1:10808").unwrap();
        let shown = redacted(&url);
        assert!(!shown.contains("secret"));
        assert!(shown.contains("***"));
    }
}
