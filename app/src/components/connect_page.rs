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

/// The device's own endpoint id, with buttons to copy it or show it as a QR
/// code. The QR encodes an `irohdoctor://connect` deep link, so another device
/// can scan it with the system camera and open straight into a connect instead
/// of copy-pasting the id.
#[component]
fn Header(endpoint_id: Signal<String>) -> Element {
    let show_qr = use_signal(|| false);

    let id = endpoint_id();
    let display = if id.is_empty() {
        "...".to_string()
    } else {
        id.clone()
    };
    let controls_disabled = id.is_empty();

    // Build the QR only while shown, and re-derive it from the current id every
    // render: it is a deep link to this id, so a stale code must never linger
    // past an id change.
    let qr = (show_qr() && !id.is_empty())
        .then(|| render_qr_svg(&crate::deeplink::connect_url(&id)))
        .flatten();

    rsx! {
        div { class: "header",
            span { class: "label", "My id:" }
            span { class: "endpoint-id", title: "{id}", "{display}" }
            button {
                class: "btn",
                disabled: controls_disabled,
                onclick: move |_| {
                    let next = !show_qr();
                    show_qr.clone().set(next);
                },
                if show_qr() { "Hide QR" } else { "QR" }
            }
            button {
                class: "btn",
                disabled: controls_disabled,
                onclick: move |_| {
                    let id = endpoint_id();
                    if !id.is_empty() {
                        copy_to_clipboard(&id);
                    }
                },
                "Copy"
            }
        }
        {qr.map(|svg| rsx! {
            div { class: "qr-panel", dangerous_inner_html: "{svg}" }
        })}
    }
}

/// Renders `data` as an SVG QR code document. Returns `None` only when `data`
/// exceeds QR capacity, which an endpoint-id deep link never does; the encoder
/// is fallible, so the caller degrades to showing nothing rather than panicking.
fn render_qr_svg(data: &str) -> Option<String> {
    use qrcode::render::svg;
    use qrcode::QrCode;

    let code = QrCode::new(data).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(200, 200)
            .dark_color(svg::Color("#000000"))
            .light_color(svg::Color("#ffffff"))
            .build(),
    )
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
    // Accept either a bare endpoint id or a full irohdoctor://connect deep link,
    // so an id scanned as plain text by a generic QR reader and pasted in still
    // connects. Validate the resolved id with the same parse the node runs on
    // Connect, so the button enables exactly when the dial can actually start.
    let resolved = crate::deeplink::resolve_peer_id(&input_value);
    let connect_disabled = EndpointId::from_str(&resolved).is_err();
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
                        let hex_id = crate::deeplink::resolve_peer_id(&peer_id_input());
                        if let Some(handle) = cmd_handle.read().clone() {
                            let _ = handle.try_send(NodeCommand::Connect { hex_id });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_qr_svg_encodes_a_connect_deep_link() {
        // A full endpoint-id deep link is the longest payload the QR carries;
        // it must still fit and render a real SVG document.
        let url = crate::deeplink::connect_url(&"a".repeat(64));
        let svg = render_qr_svg(&url).expect("an endpoint id deep link fits in a QR code");
        assert!(svg.contains("<svg"));
    }
}
