//! `--txt`: one page's characters as UTF-32LE, in its own file.
//!
//! Unlike the `--show-*` dumps this does not go to stdout: the oracle writes
//! `<input>.<page>.txt` beside the input PDF, and the harness harvests those
//! files. Three details of the format are load-bearing:
//!
//! - **One byte-order mark per file**, then four bytes per character and
//!   nothing else. A page with no text is a four-byte file.
//! - **No filtering.** The stream is `TextPage::chars`, not the search-facing
//!   text, so control characters, `\0` and hyphen sentinels are all written.
//!   Routing it through the text string would silently drop them, and 19 of
//!   the corpus's goldens hold exactly those values.
//! - **No separator between pages**, because each page is its own file.

use std::path::{Path, PathBuf};

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Name, Resolve};
use pdfrum_page::BuildContext;
use pdfrum_parser::PageDict;
use pdfrum_text::{ExtractOptions, TextPage};

/// The name the oracle writes a page's text to: the input path with the page
/// number and `txt` appended.
///
/// `None` when the result would be 256 bytes or longer, which is the oracle's
/// own cap — it prints a complaint and writes nothing.
#[must_use]
pub fn output_path(input: &Path, page: u32) -> Option<PathBuf> {
    let name = format!("{}.{page}.txt", input.display());
    (name.len() < 256).then(|| PathBuf::from(name))
}

/// The character stream as UTF-32LE with a leading byte-order mark —
/// exactly the bytes the oracle's `--txt` writes for one page.
///
/// One code unit per character, **unfiltered**: this reads
/// [`TextPage::chars`], not the search-facing text, and routing it through the
/// latter would drop the control characters and the hyphen sentinels that the
/// goldens contain.
///
/// This is `--txt`'s output format, so it lives with the tool that writes it
/// rather than on the library type it reads. `pdfrum-text` publishes the
/// data — `chars`, one `CharBox` per character with its `unicode` — and this
/// is one of the shapes a caller can put it in.
#[must_use]
pub fn to_utf32le(page: &TextPage) -> Vec<u8> {
    let mut out = Vec::with_capacity((page.chars.len() + 1) * 4);
    out.extend_from_slice(&0x0000_FEFFu32.to_le_bytes());
    for info in &page.chars {
        out.extend_from_slice(&info.unicode.to_le_bytes());
    }
    out
}

/// Extracts one page's text.
///
/// The graphics-state and font work happens in `pdfrum-page`; everything from
/// there on is `pdfrum-text`. A page that will not build yields no characters
/// rather than an error, matching the oracle, whose text page over a failed
/// load is simply empty.
#[must_use]
pub fn extract_page<R: Resolve>(
    page: &PageDict,
    resolver: &R,
    rtl: bool,
    ctx: &mut BuildContext,
) -> TextPage {
    let limits = Limits::default();
    let mut diags = Diagnostics::default();
    let built = crate::content::build(page, resolver, ctx, &limits, &mut diags);
    pdfrum_text::extract(
        &built,
        resolver,
        &ExtractOptions { rtl },
        &limits,
        &mut diags,
    )
}

/// The words one page draws, for `Doc.getPageNthWord` and
/// `Doc.getPageNumWords`.
///
/// A different reading of "word" from extraction's, and deliberately so —
/// see `pdfrum_text::words`. A page that will not build yields no words,
/// which is what an empty page gives.
///
/// Behind the `script` feature because `--js-transcript` is its only caller:
/// nothing else the tool prints counts words.
#[cfg(feature = "javascript")]
#[must_use]
pub fn page_words<R: Resolve>(
    page: &PageDict,
    resolver: &R,
    ctx: &mut BuildContext,
) -> Vec<String> {
    let limits = Limits::default();
    let mut diags = Diagnostics::default();
    let built = crate::content::build(page, resolver, ctx, &limits, &mut diags);
    pdfrum_text::words(&built)
}

