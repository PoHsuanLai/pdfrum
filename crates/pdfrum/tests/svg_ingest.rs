//! The ingestion round trip, because the conformance board cannot score an
//! SVG input.
//!
//! The board scores PDFs against `pdfium_test`'s pixels, and an SVG fixture
//! has no oracle PNG at all — there is no `pdfium_test` for SVG. M25's proof
//! is the mirror of M24's: draw the SVG into a page with
//! [`Canvas::draw_svg`](pdfrum::Canvas::draw_svg), render *that page* with
//! our own engine, and compare the result to `resvg` — a second, independent
//! implementation with its own parser, its own geometry and its own
//! rasterizer — rendering the same SVG directly, scored with the board's own
//! SSIM.
//!
//! The bar is legitimately below the raster board's, for the reason M24 gives
//! and one more: `resvg` is not our rasterizer *and* the page render goes
//! through a PDF content stream in between, where a colour is quantised to
//! eight bits per channel and a gradient becomes a sampled shading function.
//! The floors below are **the numbers this pipeline achieves**, recorded
//! after the fact rather than chosen and then cleared, and
//! `docs/design/svg-ingest.md` §5 carries them with the run that produced
//! them.
//!
//! # The fixture store, and why a missing one is not a pass
//!
//! Unlike M24's, this corpus **is** committed: `tests/fixtures/svg` is
//! seventeen small hand-written documents, not multi-megabyte artifacts
//! generated from a build of the oracle. There is therefore no legitimate
//! "the store is absent" case for the corpus itself, and the test says so:
//! an empty or missing fixture directory **fails**.
//!
//! What can legitimately be absent is `$PDFRUM_GOLDENS`, which the rest of
//! this repository's oracle tests use. This file does not consult it — there
//! is no oracle artifact for an SVG — so it is named here only to say that
//! its absence changes nothing about what this test proves.

// A fixture that will not open, a page that will not render or a floor that
// will not clear is the failure this file exists to catch, and `expect` is
// how a test says so. Same allowance `tests/traits.rs` takes.
#![allow(clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use pdfrum::{Document, Rect, SaveOptions, SvgFit, Unsupported};
use pdfrum_common::Diagnostics;
use pdfrum_raster_tinyskia::TinySkiaBackend;
use pdfrum_render::RenderOptions;

// The board's SSIM, shared with `pdfrum-svg`'s export round trip rather than
// transcribed a second time, so the two milestones' published numbers mean
// the same thing. Its `decode_png` is for the export side, which reads the
// oracle's PNG off disk; this side has no oracle artifact to read, so the
// function is unused here and that is a property of sharing the module, not
// dead code in it.
#[path = "../../pdfrum-svg/tests/roundtrip/ssim.rs"]
#[allow(dead_code, reason = "decode_png serves the export round trip")]
mod ssim;

/// The side of every fixture's square page and of every rendered image, in
/// points and in pixels alike.
///
/// One pixel per point, which is the board's scale and the scale at which the
/// two images are directly comparable without either being resampled.
const SIDE: u32 = 200;

/// What a fixture is expected to exercise, and what it is scored against.
///
/// An enum rather than a `strict: bool`, because the two groups are asked
/// genuinely different questions: a **Carried** fixture must both clear a
/// pixel floor and produce an *empty* report, while a **Declined** one exists
/// to prove a named construct is reported rather than swallowed, and its
/// pixels are not the point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The whole document maps. The report must be empty and the render must
    /// clear the class floor.
    Carried(Class),
    /// The named construct has no PDF spelling and must appear in the report.
    /// No pixel floor: the two renderers are drawing different pictures by
    /// design, which is the whole point of the fixture.
    Declined(Unsupported),
}

