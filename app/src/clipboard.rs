//! Platform clipboard access for the Copy buttons.

pub fn copy_to_clipboard(_text: &str) {
    // On iOS, write through UIPasteboard instead of the JS Clipboard API.
    // When the iOS build runs on an Apple silicon Mac ("iOS app on Mac"),
    // navigator.clipboard.writeText resolves ok but only WebKit's private
    // com.apple.WebKit.custom-pasteboard-data type crosses the
    // UIPasteboard -> NSPasteboard bridge; the text/plain representation
    // is dropped, so pasting into any other app yields nothing.
    // UIPasteboard bridges correctly in both environments.
    #[cfg(target_os = "ios")]
    {
        use objc2_foundation::NSString;
        use objc2_ui_kit::UIPasteboard;

        let text = NSString::from_str(_text);
        // SAFETY: setString is unsafe only because UIPasteboard is not
        // documented as thread-safe. We are on the main thread here: this
        // is only called from Dioxus event handlers, which run on the UI
        // thread on mobile.
        unsafe { UIPasteboard::generalPasteboard().setString(Some(&text)) };
    }
    #[cfg(all(any(feature = "desktop", feature = "mobile"), not(target_os = "ios")))]
    {
        let text = _text.to_string();
        dioxus::prelude::document::eval(&format!(
            "navigator.clipboard.writeText({});",
            serde_escape(&text)
        ));
    }
}

#[cfg(all(any(feature = "desktop", feature = "mobile"), not(target_os = "ios")))]
fn serde_escape(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
