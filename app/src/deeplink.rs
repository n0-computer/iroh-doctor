//! `irohdoctor://` deep links: the payload a device encodes in its QR code so
//! another device can scan it with the system camera and open straight into a
//! connect, without copy-pasting a 64-hex endpoint id.
//!
//! The scheme is registered per platform through `Dioxus.toml`'s `[deep_links]`
//! table, which dx maps to the iOS `CFBundleURLTypes` and an Android
//! `<intent-filter>`. Capturing an incoming link is platform-specific and lives
//! with the rest of the app glue: the tao `Event::Opened` hook here, the
//! Android launch-intent read in [`crate::android`].

/// URL scheme registered for deep links. Kept in sync with the `[deep_links]`
/// `schemes` entry in `Dioxus.toml`.
const SCHEME: &str = "irohdoctor";

/// Builds the deep-link URL a device encodes in its QR code. Scanning it opens
/// iroh-doctor with `id` prefilled as the peer to connect to.
///
/// The id is a 64-character hex endpoint id, so it needs no percent-encoding.
#[must_use]
pub(crate) fn connect_url(id: &str) -> String {
    format!("{SCHEME}://connect?id={id}")
}

/// Extracts the endpoint id from a connect deep link, or `None` when `input` is
/// not an `irohdoctor://connect?id=...` URL. Both ends of the exchange are ours,
/// so the id needs no percent-decoding; extra query params are tolerated in case
/// the link ever grows, and the first non-empty `id` wins. Matching is
/// case-sensitive, which is fine because our own QR always emits lowercase.
#[must_use]
pub(crate) fn parse_connect_url(input: &str) -> Option<String> {
    let query = input.trim().strip_prefix(&format!("{SCHEME}://connect?"))?;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("id=").filter(|id| !id.is_empty()))
        .map(str::to_string)
}

/// Resolves what the user put in the peer-id box to a bare endpoint id: the id
/// from a connect deep link if it is one (pasted, or scanned as plain text by a
/// generic QR reader), otherwise the trimmed input unchanged.
#[must_use]
pub(crate) fn resolve_peer_id(input: &str) -> String {
    parse_connect_url(input).unwrap_or_else(|| input.trim().to_string())
}

/// Applies a captured deep-link id: prefills the peer-id box, switches to the
/// Connect tab, and dials the peer immediately. A session that is already
/// active is dropped first, so scanning a QR while connected re-targets to the
/// scanned peer.
#[cfg(any(feature = "desktop", feature = "mobile"))]
fn apply_connect(
    mut peer_id_input: dioxus::prelude::Signal<String>,
    mut current_tab: dioxus::prelude::Signal<crate::components::Tab>,
    cmd_handle: dioxus::prelude::Signal<Option<crate::NodeHandle>>,
    conn_state: dioxus::prelude::Signal<crate::node::ConnectionState>,
    id: String,
) {
    use dioxus::prelude::*;

    use crate::node::{ConnectionState, NodeCommand};

    peer_id_input.set(id.clone());
    current_tab.set(crate::components::Tab::Connect);

    let Some(handle) = cmd_handle.read().clone() else {
        return;
    };
    // Cancel an in-flight dial or tear down a live session before dialing the
    // scanned peer; the node processes the two commands in order.
    let active = matches!(
        *conn_state.read(),
        ConnectionState::Connecting | ConnectionState::Connected { .. }
    );
    if active {
        let _ = handle.try_send(NodeCommand::Disconnect);
    }
    let _ = handle.try_send(NodeCommand::Connect { hex_id: id });
}