/// Which floor a carried fixture clears.
///
/// The classes separate what actually differs between the two pipelines, and
/// each floor below is set just under its class's worst file with that file
/// named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Filled and stroked paths, transforms, clips: where the two
    /// implementations agree most closely.
    Vector,
    /// A gradient, which becomes a PDF sampled shading function and is
    /// therefore quantised on the way through.
    Gradient,
    /// Constant alpha and group opacity, composited by two different
    /// compositors.
    Alpha,
    /// An embedded raster image, resampled by each engine's own filter.
    Image,
    /// `<text>` laid out by `usvg` and drawn as glyph outlines. Only reachable
    /// with the `svg-text` feature; without it a `<text>` is a declined
    /// fixture instead.
    #[cfg(feature = "svg-text")]
    Text,
}

/// The floor each class clears against `resvg`'s own render of the same file.
///
/// **Published, not negotiated.** Each is set just below the worst file in
/// its class on the run recorded in `docs/design/svg-ingest.md` §5, so the
/// number says what this pipeline achieves rather than what would be
/// comfortable to clear. The worst file of each class is named in its arm.
fn floor(class: Class) -> f64 {
    match class {
        Class::Vector => VECTOR_FLOOR,
        Class::Gradient => GRADIENT_FLOOR,
        Class::Alpha => ALPHA_FLOOR,
        Class::Image => IMAGE_FLOOR,
        #[cfg(feature = "svg-text")]
        Class::Text => TEXT_FLOOR,
    }
}

/// Paths, transforms, clips, `use`, CSS. The two implementations agree
/// almost exactly here — five of the eight files score a flat 1.0000 — and
/// the floor is set just under the one that does not. Worst:
/// `shapes_stroked`, 0.9989, where a stroke's join is inked slightly
/// differently by the two rasterizers.
const VECTOR_FLOOR: f64 = 0.99;
/// A gradient becomes a PDF type 2 or 3 shading over a stitched type 2
/// function, evaluated by our own engine rather than interpolated by
/// `resvg`'s. Worst: `gradient_linear`, 0.9991.
const GRADIENT_FLOOR: f64 = 0.99;
/// Constant alpha and group opacity, composited by two different
/// compositors: ours composites the group's members into the page one at a
/// time under a shared `/ca`, `resvg` renders the group to its own buffer and
/// blends it once, and where two translucent circles overlap that is a
/// genuinely different number. Worst — and the only file in the class:
/// `opacity_groups`, 0.9669. It is the one place the mapping is an
/// approximation rather than a translation, and
/// `docs/design/svg-ingest.md` §4 says so.
const ALPHA_FLOOR: f64 = 0.95;
/// An embedded raster, resampled from 16x16 to 160x160 by each engine's own
/// filter — ours through the PDF image pipeline, `resvg`'s through
/// `tiny-skia`. The lowest carried score in the corpus, and it is resampling
/// and nothing else. Worst — and the only file in the class: `image_png`,
/// 0.9315.
const IMAGE_FLOOR: f64 = 0.92;
/// Glyph outlines. `usvg` lays the text out on both sides from the *same*
/// committed face and flattens it to filled paths, so what our pipeline
/// carries is ordinary path geometry — and the measurement says so: all four
/// files carrying `<text>` score a flat **1.0000**, `declined_text` included,
/// which is the highest agreement of any class in the corpus. The class is
/// separate from `Vector` anyway, because what it proves is different: that
/// `usvg`'s layout, anchoring and `tspan` runs survive the trip, not that a
/// rectangle does.
///
/// The floor is 0.99 rather than 1.0 for the reason every other class's is
/// set just under its worst file: a face's hinting or a rasterizer's
/// antialiasing changing under us should be a review, not a red build.
#[cfg(feature = "svg-text")]
const TEXT_FLOOR: f64 = 0.99;

