//! iroh-services helpers shared by the cli and the app.
//!
//! Owns the bundled default API secret and its `IROH_SERVICES_API_SECRET`
//! override/opt-out, the device-name convention, building the client (which
//! registers the device with the services backend), and the two queries both
//! binaries run: a `ping` round-trip and a `net_diagnostics` report.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iroh::Endpoint;
use iroh_services::Client;
use serde::Serialize;

/// Bundled API secret used out of the box by both the cli and the app, so
/// iroh-services telemetry works without the user supplying a key. The app
/// is a diagnostics tool and collects this telemetry by default; the user can
/// opt out via the in-app toggle (see [`SecretSource::AppDefault`]) or the
/// `IROH_SERVICES_API_SECRET` empty-string override.
pub const DEFAULT_API_SECRET: &str =
    "servicesaaqg6nnf7kr3uiacviqgbxeqconvhuz4ldr5dem4gqhsp3cyat6qxexoctwjsi7m6dh2t2qvfu2yhdoaav6eibaj4aaavhonlixbohceu4aa";

/// Environment variable that overrides the API secret, or opts out of
/// iroh-services entirely when set to an empty string.
pub const API_SECRET_ENV: &str = "IROH_SERVICES_API_SECRET";

/// Where a binary wants the API secret to come from when
/// `IROH_SERVICES_API_SECRET` is unset. Naming the source at the call site
/// keeps each binary's telemetry policy explicit rather than implied by a
/// bare string.
#[derive(Debug, Clone, Copy)]
pub enum SecretSource<'a> {
    /// The app's policy: telemetry is on by default with the bundled key.
    /// `disabled` is the user's saved opt-out (the in-app toggle); when set,
    /// telemetry resolves to `None`. `custom` is an optional user-supplied
    /// key that overrides the bundled one; an empty `custom` with `disabled`
    /// false uses [`DEFAULT_API_SECRET`].
    AppDefault { disabled: bool, custom: &'a str },
    /// Fall back to [`DEFAULT_API_SECRET`]. The cli uses this so the
    /// foreground dev tool works out of the box.
    BundledDefault,
}

/// Resolves which API secret to use, or `None` to disable iroh-services.
///
/// `IROH_SERVICES_API_SECRET` wins when set: a non-empty value is used as-is,
/// an empty value opts out (returns `None`). Otherwise the [`SecretSource`]
/// decides: the app stays on by default unless the user disabled it, and the
/// cli always falls back to the bundled key.
#[must_use]
pub fn resolve_api_secret(source: SecretSource<'_>) -> Option<String> {
    if let Ok(env) = std::env::var(API_SECRET_ENV) {
        let trimmed = env.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
    match source {
        SecretSource::AppDefault { disabled: true, .. } => None,
        SecretSource::AppDefault {
            disabled: false,
            custom,
        } => {
            let trimmed = custom.trim();
            Some(if trimmed.is_empty() {
                DEFAULT_API_SECRET.to_string()
            } else {
                trimmed.to_string()
            })
        }
        SecretSource::BundledDefault => Some(DEFAULT_API_SECRET.to_string()),
    }
}

/// A short, platform-tagged device name derived from the endpoint id, used
/// when registering with the services backend.
#[must_use]
pub fn device_name(endpoint_id_hex: &str) -> String {
    let short: String = endpoint_id_hex.chars().take(8).collect();
    if cfg!(target_os = "macos") {
        format!("macos-dx-{short}")
    } else if cfg!(target_os = "linux") {
        format!("linux-dx-{short}")
    } else if cfg!(target_os = "windows") {
        format!("win-dx-{short}")
    } else {
        format!("dx-{short}")
    }
}

/// Builds an iroh-services [`Client`] for `endpoint`. Building registers the
/// device with the services backend under `name`.
///
/// # Errors
///
/// Returns an error if the API secret is malformed, the name is rejected, or
/// the client cannot be built (e.g. the services endpoint is unreachable).
pub async fn build_client(endpoint: &Endpoint, api_secret: &str, name: &str) -> Result<Client> {
    Client::builder(endpoint)
        .api_secret_from_str(api_secret)
        .context("api secret")?
        .name(name.to_string())
        .context("device name")?
        .build()
        .await
        .context("build services client")
}

/// Pings the services endpoint and returns the round-trip time.
///
/// # Errors
///
/// Returns an error if the round-trip fails.
pub async fn ping(client: &Client) -> Result<Duration> {
    let started = Instant::now();
    client.ping().await.context("services ping")?;
    Ok(started.elapsed())
}

/// Runs the services-side `net_diagnostics` and returns a UI/cli-facing
/// summary.
///
/// # Errors
///
/// Returns an error if the diagnostics request fails.
pub async fn net_diagnostics(client: &Client) -> Result<DiagnosticsReport> {
    let report = client
        .net_diagnostics(false)
        .await
        .context("services net_diagnostics")?;
    Ok(DiagnosticsReport::from(report))
}

/// Summary of the services-side `net_diagnostics` report, reshaped for
/// rendering and JSON.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsReport {
    pub endpoint_id: String,
    pub direct_addrs: Vec<String>,
    pub iroh_version: String,
    pub iroh_services_version: String,
    pub has_net_report: bool,
    pub upnp: Option<bool>,
    pub pcp: Option<bool>,
    pub nat_pmp: Option<bool>,
}

