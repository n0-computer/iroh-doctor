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
