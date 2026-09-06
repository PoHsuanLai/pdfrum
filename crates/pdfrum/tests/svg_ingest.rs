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
    ("stroke_pen", Expect::Carried(Class::Vector)),
    ("opacity_groups", Expect::Carried(Class::Alpha)),
    ("gradient_linear", Expect::Carried(Class::Gradient)),
    ("gradient_radial", Expect::Carried(Class::Gradient)),
    ("image_png", Expect::Carried(Class::Image)),
    ("declined_filter", Expect::Declined(Unsupported::Filter)),
    ("declined_text", Expect::Declined(Unsupported::Text)),
    ("declined_pattern", Expect::Declined(Unsupported::Pattern)),
    ("declined_mask", Expect::Declined(Unsupported::Mask)),
    ("declined_blend", Expect::Declined(Unsupported::BlendMode)),
];

/// The committed fixture directory.
fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/svg")
}

/// One fixture's source.
fn source(stem: &str) -> String {
    std::fs::read_to_string(fixture_dir().join(format!("{stem}.svg")))
        .unwrap_or_else(|e| panic!("fixture {stem}.svg: {e}"))
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
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default())
        .expect("resvg parses the fixture");
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
    for what in [
        Unsupported::Filter,
        Unsupported::Mask,
        Unsupported::Text,
        Unsupported::Pattern,
        Unsupported::BlendMode,
    ] {
        assert!(
            seen.contains(&what),
            "no fixture raises {}, so the report path for it is unproven",
            what.name()
        );
    }
}