/// Every fixture, with what it is there to prove.
///
/// A table rather than a directory walk, because "the file is present" is not
/// the property under test — *what each file exercises* is, and a fixture
/// added without a row here should fail the completeness check below rather
/// than be scored by accident under a floor nobody chose for it.
const FIXTURES: &[(&str, Expect)] = &[
    ("shapes_basic", Expect::Carried(Class::Vector)),
    ("shapes_stroked", Expect::Carried(Class::Vector)),
    ("paths_curves", Expect::Carried(Class::Vector)),
    ("fill_evenodd", Expect::Carried(Class::Vector)),
    ("transforms_nested", Expect::Carried(Class::Vector)),
    ("use_and_defs", Expect::Carried(Class::Vector)),
    ("css_styles", Expect::Carried(Class::Vector)),
    ("clip_path", Expect::Carried(Class::Vector)),
    ("opacity_groups", Expect::Carried(Class::Alpha)),
    ("gradient_linear", Expect::Carried(Class::Gradient)),
    ("gradient_radial", Expect::Carried(Class::Gradient)),
    ("image_png", Expect::Carried(Class::Image)),
    ("declined_filter", Expect::Declined(Unsupported::Filter)),
    ("declined_text", TEXT_EXPECT),
    ("declined_pattern", Expect::Declined(Unsupported::Pattern)),
    ("declined_mask", Expect::Declined(Unsupported::Mask)),
    ("declined_blend", Expect::Declined(Unsupported::BlendMode)),
    // M25 item 3. What these three prove depends on the feature, which is
    // why `TEXT_EXPECT` is a constant rather than a literal: with `svg-text`
    // the text is laid out in the face this test registers and drawn as
    // outlines, and it must clear the Text floor with an empty report; with
    // the feature off there is no text stack, nothing is drawn, and the same
    // files must **report** the loss. Both are properties worth holding, and
    // a fixture that is scored under one of them by accident is exactly what
    // the completeness check below exists to prevent.
    ("text_basic", TEXT_EXPECT),
    ("text_anchored", TEXT_EXPECT),
    ("text_styled", TEXT_EXPECT),
];

/// What a fixture carrying `<text>` proves in this build.
///
/// See the note on the rows above. `declined_text` shares this constant
/// rather than staying declined, and the reason is a measured surprise worth
/// recording: `usvg` falls back to the first registered face when a `<text>`
/// names a family nothing registers, so with a face
/// registered *no* `<text>` is ever declined for a missing family — it is set
/// in the fallback and it scores 1.0000, because `resvg` given the same
/// database does exactly the same thing. The genuinely-reported path is an
/// **empty** font set, which
/// [`text_is_reported_when_no_face_is_registered`] proves on its own rather
/// than by hoping a fixture reaches it.
#[cfg(feature = "svg-text")]
const TEXT_EXPECT: Expect = Expect::Carried(Class::Text);
/// Without the text stack a `<text>` leaves no node in the tree, and the loss
/// is raised from the source XML instead — the behaviour M25 shipped.
#[cfg(not(feature = "svg-text"))]
const TEXT_EXPECT: Expect = Expect::Declined(Unsupported::Text);

/// The committed fixture directory.
fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/svg")
}

/// One fixture's source.
fn source(stem: &str) -> String {
    std::fs::read_to_string(fixture_dir().join(format!("{stem}.svg")))
        .unwrap_or_else(|e| panic!("fixture {stem}.svg: {e}"))
}

/// The one face every `text_*` fixture names, as bytes.
///
/// Already committed for the font tests, so the text fixtures cost the
/// repository no new artifact. Both sides of the comparison are given *this*
/// face and nothing else — neither ours nor `resvg`'s side may reach the
/// host's installed fonts, or the score would measure whichever Roboto the
/// build machine happens to have.
#[cfg(feature = "svg-text")]
fn text_face() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/roboto.ttf");
    std::fs::read(&path).unwrap_or_else(|e| panic!("the committed face {}: {e}", path.display()))
}

/// The faces an ingestion session lays `<text>` out in.
#[cfg(feature = "svg-text")]
fn svg_fonts() -> pdfrum::SvgFonts {
    let mut fonts = pdfrum::SvgFonts::new();
    assert!(fonts.register(text_face()), "the committed face registers");
    assert_eq!(
        fonts.families(),
        ["Roboto"],
        "the text fixtures name exactly this family"
    );
    fonts
}

