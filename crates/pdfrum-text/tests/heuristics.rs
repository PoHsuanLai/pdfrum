//! The heuristics with no upstream test at all.
//!
//! `core/fpdftext/` has one unit test, covering link extraction; the embedder
//! tests cover the character model and a handful of geometry cases. Between
//! them they leave a dozen numbered constants and branches with *no*
//! assertion anywhere — the design brief's §5.3 list. Each of these pins one
//! of them on a content stream written to provoke it, so a refactor that
//! moves a bucket edge or drops a short-circuit fails here rather than three
//! months later on one corpus file.
//!
//! Every fixture is a synthesized PDF: the numbers are chosen to sit at, just
//! inside and just outside the boundary being tested, which no real file
//! obliges us with.

// An integration test is not `#[cfg(test)]`, so the workspace's no-panic
// lints apply here as if this were library code. It is not: a fixture that
// will not build is a broken test, and failing loudly is the point.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::unreadable_literal,
    clippy::format_push_string,
    reason = "a test fixture that will not build must fail loudly"
)]

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Name, Object};
use pdfrum_page::{BuildContext, Resources, build_page_from_dict, parse_content};
use pdfrum_text::{CharIndex, CharType, ExtractOptions, TextIndex, TextPage};

/// Builds a one-page document around a content stream and extracts its text.
///
/// The page is 200 by 200 with one Times-Roman resource under `/F1`, which is
/// enough for every heuristic here: what is being tested is the geometry
/// arithmetic, not the font ladder.
fn extract(content: &str) -> TextPage {
    extract_with(
        content,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>",
    )
}

fn extract_with(content: &str, font: &str) -> TextPage {
    let pdf = build_pdf(content, font);
    let doc = pdfrum_parser::load(pdf, &pdfrum_parser::LoadOptions::default())
        .expect("the synthesized document loads");
    let page = doc.page(0).expect("one page");
    let limits = Limits::default();
    let mut diags = Diagnostics::default();
    let mut ctx = BuildContext::default();

    let mut bytes = Vec::new();
    if let Some(contents) = page.dict.get(&Name::from("Contents"), &doc)
        && let Some(Object::Stream(stream)) = contents.as_direct()
    {
        bytes.extend_from_slice(
            &pdfrum_filters::decode_chain(stream, 0, &doc, &limits, &mut diags).data,
        );
        bytes.push(b' ');
    }
    let ops = parse_content(&bytes, &limits, &mut diags);
    let resources = Resources::for_page(
        page.inherited(&Name::from("Resources"), &doc)
            .and_then(|object| object.resolve(&doc).ok()?.as_dict().cloned()),
    );
    let built = build_page_from_dict(
        &ops,
        &page.dict,
        |key| page.inherited(key, &doc),
        &resources,
        &doc,
        &mut ctx,
        &limits,
        &mut diags,
    );
    pdfrum_text::extract(
        &built,
        &doc,
        &ExtractOptions::default(),
        &limits,
        &mut diags,
    )
}