/// Whether the document asks for right-to-left reading order
/// (`/Root /ViewerPreferences /Direction /R2L`).
///
/// Read as a **name**, which is how the viewer-preferences accessor reads it.
/// Extraction's whole line ordering turns on this, so a tool that ignored it
/// would diverge on every document that sets it.
#[must_use]
pub fn direction_is_r2l<R: Resolve>(catalog: &Dict, resolver: &R) -> bool {
    let Some(prefs) = catalog.dict(&Name::from("ViewerPreferences"), resolver) else {
        return false;
    };
    prefs
        .get(&Name::from("Direction"), resolver)
        .and_then(|value| {
            value
                .as_direct()
                .and_then(pdfrum_object::Object::as_name)
                .cloned()
        })
        .as_ref()
        .map(pdfrum_object::Name::as_bytes)
        == Some(b"R2L".as_slice())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unreadable_literal, reason = "test fixtures pin exact bytes")]

    use super::*;
    use pdfrum_object::{NoResolve, Object};

    #[test]
    fn the_utf32_dump_is_a_bare_mark_for_an_empty_page() {
        // The oracle writes a four-byte file for a page with no text, and the
        // harness's transcode reads that back as the empty string.
        assert_eq!(to_utf32le(&TextPage::default()), [0xFF, 0xFE, 0x00, 0x00]);
    }

    #[test]
    fn the_utf32_dump_writes_every_character_unfiltered() {
        use kurbo::{Affine, Point, Rect};
        use pdfrum_text::{CharBox, CharType};
        let unit = |unicode: u32| CharBox {
            char_type: CharType::Normal,
            unicode,
            code: None,
            origin: Point::ZERO,
            char_box: Rect::ZERO,
            loose_char_box: Rect::ZERO,
            matrix: Affine::IDENTITY,
            object: None,
            font_size: 1.0,
            angle: 0.0,
        };
        // 'a', the hyphen sentinel, 's' -- the `bug_781804.pdf` shape.
        let page = TextPage {
            chars: vec![unit(0x61), unit(0x02), unit(0x73)],
            ..TextPage::default()
        };
        assert_eq!(
            to_utf32le(&page),
            [
                0xFF, 0xFE, 0x00, 0x00, // BOM
                0x61, 0x00, 0x00, 0x00, // 'a'
                0x02, 0x00, 0x00, 0x00, // U+0002: the char list, not the buffer
                0x73, 0x00, 0x00, 0x00, // 's'
            ]
        );
        // And a zero is written as four zero bytes rather than skipped.
        let page = TextPage {
            chars: vec![unit(0)],
            ..TextPage::default()
        };
        assert_eq!(to_utf32le(&page), [0xFF, 0xFE, 0x00, 0x00, 0, 0, 0, 0]);
    }

    #[test]
    fn the_output_name_is_the_input_plus_page_and_extension() {
        assert_eq!(
            output_path(Path::new("/tmp/input.pdf"), 0),
            Some(PathBuf::from("/tmp/input.pdf.0.txt"))
        );
        assert_eq!(
            output_path(Path::new("a.pdf"), 12),
            Some(PathBuf::from("a.pdf.12.txt"))
        );
    }

    #[test]
    fn an_over_long_name_is_refused_rather_than_truncated() {
        let long = "x".repeat(260);
        assert_eq!(output_path(Path::new(&long), 0), None);
        // Just under the cap is fine.
        let short = "x".repeat(240);
        assert!(output_path(Path::new(&short), 0).is_some());
    }

    #[test]
    fn the_direction_preference_is_read_as_a_name() {
        // Absent.
        assert!(!direction_is_r2l(&Dict::new(), &NoResolve));
        // Present and R2L.
        let prefs = Dict::from_pairs([(Name::from("Direction"), Object::Name(Name::from("R2L")))]);
        let catalog = Dict::from_pairs([(Name::from("ViewerPreferences"), Object::Dict(prefs))]);
        assert!(direction_is_r2l(&catalog, &NoResolve));
        // Present and left-to-right.
        let prefs = Dict::from_pairs([(Name::from("Direction"), Object::Name(Name::from("L2R")))]);
        let catalog = Dict::from_pairs([(Name::from("ViewerPreferences"), Object::Dict(prefs))]);
        assert!(!direction_is_r2l(&catalog, &NoResolve));
        // A *string* "R2L" is not a name and does not count.
        let prefs = Dict::from_pairs([(
            Name::from("Direction"),
            Object::Str(pdfrum_object::PdfString::literal(b"R2L")),
        )]);
        let catalog = Dict::from_pairs([(Name::from("ViewerPreferences"), Object::Dict(prefs))]);
        assert!(!direction_is_r2l(&catalog, &NoResolve));
    }
}
