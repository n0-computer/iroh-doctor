//! Relay URLs command implementation

use iroh_doctor_core::relay_probe::{RelayProbeResult, RelayProber};

use crate::config::NodeConfig;

/// Checks a certain amount (`count`) of the nodes given by the [`NodeConfig`].
pub async fn relay_urls(count: usize, config: &NodeConfig) -> anyhow::Result<()> {
    if config.relay_nodes.is_empty() {
        println!("No relay nodes specified in the config file.");
    }

    let prober = RelayProber::new().map_err(|e| anyhow::anyhow!("build relay prober: {e}"))?;

    let mut success = Vec::new();
    let mut fail = Vec::new();

    for i in 0..count {
        println!("Round {}/{count}", i + 1);
        for node in &config.relay_nodes {
            let result = prober.probe(&node.url).await;
            if result.error.is_none() {
                success.push(result);
            } else {
                fail.push(result);
            }
        }
    }

    if !success.is_empty() {
        println!("Relay Node Latencies:");
        println!();
    }
    for node in success {
        print_result(&node);
        println!();
    }
    if !fail.is_empty() {
        println!("Connection Failures:");
        println!();
    }
    for node in fail {
        print_result(&node);
        println!();
    }

    Ok(())
}

fn print_result(r: &RelayProbeResult) {
    match &r.error {
        None => println!(
            "Node {}\nConnect: {}\nLatency: {}",
            r.url,
            fmt_ms(r.connect_ms),
            fmt_ms(r.ping_ms)
        ),
        Some(err) => println!("Node {}\nConnection Error: {err:?}", r.url),
    }
}

fn fmt_ms(ms: Option<f64>) -> String {
    ms.map_or_else(|| "-".to_string(), |v| format!("{v:.1}ms"))
}
