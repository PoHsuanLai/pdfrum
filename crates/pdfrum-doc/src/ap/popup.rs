//! The appearance a synthesized pop-up note draws when it is open.
//!
//! A pop-up carries no geometry of its own worth drawing — it is a yellow
//! card with a black rule around it — and everything interesting about it is
//! the two pieces of text stacked inside: the parent's title (`/T`) on the
//! first line and the parent's note (`/Contents`) below it, wrapped to the
//! card's width.
//!
//! # Why it looks nothing like the other generators
//!
//! Every other generator in [`crate::ap::markup`] reads a size, a colour or a
//! dash pattern out of the annotation's own dictionary. This one reads
//! *nothing*: the border is one unit wide, the fill is pure yellow, the ink
//! is pure black and the type is 12-point, all hard-coded, and none of them
//! answer to `/BS`, `/C`, `/IC` or `/DA`. A viewer's note card is chrome, not
//! document content, so the file gets no say in how it is drawn.
//!
//! # The two joins that decide the pixels
//!
//! - **The title and the note are one string, joined by a newline.** Not two
//!   layouts stacked, not two paragraphs with spacing between them — a single
//!   run of text with a `\n` in the middle, handed to one layout pass. A
//!   pop-up whose parent has no `/T` therefore begins with an *empty* first
//!   line and the note starts on the second, which is why a title-less card
//!   still pushes its text down by one line rather than starting at the top.
//! - **The text box is the card's full rectangle, inset by three units.**
//!   Not the deflated rectangle the border was stroked into: the layout plate
//!   is the raw `/Rect`, and the inset is applied afterwards as a translation
//!   of every glyph by `(+3, -3)`. The distinction shows on the last line a
//!   tall card can fit, because the plate that decided the wrap was three
//!   units taller than the space the glyphs actually landed in.
//!
//! The card is only drawn while it is open, which is a state no file records
//! and only a pointer entering the parent annotation produces — see
//! [`crate::annot_render`] for where that arrives.

use pdfrum_object::{Dict, Name, Object, Resolve, names as obj_names};

use crate::ap::emit::{Content, Float, PaintOp, color_op};
use crate::ap::freetext;
use crate::ap::markup::Generated;
use crate::color::Color;
use crate::geom;
use crate::vt;

/// The type size a pop-up sets its text at, whatever the document says.
const FONT_SIZE: f32 = 12.0;

/// The rule around the card, in units.
const BORDER_WIDTH: f32 = 1.0;

/// How far the text is inset from the card's rectangle, as `(dx, dy)`. The
/// second is negative because the inset moves the text *down* from the top.
const TEXT_OFFSET: (f32, f32) = (3.0, -3.0);

/// The resource name the `Tf` operator carries. Fixed, because the font is
/// fixed: there is no `/DA` to name one.
const FONT_ALIAS: &[u8] = b"FONT";

/// Draws one pop-up's card and the text on it.
///
/// `metrics` measures the wrap and `encode` writes the glyphs, the same pair
/// [`crate::ap::freetext::free_text`] takes and for the same reason: the
/// layout is a pure function of the numbers, so it can be exercised against a
/// stub face. Answers [`None`] only when the layout produced no glyphs at all
/// — an empty title *and* an empty note — because upstream returns an empty
/// contents string in that case and the card is left blank rather than being
/// drawn with an empty text object inside it.
#[must_use]
pub fn popup<R: Resolve>(
    dict: &Dict,
    metrics: &vt::Metrics<'_>,
    encode: &dyn Fn(u32) -> Vec<u8>,
    r: &R,
) -> Option<Generated> {
    let rect = geom::normalize(dict.rect(obj_names::RECT, r));

    let mut out = Content::new();
    // A newline here, where every other generator writes a space.
    out.raw("/GS gs\n");
    out.raw(&color_op(Color::Rgb(1.0, 1.0, 0.0), PaintOp::Fill));
    out.raw(&color_op(Color::Rgb(0.0, 0.0, 0.0), PaintOp::Stroke));
    out.raw("1 w\n");
    // A stroke paints half a width either side of the path, so the path moves
    // inward by half to keep the rule inside the card.
    let stroked = geom::deflate(rect, BORDER_WIDTH / 2.0, BORDER_WIDTH / 2.0);
    out.rect(stroked, Float::G6);
    out.raw("re b\n");

    let written = body(dict, metrics, encode, rect, r);
    if written.is_empty() {
        return None;
    }
    out.raw("BT\n");
    out.raw(&color_op(Color::Rgb(0.0, 0.0, 0.0), PaintOp::Fill));
    out.raw(&written);
    out.raw("ET\n");
    // An unmatched restore, reproduced: upstream closes the contents string
    // with one and never wrote the save it answers.
    out.raw("Q\n");

    Some(Generated {
        stream: out.into_bytes(),
        rect_override: None,
        is_text_markup: false,
        blend_multiply: false,
        font_resources: Some(Dict::from_pairs([(
            Name::new(FONT_ALIAS.to_vec()),
            Object::Dict(freetext::fallback_font()),
        )])),
    })
}

