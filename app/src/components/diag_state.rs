//! Shared diagnostics plumbing: the [`DiagState`] of each probe and the
//! `trigger_*` dispatchers that fire a [`PeerCommand`] and fold the reply
//! into a state, used by the Diagnostics tab.

use std::time::Duration;

use dioxus::prelude::*;
use tokio::sync::oneshot;

use crate::peer::{DiagnosticsReport, NetReportSummary, PeerCommand};
use crate::portmap_probe::PortMapProbeResult;
use crate::relay_probe::RelayProbeResult;
use crate::PeerHandle;

#[derive(Clone)]
pub enum DiagState<T: Clone + 'static> {
    Idle,
    Running,
    Ok(T),
    Err(String),
}

pub fn trigger_pings(
    cmd_handle: Signal<Option<PeerHandle>>,
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
    cmd_handle: Signal<Option<PeerHandle>>,
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
    cmd_handle: Signal<Option<PeerHandle>>,
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
    cmd_handle: Signal<Option<PeerHandle>>,
    mut relays_state: Signal<DiagState<Vec<RelayProbeResult>>>,
) {
    relays_state.set(DiagState::Running);
    let handle = cmd_handle.read().clone();
    spawn(async move {
        let result = run_probe_relays(handle).await;
        relays_state.set(into_state(result));
    });
}

pub fn trigger_probe_portmap(
    cmd_handle: Signal<Option<PeerHandle>>,
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

async fn run_ping_services(handle: Option<PeerHandle>) -> Result<Duration, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(PeerCommand::PingServices { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

async fn run_net(handle: Option<PeerHandle>) -> Result<DiagnosticsReport, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(PeerCommand::RunNetDiagnostics { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

async fn run_probe_net_report(handle: Option<PeerHandle>) -> Result<NetReportSummary, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(PeerCommand::ProbeNetReport { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

async fn run_probe_relays(handle: Option<PeerHandle>) -> Result<Vec<RelayProbeResult>, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(PeerCommand::ProbeRelayLatencies { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

async fn run_probe_portmap(handle: Option<PeerHandle>) -> Result<PortMapProbeResult, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(PeerCommand::ProbePortMap { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}