/// Registers deep-link capture for the life of the app: an incoming
/// `irohdoctor://connect` link prefills `peer_id_input`, switches `current_tab`
/// to Connect, and dials the peer (see [`apply_connect`]).
///
/// iOS (and desktop macOS) receive custom-scheme opens as a tao `Event::Opened`,
/// which fires live while the app runs and is replayed from tao's pre-launch
/// queue on a cold start. Android has no such event, so the launch intent is
/// read once at startup and warm-start intents arrive through the `onNewIntent`
/// glue (see `scripts/bundle-mobile.sh`); both are bridged over a channel.
///
/// The signals are `Copy`, so each capture path takes its own copy.
#[cfg(any(feature = "desktop", feature = "mobile"))]
pub(crate) fn use_connect_links(
    peer_id_input: dioxus::prelude::Signal<String>,
    current_tab: dioxus::prelude::Signal<crate::components::Tab>,
    cmd_handle: dioxus::prelude::Signal<Option<crate::NodeHandle>>,
    conn_state: dioxus::prelude::Signal<crate::node::ConnectionState>,
) {
    #[cfg(not(target_os = "android"))]
    {
        #[cfg(all(feature = "desktop", not(feature = "mobile")))]
        use dioxus::desktop::tao::event::Event;
        #[cfg(all(feature = "desktop", not(feature = "mobile")))]
        use dioxus::desktop::use_wry_event_handler;
        #[cfg(feature = "mobile")]
        use dioxus::mobile::tao::event::Event;
        #[cfg(feature = "mobile")]
        use dioxus::mobile::use_wry_event_handler;

        use_wry_event_handler(move |event, _| {
            if let Event::Opened { urls } = event {
                for url in urls {
                    if let Some(id) = parse_connect_url(url.as_str()) {
                        apply_connect(peer_id_input, current_tab, cmd_handle, conn_state, id);
                    }
                }
            }
        });
    }

    #[cfg(target_os = "android")]
    {
        dioxus::prelude::use_future(move || async move {
            // One consumer for both delivery paths: the cold-start launch intent
            // (seeded below) and warm-start intents pushed by the onNewIntent
            // JNI callback. tao forwards neither on Android, so we bridge them
            // through a channel ourselves.
            let mut rx = crate::android::deep_link_channel();
            if let Some(url) = crate::android::launch_deep_link() {
                crate::android::push_deep_link(url);
            }
            while let Some(url) = rx.recv().await {
                if let Some(id) = parse_connect_url(&url) {
                    apply_connect(peer_id_input, current_tab, cmd_handle, conn_state, id);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_url_encodes_the_id() {
        assert_eq!(connect_url("0123abcd"), "irohdoctor://connect?id=0123abcd");
    }

    #[test]
    fn connect_url_and_parse_round_trip() {
        let id = "a".repeat(64);
        assert_eq!(
            parse_connect_url(&connect_url(&id)).as_deref(),
            Some(id.as_str())
        );
    }

    #[test]
    fn parse_connect_url_tolerates_extra_params() {
        assert_eq!(
            parse_connect_url("irohdoctor://connect?id=abc&v=2").as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn parse_connect_url_rejects_non_deep_links() {
        assert_eq!(parse_connect_url("0123abcd"), None);
        assert_eq!(parse_connect_url("https://example.com/?id=abc"), None);
        assert_eq!(parse_connect_url("irohdoctor://connect?id="), None);
    }

    #[test]
    fn parse_connect_url_takes_the_first_non_empty_id() {
        assert_eq!(
            parse_connect_url("irohdoctor://connect?id=&id=abc").as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn parse_connect_url_trims_surrounding_whitespace() {
        assert_eq!(
            parse_connect_url("  irohdoctor://connect?id=abcd  ").as_deref(),
            Some("abcd")
        );
    }

    #[test]
    fn resolve_peer_id_passes_through_a_bare_id() {
        assert_eq!(resolve_peer_id("  0123abcd  "), "0123abcd");
    }

    #[test]
    fn resolve_peer_id_unwraps_a_deep_link() {
        assert_eq!(
            resolve_peer_id("irohdoctor://connect?id=0123abcd"),
            "0123abcd"
        );
    }
}
