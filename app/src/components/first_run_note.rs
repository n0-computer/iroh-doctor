//! Dismissible first-run card on the Connect tab explaining the trust
//! model: the app listens for probes while open, and the endpoint id is
//! the only thing gating who can connect.

use dioxus::prelude::*;

use crate::first_run;

/// The first-run trust note. Renders nothing once dismissed; the
/// dismissal is persisted via [`first_run::dismiss_trust_note`] so the
/// card shows only until the user acknowledges it. If persisting fails
/// we still hide the card for this session and log a warning rather
/// than trapping the user with an undismissable banner.
#[component]
pub fn FirstRunNote() -> Element {
    let mut dismissed = use_signal(first_run::trust_note_dismissed);
    if dismissed() {
        return rsx! {};
    }
    rsx! {
        section { class: "settings-section first-run-note",
            label { class: "label", "Before you share your id" }
            p { class: "hint-text",
                "While this app is open it listens for incoming probes. "
                "Anyone with your endpoint id can connect and measure the "
                "connection while the app runs. Share your id only with "
                "people you trust. Close the app to stop listening."
            }
            div { class: "settings-actions",
                button {
                    class: "btn",
                    onclick: move |_| {
                        if let Err(e) = first_run::dismiss_trust_note() {
                            tracing::warn!(err = %e, "persisting trust-note dismissal");
                        }
                        dismissed.set(true);
                    },
                    "Got it"
                }
            }
        }
    }
}