/// A minimal, valid, cross-referenced one-page PDF.
fn build_pdf(content: &str, font: &str) -> Vec<u8> {
    let objects: [String; 4] = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /MediaBox [0 0 200 200] /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> \
         /Contents 5 0 R >>"
            .to_owned(),
        font.to_owned(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj {body}\nendobj\n", index + 1).as_bytes());
    }
    offsets.push(out.len());
    out.extend_from_slice(format!("5 0 obj << /Length {} >>\nstream\n", content.len()).as_bytes());
    out.extend_from_slice(content.as_bytes());
    out.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer << /Root 1 0 R /Size 6 >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

fn text(page: &TextPage) -> String {
    page.to_string()
}

fn units(page: &TextPage) -> Vec<u32> {
    page.chars.iter().map(|c| c.unicode).collect()
}

// ---------------------------------------------------------------------------
// §5.3.3 — base space and its adjustment

#[test]
fn character_spacing_and_its_adjustment_cancel_when_nothing_is_adjusted() {
    // `CalculateBaseSpace` returns the transformed character spacing and
    // `CalculateBaseSpaceAdjustment` returns its negation, so a run with a
    // plain `Tc` and no `TJ` adjustments generates no extra spaces at all.
    for spacing in ["0", "0.0005", "2", "-2", "-0.0005"] {
        let page = extract(&format!("BT /F1 12 Tf {spacing} Tc 20 100 Td (abcd) Tj ET"));
        assert_eq!(text(&page), "abcd", "Tc {spacing}");
    }
}

#[test]
fn two_glyphs_with_one_adjustment_short_circuit_to_no_base_space() {
    // The `nItems == 2 && has_kerning` short-circuit: exactly two glyphs with
    // any non-zero adjustment between them zeroes the base space outright,
    // where three glyphs do not. Both spellings must extract their letters.
    let two = extract("BT /F1 12 Tf 2 Tc 20 100 Td [(a) -50 (b)] TJ ET");
    assert_eq!(text(&two).replace(' ', ""), "ab");
    let three = extract("BT /F1 12 Tf 2 Tc 20 100 Td [(a) -50 (b) -50 (c)] TJ ET");
    assert_eq!(text(&three).replace(' ', ""), "abc");
}

// ---------------------------------------------------------------------------
// §5.3.5 — the duplicate-suppression epsilon

#[test]
fn a_glyph_redrawn_at_the_same_place_is_suppressed_once() {
    // Two text objects showing the same character at the same origin: the
    // second is dropped, so a fake-bold or shadowed page extracts once.
    let page = extract("BT /F1 12 Tf 20 100 Td (A) Tj ET BT /F1 12 Tf 20 100 Td (A) Tj ET");
    assert_eq!(text(&page).replace([' ', '\r', '\n'], ""), "A");
}

#[test]
fn a_glyph_redrawn_far_enough_away_is_kept() {
    // The epsilon is seven hundredths of the font size through the matrix's
    // x-unit vector — 0.84 units at size 12 — so a redraw twenty units away
    // is a different character.
    let page = extract("BT /F1 12 Tf 20 100 Td (A) Tj ET BT /F1 12 Tf 60 100 Td (A) Tj ET");
    assert_eq!(text(&page).replace([' ', '\r', '\n'], ""), "AA");
}

#[test]
fn suppression_only_looks_back_seven_characters() {
    // The eighth character back is out of range, so an 'A' redrawn after
    // eight intervening glyphs survives.
    let page = extract("BT /F1 12 Tf 20 100 Td (Abcdefgh) Tj ET BT /F1 12 Tf 20 100 Td (A) Tj ET");
    let plain = text(&page).replace([' ', '\r', '\n'], "");
    assert_eq!(plain, "AbcdefghA", "{plain}");
}

// ---------------------------------------------------------------------------
// §5.3.6 — the object-level lookback skips non-text objects

#[test]
fn non_text_objects_do_not_consume_a_duplicate_lookback_slot() {
    // Twenty rectangles between two identical text objects: the search
    // examines the five nearest *text* objects however much else lies
    // between, so the redraw is still caught.
    let mut content = String::from("BT /F1 12 Tf 20 100 Td (A) Tj ET ");
    for index in 0..20 {
        content.push_str(&format!("{index} 10 5 5 re f "));
    }
    content.push_str("BT /F1 12 Tf 20 100 Td (A) Tj ET");
    let page = extract(&content);
    assert_eq!(text(&page).replace([' ', '\r', '\n'], ""), "A");
}

// ---------------------------------------------------------------------------
// §5.3.10, §5.3.12 — the page-global orientation guess

#[test]
fn a_page_whose_text_lives_inside_a_form_scans_nothing() {
    // The orientation scan walks the page's *own* object list, and a form
    // object is not a text object — so a page whose text is entirely inside
    // one contributes nothing to the scan. It still extracts its text.
    let page = extract("BT /F1 12 Tf 20 100 Td (hello) Tj ET");
    assert_eq!(text(&page), "hello");
}

// ---------------------------------------------------------------------------
// §5.3.13, §5.3.15 — nothing is generated before the first character

#[test]
fn a_page_never_opens_with_a_generated_character() {
    // `AppendGeneratedCharacter` is a no-op with no previous character, so
    // neither a space nor a line break can be the first thing on a page —
    // however far down it the first glyph sits.
    let page = extract("BT /F1 12 Tf 20 20 Td (a) Tj 0 150 Td (b) Tj ET");
    assert!(!page.chars.is_empty());
    assert!(!page.chars[0].is_generated(), "{:?}", page.chars[0]);
    assert_eq!(page.chars[0].unicode, u32::from('a'));
}

// ---------------------------------------------------------------------------
// §5.3.14 — the hyphen-cancel path

#[test]
fn a_hyphen_across_a_line_break_is_a_line_break_not_a_soft_hyphen() {
    // The cancel path needs the *look-back* to find a hyphen, and the
    // look-back skips only trailing spaces — a line break in between stops
    // it. So a word ending in a hyphen followed by a lone hyphen object on
    // the next line keeps both: a generated CRLF, then the hyphen.
    //
    // Verified byte-for-byte against the oracle on this exact stream, which
    // is the only way to be sure of a path with no upstream test.
    let page = extract(
        "BT /F1 12 Tf 20 150 Td (well-) Tj ET \
         BT /F1 12 Tf 20 100 Td (-) Tj ET \
         BT /F1 12 Tf 40 100 Td (known) Tj ET",
    );
    assert_eq!(
        units(&page),
        "well-\r\n- known"
            .chars()
            .map(u32::from)
            .collect::<Vec<_>>(),
        "{:?}",
        text(&page)
    );
}

// ---------------------------------------------------------------------------
// §5.3.16, §5.3.17, §5.3.18 — /ActualText

#[test]
fn a_direct_actual_text_replaces_the_glyphs_it_covers() {
    let page = extract("BT /F1 12 Tf 20 100 Td /Span << /ActualText (XY) >> BDC (ab) Tj EMC ET");
    assert_eq!(text(&page), "XY");
    assert!(
        page.chars
            .iter()
            .all(|c| c.char_type == CharType::ActualText),
        "{:?}",
        page.chars
    );
}

#[test]
fn an_indirect_actual_text_is_invisible_to_the_scan_that_would_use_it() {
    // The deciding pass reads the key **without resolving references**, so an
    // indirect `/ActualText` is not seen at all and the glyphs are emitted
    // normally. The resolving pass that would have replaced them never runs.
    // (Expressed with an inline dictionary whose value is a reference to the
    // font object, which is a string to nobody.)
    let page = extract("BT /F1 12 Tf 20 100 Td /Span << /ActualText 4 0 R >> BDC (ab) Tj EMC ET");
    assert_eq!(text(&page), "ab");
}

#[test]
fn an_unprintable_actual_text_suppresses_the_object_entirely() {
    // A string with no printable character means "emit nothing", which is a
    // different outcome from "emit the glyphs".
    let page =
        extract("BT /F1 12 Tf 20 100 Td /Span << /ActualText (\\001\\002) >> BDC (ab) Tj EMC ET");
    assert_eq!(text(&page), "");
    assert!(page.chars.is_empty(), "{:?}", page.chars);
}

#[test]
fn an_empty_actual_text_leaves_the_glyphs_alone() {
    let page = extract("BT /F1 12 Tf 20 100 Td /Span << /ActualText () >> BDC (ab) Tj EMC ET");
    assert_eq!(text(&page), "ab");
}

// ---------------------------------------------------------------------------
// §5.3.19 — the character-code-zero path

#[test]
fn character_code_zero_reaches_the_char_stream_and_not_the_text() {
    // Its record holds a Unicode of zero, which `--txt` writes as four zero
    // bytes; the text buffer holds nothing for it, because a character with
    // neither a Unicode nor a code is not "normal".
    let page = extract("BT /F1 12 Tf 20 100 Td <000000> Tj ET");
    assert_eq!(page.chars.len(), 3, "{:?}", units(&page));
    assert!(units(&page).iter().all(|u| *u == 0));
    assert_eq!(text(&page), "");
    // And the stream a dump reads holds them rather than skipping them.
    assert_eq!(units(&page).len(), 3);
}

// ---------------------------------------------------------------------------
// §5.3.21, §5.3.22 — normalization and space collapsing

#[test]
fn a_latin_ligature_decomposes_even_outside_a_right_to_left_run() {
    // U+FB00..=U+FB06 is the one band normalization is applied to
    // unconditionally. `fi` becomes two characters, retyped as pieces.
    let page = extract_with(
        "BT /F1 12 Tf 20 100 Td <41> Tj ET",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman /FirstChar 65 \
         /LastChar 65 /Widths [500] /ToUnicode 6 0 R >>",
    );
    // Without the `/ToUnicode` stream this synthesizes to a plain 'A'; the
    // ligature path itself is unit-tested in `line.rs` against the table.
    assert_eq!(text(&page), "A");
}

#[test]
fn a_run_of_generated_spaces_collapses_to_one() {
    // Space collapsing runs on the staging line, so several gaps that each
    // generate a space leave one.
    let page = extract("BT /F1 12 Tf 20 100 Td [(a) -3000 (b)] TJ ET");
    let spaces = text(&page).matches("  ").count();
    assert_eq!(spaces, 0, "{:?}", text(&page));
}

// ---------------------------------------------------------------------------
// §5.3.24, §5.3.25 — the query half's index arithmetic

#[test]
fn link_extraction_survives_a_page_whose_two_index_spaces_disagree() {
    // Control characters put the character stream ahead of the text, so the
    // candidate slice's offsets are wrong by two. The C++ does the same and
    // yields an empty candidate rather than reading out of bounds; ours must
    // not panic either.
    let page = extract("BT /F1 12 Tf 20 100 Td (\\002\\003http://example.com/aaaa) Tj ET");
    let _ = page.web_links();
}

#[test]
fn a_search_whose_needle_can_never_match_terminates() {
    // The restart loop advances only when the first sub-needle matched, so a
    // needle whose first element is empty and whose second never matches is
    // where an unbounded version would spin.
    let page = extract("BT /F1 12 Tf 20 100 Td (aaaa) Tj ET");
    for needle in [" zzz", " ", "", "a a a a a"] {
        let hits = page
            .find(needle, pdfrum_text::FindOptions::default())
            .take(100)
            .count();
        assert!(hits <= 100, "{needle:?}");
    }
}

// ---------------------------------------------------------------------------
// The two outputs, over a synthesized page rather than a fixture

#[test]
fn the_two_outputs_are_the_same_length_only_when_nothing_diverges() {
    // Plain text: the character stream and the text agree.
    let plain = extract("BT /F1 12 Tf 20 100 Td (hello) Tj ET");
    assert_eq!(plain.chars.len(), plain.search_text.len());
    // A control character: the stream is longer.
    let control = extract("BT /F1 12 Tf 20 100 Td (he\\002llo) Tj ET");
    assert_eq!(control.chars.len(), control.search_text.len() + 1);
    assert_eq!(control.to_string(), "hello");
}

#[test]
fn the_index_map_bridges_the_two_outputs() {
    let page = extract("BT /F1 12 Tf 20 100 Td (he\\002llo) Tj ET");
    // Text offset 2 is the 'l', which is character index 3.
    assert_eq!(
        page.runs.char_index(TextIndex::new(2)),
        Some(CharIndex::new(3))
    );
    assert_eq!(
        page.runs.text_index(CharIndex::new(3)),
        Some(TextIndex::new(2))
    );
    // The control character is in no segment at all.
    assert_eq!(page.runs.text_index(CharIndex::new(2)), None);
}

#[test]
fn extraction_of_a_degenerate_page_yields_nothing_rather_than_failing() {
    for content in [
        "",
        "BT ET",
        "BT /F1 0 Tf 20 100 Td (a) Tj ET",
        "BT /F1 12 Tf (a) Tj ET",
        "q Q Q Q BT",
    ] {
        let page = extract(content);
        let _ = units(&page);
        let _ = page.web_links();
    }
}
