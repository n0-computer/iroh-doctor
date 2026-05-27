use dioxus::prelude::*;
use tokio::sync::{mpsc, oneshot};

use super::AppError;
use crate::peer::{BlobKind, BlobSummary, ConnectionState, PeerCommand};
use crate::PeerHandle;

const SIZE_OPTIONS: &[(&str, u64)] = &[
    ("1 KB", 1024),
    ("1 MB", 1024 * 1024),
    ("10 MB", 10 * 1024 * 1024),
    ("100 MB", 100 * 1024 * 1024),
    ("1 GB", 1024 * 1024 * 1024),
];

/// State for the in-flight pull operation. `None` when idle.
#[derive(Clone)]
struct PullState {
    received_bytes: u64,
    started_at_label: String,
}

#[component]
pub fn BlobsView(
    cmd_handle: Signal<Option<PeerHandle>>,
    conn_state: Signal<ConnectionState>,
    blobs_list: Signal<Vec<BlobSummary>>,
) -> Element {
    let selected_size = use_signal(|| SIZE_OPTIONS[1].1);
    let hash_input = use_signal(String::new);
    let generating = use_signal(|| false);
    let pull_state: Signal<Option<PullState>> = use_signal(|| None);
    let last_error = use_signal(|| Option::<String>::None);

    // Mirror any blob-section error into the global modal so the user
    // sees the same dialog as every other error path. The inline
    // display below stays for the after-dismiss case.
    let mut app_error = use_context::<Signal<Option<AppError>>>();
    use_effect(move || {
        if let Some(msg) = last_error() {
            app_error.set(Some(AppError::new("blobs", msg)));
        }
    });

    rsx! {
        div { class: "blobs-view",
            GenerateSection {
                cmd_handle, blobs_list,
                selected_size, generating, last_error,
            }

            PullSection {
                cmd_handle, conn_state, blobs_list,
                hash_input, pull_state, last_error,
            }

            BlobsList { blobs_list }

            if let Some(err) = last_error.read().as_ref() {
                section { class: "settings-section",
                    div { class: "diag-err", "{err}" }
                }
            }
        }
    }
}

#[component]
fn GenerateSection(
    cmd_handle: Signal<Option<PeerHandle>>,
    mut blobs_list: Signal<Vec<BlobSummary>>,
    mut selected_size: Signal<u64>,
    mut generating: Signal<bool>,
    mut last_error: Signal<Option<String>>,
) -> Element {
    let size = selected_size();
    let busy = generating();

    rsx! {
        section { class: "settings-section",
            label { class: "label", "Generate blob" }
            div { class: "blob-size-row",
                for (name, bytes) in SIZE_OPTIONS.iter().copied() {
                    button {
                        class: if size == bytes { "btn btn-primary" } else { "btn" },
                        onclick: move |_| selected_size.set(bytes),
                        "{name}"
                    }
                }
            }
            div { class: "settings-actions",
                button {
                    class: "btn btn-primary",
                    disabled: busy,
                    onclick: move |_| {
                        let Some(handle) = cmd_handle.read().clone() else { return };
                        last_error.set(None);
                        generating.set(true);
                        let size_bytes = selected_size();
                        spawn(async move {
                            let (tx, rx) = oneshot::channel();
                            if handle
                                .tx
                                .send(PeerCommand::AddBlob {
                                    size_bytes,
                                    reply: tx,
                                })
                                .await
                                .is_err()
                            {
                                last_error.set(Some("peer command channel closed".into()));
                                generating.set(false);
                                return;
                            }
                            match rx.await {
                                Ok(Ok(summary)) => {
                                    let mut list = blobs_list.write();
                                    list.insert(0, summary);
                                }
                                Ok(Err(e)) => last_error.set(Some(e)),
                                Err(_) => last_error.set(Some("reply dropped".into())),
                            }
                            generating.set(false);
                        });
                    },
                    if busy { "Generating..." } else { "Generate" }
                }
            }
        }
    }
}

