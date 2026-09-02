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
//! - `[oracle-bug]` The candidate is cut out of the **text** by offsets
//!   counted over the **character list**, and those two index spaces are not
//!   the same one. `cpdf_linkextract.cpp:123` and `:126` walk the char list
//!   (`CountChars`, `GetCharInfo`) while `:148` cuts with
//!   `page_text.Substr(start, nCount)`; they diverge wherever a character is
//!   in one and not the other — `AddCharInfo` (`cpdf_textpage.cpp:783-786`)
//!   pushes a non-normal char into `char_list_` without touching `text_buf_`,
//!   and normalization at `:808-813` appends several text chars for one input.
//!   PDFium **owns the converter it never calls**,
//!   `CharIndexFromTextIndex` (`cpdf_textpage.cpp:409`), and the wrong offsets
//!   flow on to `FPDFLink_GetTextRange` (`fpdf_text.cpp:599`), whose header
//!   documents them as *char* indices. pdf.js's autolinker carries exactly the
//!   reverse map PDFium skips (`autolinker.js:147`, `:176-180`). Here the
//!   candidate is cut in **text** space, converted through
//!   [`CharIndex`], and the reported range is
//!   converted back to char space.

use crate::charinfo::{CharBox, CharType};
use crate::index::CharIndex;
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
/// `chars` is the character list and `text` the search-facing text — two
/// different sequences, bridged by `index` rather than conflated.
#[must_use]
pub fn extract(chars: &[CharBox], text: &[char], index: &CharIndex) -> Vec<WebLink> {
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

        // `[oracle-bug]` Convert the char-list span into the text span before
        // cutting. A candidate whose characters are all absent from the text
        // has no text span at all, which is an empty candidate — the same
        // answer `Substr` gives out of range, reached for the right reason.
        let text_start = index.text_index_at_or_after(start);
        let mut candidate: String = match text_start {
            Some(first) if count > 0 => {
                let last = index.text_index_end(start + count - 1);
                substr(text, first, last.saturating_sub(first))
            }
            _ => String::new(),
        };
        if line_break {
            candidate.retain(|ch| ch != '\n' && ch != '\r');
            line_break = false;
        }
        // The soft hyphen the search-facing text carries at a line break reads
        // back as the hyphen it stood for, so a URL split across two lines is
        // still matched. `cpdf_linkextract.cpp:154-155` does exactly this —
        // over `U+FFFE`, because that is what its buffer holds; audit A41's
        // buffer half made ours hold the real `U+00AD` instead, so the repair
        // is the same repair over a character that is no longer a
        // noncharacter.
        candidate = candidate.replace('\u{00AD}', "-");

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
                    // `[oracle-bug]` `link.range` is an offset into the
                    // candidate, which was cut from the **text** at
                    // `text_start`; convert it back to char space, which is
                    // what `FPDFLink_GetTextRange` documents its output as.
                    // `cpdf_linkextract.cpp:157` adds the candidate offset to
                    // a char-list `start` instead, mixing the two spaces a
                    // second time.
                    let range = char_range(index, text_start, &link.range, start, count);
                    links.push(WebLink {
                        url: link.url,
                        range,
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

/// `[oracle-bug]` A candidate-relative range, converted back into char space.
///
/// `found` counts from `text_start` in the **text**; the reported range is a
/// **char** offset, which is what `FPDFLink_GetTextRange` (`fpdf_text.cpp:599`)
/// documents its output as. Falls back to the whole char span when a bound has
/// no char of its own — a text character the char list cannot name is a
/// malformed page, not a reason to report a wrong offset.
fn char_range(
    index: &CharIndex,
    text_start: Option<usize>,
    found: &Range<usize>,
    start: usize,
    count: usize,
) -> Range<usize> {
    let whole = start..start + count;
    let Some(first) = text_start else {
        return whole;
    };
    let (Some(from), Some(to)) = (
        index.char_index(first + found.start),
        index.char_index(first + found.end.saturating_sub(1)),
    ) else {
        return whole;
    };
    from..to + 1
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

    /// Audit item **A46**. The two index spaces diverge exactly where a
    /// character is in the char list but not in the text — which is what
    /// `AddCharInfo` (`cpdf_textpage.cpp:783-786`) produces for a non-normal
    /// character. Cutting the text by char-list offsets then slices the wrong
    /// bytes; `cpdf_linkextract.cpp:148` does exactly that.
    #[test]
    fn a_candidate_is_cut_in_text_space_and_reported_in_char_space() {
        use crate::charinfo::CharType;
        use kurbo::{Affine, Point, Rect};

        fn boxed(char_type: CharType, ch: char) -> CharBox {
            CharBox {
                char_type,
                unicode: u32::from(ch),
                code: Some(pdfrum_font::CharCode(u32::from(ch))),
                origin: Point::ZERO,
                char_box: Rect::ZERO,
                loose_char_box: Rect::ZERO,
                matrix: Affine::IDENTITY,
                object: None,
                font_size: 1.0,
                angle: 0.0,
            }
        }

        // Two hidden characters ahead of the URL: they are in the char list
        // and *not* in the text, so the two index spaces are offset by two.
        let mut chars: Vec<CharBox> = vec![
            boxed(CharType::NotUnicode, '\u{0002}'),
            boxed(CharType::NotUnicode, '\u{0003}'),
        ];
        chars.extend(
            "http://a.com "
                .chars()
                .map(|ch| boxed(CharType::Normal, ch)),
        );
        let text: Vec<char> = "http://a.com ".chars().collect();

        let links = extract(&chars, &text, &crate::index::build(&chars));
        assert_eq!(links.len(), 1, "the URL is found");
        // Cut in text space: the whole URL, not the two-character-short
        // prefix the char-list offsets would have taken.
        assert_eq!(links[0].url, "http://a.com");
        // Reported in char space: the URL starts at char 2, past the two
        // hidden characters.
        assert_eq!(links[0].range, 2..14);
    }

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