/// The text operators for one card: the title, a newline, and the note,
/// wrapped into the card and shifted by the inset.
fn body<R: Resolve>(
    dict: &Dict,
    metrics: &vt::Metrics<'_>,
    encode: &dyn Fn(u32) -> Vec<u8>,
    rect: kurbo::Rect,
    r: &R,
) -> String {
    let title = dict.text(obj_names::T, r).unwrap_or_default();
    let contents = dict.text(obj_names::CONTENTS, r).unwrap_or_default();
    let text = format!("{title}\n{contents}");

    let config = vt::Config {
        plate: rect,
        alignment: vt::Alignment::Left,
        font_size: FONT_SIZE,
        multi_line: true,
        auto_return: true,
        ..vt::Config::default()
    };
    let layout = vt::layout(&text, &config, metrics);
    vt::edit_ap::generate(
        &layout,
        &config,
        metrics,
        TEXT_OFFSET,
        vt::edit_ap::Grouping::Continuous,
        FONT_ALIAS,
        encode,
    )
}

#[cfg(test)]
mod tests {
    use super::{FONT_ALIAS, popup};
    use crate::vt::Metrics;
    use pdfrum_object::{Array, Dict, NoResolve, Object, PdfString};

    /// The highlight's own left edge in `annotation_highlight_author_content`.
    const LEFT: f32 = 115.750_62;

    /// Its bottom, which is where the card hangs from.
    const TOP: f32 = 706.328_57;

    /// A card at the place `CreatePopupAnnot` puts one beside the highlight in
    /// `annotation_highlight_author_content.pdf`.
    fn card(title: &str, contents: &str) -> Dict {
        Dict::from_pairs([
            (
                pdfrum_object::names::RECT.clone(),
                // The parent's left edge and its bottom, and the 200-unit
                // square `CreatePopupAnnot` hangs below and to the right.
                Object::Array(Array::of([
                    Object::Real(LEFT),
                    Object::Real(TOP - 200.0),
                    Object::Real(LEFT + 200.0),
                    Object::Real(TOP),
                ])),
            ),
            (
                pdfrum_object::names::T.clone(),
                Object::Str(PdfString::literal(title.as_bytes())),
            ),
            (
                pdfrum_object::names::CONTENTS.clone(),
                Object::Str(PdfString::literal(contents.as_bytes())),
            ),
        ])
    }

