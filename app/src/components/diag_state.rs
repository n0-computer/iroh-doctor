//! Shared diagnostics plumbing: the [`DiagState`] of each probe and the
//! `trigger_*` dispatchers that fire a [`NodeCommand`] and fold the reply
//! into a state, used by the Diagnostics tab.

use std::time::Duration;

use dioxus::prelude::*;
use tokio::sync::oneshot;

use iroh_doctor_core::report::RelayLatencyRow;

use crate::node::{DiagnosticsReport, NetReportSummary, NodeCommand};
use crate::NodeHandle;

#[derive(Clone)]
pub enum DiagState<T: Clone + 'static> {
    Idle,
    Running,
    Ok(T),
    Err(String),
}

pub fn trigger_pings(
    cmd_handle: Signal<Option<NodeHandle>>,
    services_state: Signal<DiagState<Duration>>,
) {
    trigger(cmd_handle, services_state, |reply| {
        NodeCommand::PingServices { reply }
    });
}

pub fn trigger_net_diagnostics(
    cmd_handle: Signal<Option<NodeHandle>>,
    net_state: Signal<DiagState<DiagnosticsReport>>,
) {
    trigger(cmd_handle, net_state, |reply| {
        NodeCommand::RunNetDiagnostics { reply }
    });
}

pub fn trigger_probe_net_report(
    cmd_handle: Signal<Option<NodeHandle>>,
    net_report_state: Signal<DiagState<NetReportSummary>>,
) {
    trigger(cmd_handle, net_report_state, |reply| {
        NodeCommand::ProbeNetReport { reply }
    });
}

pub fn trigger_probe_relays(
    cmd_handle: Signal<Option<NodeHandle>>,
    relays_state: Signal<DiagState<Vec<RelayLatencyRow>>>,
) {
    trigger(cmd_handle, relays_state, |reply| {
        NodeCommand::ProbeRelayLatencies { reply }
    });
}

/// Marks `state` running, sends the command built by `make_cmd`, and folds
/// the reply (or the failure to deliver it) back into `state`.
fn trigger<T: Clone + 'static>(
    cmd_handle: Signal<Option<NodeHandle>>,
    mut state: Signal<DiagState<T>>,
    make_cmd: impl FnOnce(oneshot::Sender<Result<T, String>>) -> NodeCommand + 'static,
) {
    state.set(DiagState::Running);
    let handle = cmd_handle.read().clone();
    spawn(async move {
        let result = request(handle, make_cmd).await;
        state.set(match result {
            Ok(v) => DiagState::Ok(v),
            Err(e) => DiagState::Err(e),
        });
    });
}

async fn request<T>(
    handle: Option<NodeHandle>,
    make_cmd: impl FnOnce(oneshot::Sender<Result<T, String>>) -> NodeCommand,
) -> Result<T, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx.try_send(make_cmd(tx)).is_err() {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}
