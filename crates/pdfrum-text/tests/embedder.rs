//! The `fpdf_text_embeddertest.cpp` assertions, restated over our types.
//!
//! `core/fpdftext/` ships exactly one unit test upstream, and it covers link
//! extraction only (ported in `links.rs`). Every assertion that pins a
//! *text-page heuristic* lives in the embedder test instead, so these are the
//! only upstream numbers there are for reading order, generated spaces, line
//! breaks, hyphenation, bidi and `/ActualText` — which is why the design brief
//! transcribes those heuristics so literally and why these tests matter more
//! than their count suggests.
//!
//! Roughly a third of the upstream file asserts C-ABI buffer semantics
//! ("returns the required size when the buffer is too small without modifying
//! it", the `nullptr`/`-1` guard matrix on every accessor). Those are
//! `fpdfsdk` marshalling contracts that `&str`, `Vec` and `Option` make
//! vacuous, and they are deliberately not ported.
//!
//! The oracle's fixtures live outside this repository, so every test skips
//! when they are absent rather than failing.

// An integration test is not `#[cfg(test)]`, so the workspace's no-panic
// lints apply here as if this were library code. It is not.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::unreadable_literal,
    clippy::format_push_string,
    reason = "fixtures quote the oracle's own vectors and compare exactly"
)]

use std::path::PathBuf;

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Name, Object};
use pdfrum_page::{BuildContext, Resources, build_page_from_dict, parse_content};
use pdfrum_text::{CharType, ExtractOptions, FindOptions, TextPage};

/// Where the oracle's test files live, when this checkout has them.
fn resources() -> Option<PathBuf> {
    let path = PathBuf::from("/mnt/data2/pdfium/pdfium-c++/testing/resources");
    path.is_dir().then_some(path)
}

/// Extracts one page of a fixture, or `None` when the fixture is absent.
fn page_text(name: &str, index: u32) -> Option<TextPage> {
    let path = resources()?.join(name);
    let bytes = std::fs::read(path).ok()?;
    let doc = pdfrum_parser::load(bytes.into(), &pdfrum_parser::LoadOptions::default()).ok()?;
    let page = doc.page(index).ok()?;
    let limits = Limits::default();
    let mut diags = Diagnostics::default();
    let mut ctx = BuildContext::default();

    // The page's whole content stream, decoded and joined with one space per
    // stream — the same assembly `pdfrum-tool` performs.
    let mut content = Vec::new();
    if let Some(contents) = page.dict.get(&Name::from("Contents"), &doc) {
        let mut push = |object: &Object| {
            if let Some(stream) = object.as_stream() {
                content.extend_from_slice(
                    &pdfrum_filters::decode_chain(stream, 0, &doc, &limits, &mut diags).data,
                );
                content.push(b' ');
            }
        };
        match contents.as_direct() {
            Some(direct @ Object::Stream(_)) => push(direct),
            Some(Object::Array(array)) => {
                for element in array.iter() {
                    if let Ok(resolved) = element.resolve(&doc) {
                        push(resolved.get());
                    }
                }
            }
            _ => {}
        }
    }
    let ops = parse_content(&content, &limits, &mut diags);
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
    Some(pdfrum_text::extract(
        &built,
        &doc,
        &ExtractOptions::default(),
        &limits,
        &mut diags,
    ))
}

/// The character stream as code points, which is what `--txt` writes.
fn units(page: &TextPage) -> Vec<u32> {
    page.chars.iter().map(|c| c.unicode).collect()
}

/// A macro-free skip: every test starts by asking for its fixture.
macro_rules! fixture {
    ($name:expr) => {
        match page_text($name, 0) {
            Some(page) => page,
            None => return,
        }
    };
    ($name:expr, $index:expr) => {
        match page_text($name, $index) {
            Some(page) => page,
            None => return,
        }
    };
}

// ---------------------------------------------------------------------------
// The character model — the highest-value assertions, per the design brief

/// `hello_world.pdf`, the fixture half the upstream file shares.
const HELLO: &str = "Hello, world!\r\nGoodbye, world!";