    /// Helvetica's own numbers, so the wrap lands where the oracle's does.
    /// Every character is half an em wide, which keeps the arithmetic in the
    /// assertions below checkable by hand.
    fn helvetica(width: &dyn Fn(u32) -> i32) -> Metrics<'_> {
        Metrics {
            width,
            ascent: 718,
            descent: -207,
        }
    }

    #[test]
    fn the_card_opens_with_the_chrome_upstream_writes_byte_for_byte() {
        let width = |_: u32| 500;
        let metrics = helvetica(&width);
        let encode = |code: u32| vec![u8::try_from(code).unwrap_or(b'?')];
        let made =
            popup(&card("A", "b"), &metrics, &encode, &NoResolve).expect("a card with text draws");
        let stream = String::from_utf8(made.stream).expect("ascii");
        // The graphics state, the two colours, the width and the deflated
        // rectangle, in upstream's order and upstream's spelling — note the
        // newline after `gs`, which no other generator writes.
        assert!(
            stream.starts_with("/GS gs\n1 1 0 rg\n0 0 0 RG\n1 w\n116.251 506.829 199 199 re b\n"),
            "{stream}"
        );
    }

    #[test]
    fn the_text_is_black_and_set_in_the_fixed_twelve_point_face() {
        let width = |_: u32| 500;
        let metrics = helvetica(&width);
        let encode = |code: u32| vec![u8::try_from(code).unwrap_or(b'?')];
        let made =
            popup(&card("A", "b"), &metrics, &encode, &NoResolve).expect("a card with text draws");
        let stream = String::from_utf8(made.stream).expect("ascii");
        assert!(stream.contains("BT\n0 0 0 rg\n"), "{stream}");
        assert!(stream.contains("/FONT 12 Tf\n"), "{stream}");
        // The unmatched restore upstream ends its contents string with.
        assert!(stream.ends_with("ET\nQ\n"), "{stream}");
    }

    #[test]
    fn the_title_and_the_note_are_one_string_joined_by_a_newline() {
        let width = |_: u32| 500;
        let metrics = helvetica(&width);
        let encode = |code: u32| vec![u8::try_from(code).unwrap_or(b'?')];
        let made =
            popup(&card("A", "b"), &metrics, &encode, &NoResolve).expect("a card with text draws");
        let stream = String::from_utf8(made.stream).expect("ascii");
        // Two `Tj`s on two lines, in that order: one layout, not two.
        let title = stream.find("(A) Tj").expect("the title is set");
        let note = stream.find("(b) Tj").expect("the note is set");
        assert!(title < note, "{stream}");
    }

    #[test]
    fn a_title_less_card_still_spends_its_first_line_on_the_empty_title() {
        let width = |_: u32| 500;
        let metrics = helvetica(&width);
        let encode = |code: u32| vec![u8::try_from(code).unwrap_or(b'?')];
        let titled = popup(&card("A", "b"), &metrics, &encode, &NoResolve).expect("draws");
        let bare = popup(&card("", "b"), &metrics, &encode, &NoResolve).expect("draws");
        let titled = String::from_utf8(titled.stream).expect("ascii");
        let bare = String::from_utf8(bare.stream).expect("ascii");
        // With a title the note is stepped down one line from it; without one
        // the empty first line emits no `Tj`, so the step is folded into the
        // note's own opening `Td` instead. Either way the note lands on the
        // *second* line — 694.7125 less one 11.099976-unit line — which is the
        // property that matters, and it is worth asserting through the two
        // different spellings rather than around them.
        assert!(titled.contains("118.75062 694.7125 Td"), "{titled}");
        assert!(
            titled.contains("(A) Tj\n0 -11.099976 Td\n(b) Tj"),
            "{titled}"
        );
        assert!(!bare.contains("(A) Tj"), "{bare}");
        assert!(bare.contains("118.75062 683.61255 Td"), "{bare}");
    }

    #[test]
    fn a_card_with_neither_a_title_nor_a_note_draws_nothing_at_all() {
        let width = |_: u32| 500;
        let metrics = helvetica(&width);
        let encode = |code: u32| vec![u8::try_from(code).unwrap_or(b'?')];
        assert_eq!(popup(&card("", ""), &metrics, &encode, &NoResolve), None);
    }

    #[test]
    fn the_stream_names_the_helvetica_it_sets_the_text_in() {
        let width = |_: u32| 500;
        let metrics = helvetica(&width);
        let encode = |code: u32| vec![u8::try_from(code).unwrap_or(b'?')];
        let made = popup(&card("A", "b"), &metrics, &encode, &NoResolve).expect("draws");
        let fonts = made
            .font_resources
            .expect("a stream with a Tf carries a font");
        let named = fonts
            .dict(&pdfrum_object::Name::new(FONT_ALIAS.to_vec()), &NoResolve)
            .expect("under the alias the Tf names");
        assert_eq!(
            named
                .name(&pdfrum_object::Name::from("BaseFont"))
                .map(pdfrum_object::Name::as_bytes),
            Some(b"Helvetica".as_slice())
        );
    }
}
