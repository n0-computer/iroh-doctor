use std::collections::{HashSet, VecDeque};

use dioxus::prelude::*;
use tokio::sync::{mpsc, oneshot};

use super::{short_id, AppError};
use crate::node::{looks_like_endpoint_id, ConnectionState, GossipEvent, NodeCommand};
use crate::NodeHandle;

const MESSAGE_LOG_LEN: usize = 200;
const EVENTS_CAPACITY: usize = 256;

#[derive(Clone)]
struct MessageEntry {
    from: String,
    body: String,
    elapsed_label: String,
}

#[component]
pub fn GossipView(
    cmd_handle: Signal<Option<NodeHandle>>,
    conn_state: Signal<ConnectionState>,
) -> Element {
    let topic_input = use_signal(String::new);
    let bootstrap_input = use_signal(String::new);
    let compose_input = use_signal(String::new);
    let joined_topic: Signal<Option<String>> = use_signal(|| None);
    let neighbors: Signal<HashSet<String>> = use_signal(HashSet::new);
    let messages: Signal<VecDeque<MessageEntry>> =
        use_signal(|| VecDeque::with_capacity(MESSAGE_LOG_LEN));
    let joining = use_signal(|| false);
    let sending = use_signal(|| false);
    let last_error: Signal<Option<String>> = use_signal(|| None);

    // Mirror gossip errors into the global modal.
    let mut app_error = use_context::<Signal<Option<AppError>>>();
    use_effect(move || {
        if let Some(msg) = last_error() {
            app_error.set(Some(AppError::new("gossip", msg)));
        }
    });

    rsx! {
        div { class: "gossip-view",
            JoinSection {
                cmd_handle, conn_state, topic_input, bootstrap_input,
                joined_topic, neighbors, messages,
                joining, last_error,
            }
            NeighborsList { neighbors, joined_topic }
            MessagesList { messages }
            ComposeSection {
                cmd_handle, joined_topic, compose_input, sending, last_error,
            }
            if let Some(err) = last_error.read().as_ref() {
                section { class: "settings-section",
                    div { class: "diag-err", "{err}" }
                }
            }
        }
    }
}

#[component]
fn JoinSection(
    cmd_handle: Signal<Option<NodeHandle>>,
    conn_state: Signal<ConnectionState>,
    mut topic_input: Signal<String>,
    mut bootstrap_input: Signal<String>,
    mut joined_topic: Signal<Option<String>>,
    mut neighbors: Signal<HashSet<String>>,
    mut messages: Signal<VecDeque<MessageEntry>>,
    mut joining: Signal<bool>,
    mut last_error: Signal<Option<String>>,
) -> Element {
    let busy = joining();
    let bootstrap_text = bootstrap_input();
    let connected_peer = connected_peer_id(&conn_state.read());
    let bootstrap_status = bootstrap_status_line(&bootstrap_text, connected_peer.as_deref());

    rsx! {
        section { class: "settings-section",
            label { class: "label", "Topic" }
            input {
                class: "api-input",
                r#type: "text",
                placeholder: "topic id (64 hex) or any friendly string",
                value: "{topic_input}",
                autocapitalize: "off",
                autocorrect: "off",
                spellcheck: "false",
                oninput: move |evt| topic_input.set(evt.value()),
            }
            label { class: "label", "Bootstrap peers (one per line)" }
            textarea {
                class: "api-input gossip-bootstrap",
                rows: "4",
                placeholder: "endpoint ids, one per line",
                value: "{bootstrap_input}",
                oninput: move |evt| bootstrap_input.set(evt.value()),
            }
            div { class: "label gossip-bootstrap-status",
                "{bootstrap_status}"
            }
            div { class: "settings-actions",
                button {
                    class: "btn btn-primary",
                    disabled: busy,
                    onclick: move |_| {
                        let Some(handle) = cmd_handle.read().clone() else { return };
                        last_error.set(None);
                        joining.set(true);
                        neighbors.write().clear();
                        messages.write().clear();
                        joined_topic.set(None);

                        let topic_str = topic_input();
                        let mut bootstrap: Vec<String> = bootstrap_input()
                            .lines()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        if let Some(peer) = connected_peer_id(&conn_state.read()) {
                            if !bootstrap.iter().any(|b| b.eq_ignore_ascii_case(&peer)) {
                                bootstrap.insert(0, peer);
                            }
                        }

                        let (events_tx, mut events_rx) = mpsc::channel::<GossipEvent>(EVENTS_CAPACITY);
                        let (reply_tx, reply_rx) = oneshot::channel();

                        let send_handle = handle.clone();
                        spawn(async move {
                            let _ = send_handle
                                .tx
                                .send(NodeCommand::JoinGossip {
                                    topic_input: topic_str,
                                    bootstrap,
                                    events_tx,
                                    reply: reply_tx,
                                })
                                .await;
                        });

                        // Reply pump.
                        spawn(async move {
                            match reply_rx.await {
                                Ok(Ok(topic_hash)) => joined_topic.set(Some(topic_hash)),
                                Ok(Err(e)) => last_error.set(Some(e)),
                                Err(_) => last_error.set(Some("join reply dropped".into())),
                            }
                            joining.set(false);
                        });

                        // Events pump. Runs until peer-side sender is dropped
                        // (which happens when the peer aborts the prior recv
                        // task on the next JoinGossip).
                        let started_at = std::time::Instant::now();
                        spawn(async move {
                            while let Some(event) = events_rx.recv().await {
                                apply_event(event, neighbors, messages, started_at);
                            }
                        });
                    },
                    if busy { "Joining..." } else { "Join" }
                }
            }
        }
    }
}