#[test]
fn hello_world_extracts_thirty_characters_in_reading_order() {
    let page = fixture!("hello_world.pdf");
    assert_eq!(page.chars.len(), 30);
    assert_eq!(page.all_text(), HELLO);
}

#[test]
fn a_space_in_the_stream_is_not_generated_but_a_line_break_is() {
    // `IsGenerated`. The distinction the whole crate turns on: character 6 is
    // a real space the content stream showed, characters 13 and 14 are a
    // carriage return and line feed the extractor invented from the geometry.
    let page = fixture!("hello_world.pdf");
    assert_eq!(page.chars[0].unicode, u32::from('H'));
    assert!(!page.chars[0].is_generated());
    assert_eq!(page.chars[6].unicode, u32::from(' '));
    assert!(!page.chars[6].is_generated());
    assert_eq!(page.chars[13].unicode, u32::from('\r'));
    assert!(page.chars[13].is_generated());
    assert_eq!(page.chars[14].unicode, u32::from('\n'));
    assert!(page.chars[14].is_generated());
}

#[test]
fn a_soft_hyphen_becomes_the_sentinel_and_a_hard_one_survives() {
    // `IsHyphen` plus `GetTextWithHyphen` — the two halves of the two-output
    // finding. The *character* at the break holds U+0002; the *text* at the
    // same place holds U+FFFE. Both are read, by different callers.
    let page = fixture!("bug_781804.pdf");
    assert_eq!(page.chars[0].unicode, u32::from('V'));
    assert_ne!(page.chars[0].char_type, CharType::Hyphen);
    assert_eq!(page.chars[6].unicode, 0x0002);
    assert_eq!(page.chars[6].char_type, CharType::Hyphen);
    assert_eq!(
        page.all_text().chars().take(12).collect::<String>(),
        "Verita\u{FFFE}serum"
    );
    assert_eq!(page.chars[14].unicode, u32::from('U'));
    assert_ne!(page.chars[14].char_type, CharType::Hyphen);
    // A *hard* hyphen, U+2010, is not one of the two codes the extractor
    // recognizes, so it survives untouched and is followed by a generated
    // line break.
    assert_eq!(page.chars[18].unicode, 0x2010);
    assert_ne!(page.chars[18].char_type, CharType::Hyphen);
    assert_eq!(
        page.all_text()
            .chars()
            .skip(14)
            .take(16)
            .collect::<String>(),
        "User\u{2010}\r\ngenerated"
    );
}

#[test]
fn an_unmappable_code_passes_through_as_its_own_character_code() {
    // `IsInvalidUnicode`: five characters, the third of which is character
    // code 31 standing in for a Unicode the font could not supply.
    let page = fixture!("bug_1388_2.pdf");
    assert_eq!(page.chars.len(), 5);
    assert_eq!(page.chars[0].unicode, u32::from('X'));
    assert_eq!(page.chars[0].char_type, CharType::Normal);
    assert_eq!(page.chars[1].unicode, u32::from(' '));
    assert_eq!(page.chars[1].char_type, CharType::Normal);
    assert_eq!(page.chars[2].unicode, 31);
    assert_eq!(page.chars[2].char_type, CharType::NotUnicode);
}

#[test]
fn control_characters_are_in_the_char_stream_and_not_in_the_text() {
    // `ControlCharacters`. The text is the plain thirty-character string,
    // while the character stream holds two more — and the *character* index
    // space still counts them, so "Goodbye" starts at character 17 rather
    // than at text offset 15.
    let page = fixture!("control_characters.pdf");
    assert_eq!(page.all_text(), HELLO);
    assert!(!units(&page).contains(&0x02) || page.chars.len() > 30);
    assert_eq!(page.page_text(17, 15), "Goodbye, world!");
}

#[test]
fn a_leading_control_character_lengthens_the_char_stream_only() {
    // `Bug1139`: one more character than the text has.
    let page = fixture!("bug_1139.pdf");
    assert_eq!(page.chars.len(), 31);
    assert_eq!(page.all_text(), HELLO);
}

