//! The Connect tab: the device's own endpoint id, the connect/disconnect
//! bar, and the live connection view.

use std::collections::VecDeque;
use std::str::FromStr;
use std::time::Duration;

use dioxus::prelude::*;
use iroh::EndpointId;

use crate::clipboard::{copy_to_clipboard, read_clipboard};
use crate::node::{ConnectionState, NodeCommand, PathSnapshot, ThroughputSnapshot};
use crate::NodeHandle;

use super::status::status_line;
use super::{ConnectView, EventEntry, FirstRunNote};

#[component]
pub fn ConnectPage(
    endpoint_id: Signal<String>,
    conn_state: Signal<ConnectionState>,
    cmd_handle: Signal<Option<NodeHandle>>,
    peer_id_input: Signal<String>,
    paths: Signal<Vec<PathSnapshot>>,
    rtt_history: Signal<VecDeque<f64>>,
    event_log: Signal<VecDeque<EventEntry>>,
    ttfdb: Signal<Option<Duration>>,
    throughput: Signal<Option<ThroughputSnapshot>>,
) -> Element {
    rsx! {
        div { class: "page",
            h2 { class: "page-title", "Connect" }
            FirstRunNote {}
            Header { endpoint_id }
            ConnectBar { cmd_handle, peer_id_input, conn_state }
            ConnectView {
                conn_state,
                paths,
                rtt_history,
                event_log,
                ttfdb,
                throughput,
            }
        }
    }
}

/// The device's own endpoint id with a Copy button.
#[component]
fn Header(endpoint_id: Signal<String>) -> Element {
    let id = endpoint_id();
    let display = if id.is_empty() {
        "...".to_string()
    } else {
        id.clone()
    };
    let copy_disabled = id.is_empty();

    rsx! {
        div { class: "header",
            span { class: "label", "My id:" }
            span { class: "endpoint-id", title: "{id}", "{display}" }
            button {
                class: "btn",
                disabled: copy_disabled,
                onclick: move |_| {
                    let id = endpoint_id();
                    if !id.is_empty() {
                        copy_to_clipboard(&id);
                    }
                },
                "Copy"
            }
        }
    }
}

#[component]
fn ConnectBar(
    cmd_handle: Signal<Option<NodeHandle>>,
    peer_id_input: Signal<String>,
    conn_state: Signal<ConnectionState>,
) -> Element {
    // Connect and disconnect are distinct steps. Once a session is dialing or
    // live, the input gives way to a single Disconnect button (Cancel while a
    // dial is still in flight); otherwise we show the input and Connect.
    let state = conn_state();
    let connecting = matches!(state, ConnectionState::Connecting);
    let active = connecting || matches!(state, ConnectionState::Connected { .. });

    if active {
        let label = if connecting { "Cancel" } else { "Disconnect" };
        return rsx! {
            div { class: "connect-bar",
                span { class: "connect-status", "{status_line(&state)}" }
                button {
                    class: "btn btn-danger",
                    onclick: move |_| {
                        if let Some(handle) = cmd_handle.read().clone() {
                            let _ = handle.try_send(NodeCommand::Disconnect);
                        }
                    },
                    "{label}"
                }
            }
        };
    }

    let input_value = peer_id_input();
    // Validate with the same parse the node runs on Connect, so the button
    // enables exactly when the dial can actually start.
    let connect_disabled = EndpointId::from_str(input_value.trim()).is_err();
    // The Connect button stays disabled on malformed input; without a
    // hint a first-time user pasting a truncated id only sees a button
    // that will not press. Explain what a valid id looks like.
    let show_invalid_hint = connect_disabled && !input_value.trim().is_empty();

    rsx! {
        div { class: "connect-bar-wrap",
            div { class: "connect-bar",
                input {
                    class: "peer-id-input",
                    r#type: "text",
                    placeholder: "Peer endpoint id",
                    value: "{input_value}",
                    autocapitalize: "off",
                    autocorrect: "off",
                    autocomplete: "off",
                    spellcheck: "false",
                    oninput: move |evt| { peer_id_input.clone().set(evt.value()); },
                }
                button {
                    class: "btn",
                    onclick: move |_| {
                        let mut peer = peer_id_input;
                        spawn(async move {
                            if let Some(text) = read_clipboard().await {
                                let text = text.trim();
                                if !text.is_empty() {
                                    peer.set(text.to_string());
                                }
                            }
                        });
                    },
                    "Paste"
                }
                button {
                    class: "btn btn-primary",
                    disabled: connect_disabled,
                    onclick: move |_| {
                        let id = peer_id_input();
                        if let Some(handle) = cmd_handle.read().clone() {
                            let _ = handle.try_send(NodeCommand::Connect { hex_id: id });
                        }
                    },
                    "Connect"
                }
            }
            if show_invalid_hint {
                div { class: "input-hint",
                    "Not a valid endpoint id yet: ids are 64 hex characters. "
                    "Paste the full id from the other device's Copy button."
                }
            }
        }
    }
}
