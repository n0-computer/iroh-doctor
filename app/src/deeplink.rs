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
pub(crate) fn connect_url(id: &str) -> String {
    format!("{SCHEME}://connect?id={id}")
}

/// Extracts the endpoint id from a connect deep link, or `None` when `input` is
/// not an `irohdoctor://connect?id=...` URL. Both ends of the exchange are ours,
/// so the id needs no percent-decoding; extra query params are tolerated in case
/// the link ever grows.
pub(crate) fn parse_connect_url(input: &str) -> Option<String> {
    let query = input.trim().strip_prefix(&format!("{SCHEME}://connect?"))?;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("id="))
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// Resolves what the user put in the peer-id box to a bare endpoint id: the id
/// from a connect deep link if it is one (pasted, or scanned as plain text by a
/// generic QR reader), otherwise the trimmed input unchanged.
pub(crate) fn resolve_peer_id(input: &str) -> String {
    parse_connect_url(input).unwrap_or_else(|| input.trim().to_string())
}

/// Registers deep-link capture for the life of the app, invoking `on_connect`
/// with the peer id from each `irohdoctor://connect` link the OS delivers.
///
/// iOS (and desktop macOS) receive custom-scheme opens as a tao `Event::Opened`,
/// which fires live while the app runs and is replayed from tao's pre-launch
/// queue on a cold start. Android has no such event, so the launch intent is
/// read once at startup; a scan while the app is already running is delivered
/// through the `onNewIntent` glue instead (see `scripts/bundle-mobile.sh`).
#[cfg(any(feature = "desktop", feature = "mobile"))]
pub(crate) fn use_connect_links(mut on_connect: impl FnMut(String) + 'static) {
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
                        on_connect(id);
                    }
                }
            }
        });
    }

    #[cfg(target_os = "android")]
    {
        dioxus::prelude::use_effect(move || {
            if let Some(id) = crate::android::launch_deep_link()
                .as_deref()
                .and_then(parse_connect_url)
            {
                on_connect(id);
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