#[component]
fn PullSection(
    cmd_handle: Signal<Option<PeerHandle>>,
    conn_state: Signal<ConnectionState>,
    mut blobs_list: Signal<Vec<BlobSummary>>,
    mut hash_input: Signal<String>,
    mut pull_state: Signal<Option<PullState>>,
    mut last_error: Signal<Option<String>>,
) -> Element {
    let active = pull_state.read().is_some();
    let hash_value = hash_input();
    let connected_peer = match conn_state() {
        ConnectionState::Connected { peer_id, .. } => Some(peer_id),
        _ => None,
    };
    let hash_present = !hash_value.trim().is_empty();
    let can_pull = connected_peer.is_some() && hash_present && !active;

    let progress_view = match pull_state.read().as_ref() {
        Some(state) => rsx! {
            div { class: "blob-progress",
                span { class: "mono", "received: {format_bytes(state.received_bytes)}" }
                span { class: "label blob-progress-started", "since {state.started_at_label}" }
            }
        },
        None => rsx! { div { class: "diag-idle", "idle" } },
    };

    let peer_display = connected_peer
        .clone()
        .unwrap_or_else(|| "(not connected)".into());

    rsx! {
        section { class: "settings-section",
            label { class: "label", "Pull blob" }
            input {
                class: "api-input",
                r#type: "text",
                placeholder: "hash (hex or base32)",
                value: "{hash_value}",
                autocapitalize: "off",
                autocorrect: "off",
                spellcheck: "false",
                oninput: move |evt| hash_input.set(evt.value()),
            }
            div { class: "pull-peer-row",
                span { class: "label", "from peer:" }
                span { class: "mono pull-peer-id", "{peer_display}" }
            }
            div { class: "settings-actions",
                button {
                    class: "btn btn-primary",
                    disabled: !can_pull,
                    onclick: move |_| {
                        let Some(handle) = cmd_handle.read().clone() else { return };
                        let hash = hash_input().trim().to_string();
                        let Some(peer) = (match conn_state() {
                            ConnectionState::Connected { peer_id, .. } => Some(peer_id),
                            _ => None,
                        }) else {
                            last_error.set(Some("not connected to a peer".into()));
                            return;
                        };
                        if hash.is_empty() {
                            last_error.set(Some("hash is required".into()));
                            return;
                        }
                        last_error.set(None);
                        let started_at = std::time::Instant::now();
                        let started_at_label = "now".to_string();
                        pull_state.set(Some(PullState { received_bytes: 0, started_at_label }));

                        let (progress_tx, mut progress_rx) = mpsc::channel::<u64>(64);
                        let (reply_tx, reply_rx) = oneshot::channel();

                        let cmd_handle_clone = handle.clone();
                        spawn(async move {
                            let _ = cmd_handle_clone
                                .tx
                                .send(PeerCommand::PullBlob {
                                    peer,
                                    hash,
                                    progress_tx,
                                    reply: reply_tx,
                                })
                                .await;
                        });

                        spawn(async move {
                            while let Some(offset) = progress_rx.recv().await {
                                let elapsed = started_at.elapsed().as_secs_f64();
                                pull_state.set(Some(PullState {
                                    received_bytes: offset,
                                    started_at_label: format!("{elapsed:.1}s ago"),
                                }));
                            }
                            match reply_rx.await {
                                Ok(Ok(summary)) => {
                                    let mut list = blobs_list.write();
                                    list.insert(0, summary);
                                }
                                Ok(Err(e)) => last_error.set(Some(e)),
                                Err(_) => last_error.set(Some("pull reply dropped".into())),
                            }
                            pull_state.set(None);
                        });
                    },
                    if active { "Pulling..." } else { "Pull" }
                }
            }
            {progress_view}
        }
    }
}