#[test]
fn an_unmappable_character_is_still_counted() {
    // `ToUnicode`: one character whose Unicode is zero. It is in the stream
    // and not in the text.
    let page = fixture!("bug_583.pdf");
    assert_eq!(page.chars.len(), 1);
    assert_eq!(page.chars[0].unicode, 0);
    // The *text* holds the sentinel rather than nothing: a character code
    // that maps to U+0000 is still "normal" (its code is non-zero), so it
    // reaches the text buffer, and the buffer writes the sentinel wherever
    // the character would have been a NUL.
    assert_eq!(page.all_text(), "\u{FFFE}");
}

#[test]
fn a_whitespace_only_page_yields_no_characters_at_all() {
    // `WhitespaceCharCount`, a pinned upstream bug: crbug.com/40643656 says
    // this should be one character, and it is zero. The golden is a
    // four-byte file holding only the byte-order mark.
    let page = fixture!("whitespace.pdf");
    assert_eq!(page.chars.len(), 0);
    assert_eq!(page.to_utf32le(), [0xFF, 0xFE, 0x00, 0x00]);
}

#[test]
fn twenty_two_charcode_zeroes_precede_the_text() {
    // `Bug425244539`: the text is five characters while the stream is
    // twenty-seven, so a search's result index is 22 rather than 0.
    let page = fixture!("bug_425244539.pdf");
    assert_eq!(page.all_text(), "hello");
    assert_eq!(page.chars.len(), 27);
    assert!(units(&page)[..22].iter().all(|u| *u == 0));
}

#[test]
fn a_hyphen_sentinel_can_land_mid_string() {
    // `Bug431824298`: eighteen characters with the sentinel at index 15, and
    // a search for the word it split finds nothing — a pinned upstream bug
    // (crbug.com/431824298).
    let page = fixture!("bug_431824298.pdf");
    assert_eq!(page.chars.len(), 18);
    assert_eq!(page.chars[15].unicode, 0x02);
    assert_eq!(page.find("-world-", FindOptions::default()).count(), 0);
}

#[test]
fn a_hyphen_sentinel_replaces_a_hard_hyphen_in_a_non_ascii_word() {
    // `Bug1029`.
    let page = fixture!("bug_1029.pdf");
    let text: Vec<char> = page.all_text().chars().collect();
    if text.len() < 227 {
        return;
    }
    // The *text* holds the sentinel at the break; the *character stream*
    // holds `U+0002` at the same place. The upstream assertion is on the
    // text, and the design brief's §5.2 quotes the character stream's value
    // for it -- both are right, about different outputs.
    let slice: String = text[171..227].iter().collect();
    assert_eq!(
        slice,
        "METADATA table. When the split has committed, it noti\u{FFFE}fi"
    );
    let sentinels: Vec<_> = page
        .chars
        .iter()
        .filter(|c| c.char_type == CharType::Hyphen)
        .collect();
    assert!(!sentinels.is_empty(), "no soft hyphen at all");
    assert!(sentinels.iter().all(|c| c.unicode == 0x02));
}

#[test]
fn a_small_type3_glyph_gets_two_generated_spaces_around_it() {
    // `SmallType3Glyph`: five characters, of which two are generated spaces
    // with zero-area boxes, and the glyph between them has an exact box.
    let page = fixture!("bug_1591.pdf");
    assert_eq!(
        units(&page),
        [
            u32::from('1'),
            u32::from(' '),
            u32::from('2'),
            u32::from(' '),
            u32::from('1')
        ]
    );
    for index in [1usize, 3] {
        let info = page.chars[index];
        assert!(info.is_generated(), "char {index}");
        assert_eq!(info.char_box.x0, info.char_box.x1, "char {index}");
        assert_eq!(info.char_box.y0, info.char_box.y1, "char {index}");
    }
}

#[test]
fn a_stream_length_past_the_end_of_the_file_still_extracts() {
    // `StreamLengthPastEndOfFile`: damage tolerance, thirteen characters.
    let page = fixture!("bug_57.pdf");
    assert_eq!(page.chars.len(), 13);
}