/// Draw `svg` into a fresh square page and render the saved PDF.
///
/// The *saved* document is what is rendered, not the editing session: that
/// makes the content stream, the resource merge and the shading dictionaries
/// go through a real write-and-reparse, so a stream this crate emits but
/// cannot read back fails here rather than passing on an in-memory shortcut.
fn ingest_and_render(svg: &str) -> (ssim::Image, Vec<Unsupported>) {
    let doc = Document::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello_world.pdf"),
    )
    .expect("the fixture opens");
    let mut edit = doc.edit();
    // A page exactly the size of every fixture's viewBox, so the placement is
    // the identity beyond the y flip and nothing is scaled on the way in.
    edit.set_page_box(
        0,
        pdfrum::PageBox::Media,
        Rect::new(0.0, 0.0, f64::from(SIDE), f64::from(SIDE)),
    )
    .expect("the page box is set");
    #[cfg(feature = "svg-text")]
    edit.set_svg_fonts(svg_fonts());

    let mut reported = Vec::new();
    edit.draw_page(0, |canvas| {
        // The fixture PDF carries its own "Hello, world!", which the
        // reference render has no reason to draw and which would otherwise be
        // charged to the conversion. An opaque white ground covers it, and it
        // is also what `resvg` is rendered onto — so after this the two
        // images differ only by what the SVG put on them.
        canvas.fill_rect(
            Rect::new(0.0, 0.0, f64::from(SIDE), f64::from(SIDE)),
            pdfrum::Color::WHITE,
        );
        let report = canvas
            .draw_svg(
                svg,
                Rect::new(0.0, 0.0, f64::from(SIDE), f64::from(SIDE)),
                SvgFit::Contain,
            )
            .expect("the fixture resolves");
        reported = report.items().iter().map(|item| item.what).collect();
    })
    .expect("the drawing succeeds");

    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("the document saves");
    let saved = Document::from_bytes(bytes.into()).expect("what we wrote reopens");
    let page = saved.page(0).expect("the page survives the save");
    let pixmap = pdfrum_render::render_page(
        &page.objects(),
        &RenderOptions::default(),
        &TinySkiaBackend::new(),
        &mut Diagnostics::default(),
    )
    .expect("the page renders");

    (
        ssim::Image {
            width: pixmap.width(),
            height: pixmap.height(),
            rgba: pixmap.to_straight_rgba(),
        },
        reported,
    )
}

/// Render `svg` directly with `resvg`, on white.
///
/// The independent reference. On white because that is what an opaque PDF
/// page is cleared to, so the two images composite the same way.
fn reference(svg: &str) -> ssim::Image {
    #[cfg_attr(
        not(feature = "svg-text"),
        expect(unused_mut, reason = "the text fixtures fill it in")
    )]
    let mut options = resvg::usvg::Options::default();
    // The reference is given *only* the committed face, never the host's
    // installed fonts: `resvg`'s own `Options::default` carries an empty
    // database and nothing here calls `load_system_fonts`, so the score
    // compares two layouts of the same face rather than two machines.
    #[cfg(feature = "svg-text")]
    {
        options.fontdb_mut().load_font_data(text_face());
        "Roboto".clone_into(&mut options.font_family);
    }
    let tree = resvg::usvg::Tree::from_str(svg, &options).expect("resvg parses the fixture");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(SIDE, SIDE).expect("a 200x200 pixmap");
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    ssim::Image {
        width: SIDE,
        height: SIDE,
        rgba: pixmap.take(),
    }
}

