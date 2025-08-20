//! Report command implementation

use futures_lite::StreamExt;
use iroh::{Endpoint, RelayMap, RelayMode};
use n0_watcher::Watcher;

use crate::config::NodeConfig;

/// Prints a client report.
pub async fn report(
    config: &NodeConfig,
    mut quic_ipv4: bool,
    mut quic_ipv6: bool,
    mut https: bool,
) -> anyhow::Result<()> {
    // if all protocol flags are false, set them all to true
    if !(quic_ipv4 || quic_ipv6 || https) {
        quic_ipv4 = true;
        quic_ipv6 = true;
        https = true;
    }
    println!("Probe protocols selected:");
    if quic_ipv4 {
        println!("quic ipv4")
    }
    if quic_ipv6 {
        println!("quic ipv6")
    }
    if https {
        println!("https")
    }
    let relay_map = config.relay_map()?.unwrap_or_else(RelayMap::empty);

    let endpoint = Endpoint::builder()
        .relay_mode(RelayMode::Custom(relay_map.clone()))
        .bind()
        .await?;

    println!("\nRelay Map:");
    for (url, node) in relay_map.urls().zip(relay_map.nodes()) {
        println!(
            r#"- {url}
  QUIC port: {:?}"#,
            node.quic.as_ref().map(|c| c.port),
        );
    }

    let mut stream = endpoint.net_report().stream();
    while let Some(report) = stream.next().await {
        println!("{report:#?}");
    }

    endpoint.close().await;
    Ok(())
}
