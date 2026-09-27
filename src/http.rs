//! Shared HTTP agent configuration.
//!
//! `ureq`'s default TLS provider is rustls, which is not compiled in (it needs
//! a C toolchain for its crypto backend). We build agents explicitly on
//! `native-tls` instead, which uses Schannel and the Windows root store.

use std::time::Duration;

use ureq::config::Config;
use ureq::tls::{TlsConfig, TlsProvider};

/// User-Agent sent with every request.
pub const USER_AGENT: &str = "wx-downloader/0.1";

/// How long to wait for DNS resolution and for the TCP/TLS connection.
///
/// A stalled connection should fail rather than hang forever. This is separate
/// from any whole-request timeout, which downloads deliberately leave unset.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Build an agent using the OS TLS stack.
///
/// `global_timeout` caps the entire request; pass `None` for large downloads,
/// where only the connection phase is time-limited. 4xx/5xx responses are
/// returned as errors.
pub fn agent(global_timeout: Option<Duration>) -> ureq::Agent {
    Config::builder()
        .timeout_resolve(Some(CONNECT_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(global_timeout)
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .build(),
        )
        .build()
        .into()
}

/// Build an agent for the GitHub JSON API.
///
/// Error statuses are returned as responses rather than `Err`, so the caller can
/// read GitHub's `{"message": ...}` body — which is where the rate-limit
/// explanation lives — instead of a bare status code.
pub fn api_agent() -> ureq::Agent {
    Config::builder()
        .http_status_as_error(false)
        .timeout_resolve(Some(CONNECT_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(Duration::from_secs(30)))
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .build(),
        )
        .build()
        .into()
}

/// A GitHub token from the environment, if set.
///
/// Unauthenticated GitHub API access allows only 60 requests per hour per IP;
/// a token raises that to 5000. Set `GITHUB_TOKEN` (or `GH_TOKEN`) to raise the
/// limit — the app works without it, but may hit the cap under heavy use.
pub fn github_token() -> Option<String> {
    for key in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}
