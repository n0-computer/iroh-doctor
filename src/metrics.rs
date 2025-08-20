use std::sync::{Arc, RwLock};

use iroh_metrics::Registry;

/// The registry to collect endpoint metrics for spawned nodes.
pub type IrohMetricsRegistry = Arc<RwLock<Registry>>;

#[derive(Default, Clone, Debug)]
pub struct MetricsRegistry {
    pub iroh: IrohMetricsRegistry,
    pub(crate) prometheus: Arc<RwLock<prometheus_client::registry::Registry>>,
}

impl iroh_metrics::MetricsSource for MetricsRegistry {
    fn encode_openmetrics(
        &self,
        writer: &mut impl std::fmt::Write,
    ) -> std::result::Result<(), iroh_metrics::Error> {
        prometheus_client::encoding::text::encode_registry(
            writer,
            &self.prometheus.read().expect("poisoned"),
        )?;
        self.iroh
            .read()
            .expect("poisoned")
            .encode_openmetrics(writer)?;
        Ok(())
    }
}
