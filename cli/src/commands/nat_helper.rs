//! `iroh-doctor nat-helper` command.
//!
//! Runs two QUIC address discovery (QAD) servers on one machine so a peer
//! behind a NAT can measure whether its mapping varies by destination port,
//! the input that lets the NAT classifier return `Easy`. Run this on a
//! machine the diagnosing side can reach directly (public IP or same
//! network), then run `iroh-doctor diagnostics --nat-probe` over there.

use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, Result};
use iroh_doctor_core::port_variation::QadHelper;

/// Binds the two helpers and serves until ctrl-c.
pub async fn nat_helper(bind: IpAddr, ports: (u16, u16)) -> Result<()> {
    let helper_a = QadHelper::spawn(SocketAddr::new(bind, ports.0)).context("first helper")?;
    let helper_b = QadHelper::spawn(SocketAddr::new(bind, ports.1)).context("second helper")?;

    let port_a = helper_a.local_addr().port();
    let port_b = helper_b.local_addr().port();
    println!("NAT helper listening on {bind} ports {port_a} and {port_b} (UDP)");
    println!();
    println!("On the machine under test, run:");
    println!();
    println!("  iroh-doctor diagnostics --nat-probe <this-host>:{port_a},<this-host>:{port_b}");
    println!();
    println!("where <this-host> is an address this machine is reachable at.");
    println!("Press ctrl-c to stop.");

    tokio::signal::ctrl_c()
        .await
        .context("waiting for ctrl-c")?;
    helper_a.shutdown().await;
    helper_b.shutdown().await;
    Ok(())
}
