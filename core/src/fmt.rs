//! Tiny text renderings shared by the cli tables and the app UI, so the two
//! frontends label the same probe states with the same words.

/// "yes"/"no" for a known boolean, "unknown" when the report did not
/// determine it (or there was no report).
#[must_use]
pub fn opt_bool(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

/// "yes"/"no" for a known boolean, "(not probed)" when the protocol was not
/// probed. Used for the port-mapping block, where `None` is a deliberate skip
/// rather than an unknown.
#[must_use]
pub fn tribool_text(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "yes",
        Some(false) => "no",
        None => "(not probed)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_bool_covers_all_states() {
        assert_eq!(opt_bool(Some(true)), "yes");
        assert_eq!(opt_bool(Some(false)), "no");
        assert_eq!(opt_bool(None), "unknown");
    }

    #[test]
    fn tribool_text_covers_all_states() {
        assert_eq!(tribool_text(Some(true)), "yes");
        assert_eq!(tribool_text(Some(false)), "no");
        assert_eq!(tribool_text(None), "(not probed)");
    }
}