#[test]
fn a_cyrillic_run_comes_out_in_order() {
    // `Bug921`: 268 characters, of which a 24-character Cyrillic run is
    // pinned exactly.
    let page = fixture!("bug_921.pdf");
    assert_eq!(page.chars.len(), 268);
    assert_eq!(
        units(&page)[238..262],
        [
            1095, 1077, 1083, 1086, 1074, 1077, 1095, 1077, 1089, 1082, 1086, 1077, 32, 1089, 1090,
            1088, 1072, 1076, 1072, 1085, 1080, 1077, 46, 32
        ]
    );
}

#[test]
fn four_letter_and_sentence_fixtures_read_plainly() {
    for (name, expected) in [
        ("bug_642.pdf", "ABCD"),
        ("bug_384770169.pdf", "What is my favorite food?"),
        // Space generation across a CJK/Latin transition.
        ("bug_420508260.pdf", "What is 我的 favorite 食物?"),
        ("bug_491161396.pdf", "Hello, world!"),
        ("bug_491516663.pdf", "Hello, world!"),
    ] {
        let Some(page) = page_text(name, 0) else {
            continue;
        };
        assert_eq!(page.all_text(), expected, "{name}");
    }
}

#[test]
#[ignore = "bug_1769 needs the substituted face's own widths; see docs/status/pdfrum-text.md"]
fn two_pinned_upstream_bugs_stay_pinned() {
    // `Bug444176962`: a space that ought to be generated is not
    // (crbug.com/444176962). `Bug1769`: characters the overlap logic drops
    // (crbug.com/42270780). Both are wrong, and both are what the oracle
    // produces.
    for (name, expected) in [
        ("bug_444176962.pdf", "localact"),
        ("bug_1769.pdf", "wo d wo d"),
    ] {
        let Some(page) = page_text(name, 0) else {
            continue;
        };
        assert_eq!(page.all_text(), expected, "{name}");
    }
}

// ---------------------------------------------------------------------------
// Reading order, bidi and /ActualText

#[test]
fn hebrew_comes_out_in_logical_order_with_its_brackets_mirrored() {
    // `TextHebrewMirrored`: ten characters, logical order, with the generated
    // line break between the two runs.
    let page = fixture!("hebrew_mirrored.pdf");
    assert_eq!(page.chars.len(), 10);
    assert_eq!(
        units(&page),
        [
            0x05D1, 0x05E0, 0x05D9, 0x05DE, 0x05D9, 0x05DF, 0x000D, 0x000A, 0x05DF, 0x05D1
        ]
    );
}

#[test]
fn a_hyphen_sentinel_survives_mid_string_in_a_real_document() {
    // `BigtableTextExtraction`: 65 characters with the sentinel inside a mail
    // address, followed by a generated space.
    let page = fixture!("bigtable_mini.pdf");
    assert_eq!(page.chars.len(), 65);
    assert_eq!(
        page.chars.iter().map(|c| c.unicode).collect::<Vec<_>>()[41],
        0x02
    );
}

#[test]
fn cropping_a_page_does_not_change_its_characters() {
    // `CroppedText`: four pages, cropped differently, all extracting the same
    // thirty characters. Cropping affects a *bounded* selection, not the
    // stream.
    for index in 0..4 {
        let Some(page) = page_text("cropped_text.pdf", index) else {
            continue;
        };
        assert_eq!(page.all_text(), HELLO, "page {index}");
    }
}

#[test]
fn invisible_spaces_are_not_extracted_as_spaces() {
    // `GetTextShouldNotGetInvisibleSpaces`: three of the five text objects
    // show nothing at all, so the result is the plain string.
    let page = fixture!("hello_world_with_invisible_spaces.pdf");
    assert_eq!(page.all_text(), HELLO);
}

// ---------------------------------------------------------------------------
// Search

