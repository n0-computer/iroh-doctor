//! The Diagnostics tab: the local network environment, independent of any
//! peer. Renders the net_report/NAT summary, the direct port-map probe, the
//! per-relay latency panel, and the iroh-services diagnostics.

use std::time::Duration;

use dioxus::prelude::*;

use iroh_doctor_core::report::RelayLatencyRow;

use crate::identity;
use crate::node::{DiagnosticsReport, NetReportSummary, NodeCommand, TelemetryState};
use crate::portmap_probe::PortMapProbeResult;
use crate::telemetry_pref;
use crate::NodeHandle;

use super::diag_state::{
    trigger_net_diagnostics, trigger_pings, trigger_probe_net_report, trigger_probe_portmap,
    trigger_probe_relays, DiagState,
};

/// The network-environment report: a local net_report with a NAT
/// classification, the direct port-map probe, per-relay latency, and the
/// iroh-services diagnostics. These probe the local endpoint, the relays, and
/// iroh-services rather than the connected peer, so they stay available even
/// when disconnected (the same picture `iroh-doctor diagnostics` prints).
#[component]
pub fn DiagnosticsView(
    cmd_handle: Signal<Option<NodeHandle>>,
    telemetry: Signal<TelemetryState>,
    services_state: Signal<DiagState<Duration>>,
    net_state: Signal<DiagState<DiagnosticsReport>>,
    net_report_state: Signal<DiagState<NetReportSummary>>,
    relays_state: Signal<DiagState<Vec<RelayLatencyRow>>>,
    portmap_state: Signal<DiagState<PortMapProbeResult>>,
) -> Element {
    let busy = matches!(services_state(), DiagState::Running)
        || matches!(net_state(), DiagState::Running)
        || matches!(net_report_state(), DiagState::Running)
        || matches!(relays_state(), DiagState::Running)
        || matches!(portmap_state(), DiagState::Running);

    rsx! {
        div { class: "diagnostics",
            section { class: "settings-section",
                div { class: "section-head",
                    label { class: "label", "Local network report" }
                    button {
                        class: "btn btn-primary",
                        disabled: busy,
                        onclick: move |_| {
                            trigger_pings(cmd_handle, services_state);
                            trigger_net_diagnostics(cmd_handle, net_state);
                            trigger_probe_net_report(cmd_handle, net_report_state);
                            trigger_probe_relays(cmd_handle, relays_state);
                            trigger_probe_portmap(cmd_handle, portmap_state);
                        },
                        "Refresh"
                    }
                }
                dl { class: "diag-table",
                    {render_net_report_rows(&net_report_state())}
                }
            }

            section { class: "settings-section",
                label { class: "label", "Direct port-map probe" }
                dl { class: "diag-table",
                    {render_portmap_rows(&portmap_state())}
                }
            }

            RelayLatencyPanel { relays_state }

            section { class: "settings-section",
                label { class: "label", "Services diagnostics" }
                dl { class: "diag-table",
                    dt { "ping" }
                    dd { {render_rtt(&services_state())} }
                    {render_net_rows(&net_state())}
                }
            }

            IrohServicesSection { cmd_handle, telemetry }
        }
    }
}

#[component]
fn RelayLatencyPanel(relays_state: Signal<DiagState<Vec<RelayLatencyRow>>>) -> Element {
    let state = relays_state();
    rsx! {
        section { class: "settings-section",
            label { class: "label", "Relay latency" }
            {render_relay_rows(&state)}
        }
    }
}