impl From<iroh_services::net_diagnostics::DiagnosticsReport> for DiagnosticsReport {
    fn from(r: iroh_services::net_diagnostics::DiagnosticsReport) -> Self {
        let (upnp, pcp, nat_pmp) = match r.portmap_probe {
            Some(p) => (Some(p.upnp), Some(p.pcp), Some(p.nat_pmp)),
            None => (None, None, None),
        };
        Self {
            endpoint_id: r.endpoint_id.to_string(),
            direct_addrs: r.direct_addrs.into_iter().map(|s| s.to_string()).collect(),
            iroh_version: r.iroh_version,
            iroh_services_version: r.iroh_services_version,
            has_net_report: r.net_report.is_some(),
            upnp,
            pcp,
            nat_pmp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `IROH_SERVICES_API_SECRET` is process-global and Cargo runs tests in
    /// one binary on multiple threads, so this single test exercises every
    /// resolution branch in sequence rather than racing separate tests
    /// against the same variable.
    #[test]
    fn resolve_api_secret_precedence() {
        use SecretSource::{AppDefault, BundledDefault};
        // The env var is shared process state; this is the only test that
        // touches it, so the set/remove calls below do not race.
        std::env::remove_var(API_SECRET_ENV);

        // The cli always falls back to the bundled key.
        assert_eq!(
            resolve_api_secret(BundledDefault).as_deref(),
            Some(DEFAULT_API_SECRET)
        );

        // The app is on by default: no custom key, not disabled, uses the
        // bundled key.
        assert_eq!(
            resolve_api_secret(AppDefault {
                disabled: false,
                custom: "",
            })
            .as_deref(),
            Some(DEFAULT_API_SECRET)
        );
        // A blank custom key still resolves to the bundled key, not off.
        assert_eq!(
            resolve_api_secret(AppDefault {
                disabled: false,
                custom: "  ",
            })
            .as_deref(),
            Some(DEFAULT_API_SECRET)
        );
        // A non-empty custom key overrides the bundled one and is trimmed.
        assert_eq!(
            resolve_api_secret(AppDefault {
                disabled: false,
                custom: "  key  ",
            })
            .as_deref(),
            Some("key")
        );
        // The user's opt-out wins over any custom key.
        assert_eq!(
            resolve_api_secret(AppDefault {
                disabled: true,
                custom: "key",
            }),
            None
        );

        // A non-empty env var overrides everything: a disabled app, and a
        // custom key.
        std::env::set_var(API_SECRET_ENV, "from-env");
        assert_eq!(
            resolve_api_secret(AppDefault {
                disabled: true,
                custom: "key",
            })
            .as_deref(),
            Some("from-env")
        );
        assert_eq!(
            resolve_api_secret(AppDefault {
                disabled: false,
                custom: "key",
            })
            .as_deref(),
            Some("from-env")
        );

        // An empty env var is the dev/CI opt-out and beats on-by-default.
        std::env::set_var(API_SECRET_ENV, "");
        assert_eq!(
            resolve_api_secret(AppDefault {
                disabled: false,
                custom: "",
            }),
            None
        );

        std::env::remove_var(API_SECRET_ENV);
    }
}