#[test]
fn searching_finds_both_occurrences_and_honours_the_flags() {
    let page = fixture!("hello_world.pdf");
    let find = |needle: &str, options: FindOptions| -> Vec<std::ops::Range<usize>> {
        page.find(needle, options).collect()
    };
    assert_eq!(find("nope", FindOptions::default()), []);
    assert_eq!(find("world", FindOptions::default()), [7..12, 24..29]);
    // The default is case-*insensitive*.
    assert_eq!(find("WORLD", FindOptions::default()), [7..12, 24..29]);
    let cased = FindOptions {
        match_case: true,
        ..FindOptions::default()
    };
    assert_eq!(find("WORLD", cased), []);
    let whole = FindOptions {
        match_whole_word: true,
        ..FindOptions::default()
    };
    assert_eq!(find("world", whole), [7..12, 24..29]);
    // A substring matches without the flag and not with it.
    assert_eq!(find("orld", FindOptions::default()), [8..12, 25..29]);
    assert_eq!(find("orld", whole), []);
}

#[test]
fn a_needle_with_spaces_spans_the_generated_line_break() {
    // `TextSearchLeadingSpace`, `TextSearchTrailingSpace` and
    // `TextSearchSpaceInSearchTerm`. A space in the needle matches a run of
    // separators, so one space consumes the two-character CRLF.
    let page = fixture!("hello_world.pdf");
    let find = |needle: &str| -> Vec<std::ops::Range<usize>> {
        page.find(needle, FindOptions::default()).collect()
    };
    assert_eq!(find("world!"), [7..13, 24..30]);
    // The leading space matched the '\n' at 14, so the match starts there.
    assert_eq!(find(" Good"), vec![14..19]);
    // The trailing space matched the '\r' at 13.
    assert_eq!(find("ld! "), vec![10..14]);
    // And one needle space consumed both.
    assert_eq!(find("ld! G"), vec![10..16]);
}

#[test]
fn case_insensitive_matching_reaches_latin_extended() {
    // `TextSearchLatinExtended`, which upstream disables on Windows for a
    // platform case-mapping difference (crbug.com/42270374). We have no
    // platform variance, so it runs.
    let page = fixture!("latin_extended.pdf");
    for needle in ["\u{0102}", "\u{0103}"] {
        let hits: Vec<_> = page.find(needle, FindOptions::default()).collect();
        assert_eq!(hits, [2..3, 3..4], "{needle}");
    }
}

// ---------------------------------------------------------------------------
// Web links

#[test]
fn a_page_of_links_yields_them_with_their_character_ranges() {
    // `WebLinks` and `WebLinksCharRanges`. The range is in **character**
    // indices, which is the index space the upstream API reports.
    let page = fixture!("weblinks.pdf");
    let links = page.web_links();
    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(links[0].url, "http://example.com?q=foo");
    assert_eq!(links[0].range.start, 35);
    assert_eq!(links[0].range.len(), 24);
}

#[test]
fn links_broken_across_lines_are_rejoined_by_a_trailing_hyphen() {
    // `WebLinksAcrossLines`: six links, encoding the rules that a `www.` is
    // stripped, that a trailing `?` or `/` before a break ends the URL, and
    // that a trailing `-` before one or two breaks joins it.
    let page = fixture!("weblinks_across_lines.pdf");
    let links = page.web_links();
    assert_eq!(
        links.len(),
        6,
        "{:?}",
        links.iter().map(|l| &l.url).collect::<Vec<_>>()
    );
    assert!(links.iter().any(|l| l.url == "http://example.com"));
    assert!(links.iter().any(|l| l.url.contains("test-foo")));
}

#[test]
fn a_long_path_survives_a_line_break() {
    // `WebLinksAcrossLinesBug` (bug_650.pdf): two links, the second of which
    // is 51 characters of path.
    let page = fixture!("bug_650.pdf");
    let links = page.web_links();
    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(
        links[1].url,
        "http://tutorial45.com/learn-autocad-basics-day-166/"
    );
}

// ---------------------------------------------------------------------------
// Geometry

/// Whether two floats agree to three decimal places, which is the tolerance
/// the upstream `_Three_Places` comparators use.
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.001
}

