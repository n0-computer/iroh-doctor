//! Per-destination-port NAT mapping probe: the missing input for
//! [`crate::nat::NatType::Easy`].
//!
//! iroh's net_report compares the addresses observed by different relays,
//! which all serve QUIC address discovery (QAD) on one UDP port, so it can
//! only tell whether the NAT mapping varies by destination *address*.
//! Distinguishing an endpoint-independent mapping (RFC 4787, the Easy case)
//! from a port-dependent one needs the other axis: the same destination host
//! reached on two different ports, from the same local socket.
//!
//! No public iroh infrastructure offers QAD on two ports of one host, so this
//! module ships both halves, matching the doctor's two-party model:
//!
//! - [`QadHelper`] is a tiny QAD server. A collaborator runs two of them on a
//!   publicly reachable machine (`iroh-doctor nat-helper` does exactly that).
//! - [`probe_port_variation`] dials every target from one local socket per
//!   address family, reads the address each target observed, and
//!   [`compute_port_variation`] compares the mappings across same-host,
//!   different-port pairs.
//!
//! The probe does not verify the helper's TLS certificate: helpers are
//! ephemeral, self-signed, and only ever asked "what address do you see?".
//! A lying helper can spoil a diagnostic, not compromise the host; this is
//! the same trust model as classic STUN.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use iroh_relay::quic::{QuicClient, ALPN_QUIC_ADDR_DISC, QUIC_ADDR_DISC_CLOSE_CODE};
use n0_future::StreamExt;
use serde::Serialize;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;
use tracing::{debug, info, warn};

/// Ceiling for one target: QUIC handshake plus the first observed-address
/// report. QAD sends the report right after the handshake, so a healthy
/// helper answers in one round trip.
const PER_TARGET_TIMEOUT: Duration = Duration::from_secs(5);

/// SNI sent to helpers. Never verified (the probe skips certificate
/// checks); it only has to be a well-formed DNS name.
const HELPER_SNI: &str = "iroh-doctor-nat-helper";

/// What one QAD target reported. `observed` is the public address (and,
/// crucially, port) the helper saw our packets arrive from.
#[derive(Debug, Clone, Serialize)]
pub struct PortObservation {
    /// The helper address we dialed.
    pub target: SocketAddr,
    /// The external address the helper observed, when the probe succeeded.
    pub observed: Option<SocketAddr>,
    /// Why the probe failed, when it did.
    pub error: Option<String>,
}

/// Result of [`probe_port_variation`]: the per-target observations plus the
/// per-family conclusion in the shape
/// [`crate::nat::ExtendedNetworkReport`] expects.
#[derive(Debug, Clone, Serialize)]
pub struct PortVariationReport {
    pub observations: Vec<PortObservation>,
    /// Whether the IPv4 mapping changed across same-host, different-port
    /// targets. `None` when no such pair completed.
    pub varies_ipv4: Option<bool>,
    /// IPv6 counterpart of `varies_ipv4`.
    pub varies_ipv6: Option<bool>,
}

/// Dials every target over QAD and reports whether the NAT mapping varies
/// by destination port.
///
/// All targets of one address family are probed concurrently from a single
/// local UDP socket; a mapping comparison across sockets would measure
/// per-socket behavior instead of per-destination behavior. Targets that
/// fail (unreachable helper, timeout) are recorded in the observations and
/// excluded from the comparison.
///
/// # Errors
///
/// Returns an error only when no local socket could be bound or the TLS
/// client configuration could not be built; per-target failures land in
/// [`PortObservation::error`].
pub async fn probe_port_variation(targets: &[SocketAddr]) -> Result<PortVariationReport> {
    let v4: Vec<SocketAddr> = targets.iter().copied().filter(|a| a.is_ipv4()).collect();
    let v6: Vec<SocketAddr> = targets.iter().copied().filter(|a| a.is_ipv6()).collect();

    let mut observations = Vec::with_capacity(targets.len());
    for (bind, family_targets) in [("0.0.0.0:0", v4), ("[::]:0", v6)] {
        if family_targets.is_empty() {
            continue;
        }
        let endpoint = noq::Endpoint::client(bind.parse().expect("static addr"))
            .context("binding probe socket")?;
        let client = QuicClient::new(endpoint.clone(), insecure_client_config()?);

        let probes = family_targets
            .iter()
            .map(|&target| observe_one(&client, target));
        observations.extend(n0_future::join_all(probes).await);

        endpoint.close(QUIC_ADDR_DISC_CLOSE_CODE, b"done");
        endpoint.wait_idle().await;
    }

    let (varies_ipv4, varies_ipv6) = compute_port_variation(&observations);
    Ok(PortVariationReport {
        observations,
        varies_ipv4,
        varies_ipv6,
    })
}

