use std::collections::VecDeque;

use dioxus::prelude::*;
use tokio::sync::{mpsc, oneshot};

use super::short_id;
use crate::peer::{DocEntrySummary, DocEventUi, PeerCommand};
use crate::PeerHandle;

const DOC_EVENT_LOG_LEN: usize = 200;
const EVENTS_CAPACITY: usize = 256;

/// One row in the live-events log shown on the Docs view. Public so
/// `main.rs` can construct the persistent ring buffer that survives
/// tab switches.
#[derive(Clone)]
pub struct EventRow {
    pub kind: &'static str,
    pub label: String,
}

/// Maximum number of entries the live-events log retains. Public so
/// `main.rs` can pre-allocate the persistent ring buffer.
pub const EVENT_LOG_CAPACITY: usize = DOC_EVENT_LOG_LEN;

#[component]
pub fn DocsView(
    cmd_handle: Signal<Option<PeerHandle>>,
    active_doc: Signal<Option<String>>,
    entries: Signal<Vec<DocEntrySummary>>,
    events: Signal<VecDeque<EventRow>>,
    last_error: Signal<Option<String>>,
    listing: Signal<bool>,
) -> Element {
    let ticket_input = use_signal(String::new);
    let key_input = use_signal(String::new);
    let value_input = use_signal(String::new);
    let share_output: Signal<Option<String>> = use_signal(|| None);
    let importing = use_signal(|| false);
    let sharing = use_signal(|| false);
    let setting = use_signal(|| false);

    rsx! {
        div { class: "docs-view",
            ImportSection {
                cmd_handle, active_doc, ticket_input,
                share_output, events, importing, last_error,
                entries, listing,
            }
            if let Some(doc_id) = active_doc.read().as_ref() {
                ActiveDocPanel {
                    cmd_handle, doc_id: doc_id.clone(),
                    key_input, value_input, share_output,
                    sharing, setting, last_error,
                    entries, listing,
                }
            }
            DocEventsList { events }
            if let Some(err) = last_error.read().as_ref() {
                section { class: "settings-section",
                    div { class: "diag-err", "{err}" }
                }
            }
        }
    }
}

/// Auto-creates a fresh document and pumps live events into the
/// caller-owned signals. Intended to be called once from `App` when the
/// peer handle becomes available, so the document persists across tab
/// switches.
pub fn auto_create_doc(
    cmd_handle: Signal<Option<PeerHandle>>,
    mut active_doc: Signal<Option<String>>,
    entries: Signal<Vec<DocEntrySummary>>,
    events: Signal<VecDeque<EventRow>>,
    listing: Signal<bool>,
    mut last_error: Signal<Option<String>>,
) {
    let Some(handle) = cmd_handle.read().clone() else {
        return;
    };
    let (events_tx, events_rx) = mpsc::channel::<DocEventUi>(EVENTS_CAPACITY);
    let (reply_tx, reply_rx) = oneshot::channel();
    let cmd_handle_clone = handle.clone();
    spawn(async move {
        let _ = cmd_handle_clone
            .tx
            .send(PeerCommand::CreateDoc {
                events_tx,
                reply: reply_tx,
            })
            .await;
    });
    spawn(async move {
        match reply_rx.await {
            Ok(Ok(doc_id)) => {
                active_doc.set(Some(doc_id));
                refresh_entries(cmd_handle, entries, listing, last_error);
            }
            Ok(Err(e)) => last_error.set(Some(e)),
            Err(_) => last_error.set(Some("auto-create reply dropped".into())),
        }
    });
    let mut events_signal = events;
    spawn(async move {
        drain_doc_events(
            events_rx,
            &mut events_signal,
            cmd_handle,
            entries,
            listing,
            last_error,
        )
        .await;
    });
}

