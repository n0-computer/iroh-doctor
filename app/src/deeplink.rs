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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_url_encodes_the_id() {
        assert_eq!(connect_url("0123abcd"), "irohdoctor://connect?id=0123abcd");
    }
}
