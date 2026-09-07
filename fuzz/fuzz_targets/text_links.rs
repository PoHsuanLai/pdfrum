//! Web and mail link recognition over arbitrary text.
//!
//! Property: never panics; reported offsets address the candidate they came from.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    // A single enormous line would spend the whole run inside one scan.
    if text.len() > 4096 {
        return;
    }

    if let Some(link) = pdfrum_text::check_web_link(text) {
        let chars = text.chars().count();
        assert!(
            link.range.start <= link.range.end && link.range.end <= chars,
            "web link range {:?} escapes a {chars}-character candidate",
            link.range
        );
        assert!(!link.url.is_empty(), "an accepted web link needs a URL");
    }
    if let Some(url) = pdfrum_text::check_mail_link(text) {
        assert!(
            url.contains('@'),
            "an accepted mail link keeps its at sign: {url:?}"
        );
        assert!(url.contains("mailto:"), "and gains its scheme: {url:?}");
    }
});