#[component]
fn ImportSection(
    cmd_handle: Signal<Option<PeerHandle>>,
    mut active_doc: Signal<Option<String>>,
    mut ticket_input: Signal<String>,
    mut share_output: Signal<Option<String>>,
    events: Signal<VecDeque<EventRow>>,
    mut importing: Signal<bool>,
    mut last_error: Signal<Option<String>>,
    entries: Signal<Vec<DocEntrySummary>>,
    listing: Signal<bool>,
) -> Element {
    let busy_import = importing();
    rsx! {
        section { class: "settings-section",
            label { class: "label", "Import by ticket" }
            div { class: "footer-note",
                "Replaces the auto-created document with the one carried by the ticket."
            }
            input {
                class: "api-input",
                r#type: "text",
                placeholder: "paste a doc write ticket",
                value: "{ticket_input}",
                autocapitalize: "off",
                autocorrect: "off",
                spellcheck: "false",
                oninput: move |evt| ticket_input.set(evt.value()),
            }
            div { class: "settings-actions",
                button {
                    class: "btn btn-primary",
                    disabled: busy_import,
                    onclick: move |_| {
                        let Some(handle) = cmd_handle.read().clone() else { return };
                        let ticket = ticket_input().trim().to_string();
                        if ticket.is_empty() {
                            last_error.set(Some("ticket is required".into()));
                            return;
                        }
                        last_error.set(None);
                        share_output.set(None);
                        importing.set(true);
                        let (events_tx, events_rx) = mpsc::channel::<DocEventUi>(EVENTS_CAPACITY);
                        let (reply_tx, reply_rx) = oneshot::channel();
                        let cmd_handle_clone = handle.clone();
                        spawn(async move {
                            let _ = cmd_handle_clone
                                .tx
                                .send(PeerCommand::ImportDoc {
                                    ticket,
                                    events_tx,
                                    reply: reply_tx,
                                })
                                .await;
                        });
                        spawn(async move {
                            match reply_rx.await {
                                Ok(Ok(doc_id)) => active_doc.set(Some(doc_id)),
                                Ok(Err(e)) => last_error.set(Some(e)),
                                Err(_) => last_error.set(Some("import reply dropped".into())),
                            }
                            importing.set(false);
                        });
                        let mut events_signal = events;
                        spawn(async move {
                            drain_doc_events(
                                events_rx,
                                &mut events_signal,
                                cmd_handle,
                                entries,
                                listing,
                                last_error,
                            )
                            .await;
                        });
                    },
                    if busy_import { "Importing..." } else { "Import" }
                }
            }
        }
    }
}

#[component]
fn ActiveDocPanel(
    cmd_handle: Signal<Option<PeerHandle>>,
    doc_id: String,
    mut key_input: Signal<String>,
    mut value_input: Signal<String>,
    mut share_output: Signal<Option<String>>,
    mut sharing: Signal<bool>,
    mut setting: Signal<bool>,
    mut last_error: Signal<Option<String>>,
    entries: Signal<Vec<DocEntrySummary>>,
    listing: Signal<bool>,
) -> Element {
    let busy_share = sharing();
    let just_copied = share_output.read().is_some();
    let busy_set = setting();
    let short = short_id(&doc_id, 8, 4);
    let full = doc_id.clone();
    let share_label = if busy_share {
        "Copying..."
    } else if just_copied {
        "Copied!"
    } else {
        "Copy write ticket"
    };
    rsx! {
        section { class: "settings-section",
            label { class: "label", "Active doc" }
            div { class: "doc-id-row",
                span { class: "mono", title: "{full}", "{short}" }
                button {
                    class: "btn",
                    disabled: busy_share,
                    onclick: move |_| {
                        let Some(handle) = cmd_handle.read().clone() else { return };
                        last_error.set(None);
                        share_output.set(None);
                        sharing.set(true);
                        let (tx, rx) = oneshot::channel();
                        spawn(async move {
                            let _ = handle.tx.send(PeerCommand::ShareDoc { reply: tx }).await;
                            match rx.await {
                                Ok(Ok(ticket)) => {
                                    crate::copy_to_clipboard(&ticket);
                                    share_output.set(Some(ticket));
                                }
                                Ok(Err(e)) => last_error.set(Some(e)),
                                Err(_) => last_error.set(Some("share reply dropped".into())),
                            }
                            sharing.set(false);
                        });
                    },
                    "{share_label}"
                }
            }
            label { class: "label", "Set entry" }
            input {
                class: "api-input",
                r#type: "text",
                placeholder: "key",
                value: "{key_input}",
                oninput: move |evt| key_input.set(evt.value()),
            }
            input {
                class: "api-input",
                r#type: "text",
                placeholder: "value (utf-8)",
                value: "{value_input}",
                oninput: move |evt| value_input.set(evt.value()),
            }
            div { class: "settings-actions",
                button {
                    class: "btn btn-primary",
                    disabled: busy_set,
                    onclick: move |_| {
                        let Some(handle) = cmd_handle.read().clone() else { return };
                        let key = key_input();
                        let value = value_input();
                        if key.is_empty() {
                            last_error.set(Some("key is required".into()));
                            return;
                        }
                        last_error.set(None);
                        setting.set(true);
                        let (tx, rx) = oneshot::channel();
                        spawn(async move {
                            let _ = handle
                                .tx
                                .send(PeerCommand::SetDocEntry { key, value, reply: tx })
                                .await;
                            match rx.await {
                                Ok(Ok(_hash)) => {
                                    value_input.set(String::new());
                                }
                                Ok(Err(e)) => last_error.set(Some(e)),
                                Err(_) => last_error.set(Some("set reply dropped".into())),
                            }
                            setting.set(false);
                        });
                    },
                    if busy_set { "Setting..." } else { "Set" }
                }
            }
        }

        EntriesList { entries, listing }
    }
}

