//! Finding web and mail addresses in extracted text
//! (`docs/design/pdfrum-text.md` §1.15).
//!
//! A PDF has no idea that `http://example.com` is a link — it is just glyphs.
//! So the page's text is chopped into candidates at every generated character
//! and every space, each candidate is trimmed of trailing punctuation, and
//! what is left is tested for a scheme, a `www.` prefix, or an `@`.
//!
//! Two behaviours worth naming because they look wrong:
//!
//! - A trailing hyphen before a line break **joins** the two halves, so a URL
//!   broken across lines is found whole. A trailing `?` or `/` before a break
//!   does not, and the URL ends there.
//! - The candidate is sliced out of the **text** by offsets counted over the
//!   **character list**, and those two index spaces are not the same one. On a
//!   page with control characters or normalized ligatures the slice is
//!   misaligned. That is an upstream defect, it is observable through the
//!   reported character range, and it is ported as-is (design brief D5) — with
//!   a bounds-checked slice that yields an empty candidate where the C++'s
//!   `Substr` yields an empty string.

use crate::charinfo::{CharBox, CharType};
use crate::unicode::{is_alnum, is_decimal_digit, lower_string};
use std::ops::Range;

/// One address found in a page's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebLink {
    /// The URL, ready to open: a `www.` address has gained an `http://` and a
    /// mail address a `mailto:`.
    pub url: String,
    /// The characters it covers, as **character-list** indices.
    pub range: Range<usize>,
}

/// Every address in a page's text.
///
/// `chars` is the character list and `text` the search-facing text — the two
/// different sequences whose index spaces this deliberately mixes.
#[must_use]
pub fn extract(chars: &[CharBox], text: &[char]) -> Vec<WebLink> {
    let mut links = Vec::new();
    let mut start = 0usize;
    let mut pos = 0usize;
    let mut after_hyphen = false;
    let mut line_break = false;
    let total = chars.len();

    while pos < total {
        let Some(info) = chars.get(pos) else { break };
        // A candidate ends at a generated character, at a space, or at the
        // page's last character — and the last character is *included*.
        if info.char_type != CharType::Generated
            && info.unicode != u32::from(b' ')
            && pos != total - 1
        {
            after_hyphen = info.char_type == CharType::Hyphen
                || (info.char_type == CharType::Normal && info.unicode == u32::from(b'-'));
            pos += 1;
            continue;
        }

        let mut count = pos - start;
        if pos == total - 1 {
            count += 1;
        } else if after_hyphen
            && (info.unicode == u32::from(b'\n') || info.unicode == u32::from(b'\r'))
        {
            // A hyphen before a line break joins the halves rather than
            // ending the candidate.
            line_break = true;
            pos += 1;
            continue;
        }

        let mut candidate: String = substr(text, start, count);
        if line_break {
            candidate.retain(|ch| ch != '\n' && ch != '\r');
            line_break = false;
        }
        // The sentinel the search-facing text carries at a soft hyphen reads
        // back as the hyphen it stood for.
        candidate = candidate.replace('\u{FFFE}', "-");

        if candidate.chars().count() > 5 {
            // Trailing sentence punctuation is context, not address.
            while let Some(last) = candidate.chars().next_back() {
                if !matches!(last, ')' | ',' | '>' | '.') {
                    break;
                }
                candidate.pop();
                count = count.saturating_sub(1);
            }
            if count > 5 {
                if let Some(link) = check_web_link(&candidate) {
                    links.push(WebLink {
                        url: link.url,
                        range: start + link.range.start..start + link.range.end,
                    });
                } else if let Some(url) = check_mail_link(&candidate) {
                    links.push(WebLink {
                        url,
                        range: start..start + count,
                    });
                }
            }
        }
        pos += 1;
        start = pos;
    }
    links
}

/// `count` characters from `first`, or **nothing** when the range runs past
/// the end — which is what `WideStringView::Substr` does rather than clamping,
/// and is how the D5 index mismatch stays harmless.
fn substr(text: &[char], first: usize, count: usize) -> String {
    if count == 0 {
        return String::new();
    }
    let Some(last) = first.checked_add(count) else {
        return String::new();
    };
    if last > text.len() {
        return String::new();
    }
    text.get(first..last).unwrap_or_default().iter().collect()
}

