//! Endpoints tab: list of saved endpoint ids with rename / delete /
//! connect affordances.
//!
//! State lives in `App` so it survives tab switches; this view reads
//! the `endpoints` signal and writes back through helper closures.

use dioxus::prelude::*;

use super::short_id;
use crate::endpoints::Endpoint;
use crate::node::NodeCommand;
use crate::NodeHandle;

#[component]
pub fn EndpointsView(
    cmd_handle: Signal<Option<NodeHandle>>,
    endpoints: Signal<Vec<Endpoint>>,
    on_change: EventHandler<Vec<Endpoint>>,
    on_connect: EventHandler<()>,
) -> Element {
    let list = endpoints();
    rsx! {
        div { class: "endpoints-view",
            section { class: "settings-section",
                label { class: "label", "Saved endpoints" }
                if list.is_empty() {
                    div { class: "diag-idle",
                        "No endpoints yet. Connect to a peer from the Diagnostics tab and it shows up here."
                    }
                } else {
                    div { class: "endpoints-list",
                        for endpoint in list.iter().cloned() {
                            EndpointRow {
                                key: "{endpoint.id}",
                                endpoint,
                                cmd_handle,
                                endpoints,
                                on_change,
                                on_connect,
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn EndpointRow(
    endpoint: Endpoint,
    cmd_handle: Signal<Option<NodeHandle>>,
    mut endpoints: Signal<Vec<Endpoint>>,
    on_change: EventHandler<Vec<Endpoint>>,
    on_connect: EventHandler<()>,
) -> Element {
    let editing = use_signal(|| false);
    let name_input = use_signal(|| endpoint.name.clone());
    let id_for_connect = endpoint.id.clone();
    let id_for_rename = endpoint.id.clone();
    let id_for_delete = endpoint.id.clone();
    let display_name = if endpoint.name.is_empty() {
        short_id(&endpoint.id, 8, 4)
    } else {
        endpoint.name.clone()
    };
    let last_seen_label = format_relative(endpoint.last_seen);

    rsx! {
        div { class: "endpoint-row",
            div { class: "endpoint-row-main",
                if editing() {
                    EndpointNameEditor {
                        endpoint_id: id_for_rename.clone(),
                        name_input,
                        editing,
                        endpoints,
                        on_change,
                    }
                } else {
                    div { class: "endpoint-name", "{display_name}" }
                    div { class: "endpoint-id mono", title: "{endpoint.id}", {short_id(&endpoint.id, 8, 4)} }
                }
                div { class: "endpoint-meta",
                    span { class: "label", "last seen " }
                    span { "{last_seen_label}" }
                }
            }
            div { class: "endpoint-row-actions",
                if !editing() {
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| {
                            if let Some(handle) = cmd_handle.read().clone() {
                                let _ = handle.tx.try_send(NodeCommand::Connect {
                                    hex_id: id_for_connect.clone(),
                                });
                                on_connect.call(());
                            }
                        },
                        "Connect"
                    }
                    button {
                        class: "btn",
                        onclick: move |_| editing.clone().set(true),
                        "Rename"
                    }
                    button {
                        class: "btn btn-danger",
                        onclick: move |_| {
                            let next = crate::endpoints::remove(endpoints(), &id_for_delete);
                            on_change.call(next);
                        },
                        "Delete"
                    }
                }
            }
        }
    }
}

#[component]
fn EndpointNameEditor(
    endpoint_id: String,
    mut name_input: Signal<String>,
    mut editing: Signal<bool>,
    endpoints: Signal<Vec<Endpoint>>,
    on_change: EventHandler<Vec<Endpoint>>,
) -> Element {
    let value = name_input();
    let id_for_save = endpoint_id.clone();
    let id_for_cancel = endpoint_id.clone();
    rsx! {
        div { class: "endpoint-name-editor",
            input {
                class: "api-input",
                r#type: "text",
                placeholder: "name (blank for short id)",
                value: "{value}",
                autocapitalize: "off",
                autocorrect: "off",
                spellcheck: "false",
                oninput: move |evt| name_input.set(evt.value()),
            }
            div { class: "settings-actions",
                button {
                    class: "btn btn-primary",
                    onclick: move |_| {
                        let next = crate::endpoints::rename(endpoints(), &id_for_save, name_input());
                        on_change.call(next);
                        editing.set(false);
                    },
                    "Save"
                }
                button {
                    class: "btn",
                    onclick: move |_| {
                        // Restore the original input value so a reopen does
                        // not show the half-typed string.
                        if let Some(original) = endpoints().iter().find(|d| d.id == id_for_cancel) {
                            name_input.set(original.name.clone());
                        }
                        editing.set(false);
                    },
                    "Cancel"
                }
            }
        }
    }
}

/// Renders a Unix-second timestamp as a relative "Xs ago" / "Xm ago" /
/// "Xh ago" / "Xd ago" string. Zero or future timestamps render as
/// "just now" so an unparsed value does not show as "55 years ago".
pub fn format_relative(unix_seconds: u64) -> String {
    if unix_seconds == 0 {
        return "just now".to_string();
    }
    let now = crate::endpoints::now_secs();
    if unix_seconds >= now {
        return "just now".to_string();
    }
    let diff = now - unix_seconds;
    if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_relative_zero_is_just_now() {
        assert_eq!(format_relative(0), "just now");
    }

    #[test]
    fn format_relative_future_is_just_now() {
        let future = crate::endpoints::now_secs() + 1000;
        assert_eq!(format_relative(future), "just now");
    }

    #[test]
    fn format_relative_picks_unit_by_magnitude() {
        let now = crate::endpoints::now_secs();
        assert!(format_relative(now.saturating_sub(30)).ends_with("s ago"));
        assert!(format_relative(now.saturating_sub(300)).ends_with("m ago"));
        assert!(format_relative(now.saturating_sub(3 * 3600)).ends_with("h ago"));
        assert!(format_relative(now.saturating_sub(2 * 86_400)).ends_with("d ago"));
    }
}