#[component]
fn BlobsList(blobs_list: Signal<Vec<BlobSummary>>) -> Element {
    let list = blobs_list();
    if list.is_empty() {
        return rsx! {
            section { class: "settings-section",
                label { class: "label", "Local blobs" }
                div { class: "diag-idle", "none yet" }
            }
        };
    }

    let total: u64 = list.iter().map(|b| b.size_bytes).sum();
    let total_label = format_bytes(total);

    rsx! {
        section { class: "settings-section",
            label { class: "label",
                "Local blobs ({list.len()}) - total {total_label}"
            }
            table { class: "blobs-table",
                thead {
                    tr {
                        th { "Kind" }
                        th { "Size" }
                        th { class: "ping-rtt-col", "Elapsed" }
                        th { "Throughput" }
                        th { "Hash" }
                        th { "" }
                    }
                }
                tbody {
                    for blob in list.iter() {
                        {
                            let hash = blob.hash.clone();
                            rsx! {
                                tr {
                                    td { {kind_label(blob.kind)} }
                                    td { class: "mono", {format_bytes(blob.size_bytes)} }
                                    td { class: "ping-rtt-col mono", {format_elapsed_ms(blob.elapsed_ms)} }
                                    td { class: "mono", {format_throughput(blob.size_bytes, blob.elapsed_ms)} }
                                    td { class: "mono blobs-hash", title: "{blob.hash}", "{blob.hash}" }
                                    td {
                                        button {
                                            class: "btn blobs-copy",
                                            title: "Copy hash",
                                            onclick: move |_| crate::copy_to_clipboard(&hash),
                                            "Copy"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn kind_label(kind: BlobKind) -> &'static str {
    match kind {
        BlobKind::Generated => "gen",
        BlobKind::Pulled => "pull",
    }
}

fn format_bytes(bytes: u64) -> String {
    super::format_bytes_iec(bytes)
}

fn format_elapsed_ms(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else {
        format!("{ms:.1} ms")
    }
}

fn format_throughput(bytes: u64, ms: f64) -> String {
    if ms <= 0.0 {
        return "-".into();
    }
    let bytes_per_sec = (bytes as f64) / (ms / 1000.0);
    let mib_per_sec = bytes_per_sec / (1024.0 * 1024.0);
    if mib_per_sec >= 1.0 {
        format!("{mib_per_sec:.1} MiB/s")
    } else {
        let kib_per_sec = bytes_per_sec / 1024.0;
        format!("{kib_per_sec:.1} KiB/s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_picks_unit() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.00 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GiB");
        assert_eq!(format_bytes(10 * 1024 * 1024), "10.00 MiB");
    }

    #[test]
    fn format_elapsed_ms_under_one_second() {
        assert_eq!(format_elapsed_ms(0.5), "0.5 ms");
        assert_eq!(format_elapsed_ms(123.4), "123.4 ms");
    }

    #[test]
    fn format_elapsed_ms_over_one_second() {
        assert_eq!(format_elapsed_ms(1000.0), "1.00 s");
        assert_eq!(format_elapsed_ms(2500.0), "2.50 s");
    }

    #[test]
    fn format_throughput_zero_or_negative_elapsed_is_placeholder() {
        assert_eq!(format_throughput(1024, 0.0), "-");
        assert_eq!(format_throughput(1024, -1.0), "-");
    }

    #[test]
    fn format_throughput_uses_mib_above_one() {
        // 10 MiB in 1 s -> 10 MiB/s.
        assert_eq!(format_throughput(10 * 1024 * 1024, 1000.0), "10.0 MiB/s");
    }

    #[test]
    fn format_throughput_at_one_mib_per_second_boundary() {
        // Exactly 1 MiB/s should pick the MiB branch.
        assert_eq!(format_throughput(1024 * 1024, 1000.0), "1.0 MiB/s");
    }

    #[test]
    fn format_throughput_just_below_one_mib_falls_back_to_kib() {
        // 1023 KiB in 1 s is just under 1 MiB/s and must use the KiB branch.
        assert_eq!(format_throughput(1023 * 1024, 1000.0), "1023.0 KiB/s");
    }

    #[test]
    fn format_throughput_falls_back_to_kib() {
        // 100 KiB in 1 s -> 100 KiB/s, well under 1 MiB/s.
        assert_eq!(format_throughput(100 * 1024, 1000.0), "100.0 KiB/s");
    }

    #[test]
    fn format_bytes_just_below_kib_boundary() {
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024 * 1024 - 1), "1024.00 KiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 - 1), "1024.00 MiB");
    }

    #[test]
    fn format_elapsed_ms_just_below_one_second() {
        assert_eq!(format_elapsed_ms(999.9), "999.9 ms");
        assert_eq!(format_elapsed_ms(1000.0), "1.00 s");
    }

    #[test]
    fn kind_label_matches_variant() {
        assert_eq!(kind_label(BlobKind::Generated), "gen");
        assert_eq!(kind_label(BlobKind::Pulled), "pull");
    }
}