/// A web address found inside a candidate.
///
/// Public because [`check_web_link`] is: the two string scanners are the
/// crate's most index-heavy code and its own fuzz target drives them
/// directly, which is worth more than keeping them private.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundLink {
    /// The URL.
    pub url: String,
    /// Its offsets within the candidate, in characters.
    pub range: Range<usize>,
}

/// Whether a candidate holds a web address, and where (`CheckWebLink`).
///
/// The scheme form needs at least `://` and one more character *after*
/// `http`, which is why `"http://a"` fails and `"http://ab"` passes. The
/// offsets come from the lowercased copy while the URL text comes from the
/// original, which is safe because the case fold is one code point to one.
#[must_use]
pub fn check_web_link(candidate: &str) -> Option<FoundLink> {
    let original: Vec<char> = candidate.chars().collect();
    let lower: Vec<char> = lower_string(candidate).chars().collect();
    let len = lower.len();

    if let Some(start) = find(&lower, "http") {
        let mut off = start + 4;
        // "http" plus at least "://<char>".
        if len > off + 4 {
            if lower.get(off) == Some(&'s') {
                off += 1;
            }
            if lower.get(off) == Some(&':')
                && lower.get(off + 1) == Some(&'/')
                && lower.get(off + 2) == Some(&'/')
            {
                off += 3;
                let trimmed = trim_external_brackets(&lower, start, len.saturating_sub(1));
                let end = find_web_link_ending(&lower, off, trimmed);
                if end > off {
                    let count = end - start + 1;
                    return Some(FoundLink {
                        url: original.get(start..start + count)?.iter().collect(),
                        range: start..start + count,
                    });
                }
            }
        }
    }

    if let Some(start) = find(&lower, "www.") {
        let off = start + 4;
        if len > off {
            let trimmed = trim_external_brackets(&lower, start, len.saturating_sub(1));
            // Note the scan starts at `start`, not at `off` — the `www.`
            // itself is part of the host name here.
            let end = find_web_link_ending(&lower, start, trimmed);
            if end > off {
                let count = end - start + 1;
                let text: String = original.get(start..start + count)?.iter().collect();
                return Some(FoundLink {
                    url: format!("http://{text}"),
                    range: start..start + count,
                });
            }
        }
    }
    None
}

fn find(haystack: &[char], needle: &str) -> Option<usize> {
    let needle: Vec<char> = needle.chars().collect();
    let last = haystack.len().checked_sub(needle.len())?;
    (0..=last).find(|start| haystack.get(*start..start + needle.len()) == Some(needle.as_slice()))
}

/// Where a web address stops (`FindWebLinkEnding`).
///
/// A URL with a path is not sanitized at all — anything after the first `/`
/// is kept. Without one it is a host, optionally in IPv6 brackets with a
/// port; trailing characters that cannot be part of a host name are trimmed,
/// except that a **non-ASCII** trailing character stops the trim dead and is
/// kept, which is why an address ending in an ideographic full stop survives.
#[must_use]
pub fn find_web_link_ending(text: &[char], start: usize, mut end: usize) -> usize {
    if text.get(start..).is_some_and(|rest| rest.contains(&'/')) {
        return end;
    }
    if text.get(start) == Some(&'[') {
        // An IPv6 reference: the host ends at the closing bracket, and an
        // optional port of at least one digit extends it.
        let Some(offset) = text
            .get(start + 1..)
            .and_then(|rest| rest.iter().position(|ch| *ch == ']'))
        else {
            return end;
        };
        end = start + 1 + offset;
        if end > start + 1 {
            let len = text.len();
            let mut off = end + 1;
            if off < len && text.get(off) == Some(&':') {
                off += 1;
                while off < len
                    && text
                        .get(off)
                        .copied()
                        .is_some_and(|ch| is_decimal_digit(u32::from(ch)))
                {
                    off += 1;
                }
                if off > end + 2 && off <= len {
                    end = off - 1;
                }
            }
        }
        return end;
    }
    // RFC 1123: a host name holds alphanumerics, hyphens and periods, and a
    // hyphen may not end it.
    while end > start
        && text
            .get(end)
            .copied()
            .is_some_and(|ch| u32::from(ch) < 0x80)
    {
        let Some(&ch) = text.get(end) else { break };
        if is_decimal_digit(u32::from(ch)) || ch.is_ascii_lowercase() || ch == '.' {
            break;
        }
        end -= 1;
    }
    end
}