#[component]
fn EntriesList(entries: Signal<Vec<DocEntrySummary>>, listing: Signal<bool>) -> Element {
    let list = entries();
    let busy = listing();
    let header = if busy && list.is_empty() {
        "Entries (loading...)".to_string()
    } else {
        format!("Entries ({})", list.len())
    };
    rsx! {
        section { class: "settings-section",
            label { class: "label", "{header}" }
            if list.is_empty() {
                div { class: "diag-idle",
                    if busy { "loading..." } else { "no entries yet" }
                }
            } else {
                table { class: "blobs-table",
                    thead {
                        tr {
                            th { "Key" }
                            th { "Value" }
                            th { class: "ping-rtt-col", "Size" }
                        }
                    }
                    tbody {
                        for entry in list.iter() {
                            tr {
                                td { class: "mono", title: "{entry.key}", "{entry.key}" }
                                td { class: "doc-entry-value", {render_value(entry)} }
                                td { class: "ping-rtt-col mono", {super::format_bytes_iec(entry.content_len)} }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn render_value(entry: &DocEntrySummary) -> Element {
    match &entry.value {
        Some(v) => rsx! { span { class: "mono", title: "{v}", "{v}" } },
        None => rsx! {
            span {
                class: "diag-idle mono",
                title: "{entry.content_hash}",
                "(content not synced)"
            }
        },
    }
}

fn refresh_entries(
    cmd_handle: Signal<Option<PeerHandle>>,
    mut entries: Signal<Vec<DocEntrySummary>>,
    mut listing: Signal<bool>,
    mut last_error: Signal<Option<String>>,
) {
    let Some(handle) = cmd_handle.read().clone() else {
        return;
    };
    if listing.peek().to_owned() {
        return;
    }
    listing.set(true);
    spawn(async move {
        let (tx, rx) = oneshot::channel();
        let _ = handle
            .tx
            .send(PeerCommand::ListDocEntries { reply: tx })
            .await;
        match rx.await {
            Ok(Ok(list)) => entries.set(list),
            Ok(Err(e)) => last_error.set(Some(e)),
            Err(_) => last_error.set(Some("list reply dropped".into())),
        }
        listing.set(false);
    });
}

#[component]
fn DocEventsList(events: Signal<VecDeque<EventRow>>) -> Element {
    let log = events();
    rsx! {
        section { class: "settings-section",
            label { class: "label", "Live events ({log.len()})" }
            if log.is_empty() {
                div { class: "diag-idle", "no events yet" }
            } else {
                ul { class: "doc-events",
                    for row in log.iter().rev() {
                        li { class: "doc-event-row", "data-kind": "{row.kind}",
                            span { class: "mono doc-event-kind", "{row.kind}" }
                            span { class: "doc-event-label", "{row.label}" }
                        }
                    }
                }
            }
        }
    }
}

async fn drain_doc_events(
    mut rx: mpsc::Receiver<DocEventUi>,
    events_signal: &mut Signal<VecDeque<EventRow>>,
    cmd_handle: Signal<Option<PeerHandle>>,
    entries: Signal<Vec<DocEntrySummary>>,
    listing: Signal<bool>,
    last_error: Signal<Option<String>>,
) {
    while let Some(event) = rx.recv().await {
        let should_refresh = matches!(
            event,
            DocEventUi::InsertLocal { .. }
                | DocEventUi::InsertRemote { .. }
                | DocEventUi::ContentReady { .. }
                | DocEventUi::PendingContentReady
        );
        let row = event_to_row(event);
        {
            let mut log = events_signal.write();
            if log.len() >= DOC_EVENT_LOG_LEN {
                log.pop_front();
            }
            log.push_back(row);
        }
        if should_refresh {
            refresh_entries(cmd_handle, entries, listing, last_error);
        }
    }
}

fn event_to_row(event: DocEventUi) -> EventRow {
    match event {
        DocEventUi::InsertLocal { key, value_hash } => EventRow {
            kind: "local",
            label: format!("{key} -> {}", short_hash(&value_hash)),
        },
        DocEventUi::InsertRemote {
            key,
            value_hash,
            from,
        } => EventRow {
            kind: "remote",
            label: format!(
                "from {}: {key} -> {}",
                short_hash(&from),
                short_hash(&value_hash)
            ),
        },
        DocEventUi::ContentReady { hash } => EventRow {
            kind: "content",
            label: format!("ready: {}", short_hash(&hash)),
        },
        DocEventUi::PendingContentReady => EventRow {
            kind: "content",
            label: "all pending ready".to_string(),
        },
        DocEventUi::NeighborUp { peer } => EventRow {
            kind: "neighbor",
            label: format!("up: {}", short_hash(&peer)),
        },
        DocEventUi::NeighborDown { peer } => EventRow {
            kind: "neighbor",
            label: format!("down: {}", short_hash(&peer)),
        },
        DocEventUi::SyncFinished { peer } => EventRow {
            kind: "sync",
            label: format!("sync done: {}", short_hash(&peer)),
        },
    }
}

fn short_hash(s: &str) -> String {
    short_id(s, 6, 4)
}