/// Every fixture ingests, the carried ones clear their floor with an empty
/// report, and the declined ones report what they declined.
///
/// One test rather than seventeen, so the published table is printed in one
/// place and a failure names every file that moved rather than the first.
#[test]
fn every_fixture_ingests_and_clears_its_floor() {
    // The corpus is committed, so an absent or empty directory is a defect
    // and not a missing input. Collapsing those two cases is what produces a
    // vacuous green, and this assertion is where the distinction is made:
    // there is no legitimate skip for this test.
    let dir = fixture_dir();
    assert!(
        dir.is_dir(),
        "the committed fixture corpus is missing from {}",
        dir.display()
    );
    let present = std::fs::read_dir(&dir)
        .expect("the fixture directory reads")
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            name.strip_suffix(".svg").map(str::to_owned)
        })
        .count();
    assert_eq!(
        present,
        FIXTURES.len(),
        "every .svg in {} must have a row in FIXTURES, and every row a file",
        dir.display()
    );

    println!("\n{:<22} {:>8}  report", "fixture", "vs resvg");
    let mut failures = Vec::new();
    for (stem, expect) in FIXTURES {
        let svg = source(stem);
        let (ours, reported) = ingest_and_render(&svg);
        let score = ssim::compare(&ours, &reference(&svg)).expect("both images are 200x200");
        let names: Vec<_> = reported.iter().map(|u| u.name()).collect();
        println!(
            "{stem:<22} {score:>8.4}  {}",
            if names.is_empty() {
                "-".to_string()
            } else {
                names.join(" ")
            }
        );

        match expect {
            Expect::Carried(class) => {
                if !reported.is_empty() {
                    failures.push(format!(
                        "{stem}: expected to carry the whole document, reported {names:?}"
                    ));
                }
                if score < floor(*class) {
                    failures.push(format!(
                        "{stem}: {score:.4} against resvg, floor {:.2}",
                        floor(*class)
                    ));
                }
            }
            Expect::Declined(what) => {
                if !reported.contains(what) {
                    failures.push(format!(
                        "{stem}: {} was not reported; the report held {names:?}",
                        what.name()
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "below the published floor, or a construct swallowed:\n  {}",
        failures.join("\n  ")
    );
}

/// A construct with no PDF spelling is **reported**, never silently dropped.
///
/// The half of M25's exit condition the SSIM table cannot state. The table
/// above already checks each declined fixture names its own construct; this
/// checks the stronger property behind it — that every variant of
/// [`Unsupported`] the corpus can reach is in fact reachable, so the enum is
/// not carrying a case nothing ever raises (STYLE.md §4).
#[test]
fn every_reported_construct_has_a_fixture_that_raises_it() {
    let mut seen = Vec::new();
    for (stem, _) in FIXTURES {
        let (_, reported) = ingest_and_render(&source(stem));
        for what in reported {
            if !seen.contains(&what) {
                seen.push(what);
            }
        }
    }
    seen.sort_unstable();
    println!("\nconstructs the corpus raises: {seen:?}");

    // `ImageFormat` and `OffsetFocalGradient` are deliberately absent: the
    // first needs a GIF or WebP, which no fixture carries because embedding
    // one would add a decoder to the *test*, and the second needs a focal
    // gradient, which `usvg` normalises away in the cases a small fixture can
    // express. Both are recorded as named debt in `docs/roadmap.md`'s M25
    // entry rather than left as an unexplained gap.
    // `Text` is in this list only without `svg-text`. With the feature and a
    // face registered, the corpus draws its text rather than reporting it —
    // that is the point of the feature — and the report path is proven by
    // `text_is_reported_when_no_face_is_registered` instead, which reaches it
    // through an empty font set rather than through a fixture. Dropping it
    // from the list here rather than weakening the assertion keeps this test
    // saying exactly one thing.
    #[cfg(not(feature = "svg-text"))]
    let expected = [
        Unsupported::Filter,
        Unsupported::Mask,
        Unsupported::Text,
        Unsupported::Pattern,
        Unsupported::BlendMode,
    ];
    #[cfg(feature = "svg-text")]
    let expected = [
        Unsupported::Filter,
        Unsupported::Mask,
        Unsupported::Pattern,
        Unsupported::BlendMode,
    ];
    for what in expected {
        assert!(
            seen.contains(&what),
            "no fixture raises {}, so the report path for it is unproven",
            what.name()
        );
    }
}

/// A compiled form draws the **same picture** the inline path draws.
///
/// M25 item 2's other half. `tests/svg_form.rs` proves the file holds one
/// copy of the content however many pages place it; that is a claim about
/// objects, and it would be satisfied by a form that draws the wrong thing.
/// This is the pixel claim beside it: every carried fixture, compiled through
/// [`DocEdit::compile_svg`](pdfrum::DocEdit::compile_svg) and placed with
/// [`Canvas::place_svg`](pdfrum::Canvas::place_svg), renders the same as the
/// same fixture drawn inline.
///
/// Scored against our own inline render rather than against `resvg`, and the
/// bar is near-identity rather than a class floor: the two paths write the
/// *same* operators into different places, so anything less is a defect in
/// the form's placement transform and not a difference between two
/// implementations. The floors in the table above are what says the inline
/// path is right in the first place.
#[test]
fn a_compiled_form_draws_what_the_inline_path_draws() {
    println!("\n{:<22} {:>14}", "fixture", "form vs inline");
    let mut failures = Vec::new();
    for (stem, expect) in FIXTURES {
        // The declined fixtures are scored nowhere: what they prove is a
        // report item, and both sides here are ours, so a comparison would
        // only re-measure the inline path against itself.
        if !matches!(expect, Expect::Carried(_)) {
            continue;
        }
        let svg = source(stem);
        let (inline, _) = ingest_and_render(&svg);
        let compiled = compile_and_render(&svg);
        let score = ssim::compare(&compiled, &inline).expect("both images are 200x200");
        println!("{stem:<22} {score:>14.4}");
        if score < FORM_MATCHES_INLINE {
            failures.push(format!(
                "{stem}: {score:.4} against the inline render, floor {FORM_MATCHES_INLINE}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "a compiled form drew something the inline path did not:\n  {}",
        failures.join("\n  ")
    );
}

/// Below this, the form's placement transform has moved the drawing.
///
/// **Not** a class floor and not comparable to the four above: both sides are
/// our own renderer emitting the same operators, so the only honest bar is
/// near-identity, and antialiasing along the placement clip's edge is the
/// entire budget. Measured, like the others, and the measurement was
/// stronger than the bar: **all twelve** carried fixtures score a flat 1.0000
/// against the inline render, gradients and the embedded raster included, on
/// the run in `docs/design/svg-ingest.md` §8. The floor is left just below
/// rather than at 1.0 so that a future rasterizer's antialiasing change is a
/// review rather than a red build.
const FORM_MATCHES_INLINE: f64 = 0.999;

/// Compile `svg` into a form, place it on a fresh square page, render the
/// saved PDF.
///
/// The mirror of [`ingest_and_render`] through the form path, and it saves
/// and reopens for the same reason: a form whose `/Resources` this crate
/// writes but cannot read back must fail here rather than pass in memory.
fn compile_and_render(svg: &str) -> ssim::Image {
    let doc = Document::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello_world.pdf"),
    )
    .expect("the fixture opens");
    let mut edit = doc.edit();
    edit.set_page_box(
        0,
        pdfrum::PageBox::Media,
        Rect::new(0.0, 0.0, f64::from(SIDE), f64::from(SIDE)),
    )
    .expect("the page box is set");
    #[cfg(feature = "svg-text")]
    edit.set_svg_fonts(svg_fonts());

    let (form, _) = edit.compile_svg(svg).expect("the fixture compiles");
    edit.draw_page(0, |canvas| {
        // The same white ground `ingest_and_render` lays down, so the two
        // images differ only by what the SVG put on them.
        canvas.fill_rect(
            Rect::new(0.0, 0.0, f64::from(SIDE), f64::from(SIDE)),
            pdfrum::Color::WHITE,
        );
        canvas.place_svg(
            &form,
            Rect::new(0.0, 0.0, f64::from(SIDE), f64::from(SIDE)),
            SvgFit::Contain,
        );
    })
    .expect("the placement succeeds");

    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("the document saves");
    let saved = Document::from_bytes(bytes.into()).expect("what we wrote reopens");
    let page = saved.page(0).expect("the page survives the save");
    let pixmap = pdfrum_render::render_page(
        &page.objects(),
        &RenderOptions::default(),
        &TinySkiaBackend::new(),
        &mut Diagnostics::default(),
    )
    .expect("the page renders");

    ssim::Image {
        width: pixmap.width(),
        height: pixmap.height(),
        rgba: pixmap.to_straight_rgba(),
    }
}

/// A `<text>` this session has no face for is **reported**, never swallowed.
///
/// M25 item 3's other half, and the one the fixture table cannot state. With
/// a face registered, `usvg` falls back to the default family for any name it
/// does not know, so every `<text>` draws and none is declined — which is
/// good behaviour and is what the table above measures. The reported path is
/// reached when the session has **no** face at all, and that is what this
/// checks: the same fixtures, an empty [`SvgFonts`](pdfrum::SvgFonts), and an
/// `Unsupported::Text` for each one, carrying the element's own id.
///
/// It is also the property that keeps the feature honest against its own
/// off-state: with `svg-text` off these files report exactly the same thing,
/// raised from the source XML instead of from the walk.
#[cfg(feature = "svg-text")]
#[test]
fn text_is_reported_when_no_face_is_registered() {
    for stem in [
        "text_basic",
        "text_anchored",
        "text_styled",
        "declined_text",
    ] {
        let doc = Document::open(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello_world.pdf"),
        )
        .expect("the fixture opens");
        let mut edit = doc.edit();
        // No `set_svg_fonts`: the session's set is empty, which is the state
        // a caller who never registered a face is in.
        let mut reported = Vec::new();
        edit.draw_page(0, |canvas| {
            let report = canvas
                .draw_svg(
                    &source(stem),
                    Rect::new(0.0, 0.0, f64::from(SIDE), f64::from(SIDE)),
                    SvgFit::Contain,
                )
                .expect("the fixture resolves");
            reported = report.items().to_vec();
        })
        .expect("the drawing succeeds");

        assert!(
            reported.iter().any(|item| item.what == Unsupported::Text),
            "{stem}: no face is registered, so its text must be reported; \
             the report held {:?}",
            reported.iter().map(|i| i.what).collect::<Vec<_>>()
        );
        assert!(
            Unsupported::Text.is_dropped(),
            "text with no face draws nothing, so it is a dropped construct \
             rather than an approximated one"
        );
    }
}

/// Text goes into the page as **outlines**, not as an embedded font.
///
/// The roadmap's item 3 default, and a claim about the file rather than the
/// pixels: a page carrying the text as glyph outlines has no font resource
/// for it and no text-showing operator, so it renders identically wherever it
/// is opened. The pixel half is the `text_*` rows in the table above.
#[cfg(feature = "svg-text")]
#[test]
fn ingested_text_is_outlines_rather_than_an_embedded_font() {
    let doc = Document::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello_world.pdf"),
    )
    .expect("the fixture opens");
    // What the page already says, before anything is drawn on it: the
    // fixture's own "Hello, world!". The assertion below is that ingesting
    // `text_basic` — whose word is "Vector" — adds *nothing* to this, which
    // is what "outlines, not embedded text" means in the file.
    let before = doc
        .page(0)
        .expect("the fixture has a page")
        .text()
        .to_string();

    let mut edit = doc.edit();
    edit.set_svg_fonts(svg_fonts());
    edit.draw_page(0, |canvas| {
        canvas
            .draw_svg(
                &source("text_basic"),
                Rect::new(0.0, 0.0, f64::from(SIDE), f64::from(SIDE)),
                SvgFit::Contain,
            )
            .expect("the fixture resolves");
    })
    .expect("the drawing succeeds");

    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("the document saves");
    let saved = Document::from_bytes(bytes.into()).expect("what we wrote reopens");
    let page = saved.page(0).expect("the page survives the save");

    let after = page.text().to_string();
    assert!(
        !after.contains("Vector"),
        "an outlined `<text>` leaves no extractable text — the word the SVG \
         set must not appear in the page's text: {after:?}"
    );
    assert_eq!(
        after, before,
        "ingesting text as outlines adds no text-showing operator at all: \
         that is the trade the outline default makes, and \
         `docs/design/svg-ingest.md` §6 records it"
    );
}