/// Trims a URL back to a bracket or quote that was opened before it
/// (`TrimExternalBracketsFromWebLink`).
///
/// An unopened closing bracket is left alone, which is why
/// `http://www.abc.com)0` keeps its trailing text while
/// `0(http://www.abc.com)0` does not.
#[must_use]
pub fn trim_external_brackets(text: &[char], start: usize, mut end: usize) -> usize {
    for pos in 0..start {
        let closing = match text.get(pos) {
            Some('(') => ')',
            Some('[') => ']',
            Some('{') => '}',
            Some('<') => '>',
            Some('"') => '"',
            Some('\'') => '\'',
            _ => continue,
        };
        trim_backwards_to(text, closing, start, &mut end);
    }
    end
}

/// Scans back from `*end` for `target` and cuts just before it.
///
/// The C++ walks a `size_t` down past zero, which reads out of bounds when
/// `start` is zero — unreachable, because its only caller runs the loop body
/// only when `start > 0`. An inclusive descending range cannot underflow at
/// all (design brief D6).
fn trim_backwards_to(text: &[char], target: char, start: usize, end: &mut usize) {
    if *end < start {
        return;
    }
    for pos in (start..=*end).rev() {
        if text.get(pos) == Some(&target) {
            *end = pos.saturating_sub(1);
            return;
        }
    }
}

