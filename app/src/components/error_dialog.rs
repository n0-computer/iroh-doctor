//! Global modal that surfaces app errors and offers a "Send
//! diagnostics" zip export.
//!
//! Driven by a `Signal<Option<AppError>>` set anywhere in the app. The
//! dialog renders nothing when the signal is None; when Some, it shows
//! the error text, a Dismiss button that clears the signal, and a
//! Send-diagnostics button that fires the caller-supplied event
//! handler.

use dioxus::prelude::*;

/// One reportable error. `source` is a short human-readable string
/// identifying which subsystem produced it (e.g. "connect", "blob
/// pull"), so a user can pattern-match the modal back to the action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub source: String,
    pub message: String,
}

impl AppError {
    pub fn new(source: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            message: message.into(),
        }
    }
}

#[component]
pub fn ErrorDialog(
    error: Signal<Option<AppError>>,
    on_send_diagnostics: EventHandler<AppError>,
) -> Element {
    let Some(err) = error.read().clone() else {
        return rsx! {};
    };
    let err_for_send = err.clone();
    rsx! {
        div { class: "modal-backdrop",
            div { class: "modal",
                div { class: "modal-title", "Error" }
                div { class: "modal-source label", "{err.source}" }
                pre { class: "modal-message", "{err.message}" }
                div { class: "modal-actions",
                    button {
                        class: "btn",
                        onclick: move |_| {
                            error.clone().set(None);
                        },
                        "Dismiss"
                    }
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| {
                            on_send_diagnostics.call(err_for_send.clone());
                        },
                        "Download diagnostics"
                    }
                }
            }
        }
    }
}