#[component]
fn IrohServicesSection(
    cmd_handle: Signal<Option<NodeHandle>>,
    telemetry: Signal<TelemetryState>,
) -> Element {
    let initial = identity::load_api_secret_override();
    let api_secret_input = use_signal(|| initial.clone());
    let saved_override = use_signal(|| initial);
    // Telemetry is on by default; the marker's presence means the user opted
    // out. Track the positive sense (enabled) for the toggle.
    let enabled = use_signal(|| !telemetry_pref::telemetry_disabled());

    let telemetry_text = telemetry_line(&telemetry());

    let key_footer = if saved_override().is_empty() {
        "Optional. Paste a key from services.iroh.computer to send diagnostics to your own account instead of the default. The key is stored locally on this device only."
    } else {
        "Using your key. Tap Clear to fall back to the default."
    };

    let on = enabled();
    let input_value = api_secret_input();
    let trimmed_input = input_value.trim().to_string();
    // The custom key only takes effect while telemetry is on, so the advanced
    // controls are disabled when it is off rather than silently no-op.
    let save_disabled = !on || trimmed_input.is_empty() || trimmed_input == saved_override();
    let clear_disabled = !on || (saved_override().is_empty() && input_value.is_empty());

    rsx! {
        section { class: "settings-section",
            label { class: "label", "Telemetry" }
            label { class: "toggle-row",
                input {
                    r#type: "checkbox",
                    checked: on,
                    onchange: move |_| {
                        let next = !enabled();
                        // Persist first. If the write fails, leave the signal
                        // unchanged so the checkbox reverts to the durable
                        // state rather than showing a preference we did not
                        // save, and skip the live toggle.
                        if let Err(e) = telemetry_pref::set_telemetry_disabled(!next) {
                            tracing::warn!(err = %e, "persisting telemetry preference");
                            return;
                        }
                        enabled.clone().set(next);
                        if let Some(handle) = cmd_handle.read().clone() {
                            let _ = handle
                                .tx
                                .try_send(NodeCommand::SetTelemetryEnabled { enabled: next });
                        }
                    },
                }
                span { "Send anonymous connection diagnostics" }
            }
            div { class: "footer-note",
                "iroh doctor sends anonymous connection diagnostics to iroh-services "
                "to help improve iroh. Your endpoint id is the only identifier sent. "
                "Turn this off any time."
            }
        }

        section { class: "settings-section",
            label { class: "label", "Advanced: custom iroh services key" }
            input {
                class: "api-input",
                r#type: "password",
                placeholder: "services1...",
                value: "{input_value}",
                disabled: !on,
                autocapitalize: "off",
                autocorrect: "off",
                spellcheck: "false",
                oninput: move |evt| { api_secret_input.clone().set(evt.value()); },
            }
            div { class: "settings-actions",
                button {
                    class: "btn btn-primary",
                    disabled: save_disabled,
                    onclick: move |_| {
                        let value = api_secret_input();
                        let trimmed = value.trim().to_string();
                        if identity::save_api_secret_override(&trimmed).is_ok() {
                            saved_override.clone().set(trimmed.clone());
                        }
                        if let Some(handle) = cmd_handle.read().clone() {
                            let _ = handle.tx.try_send(NodeCommand::SaveApiSecret {
                                secret: trimmed,
                            });
                        }
                    },
                    "Save"
                }
                button {
                    class: "btn",
                    disabled: clear_disabled,
                    onclick: move |_| {
                        api_secret_input.clone().set(String::new());
                        let _ = identity::save_api_secret_override("");
                        saved_override.clone().set(String::new());
                        if let Some(handle) = cmd_handle.read().clone() {
                            let _ = handle.tx.try_send(NodeCommand::SaveApiSecret {
                                secret: String::new(),
                            });
                        }
                    },
                    "Clear"
                }
            }
            div { class: "footer-note", "{key_footer}" }
        }

        section { class: "settings-section",
            label { class: "label", "Telemetry status" }
            div { class: "telemetry-status", "{telemetry_text}" }
        }
    }
}

fn telemetry_line(state: &TelemetryState) -> String {
    match state {
        TelemetryState::Off => "off".into(),
        TelemetryState::Starting => "connecting...".into(),
        TelemetryState::Active { name } => format!("active - pushing as {name}"),
        TelemetryState::Error(msg) => format!("error: {msg}"),
    }
}

fn render_rtt(state: &DiagState<Duration>) -> Element {
    match state {
        DiagState::Idle => rsx! { span { class: "diag-idle", "-" } },
        DiagState::Running => rsx! { span { class: "diag-running", "..." } },
        DiagState::Ok(d) => {
            let ms = d.as_secs_f64() * 1000.0;
            rsx! { span { class: "diag-ok", "{ms:.1} ms" } }
        }
        DiagState::Err(e) => rsx! { span { class: "diag-err", title: "{e}", "-" } },
    }
}

fn render_net_rows(state: &DiagState<DiagnosticsReport>) -> Element {
    match state {
        DiagState::Idle => rsx! {
            dt { "status" }
            dd { class: "diag-idle", "not run yet" }
        },
        DiagState::Running => rsx! {
            dt { "status" }
            dd { class: "diag-running", "running diagnostics..." }
        },
        DiagState::Err(e) => rsx! {
            dt { "status" }
            dd { class: "diag-err", "error: {e}" }
        },
        DiagState::Ok(r) => rsx! {
            // Endpoint id is already shown in the page header, so it is
            // not repeated here.
            dt { "direct addrs" }
            dd {
                if r.direct_addrs.is_empty() {
                    "(none)"
                } else {
                    ul { class: "mono addr-list",
                        for addr in r.direct_addrs.iter() {
                            li { "{addr}" }
                        }
                    }
                }
            }

            dt { "iroh version" }
            dd { "{r.iroh_version}" }

            dt { "iroh-services version" }
            dd { "{r.iroh_services_version}" }

            dt { "net report" }
            dd { if r.has_net_report { "available" } else { "not available" } }

            dt { "UPnP" }
            dd { "{tribool(r.upnp)}" }

            dt { "PCP" }
            dd { "{tribool(r.pcp)}" }

            dt { "NAT-PMP" }
            dd { "{tribool(r.nat_pmp)}" }
        },
    }
}