#[component]
fn NeighborsList(
    neighbors: Signal<HashSet<String>>,
    joined_topic: Signal<Option<String>>,
) -> Element {
    let n = neighbors();
    let topic_label = match joined_topic().as_ref() {
        Some(t) => format!("topic {}", short_topic(t)),
        None => "not joined".to_string(),
    };
    rsx! {
        section { class: "settings-section",
            label { class: "label", "Neighbors ({n.len()}) - {topic_label}" }
            if n.is_empty() {
                div { class: "diag-idle", "no neighbors yet" }
            } else {
                ul { class: "neighbor-list mono",
                    for peer in n.iter() {
                        li { title: "{peer}", "{short_topic(peer)}" }
                    }
                }
            }
        }
    }
}

#[component]
fn MessagesList(messages: Signal<VecDeque<MessageEntry>>) -> Element {
    let msgs = messages();
    rsx! {
        section { class: "settings-section",
            label { class: "label", "Messages ({msgs.len()})" }
            if msgs.is_empty() {
                div { class: "diag-idle", "no messages yet" }
            } else {
                ul { class: "gossip-messages",
                    for entry in msgs.iter().rev() {
                        li { class: "gossip-message",
                            span { class: "mono gossip-msg-from", title: "{entry.from}", {short_topic(&entry.from)} }
                            span { class: "mono gossip-msg-time", "{entry.elapsed_label}" }
                            span { class: "gossip-msg-body", "{entry.body}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ComposeSection(
    cmd_handle: Signal<Option<NodeHandle>>,
    joined_topic: Signal<Option<String>>,
    mut compose_input: Signal<String>,
    mut sending: Signal<bool>,
    mut last_error: Signal<Option<String>>,
) -> Element {
    let joined = joined_topic.read().is_some();
    let busy = sending();
    let disabled = !joined || busy;

    rsx! {
        section { class: "settings-section",
            label { class: "label", "Broadcast" }
            input {
                class: "api-input",
                r#type: "text",
                placeholder: if joined { "message to broadcast" } else { "join a topic first" },
                value: "{compose_input}",
                disabled: !joined,
                oninput: move |evt| compose_input.set(evt.value()),
            }
            div { class: "settings-actions",
                button {
                    class: "btn btn-primary",
                    disabled,
                    onclick: move |_| {
                        let Some(handle) = cmd_handle.read().clone() else { return };
                        let msg = compose_input().trim().to_string();
                        if msg.is_empty() {
                            return;
                        }
                        last_error.set(None);
                        sending.set(true);
                        let (tx, rx) = oneshot::channel();
                        spawn(async move {
                            let _ = handle
                                .tx
                                .send(NodeCommand::GossipBroadcast { msg, reply: tx })
                                .await;
                            match rx.await {
                                Ok(Ok(())) => {
                                    compose_input.set(String::new());
                                }
                                Ok(Err(e)) => last_error.set(Some(e)),
                                Err(_) => last_error.set(Some("broadcast reply dropped".into())),
                            }
                            sending.set(false);
                        });
                    },
                    if busy { "Sending..." } else { "Send" }
                }
            }
        }
    }
}

fn apply_event(
    event: GossipEvent,
    mut neighbors: Signal<HashSet<String>>,
    messages: Signal<VecDeque<MessageEntry>>,
    started_at: std::time::Instant,
) {
    match event {
        GossipEvent::NeighborUp { peer } => {
            neighbors.write().insert(peer);
        }
        GossipEvent::NeighborDown { peer } => {
            neighbors.write().remove(&peer);
        }
        GossipEvent::Message { from, body } => {
            let elapsed = started_at.elapsed().as_secs_f64();
            let elapsed_label = if elapsed >= 60.0 {
                format!("{:.0}s", elapsed)
            } else {
                format!("{:.1}s", elapsed)
            };
            push_message(
                messages,
                MessageEntry {
                    from,
                    body,
                    elapsed_label,
                },
            );
        }
        GossipEvent::Lagged => {
            // Surface lag as a system message so the user knows we missed events.
            push_message(
                messages,
                MessageEntry {
                    from: "system".to_string(),
                    body: "lagged: missed some gossip events".to_string(),
                    elapsed_label: "-".to_string(),
                },
            );
        }
    }
}

fn short_topic(s: &str) -> String {
    short_id(s, 6, 4)
}

fn push_message(mut messages: Signal<VecDeque<MessageEntry>>, entry: MessageEntry) {
    let mut log = messages.write();
    if log.len() >= MESSAGE_LOG_LEN {
        log.pop_front();
    }
    log.push_back(entry);
}

/// Returns the connected peer's full endpoint id when one is available,
/// so the gossip join can use it as an automatic bootstrap.
fn connected_peer_id(state: &ConnectionState) -> Option<String> {
    if let ConnectionState::Connected { peer_id, .. } = state {
        Some(peer_id.clone())
    } else {
        None
    }
}

/// Counts how many non-blank lines in `text` parse as endpoint ids, and
/// renders a short status string for the UI ("0 bootstrap peers, 0
/// malformed", "3 bootstrap peers", "2 bootstrap peers, 1 malformed").
/// Joining with zero parses is legal: the topic still works once a
/// neighbor finds us via gossip discovery. When `connected_peer` is
/// `Some`, that id is auto-added by the join handler, so the count is
/// bumped by one (unless the user already listed it).
fn bootstrap_status_line(text: &str, connected_peer: Option<&str>) -> String {
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut connected_listed = false;
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if looks_like_endpoint_id(trimmed) {
            ok += 1;
            if let Some(peer) = connected_peer {
                if trimmed.eq_ignore_ascii_case(peer) {
                    connected_listed = true;
                }
            }
        } else {
            bad += 1;
        }
    }
    let auto = connected_peer.is_some() && !connected_listed;
    if auto {
        ok += 1;
    }
    let base = format!("{ok} bootstrap peer{}", if ok == 1 { "" } else { "s" });
    let mut out = base;
    if auto {
        out.push_str(" (+ connected peer)");
    }
    if bad > 0 {
        out.push_str(&format!(", {bad} malformed"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_status_empty() {
        assert_eq!(bootstrap_status_line("", None), "0 bootstrap peers");
        assert_eq!(
            bootstrap_status_line("\n\n   \n", None),
            "0 bootstrap peers"
        );
    }

    #[test]
    fn bootstrap_status_pluralization() {
        let one = "a".repeat(64);
        assert_eq!(bootstrap_status_line(&one, None), "1 bootstrap peer");
        let two = format!("{one}\n{one}");
        assert_eq!(bootstrap_status_line(&two, None), "2 bootstrap peers");
    }

    #[test]
    fn bootstrap_status_counts_malformed() {
        let one = "a".repeat(64);
        let mixed = format!("{one}\nnot-a-real-id");
        assert_eq!(
            bootstrap_status_line(&mixed, None),
            "1 bootstrap peer, 1 malformed"
        );
    }

    #[test]
    fn bootstrap_status_adds_connected_peer() {
        let peer = "b".repeat(64);
        assert_eq!(
            bootstrap_status_line("", Some(&peer)),
            "1 bootstrap peer (+ connected peer)"
        );
        let one = "a".repeat(64);
        assert_eq!(
            bootstrap_status_line(&one, Some(&peer)),
            "2 bootstrap peers (+ connected peer)"
        );
    }

    #[test]
    fn bootstrap_status_does_not_double_count_listed_peer() {
        let peer = "a".repeat(64);
        // User typed the connected peer in the textarea -> count it once,
        // no "+ connected peer" annotation.
        assert_eq!(
            bootstrap_status_line(&peer, Some(&peer)),
            "1 bootstrap peer"
        );
    }
}
