//! Shared diagnostics plumbing: the [`DiagState`] of each probe and the
//! `trigger_*` dispatchers that fire a [`NodeCommand`] and fold the reply
//! into a state, used by the Diagnostics tab.

use std::time::Duration;

use dioxus::prelude::*;
use tokio::sync::oneshot;

use iroh_doctor_core::report::RelayLatencyRow;

use crate::node::{DiagnosticsReport, NetReportSummary, NodeCommand};
use crate::portmap_probe::PortMapProbeResult;
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
    mut services_state: Signal<DiagState<Duration>>,
) {
    services_state.set(DiagState::Running);
    let handle = cmd_handle.read().clone();
    spawn(async move {
        let result = run_ping_services(handle).await;
        services_state.set(into_state(result));
    });
}

pub fn trigger_net_diagnostics(
    cmd_handle: Signal<Option<NodeHandle>>,
    mut net_state: Signal<DiagState<DiagnosticsReport>>,
) {
    net_state.set(DiagState::Running);
    let handle = cmd_handle.read().clone();
    spawn(async move {
        let result = run_net(handle).await;
        net_state.set(into_state(result));
    });
}

pub fn trigger_probe_net_report(
    cmd_handle: Signal<Option<NodeHandle>>,
    mut net_report_state: Signal<DiagState<NetReportSummary>>,
) {
    net_report_state.set(DiagState::Running);
    let handle = cmd_handle.read().clone();
    spawn(async move {
        let result = run_probe_net_report(handle).await;
        net_report_state.set(into_state(result));
    });
}

pub fn trigger_probe_relays(
    cmd_handle: Signal<Option<NodeHandle>>,
    mut relays_state: Signal<DiagState<Vec<RelayLatencyRow>>>,
) {
    relays_state.set(DiagState::Running);
    let handle = cmd_handle.read().clone();
    spawn(async move {
        let result = run_probe_relays(handle).await;
        relays_state.set(into_state(result));
    });
}

pub fn trigger_probe_portmap(
    cmd_handle: Signal<Option<NodeHandle>>,
    mut portmap_state: Signal<DiagState<PortMapProbeResult>>,
) {
    portmap_state.set(DiagState::Running);
    let handle = cmd_handle.read().clone();
    spawn(async move {
        let result = run_probe_portmap(handle).await;
        portmap_state.set(into_state(result));
    });
}

fn into_state<T: Clone + 'static>(r: Result<T, String>) -> DiagState<T> {
    match r {
        Ok(v) => DiagState::Ok(v),
        Err(e) => DiagState::Err(e),
    }
}

async fn run_ping_services(handle: Option<NodeHandle>) -> Result<Duration, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(NodeCommand::PingServices { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

async fn run_net(handle: Option<NodeHandle>) -> Result<DiagnosticsReport, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(NodeCommand::RunNetDiagnostics { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

async fn run_probe_net_report(handle: Option<NodeHandle>) -> Result<NetReportSummary, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(NodeCommand::ProbeNetReport { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

async fn run_probe_relays(handle: Option<NodeHandle>) -> Result<Vec<RelayLatencyRow>, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(NodeCommand::ProbeRelayLatencies { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

async fn run_probe_portmap(handle: Option<NodeHandle>) -> Result<PortMapProbeResult, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(NodeCommand::ProbePortMap { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}