fn tribool(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "yes",
        Some(false) => "no",
        None => "(not probed)",
    }
}

fn render_net_report_rows(state: &DiagState<NetReportSummary>) -> Element {
    match state {
        DiagState::Idle => rsx! {
            dt { "status" }
            dd { class: "diag-idle", "not run yet" }
        },
        DiagState::Running => rsx! {
            dt { "status" }
            dd { class: "diag-running", "probing..." }
        },
        DiagState::Err(e) => rsx! {
            dt { "status" }
            dd { class: "diag-err", "error: {e}" }
        },
        DiagState::Ok(r) => {
            let nat_class = format!("nat-pill nat-{}", nat_kind_class(r.nat));
            let nat_label = r.nat.to_string();
            let nat_desc = r.nat.description();
            let global_v4 = r
                .global_v4
                .clone()
                .unwrap_or_else(|| "(not observed)".into());
            let global_v6 = r
                .global_v6
                .clone()
                .unwrap_or_else(|| "(not observed)".into());
            let preferred = r.preferred_relay.clone().unwrap_or_else(|| "(none)".into());
            rsx! {
                dt { "NAT type" }
                dd {
                    span { class: "{nat_class}", "{nat_label}" }
                    span { class: "nat-desc", " - {nat_desc}" }
                }
                dt { "UDP IPv4" }
                dd { if r.udp_v4 { "reachable" } else { "not reachable" } }
                dt { "UDP IPv6" }
                dd { if r.udp_v6 { "reachable" } else { "not reachable" } }
                dt { "global IPv4" }
                dd { class: "mono", "{global_v4}" }
                dt { "global IPv6" }
                dd { class: "mono", "{global_v6}" }
                dt { "mapping varies (IPv4)" }
                dd { "{tribool(r.mapping_varies_v4)}" }
                dt { "mapping varies (IPv6)" }
                dd { "{tribool(r.mapping_varies_v6)}" }
                dt { "captive portal" }
                dd { "{tribool(r.captive_portal)}" }
                dt { "preferred relay" }
                dd { class: "mono", "{preferred}" }
                dt { "relays seen" }
                dd { "{r.relays_seen}" }
            }
        }
    }
}

fn render_relay_rows(state: &DiagState<Vec<RelayLatencyRow>>) -> Element {
    match state {
        DiagState::Idle => rsx! {
            div { class: "diag-idle", "not probed yet - hit Refresh" }
        },
        DiagState::Running => rsx! {
            div { class: "diag-running", "reading net report..." }
        },
        DiagState::Err(e) => rsx! {
            div { class: "diag-err", "error: {e}" }
        },
        DiagState::Ok(rows) => {
            if rows.is_empty() {
                return rsx! {
                    div { class: "diag-idle", "no relays returned" }
                };
            }
            rsx! {
                table { class: "transports-table",
                    thead {
                        tr {
                            th { "Relay" }
                            th { class: "ping-rtt-col", "Latency" }
                        }
                    }
                    tbody {
                        for r in rows.iter() {
                            tr {
                                td { class: "mono transports-addr", title: "{r.url}", "{r.url}" }
                                td { class: "ping-rtt-col mono",
                                    span { class: "diag-ok", "{r.latency_ms:.1} ms" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn render_portmap_rows(state: &DiagState<PortMapProbeResult>) -> Element {
    match state {
        DiagState::Idle => rsx! {
            dt { "status" }
            dd { class: "diag-idle", "not probed yet" }
        },
        DiagState::Running => rsx! {
            dt { "status" }
            dd { class: "diag-running", "probing..." }
        },
        DiagState::Err(e) => rsx! {
            dt { "status" }
            dd { class: "diag-err", "error: {e}" }
        },
        DiagState::Ok(r) => {
            let mut rows = rsx! {
                dt { "UPnP" }
                dd { "{tribool(r.upnp)}" }
                dt { "PCP" }
                dd { "{tribool(r.pcp)}" }
                dt { "NAT-PMP" }
                dd { "{tribool(r.nat_pmp)}" }
            };
            if let Some(err) = &r.error {
                rows = rsx! {
                    {rows}
                    dt { "warning" }
                    dd { class: "diag-err", "{err}" }
                };
            }
            rows
        }
    }
}

fn nat_kind_class(nat: iroh_doctor_core::nat::NatType) -> &'static str {
    use iroh_doctor_core::nat::NatType;
    match nat {
        NatType::Easy => "easy",
        NatType::Hard => "hard",
        NatType::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tribool_handles_each_case() {
        assert_eq!(tribool(Some(true)), "yes");
        assert_eq!(tribool(Some(false)), "no");
        assert_eq!(tribool(None), "(not probed)");
    }
}
