//! Relay URLs command implementation

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use futures_lite::StreamExt;
use futures_util::SinkExt;
use iroh::{dns::DnsResolver, RelayUrl, SecretKey};
use iroh_relay::protos::relay::{ClientToRelayMsg, RelayToClientMsg};

use crate::config::NodeConfig;

/// Checks a certain amount (`count`) of the nodes given by the [`NodeConfig`].
pub async fn relay_urls(count: usize, config: &NodeConfig) -> anyhow::Result<()> {
    let key = SecretKey::generate(rand::rngs::OsRng);
    if config.relay_nodes.is_empty() {
        println!("No relay nodes specified in the config file.");
    }

    let dns_resolver = DnsResolver::new();
    let mut client_builders = HashMap::new();
    for node in &config.relay_nodes {
        let secret_key = key.clone();
        let client_builder = iroh_relay::client::ClientBuilder::new(
            node.url.clone(),
            secret_key,
            dns_resolver.clone(),
        );

        client_builders.insert(node.url.clone(), client_builder);
    }

    let mut success = Vec::new();
    let mut fail = Vec::new();

    for i in 0..count {
        println!("Round {}/{count}", i + 1);
        let relay_nodes = config.relay_nodes.clone();
        for node in relay_nodes.into_iter() {
            let mut node_details = NodeDetails {
                connect: None,
                latency: None,
                error: None,
                host: node.url.clone(),
            };

            let client_builder = client_builders.get(&node.url).cloned().unwrap();

            let start = std::time::Instant::now();
            match tokio::time::timeout(Duration::from_secs(2), client_builder.connect()).await {
                Err(e) => {
                    tracing::warn!("connect timeout");
                    node_details.error = Some(e.to_string());
                }
                Ok(Err(e)) => {
                    tracing::warn!("connect error");
                    node_details.error = Some(e.to_string());
                }
                Ok(Ok(client)) => {
                    node_details.connect = Some(start.elapsed());
                    match ping(client).await {
                        Ok(latency) => {
                            node_details.latency = Some(latency);
                        }
                        Err(e) => {
                            tracing::warn!("ping error: {:?}", e);
                            node_details.error = Some(e.to_string());
                        }
                    }
                }
            }

            if node_details.error.is_none() {
                success.push(node_details);
            } else {
                fail.push(node_details);
            }
        }
    }

    // success.sort_by_key(|d| d.latency);
    if !success.is_empty() {
        println!("Relay Node Latencies:");
        println!();
    }
    for node in success {
        println!("{node}");
        println!();
    }
    if !fail.is_empty() {
        println!("Connection Failures:");
        println!();
    }
    for node in fail {
        println!("{node}");
        println!();
    }

    Ok(())
}

async fn ping(client: iroh_relay::client::Client) -> anyhow::Result<Duration> {
    let (mut client_stream, mut client_sink) = client.split();
    let data: [u8; 8] = rand::random();
    let start = Instant::now();
    client_sink.send(ClientToRelayMsg::Ping(data)).await?;
    match tokio::time::timeout(Duration::from_secs(2), async move {
        while let Some(res) = client_stream.next().await {
            let res = res?;
            if let RelayToClientMsg::Pong(d) = res {
                if d == data {
                    return Ok(start.elapsed());
                }
            }
        }
        anyhow::bail!("no pong received");
    })
    .await
    {
        Err(_) => {
            anyhow::bail!("ping timeout");
        }
        Ok(res) => res,
    }
}

/// Information about a node and its connection.
struct NodeDetails {
    connect: Option<Duration>,
    latency: Option<Duration>,
    host: RelayUrl,
    error: Option<String>,
}

impl std::fmt::Display for NodeDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.error {
            None => {
                write!(
                    f,
                    "Node {}\nConnect: {:?}\nLatency: {:?}",
                    self.host,
                    self.connect.unwrap_or_default(),
                    self.latency.unwrap_or_default(),
                )
            }
            Some(ref err) => {
                write!(f, "Node {}\nConnection Error: {:?}", self.host, err,)
            }
        }
    }
}