#[test]
fn a_characters_origin_and_rect_count_are_exact() {
    // `Text`. The **origin** is the baseline pen position, which is pure
    // text-matrix arithmetic and therefore exact whatever face is
    // substituted. The upstream test also pins the tight box to
    // `{41.120, 55.652, 46.208, 49.892}` and the loose box to
    // `{40.664001, 60.692001, 46.664001, 47.419998}`, but both come out of
    // the *substituted face's* glyph metrics: the oracle runs against the
    // hermetic `third_party/test_fonts` set, and a differently-substituted
    // face moves them by a fraction of a unit without changing a single
    // extracted character. So the boxes are pinned structurally here and
    // byte-exactly by the conformance harness, which does supply that font
    // directory (design brief Q6).
    let page = fixture!("hello_world.pdf");
    let info = page.chars[4];
    assert!(near(info.origin.x, 40.664) && near(info.origin.y, 50.0));
    assert!(info.char_box.width() > 0.0 && info.char_box.height() > 0.0);
    assert!(info.char_box.x0 >= info.origin.x - 1.0);
    // Two rects, because the page is set in two fonts.
    assert_eq!(page.rects(0, Some(30)).len(), 2);
}

#[test]
fn the_loose_box_always_contains_the_tight_one() {
    // `Bug402562387` states this for one file; it is an invariant of
    // `GetLooseBounds`, whose two computed shapes both finish by unioning the
    // tight box in. Asserted over every fixture we can open.
    for name in [
        "hello_world.pdf",
        "bug_402562387.pdf",
        "latin_extended.pdf",
        "rotated_text.pdf",
        "font_matrix.pdf",
        "vertical_text.pdf",
    ] {
        let Some(page) = page_text(name, 0) else {
            continue;
        };
        for (index, info) in page.chars.iter().enumerate() {
            let (tight, loose) = (info.char_box, info.loose_char_box);
            assert!(
                loose.x0 <= tight.x0
                    && loose.y0 <= tight.y0
                    && loose.x1 >= tight.x1
                    && loose.y1 >= tight.y1,
                "{name} char {index}: {loose:?} does not contain {tight:?}"
            );
        }
    }
}

#[test]
fn the_loose_box_is_taller_than_any_glyph_and_uniform_in_span() {
    // `CharBoxForLatinExtendedText` states that two accented capitals whose
    // tight boxes differ by the accent share a loose box, because the loose
    // box takes its height from the *font's* ascent and descent rather than
    // the glyph's. The upstream numbers depend on which face was substituted,
    // so what is asserted here is the property behind them: within one font
    // size the loose box spans at least the font's own ascent-to-descent
    // range, so it is never the tight box in disguise.
    let page = fixture!("latin_extended.pdf");
    let real: Vec<_> = page
        .chars
        .iter()
        .filter(|c| !c.is_generated() && c.char_box.height() > 0.0)
        .collect();
    if real.len() < 2 {
        return;
    }
    for info in &real {
        assert!(
            info.loose_char_box.height() > info.char_box.height(),
            "loose {:?} is not taller than tight {:?}",
            info.loose_char_box,
            info.char_box
        );
    }
    // And the spread across the run is a small fraction of the box, not the
    // glyph-by-glyph variation a tight box would show.
    let heights: Vec<f64> = real.iter().map(|c| c.loose_char_box.height()).collect();
    let low = heights.iter().copied().fold(f64::INFINITY, f64::min);
    let high = heights.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        (high - low) / high < 0.1,
        "loose heights span {low}..{high}"
    );
}
#[test]
fn a_generated_character_carries_no_object_and_the_default_font_size() {
    // `GetFontSize`, `GetFontInfo` and `GetMatrix` together: a generated
    // character belongs to no text object, so it reports font size 1 and the
    // identity matrix while the real characters around it report 12 and 16.
    let page = fixture!("hello_world.pdf");
    assert_eq!(page.chars[0].font_size, 12.0);
    assert!(page.chars[0].object.is_some());
    for index in [13usize, 14] {
        let info = page.chars[index];
        assert_eq!(info.font_size, 1.0, "char {index}");
        assert_eq!(info.object, None, "char {index}");
        assert_eq!(info.matrix, kurbo::Affine::IDENTITY, "char {index}");
    }
    assert_eq!(page.chars[15].font_size, 16.0);
}