/// Whether a candidate is a mail address, and the `mailto:` URL if so
/// (`CheckMailLink`).
///
/// The local part is scanned backwards from the `@`, trimming whatever
/// precedes the first character that cannot be in one — so `fan{abc@xyz.org`
/// yields `abc@xyz.org`. The domain is then scanned forwards and must hold at
/// least one period that is not immediately after the `@`.
#[must_use]
pub fn check_mail_link(candidate: &str) -> Option<String> {
    let mut text: Vec<char> = candidate.chars().collect();
    let at = text.iter().position(|ch| *ch == '@')?;
    if at == 0 || at == text.len() - 1 {
        return None;
    }

    // `marker` tracks the position of the `@` or of the last valid period.
    let mut marker = at;
    for i in (1..=at).rev() {
        let Some(&ch) = text.get(i - 1) else { break };
        if ch == '_' || ch == '-' || is_alnum(u32::from(ch)) {
            continue;
        }
        if ch != '.' || i == marker || i == 1 {
            if i == at {
                // A period or junk immediately before the `@` is fatal.
                return None;
            }
            let removed = if i == marker { i + 1 } else { i };
            text = text.get(removed..)?.to_vec();
            break;
        }
        marker = i - 1;
    }

    let at = text.iter().position(|ch| *ch == '@')?;
    if at == 0 {
        return None;
    }
    while text.last() == Some(&'.') {
        text.pop();
    }
    // At least one period in the domain, and not right after the `@`.
    let dot = text.get(at + 1..)?.iter().position(|ch| *ch == '.')? + at + 1;
    if dot == at + 1 {
        return None;
    }

    let len = text.len();
    // Reused with a second meaning: the position of the last period seen.
    let mut marker = 0usize;
    for i in (at + 1)..len {
        let Some(&ch) = text.get(i) else { break };
        if ch == '-' || is_alnum(u32::from(ch)) {
            continue;
        }
        if ch != '.' || i == marker + 1 {
            // The C++ subtracts on `size_t` here and relies on the wrap; the
            // checked form keeps the reachable semantics and drops the rest
            // (design brief D6).
            let host_end = if i == marker + 1 {
                i.checked_sub(2)
            } else {
                i.checked_sub(1)
            };
            let host_end = host_end?;
            if marker > 0 && host_end.checked_sub(at).is_some_and(|span| span >= 3) {
                text.truncate(host_end + 1);
                break;
            }
            return None;
        }
        marker = i;
    }

    let address: String = text.iter().collect();
    // A substring test, not a prefix test: an address that mentions `mailto:`
    // anywhere is left alone.
    if address.contains("mailto:") {
        Some(address)
    } else {
        Some(format!("mailto:{address}"))
    }
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::unreadable_literal,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::*;

    fn web(candidate: &str) -> Option<(String, usize, usize)> {
        check_web_link(candidate).map(|link| (link.url, link.range.start, link.range.len()))
    }

    // -- ported from CPDFLinkExtractTest.CheckMailLink ---------------------

    #[test]
    fn mail_addresses_that_are_rejected() {
        for invalid in [
            "",
            "peter.pan",
            "abc@server",
            "abc.@gmail.com",
            "abc@xyz&q.org",
            "abc@.xyz.org",
            "fan@g..com",
        ] {
            assert_eq!(check_mail_link(invalid), None, "{invalid:?}");
        }
    }

    #[test]
    fn mail_addresses_that_are_accepted() {
        for (input, expected) in [
            ("peter@abc.d", "mailto:peter@abc.d"),
            ("red.teddy.b@abc.com", "mailto:red.teddy.b@abc.com"),
            ("abc_@gmail.com", "mailto:abc_@gmail.com"),
            ("dummy-hi@gmail.com", "mailto:dummy-hi@gmail.com"),
            // Leading junk is trimmed off the local part.
            ("a..df@gmail.com", "mailto:df@gmail.com"),
            (".john@yahoo.com", "mailto:john@yahoo.com"),
            // Trailing junk is trimmed off the domain.
            ("abc@xyz.org?/", "mailto:abc@xyz.org"),
            ("fan{abc@xyz.org", "mailto:abc@xyz.org"),
            ("fan@g.com..", "mailto:fan@g.com"),
            // Case is preserved.
            ("CAP.cap@Gmail.Com", "mailto:CAP.cap@Gmail.Com"),
        ] {
            assert_eq!(
                check_mail_link(input).as_deref(),
                Some(expected),
                "{input:?}"
            );
        }
    }

    // -- ported from CPDFLinkExtractTest.CheckWebLink ---------------------

    #[test]
    fn web_addresses_that_are_rejected() {
        for invalid in [
            "",
            "http",
            "www.",
            "https-and-www",
            "http:/abc.com",
            "http://((()),",
            "ftp://example.com",
            "http:example.com",
            "http//[example.com",
            "http//[00:00:00:00:00:00",
            "http//[]",
            "abc.example.com",
        ] {
            assert_eq!(web(invalid), None, "{invalid:?}");
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the upstream table is one row per case and reads best whole"
    )]
    fn web_addresses_that_are_accepted() {
        // The upstream `kValidCases` table verbatim: the URL, the offset
        // it starts at within the candidate, and its length.
        for (input, url, start, count) in [
            // standard URL.
            (
                "http://www.example.com",
                "http://www.example.com",
                0usize,
                22usize,
            ),
            // with a port.
            (
                "http://www.example.com:88",
                "http://www.example.com:88",
                0usize,
                25usize,
            ),
            // with a username.
            (
                "http://test@www.example.com",
                "http://test@www.example.com",
                0usize,
                27usize,
            ),
            // with a password.
            (
                "http://test:test@example.com",
                "http://test:test@example.com",
                0usize,
                28usize,
            ),
            // a short domain.
            ("http://example", "http://example", 0usize, 14usize),
            // the www form rescues a broken scheme.
            ("http////www.server", "http://www.server", 8usize, 10usize),
            ("http:/www.abc.com", "http://www.abc.com", 6usize, 11usize),
            ("www.a.b.c", "http://www.a.b.c", 0usize, 9usize),
            ("https://a.us", "https://a.us", 0usize, 12usize),
            ("https://www.t.us", "https://www.t.us", 0usize, 16usize),
            // a hyphen in the host is fine.
            (
                "www.example-test.com",
                "http://www.example-test.com",
                0usize,
                20usize,
            ),
            // trailing junk is trimmed.
            (
                "www.example.com,",
                "http://www.example.com",
                0usize,
                15usize,
            ),
            (
                "www.example.com;(",
                "http://www.example.com",
                0usize,
                15usize,
            ),
            // leading junk is skipped.
            ("test:www.abc.com", "http://www.abc.com", 5usize, 11usize),
            // external brackets are trimmed.
            (
                "(http://www.abc.com)",
                "http://www.abc.com",
                1usize,
                18usize,
            ),
            (
                "0(http://www.abc.com)0",
                "http://www.abc.com",
                2usize,
                18usize,
            ),
            ("0(www.abc.com)0", "http://www.abc.com", 2usize, 11usize),
            // an unopened bracket is not trimmed.
            (
                "http://www.abc.com)0",
                "http://www.abc.com)0",
                0usize,
                20usize,
            ),
            // several levels of brackets.
            (
                "{(<http://www.abc.com>)}",
                "http://www.abc.com",
                3usize,
                18usize,
            ),
            // brackets inside the URL stay.
            (
                "[http://www.abc.com/z(1)]",
                "http://www.abc.com/z(1)",
                1usize,
                23usize,
            ),
            (
                "(http://www.abc.com/z(1))",
                "http://www.abc.com/z(1)",
                1usize,
                23usize,
            ),
            // quotes count as brackets.
            (
                "\"http://www.abc.com\"",
                "http://www.abc.com",
                1usize,
                18usize,
            ),
            // trailing periods are kept -- the trim is upstream of here.
            ("www.g.com..", "http://www.g.com..", 0usize, 11usize),
            // an IPv4 address.
            ("http://192.168.0.1", "http://192.168.0.1", 0usize, 18usize),
            (
                "http://192.168.0.1:80",
                "http://192.168.0.1:80",
                0usize,
                21usize,
            ),
            // an IPv6 reference.
            (
                "http://[aa::00:bb::00:cc:00]",
                "http://[aa::00:bb::00:cc:00]",
                0usize,
                28usize,
            ),
            (
                "http://[aa::00:bb::00:cc:00]:12",
                "http://[aa::00:bb::00:cc:00]:12",
                0usize,
                31usize,
            ),
            // the address itself is never validated.
            ("http://[aa]:12", "http://[aa]:12", 0usize, 14usize),
            ("http://[aa]:12abc", "http://[aa]:12", 0usize, 14usize),
            ("http://[aa]:", "http://[aa]", 0usize, 11usize),
            // a path suppresses all sanitizing.
            (
                "www.abc.com/#%%^&&*(",
                "http://www.abc.com/#%%^&&*(",
                0usize,
                20usize,
            ),
            (
                "www.a.com/#a=@?q=rr&r=y",
                "http://www.a.com/#a=@?q=rr&r=y",
                0usize,
                23usize,
            ),
            (
                "http://a.com/1/2/3/4\u{5}\u{6}",
                "http://a.com/1/2/3/4\u{5}\u{6}",
                0usize,
                22usize,
            ),
            (
                "http://www.example.com/foo;bar",
                "http://www.example.com/foo;bar",
                0usize,
                30usize,
            ),
            // invalid host characters are not validated.
            ("http://ex[am]ple", "http://ex[am]ple", 0usize, 16usize),
            (
                "http://:example.com",
                "http://:example.com",
                0usize,
                19usize,
            ),
            ("http://((())/path?", "http://((())/path?", 0usize, 18usize),
            (
                "http:////abc.server",
                "http:////abc.server",
                0usize,
                19usize,
            ),
            // non-ASCII is never validated either.
            (
                "www.\u{6d4b}\u{8bd5}.net",
                "http://www.\u{6d4b}\u{8bd5}.net",
                0usize,
                10usize,
            ),
            (
                "www.\u{6d4b}\u{8bd5}\u{3002}net\u{3002}",
                "http://www.\u{6d4b}\u{8bd5}\u{3002}net\u{3002}",
                0usize,
                11usize,
            ),
            (
                "www.\u{6d4b}\u{8bd5}.net；",
                "http://www.\u{6d4b}\u{8bd5}.net\u{ff1b}",
                0usize,
                11usize,
            ),
        ] {
            assert_eq!(
                web(input),
                Some((url.to_owned(), start, count)),
                "{input:?}"
            );
        }
    }

    #[test]
    fn the_scheme_form_needs_five_characters_after_http() {
        // "http://a" is eight characters, and the gate wants more than
        // `off + 4` = 8, so it fails; one more character passes.
        assert_eq!(web("http://a"), None);
        assert!(web("http://ab").is_some());
    }

    #[test]
    fn a_backwards_trim_from_offset_zero_cannot_underflow() {
        // The C++ reads out of bounds here; the range form simply does
        // nothing. Unreachable through the public path either way.
        let text: Vec<char> = "abc".chars().collect();
        let mut end = 2usize;
        trim_backwards_to(&text, 'z', 0, &mut end);
        assert_eq!(end, 2);
        trim_backwards_to(&text, 'b', 0, &mut end);
        assert_eq!(end, 0);
    }

    #[test]
    fn a_substring_past_the_end_yields_nothing_rather_than_clamping() {
        let text: Vec<char> = "abc".chars().collect();
        assert_eq!(substr(&text, 0, 3), "abc");
        // Past the end is empty, which is what keeps the D5 index mismatch
        // from being a panic.
        assert_eq!(substr(&text, 1, 9), "");
        assert_eq!(substr(&text, 9, 1), "");
        assert_eq!(substr(&text, 0, 0), "");
    }
}