/// Dials one target and waits for its first observed-address report.
async fn observe_one(client: &QuicClient, target: SocketAddr) -> PortObservation {
    let result = tokio::time::timeout(PER_TARGET_TIMEOUT, async {
        let conn = client
            .create_conn(target, HELPER_SNI)
            .await
            .context("connect")?;
        let observed = conn
            .observed_external_addr()
            .next()
            .await
            .context("connection closed before an observed-address report")?;
        conn.close(QUIC_ADDR_DISC_CLOSE_CODE, b"done");
        // Helpers are dialed per family, but a v4-mapped v6 form can still
        // come back; canonicalize so mappings compare structurally.
        Ok::<SocketAddr, anyhow::Error>(SocketAddr::new(
            observed.ip().to_canonical(),
            observed.port(),
        ))
    })
    .await;

    match result {
        Ok(Ok(observed)) => {
            debug!(%target, %observed, "qad observation");
            PortObservation {
                target,
                observed: Some(observed),
                error: None,
            }
        }
        Ok(Err(e)) => PortObservation {
            target,
            observed: None,
            error: Some(format!("{e:#}")),
        },
        Err(_) => PortObservation {
            target,
            observed: None,
            error: Some(format!("timed out after {PER_TARGET_TIMEOUT:?}")),
        },
    }
}

/// Compares successful observations across same-host, different-port target
/// pairs and returns the `(ipv4, ipv6)` variation verdicts.
///
/// `Some(true)` when any such pair saw different external mappings,
/// `Some(false)` when at least one pair completed and all mappings agree,
/// `None` when no two successful targets shared a host (nothing to compare).
#[must_use]
pub fn compute_port_variation(observations: &[PortObservation]) -> (Option<bool>, Option<bool>) {
    let verdict = |ipv4: bool| -> Option<bool> {
        let ok: Vec<&PortObservation> = observations
            .iter()
            .filter(|o| o.target.is_ipv4() == ipv4 && o.observed.is_some())
            .collect();
        let mut compared = false;
        for (i, a) in ok.iter().enumerate() {
            for b in &ok[i + 1..] {
                if a.target.ip() == b.target.ip() && a.target.port() != b.target.port() {
                    compared = true;
                    if a.observed != b.observed {
                        return Some(true);
                    }
                }
            }
        }
        compared.then_some(false)
    };
    (verdict(true), verdict(false))
}

/// A minimal QAD server: accepts QUIC connections with the QAD ALPN, lets
/// the transport report the observed address, and holds each connection
/// until the client closes it. Two of these on one public host are the
/// counterpart [`probe_port_variation`] needs.
///
/// The endpoint shuts down when the helper is dropped.
#[derive(Debug)]
pub struct QadHelper {
    local_addr: SocketAddr,
    endpoint: noq::Endpoint,
    _accept_task: AbortOnDropHandle<()>,
}

impl QadHelper {
    /// Binds `addr` (use port 0 for an OS-assigned port) and starts
    /// accepting QAD connections with a fresh self-signed certificate.
    ///
    /// # Errors
    ///
    /// Returns an error when the certificate cannot be generated or the
    /// socket cannot be bound.
    pub fn spawn(addr: SocketAddr) -> Result<Self> {
        let cert = rcgen::generate_simple_self_signed(vec![HELPER_SNI.to_string()])
            .context("generating self-signed certificate")?;
        let mut server_config = rustls::ServerConfig::builder_with_provider(ring_provider())
            .with_safe_default_protocol_versions()
            .context("tls protocol versions")?
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into()),
            )
            .context("tls server config")?;
        server_config.alpn_protocols = vec![ALPN_QUIC_ADDR_DISC.to_vec()];

        let server_config = noq::crypto::rustls::QuicServerConfig::try_from(server_config)
            .context("quic server config")?;
        let mut server_config = noq::ServerConfig::with_crypto(Arc::new(server_config));
        let transport = Arc::get_mut(&mut server_config.transport).expect("not shared yet");
        transport
            .max_concurrent_uni_streams(0u8.into())
            .max_concurrent_bidi_streams(0u8.into())
            .send_observed_address_reports(true);

        let endpoint =
            noq::Endpoint::server(server_config, addr).context("binding helper socket")?;
        let local_addr = endpoint.local_addr().context("helper local addr")?;
        info!(%local_addr, "qad helper listening");

        let accept_endpoint = endpoint.clone();
        let task = tokio::spawn(async move {
            let mut conns = JoinSet::new();
            while let Some(incoming) = accept_endpoint.accept().await {
                let remote = incoming.remote_address();
                conns.spawn(async move {
                    match incoming.await {
                        Ok(conn) => {
                            debug!(%remote, "qad helper connection");
                            // The observed-address report is transport-level;
                            // just hold the connection until the peer closes.
                            let reason = conn.closed().await;
                            debug!(%remote, %reason, "qad helper connection closed");
                        }
                        Err(e) => warn!(%remote, err = %e, "qad helper handshake failed"),
                    }
                });
                // Reap finished handlers so the set does not grow unbounded.
                while conns.try_join_next().is_some() {}
            }
        });

        Ok(Self {
            local_addr,
            endpoint,
            _accept_task: AbortOnDropHandle::new(task),
        })
    }

    /// The bound address (with the OS-assigned port resolved).
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Closes the endpoint and waits for in-flight connections to wind down.
    pub async fn shutdown(self) {
        self.endpoint.close(QUIC_ADDR_DISC_CLOSE_CODE, b"shutdown");
        self.endpoint.wait_idle().await;
    }
}

