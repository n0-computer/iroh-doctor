mod connect_page;
mod connection;
mod diag_state;
mod diagnostics;
mod endpoints;
mod error_dialog;
mod first_run_note;
mod gossip;
mod nav;
mod status;

pub use connect_page::ConnectPage;
pub use connection::{ConnectView, EventEntry};
pub use diag_state::{
    trigger_net_diagnostics, trigger_pings, trigger_probe_net_report, trigger_probe_relays,
    DiagState,
};
pub use diagnostics::DiagnosticsPage;
pub use endpoints::EndpointsView;
pub use error_dialog::{AppError, ErrorDialog};
pub use first_run_note::FirstRunNote;
pub use gossip::GossipView;
pub use nav::{Nav, Tab};
pub use status::{short_event_label, status_kind, status_line};

/// Truncates a long hex identifier (endpoint id, topic id, blob hash) to
/// `head` + `...` + `tail` characters. Operates on `chars()` so it never
/// panics on multi-byte input even though every caller today passes hex.
pub(crate) fn short_id(s: &str, head: usize, tail: usize) -> String {
    let total = s.chars().count();
    if total <= head + tail + 3 {
        return s.to_string();
    }
    let head_part: String = s.chars().take(head).collect();
    let tail_part: String = s
        .chars()
        .rev()
        .take(tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{head_part}...{tail_part}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_id_under_threshold_passes_through() {
        assert_eq!(short_id("abcdef", 6, 4), "abcdef");
    }

    #[test]
    fn short_id_trims_with_ellipsis() {
        let s = "0123456789abcdef0123456789abcdef";
        assert_eq!(short_id(s, 6, 4), "012345...cdef");
    }

    #[test]
    fn short_id_safe_on_multibyte() {
        // Each smiley is four UTF-8 bytes. Index-based slicing into bytes
        // would split a multi-byte sequence and panic; the chars-based
        // implementation truncates by code point.
        let s = "\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}";
        let out = short_id(s, 2, 2);
        assert_eq!(out, "\u{1F600}\u{1F600}...\u{1F600}\u{1F600}");
    }

    #[test]
    fn short_id_exact_threshold_passes_through() {
        // total == head + tail + 3 stays as-is.
        assert_eq!(short_id("abcdefghijk", 4, 4), "abcdefghijk");
    }

    #[test]
    fn short_id_one_over_threshold_trims() {
        // total > head + tail + 3 truncates.
        assert_eq!(short_id("abcdefghijkl", 4, 4), "abcd...ijkl");
    }
}
