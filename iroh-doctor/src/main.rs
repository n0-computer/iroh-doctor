use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::Result;
use clap::Parser;
use iroh_doctor::{config::NodeConfig, doctor::Commands};

/// iroh-doctor is a tool for diagnosing network issues with iroh-net.
///
/// For more information, visit: <https://iroh.computer/docs>.
#[derive(Parser, Debug, Clone)]
#[clap(version, verbatim_doc_comment)]
pub(crate) struct Cli {
    #[clap(subcommand)]
    pub(crate) command: Commands,

    /// Path to the configuration file, see https://iroh.computer/docs/reference/config.
    #[clap(long)]
    pub(crate) config: Option<PathBuf>,

    /// Address to serve metrics on. Disabled by default.
    #[clap(long)]
    pub(crate) metrics_addr: Option<SocketAddr>,

    /// Write metrics in CSV format at 100ms intervals. Disabled by default.
    #[clap(long)]
    pub(crate) metrics_dump_path: Option<PathBuf>,
}

fn main() -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .thread_name("main-runtime")
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.block_on(main_impl())?;
    // give the runtime some time to finish, but do not wait indefinitely.
    // there are cases where the a runtime thread is blocked doing io.
    // e.g. reading from stdin.
    rt.shutdown_timeout(Duration::from_millis(500));
    Ok(())
}

async fn main_impl() -> Result<()> {
    let cli = Cli::parse();
    iroh_metrics::core::Core::try_init(|reg, metrics| {
        use iroh_metrics::core::Metric;
        metrics.insert(iroh::metrics::MagicsockMetrics::new(reg));
        metrics.insert(iroh::metrics::NetReportMetrics::new(reg));
        metrics.insert(iroh::metrics::PortmapMetrics::new(reg));
    })
    .expect("should be first init");
    let config = NodeConfig::load(cli.config.as_deref()).await?;
    iroh_doctor::doctor::run(cli.command, &config).await
}