/// A rustls client config that accepts any server certificate. See the
/// module docs for why that is sound here.
fn insecure_client_config() -> Result<rustls::ClientConfig> {
    let config = rustls::ClientConfig::builder_with_provider(ring_provider())
        .with_safe_default_protocol_versions()
        .context("tls protocol versions")?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(ring_provider())))
        .with_no_client_auth();
    Ok(config)
}

fn ring_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Certificate verifier that accepts everything. Only used for QAD probes,
/// where the transported data is the observed address of our own packets.
#[derive(Debug)]
struct AcceptAnyCert(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(target: &str, observed: Option<&str>) -> PortObservation {
        PortObservation {
            target: target.parse().unwrap(),
            observed: observed.map(|o| o.parse().unwrap()),
            error: None,
        }
    }

    #[test]
    fn variation_detected_when_same_host_ports_map_differently() {
        let (v4, v6) = compute_port_variation(&[
            obs("203.0.113.7:1111", Some("198.51.100.2:40000")),
            obs("203.0.113.7:2222", Some("198.51.100.2:40017")),
        ]);
        assert_eq!(v4, Some(true));
        assert_eq!(v6, None);
    }

    #[test]
    fn stable_mapping_across_same_host_ports_is_no_variation() {
        let (v4, v6) = compute_port_variation(&[
            obs("203.0.113.7:1111", Some("198.51.100.2:40000")),
            obs("203.0.113.7:2222", Some("198.51.100.2:40000")),
        ]);
        assert_eq!(v4, Some(false));
        assert_eq!(v6, None);
    }

    #[test]
    fn different_hosts_are_not_comparable() {
        // Different-host targets test address variation, which is the base
        // report's job; this probe must stay silent on them.
        let (v4, _) = compute_port_variation(&[
            obs("203.0.113.7:1111", Some("198.51.100.2:40000")),
            obs("203.0.113.8:2222", Some("198.51.100.2:41234")),
        ]);
        assert_eq!(v4, None);
    }

    #[test]
    fn failed_observations_are_excluded() {
        let (v4, _) = compute_port_variation(&[
            obs("203.0.113.7:1111", Some("198.51.100.2:40000")),
            obs("203.0.113.7:2222", None),
        ]);
        assert_eq!(v4, None);
    }

    #[test]
    fn families_are_judged_separately() {
        let (v4, v6) = compute_port_variation(&[
            obs("203.0.113.7:1111", Some("198.51.100.2:40000")),
            obs("203.0.113.7:2222", Some("198.51.100.2:40000")),
            obs("[2001:db8::7]:1111", Some("[2001:db8:f::1]:50000")),
            obs("[2001:db8::7]:2222", Some("[2001:db8:f::1]:50099")),
        ]);
        assert_eq!(v4, Some(false));
        assert_eq!(v6, Some(true));
    }

    /// End to end over loopback: two helpers on one host, probed from one
    /// socket. No NAT sits in between, so the mapping must be stable and
    /// the verdict `Some(false)`, which is exactly the input that lets
    /// `classify_nat_type` return `Easy`.
    #[tokio::test]
    async fn loopback_helpers_report_stable_mapping() {
        let helper_a = QadHelper::spawn("127.0.0.1:0".parse().unwrap()).unwrap();
        let helper_b = QadHelper::spawn("127.0.0.1:0".parse().unwrap()).unwrap();
        // Same "host" (127.0.0.1), two different ports.
        let targets = [helper_a.local_addr(), helper_b.local_addr()];
        assert_ne!(targets[0].port(), targets[1].port());

        let report = probe_port_variation(&targets).await.unwrap();

        let observed: Vec<Option<SocketAddr>> =
            report.observations.iter().map(|o| o.observed).collect();
        assert!(
            observed.iter().all(Option::is_some),
            "all probes should succeed over loopback: {:?}",
            report.observations
        );
        assert_eq!(observed[0], observed[1], "no NAT, mapping must be stable");
        assert_eq!(report.varies_ipv4, Some(false));
        assert_eq!(report.varies_ipv6, None);

        helper_a.shutdown().await;
        helper_b.shutdown().await;
    }
}