#[test]
fn rotated_text_reports_the_four_quadrant_angles() {
    // `GetCharAngle`: the four runs of `rotated_text.pdf` sit at exactly the
    // quadrant diagonals.
    use std::f64::consts::PI;

    let page = fixture!("rotated_text.pdf");
    if page.chars.len() < 31 {
        return;
    }
    for (index, expected) in [
        (0usize, PI / 4.0),
        (7, 3.0 * PI / 4.0),
        (16, 5.0 * PI / 4.0),
        (25, 7.0 * PI / 4.0),
    ] {
        let angle = f64::from(page.chars[index].angle);
        assert!(
            (angle - expected).abs() < 0.001,
            "char {index}: {angle} vs {expected}"
        );
    }
}

#[test]
fn a_font_matrix_does_not_change_a_characters_measured_box() {
    // `CharBox` on `font_matrix.pdf`: three 'A's drawn through three
    // different font matrices come out the same size, because the matrix is
    // folded into the text matrix before the box is measured.
    let page = fixture!("font_matrix.pdf");
    if page.chars.len() < 9 {
        return;
    }
    let first = page.chars[0].char_box;
    for index in [4usize, 8] {
        let other = page.chars[index].char_box;
        assert!(near(first.width(), other.width()), "char {index}");
        assert!(near(first.height(), other.height()), "char {index}");
    }
}

#[test]
fn a_vertical_run_advances_downwards() {
    // `TextVertical`: origins move down the page and drift slightly right.
    let page = fixture!("vertical_text.pdf");
    if page.chars.len() < 3 {
        return;
    }
    assert!(page.chars[2].origin.y < page.chars[1].origin.y);
    assert!(page.chars[2].origin.x >= page.chars[1].origin.x);
}

#[test]
fn selecting_a_rectangle_returns_only_what_it_covers() {
    // `CroppedText`'s bounded half: the same page yields different text for
    // different rectangles, and the generated line break is included.
    let page = fixture!("hello_world.pdf");
    let all = page.text_in_rect(kurbo::Rect::new(-1000.0, -1000.0, 1000.0, 1000.0));
    assert_eq!(all, HELLO);
    let none = page.text_in_rect(kurbo::Rect::new(5000.0, 5000.0, 5001.0, 5001.0));
    assert_eq!(none, "");
}

#[test]
fn text_can_be_asked_for_by_the_object_that_drew_it() {
    let page = fixture!("hello_world.pdf");
    let Some(first) = page.chars[0].object else {
        return;
    };
    let text = page.text_of_object(first);
    assert!(text.starts_with("Hello"), "{text:?}");
    assert!(!text.contains("Goodbye"), "{text:?}");
}

#[test]
fn extraction_never_panics_on_any_resource_fixture() {
    // The whole of `testing/resources`, which the fuzz target seeds from and
    // which holds every deliberately-broken file upstream keeps.
    let Some(dir) = resources() else { return };
    let mut seen = 0usize;
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pdf") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Every page, not just the first: a later page is where the
        // interesting damage usually is.
        for index in 0..4 {
            let Some(page) = page_text(name, index) else {
                break;
            };
            // Exercise the query half too, which has its own index
            // arithmetic.
            let _ = page.web_links();
            let _ = page.find("e", FindOptions::default()).count();
            let _ = page.rects(0, None);
            let _ = page.to_utf32le();
        }
        seen += 1;
    }
    assert!(seen > 100, "only {seen} fixtures found");
}

/// A hand-check that the emitted bytes really are what the oracle writes.
#[test]
fn the_utf32_encoding_round_trips_through_the_harness_transcode() {
    let page = fixture!("hello_world.pdf");
    let bytes = page.to_utf32le();
    assert_eq!(&bytes[..4], &[0xFF, 0xFE, 0x00, 0x00]);
    assert_eq!(bytes.len(), (page.chars.len() + 1) * 4);
    let decoded: String = bytes[4..]
        .chunks_exact(4)
        .filter_map(|unit| char::from_u32(u32::from_le_bytes([unit[0], unit[1], unit[2], unit[3]])))
        .collect();
    assert_eq!(decoded, HELLO);
}
