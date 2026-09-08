//! The Form `XObject`: one SVG, many placements, **one** copy of the content.
//!
//! `Canvas::draw_svg` writes the SVG's operators inline into the page it is
//! drawing on; for one placement that is equivalent to a form. For the same
//! logo on twenty pages it would be twenty copies of the same content. A form
//! `XObject` is one object placed twenty times instead.
//!
//! That is a claim about the **file**, not about the picture, so it is what
//! this file measures: every stream in the saved document is decoded and the
//! ones carrying the SVG's own operators are counted. The assertion is that
//! exactly one does, however many pages placed it. A test that only rendered
//! each page correctly would pass just as happily on the inline spelling.
//!
//! Counting decoded streams rather than raw bytes is deliberate: the save
//! flate-compresses what it writes, so a search of the file's bytes would
//! find nothing and pass vacuously. `tests/svg_ingest.rs` is the pixel half —
//! that the form draws the same picture the inline path draws, scored against
//! `resvg`. The two together: same picture, one object.

// A fixture that will not open or a count that does not hold is the failure
// this file exists to catch, and `expect` is how a test says so.
#![allow(clippy::expect_used, clippy::panic)]

use std::path::Path;

use pdfrum::{Document, ObjRef, Rect, SaveOptions, SvgFit};

/// An SVG whose compiled content stream carries operators nothing else in the
/// fixture document writes.
const LOGO: &str = concat!(
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">"#,
    r##"<rect x="1" y="1" width="8" height="8" fill="#cc3319"/>"##,
    "</svg>",
);

/// The path-construction operator the logo's `<rect>` compiles to.
///
/// The rectangle rather than its colour, because a colour's spelling is the
/// number writer's business — `#cc3319` comes out as `.8000001 .20000002
/// .098039225 rg`, which would make this constant a hostage to how many
/// digits a float prints. The geometry is exact and it identifies *this*
/// drawing: the fixture document's own content paints text, and every other
/// stream the save writes is a page's `/Contents`.
const MARKER: &[u8] = b"1 1 8 8 re";

/// The two-page fixture every placement goes onto.
fn open_pages() -> Document {
    Document::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello_world_2_pages.pdf"),
    )
    .expect("the fixture opens")
}

/// How many times `needle` occurs in `haystack`.
fn occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

/// How many of `doc`'s streams carry `needle`, once decoded.
///
/// Every object number is tried and the ones that are not streams are
/// skipped, which reaches the whole document without a traversal of its page
/// tree: a form reached only through a page's `/Resources` is counted exactly
/// like a page's own `/Contents`.
fn streams_carrying(doc: &Document, needle: &[u8]) -> usize {
    // Generous for a two-page fixture plus everything a session adds, and
    // cheap: a number naming no object simply fails to fetch.
    (1..200)
        .filter(|num| {
            doc.stream_data(ObjRef::new(*num, 0))
                .is_ok_and(|data| occurrences(&data, needle) > 0)
        })
        .count()
}

/// Which spelling of the drawing a document was built with.
///
/// An enum rather than a `compiled: bool`, so the test names the two
/// spellings rather than a boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    /// `Canvas::draw_svg` on every page: the operators written once per page.
    Inline,
    /// `DocEdit::compile_svg` once, `Canvas::place_svg` on every page.
    Compiled,
}

/// A saved document whose every page carries the logo, under `spelling`.
fn saved(spelling: Spelling) -> Document {
    let doc = open_pages();
    let mut edit = doc.edit();
    let into = Rect::new(20.0, 20.0, 120.0, 120.0);

    match spelling {
        Spelling::Compiled => {
            let (form, report) = edit.compile_svg(LOGO).expect("the logo compiles");
            assert!(report.is_empty(), "the logo maps in full: {report:?}");
            edit.draw_pages(|c| c.place_svg(&form, into, SvgFit::Contain))
                .expect("every page places it");
        }
        Spelling::Inline => {
            edit.draw_pages(|c| {
                c.draw_svg(LOGO, into, SvgFit::Contain)
                    .expect("the logo resolves");
            })
            .expect("every page draws it");
        }
    }

    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("the document saves");
    Document::from_bytes(bytes).expect("what we wrote reopens")
}

/// The whole point of the item: the content is written **once**, however many
/// pages place it.
///
/// The inline spelling is measured beside it rather than merely described, so
/// the assertion is a comparison against the thing being replaced rather than
/// a bare number that could drift into meaninglessness. Two pages is enough
/// to tell one from many; twenty would prove nothing three does not.
#[test]
fn a_compiled_form_writes_its_content_once_however_many_pages_place_it() {
    let inline = streams_carrying(&saved(Spelling::Inline), MARKER);
    assert_eq!(
        inline, 2,
        "the inline spelling writes the drawing once per page — that is what \
         the form replaces, and if this is not 2 the comparison below is \
         measuring nothing"
    );

    let compiled = streams_carrying(&saved(Spelling::Compiled), MARKER);
    assert_eq!(
        compiled, 1,
        "a compiled form is one object: its content must live in exactly one \
         stream however many pages place it"
    );
}

/// Every page reaches the one form through its own `/Resources`, and draws it.
///
/// The structural claim behind the count: that what was written is a real
/// Form `XObject` a reader will follow, not merely a stream that happens to
/// appear once. A file with the content once and no `Do` would satisfy the
/// count in the test above and draw nothing at all.
#[test]
fn every_page_places_the_one_form() {
    let doc = saved(Spelling::Compiled);
    let pages = usize::try_from(doc.page_count()).expect("a small page count");
    assert_eq!(
        streams_carrying(&doc, b" Do"),
        pages,
        "every page's content stream places the form"
    );

    for index in 0..doc.page_count() {
        let page = doc.page(index).expect("the page survives the save");
        assert!(
            !page.objects().objects.is_empty(),
            "page {index} draws something after placing the form"
        );
    }
}

/// The fit is chosen per **placement**, not baked into the form.
///
/// What makes one compiled object reusable rather than merely shared: the
/// form's `/BBox` is the SVG's own box, so the same object can be `Contain`ed
/// on one page and `Cover`ed on another. If the fit were compiled in this
/// would need two objects, and the count below would be 2.
#[test]
fn one_form_serves_two_different_fits() {
    let doc = open_pages();
    let mut edit = doc.edit();
    let (form, _) = edit.compile_svg(LOGO).expect("the logo compiles");
    // Exact, deliberately: the box is the SVG's `viewBox` width carried
    // through unchanged, not a computed quantity with a tolerance.
    #[expect(
        clippy::float_cmp,
        reason = "the viewBox width is carried, not computed"
    )]
    {
        assert_eq!(form.bbox().width(), 10.0, "the form's box is the SVG's own");
    }

    // A deliberately non-square destination, so the two fits genuinely differ.
    let into = Rect::new(20.0, 20.0, 140.0, 80.0);
    edit.draw_page(0, |c| c.place_svg(&form, into, SvgFit::Contain))
        .expect("page 0 contains it");
    edit.draw_page(1, |c| c.place_svg(&form, into, SvgFit::Cover))
        .expect("page 1 covers it");

    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("the document saves");
    let saved = Document::from_bytes(bytes).expect("what we wrote reopens");
    assert_eq!(
        streams_carrying(&saved, MARKER),
        1,
        "two different fits, still one copy of the content"
    );
}
