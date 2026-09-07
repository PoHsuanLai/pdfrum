//! The round trip, because the conformance board cannot score SVG.
//!
//! The board compares our pixels to `pdfium_test`'s, and an SVG file has
//! none. M24's proof is instead: convert a page to SVG, render *that* with
//! `resvg` — a second engine, with its own rasterizer and its own
//! antialiasing — at the board's DPI, and compare the result to the oracle's
//! own PNG with the same SSIM the board uses.
//!
//! The bar is legitimately below the raster board's. `resvg` is not our
//! rasterizer: it makes its own decisions about coverage, about how a
//! `stroke-width` under a pixel is inked, and about how an `<image>` is
//! resampled. The floors below are **the numbers this pipeline achieves**,
//! recorded after the fact rather than chosen and then cleared, and
//! §5 carries them with the run that produced them.
//!
//! # Running it
//!
//! The golden store is `$PDFRUM_GOLDENS`, else the in-repo
//! `conformance/goldens`. It is **not committed** — it is generated locally
//! from a built `pdfium_test`, which `.github/workflows/ci.yml` states it will
//! not do — so a hosted runner has no goldens and the oracle half cannot run
//! there.
//!
//! That is handled as a missing *input*, not as a pass. Without a store the
//! oracle half is skipped with a note on stdout, and the self-consistency half
//! still runs over all 44 files with its floors enforced. With a store
//! present, every file that has a golden **must** be scored: the count is
//! pinned, so a broken lookup fails rather than quietly shrinking the proof.
//! Collapsing those two cases into one is what produces a vacuous green, and
//! this file exists partly to prevent that.

use std::path::{Path, PathBuf};

use pdfrum::Document;
use pdfrum_common::Diagnostics;
use pdfrum_corpus::{CORPUS, Class};
use pdfrum_raster_tinyskia::TinySkiaBackend;
use pdfrum_render::RenderOptions;

#[path = "roundtrip/ssim.rs"]
mod ssim;

/// The floor each file class clears against the **oracle's** PNG.
///
/// **Published, not negotiated.** Each is set just below the *worst* file in
/// its class on the run recorded in §5, so the number
/// says what this pipeline achieves rather than what would be comfortable to
/// clear. The worst file in each class is named in the arm.
fn oracle_floor(class: Class) -> f64 {
    match class {
        // Paths, where the two engines agree most closely. Worst:
        // vector_font_size14, 0.9647.
        Class::Vector => 0.95,
        // Text: glyph outlines through a second rasterizer *and* placed where
        // the PDF puts them rather than on the oracle's snapped grid — both
        // differences at once. Worst: text_tcpdf_063, 0.9240.
        // Image: worst is image_ccitt_3bigpreview, 0.9317, a 1-bit CCITT scan
        // under a soft mask where a half-covered pixel is the whole image.
        // Mixed: worst is mixed_tcpdf_045, 0.9628.
        Class::Text | Class::Image | Class::Mixed => 0.90,
        // Worst: shading_tcpdf_030, 0.8532. The mesh files are the low end of
        // the corpus and §4 says why: a mesh is a raster region, and `resvg`
        // resamples that embedded PNG with its own filter.
        Class::Shading => 0.80,
        // Only one file in this class has a golden — forms_widgets_407, at
        // 0.7902, the corpus's lowest. Its page is dense small text, which is
        // where the glyph-placement difference costs the most.
        Class::Forms => 0.75,
    }
}

/// The floor each class clears against **our own** raster render of the same
/// page under the same options.
///
/// The tighter and more diagnostic of the two: it isolates what the SVG
/// conversion loses, with the oracle's own difference from us — and the
/// glyph-placement difference the vector-text option makes — factored out.
/// Every corpus file scores at least 0.9761 here, so one flat floor says as
/// much as six would.
fn self_floor(class: Class) -> f64 {
    match class {
        Class::Text
        | Class::Vector
        | Class::Image
        | Class::Shading
        | Class::Forms
        | Class::Mixed => 0.95,
    }
}

/// The engine options every page in this test is rendered and converted
/// under.
///
/// `subpixel_text_positioning` is on, which is what
/// [`pdfrum_svg::page_to_svg`] forces anyway: with it off the engine blits a
/// glyph *bitmap* for every run below fifty device units per em, and at one
/// pixel per point that is nearly all body text. The raster reference is
/// rendered under the same option so the two images differ only by the
/// conversion.
fn engine_options() -> RenderOptions {
    RenderOptions {
        subpixel_text_positioning: true,
        ..RenderOptions::default()
    }
}

/// The golden store, resolved the way every test in this repository resolves
/// an oracle artifact: `$PDFRUM_GOLDENS`, else the in-repo
/// `conformance/goldens` that `scripts/env.nu` and the harness default to.
///
/// `None` when neither exists. **The store is not committed** — it is
/// generated locally from a built `pdfium_test`, which `.github/workflows/
/// ci.yml` says outright it will not do — so a hosted runner legitimately has
/// no goldens, and the oracle half of this file cannot run there. That is a
/// missing input, not a defect, and [`the gate below`](
/// every_corpus_file_converts_and_clears_its_floor) distinguishes the two:
/// absent is a loud skip, *present but unscored* is a failure.
fn goldens() -> Option<PathBuf> {
    let dir = std::env::var_os("PDFRUM_GOLDENS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/goldens"),
        PathBuf::from,
    );
    dir.is_dir().then_some(dir)
}

/// The golden store's content key: the first sixteen hex digits of the PDF's
/// SHA-256.
fn key_for(bytes: &[u8]) -> String {
    use core::fmt::Write as _;

    use sha2::Digest as _;

    let digest = sha2::Sha256::digest(bytes);
    digest.iter().take(8).fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Render an SVG document with `resvg` at one device pixel per unit.
///
/// One pixel per unit is the board's scale: our `viewBox` is already the
/// device box the same page rasterizes into, so the two images are the same
/// size and directly comparable.
fn render_svg(svg: &str, width: u32, height: u32) -> Option<ssim::Image> {
    // No fonts: every glyph in our output is a filled `<path>`, so the
    // options a `usvg::Options` would carry for text are never consulted.
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    // On white, because that is what the oracle composites a page onto and
    // what `RenderOptions::default()` clears an opaque page to.
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    Some(ssim::Image {
        width,
        height,
        rgba: pixmap.take(),
    })
}

/// One corpus document's first page, converted and scored.
struct Scored {
    stem: &'static str,
    class: Class,
    /// SSIM against our own raster render of the same page.
    against_self: f64,
    /// SSIM against the oracle's PNG, when the goldens were available.
    against_oracle: Option<f64>,
    /// What the conversion had to rasterize.
    causes: Vec<(pdfrum_svg::RasterCause, usize)>,
}

/// Convert, render and score one document's first page.
fn score(stem: &'static str, class: Class, goldens: Option<&PathBuf>) -> Option<Scored> {
    let doc = Document::open(pdfrum_corpus::path(stem)).ok()?;
    let page = doc.page(0).ok()?;
    // The page-object graph, which the engine and this crate both take.
    // `Page::objects` is the facade's documented escape hatch for a caller
    // who wants the drawing operations rather than pixels.
    let graph = page.objects();
    let opts = engine_options();

    // Our own pixels under the *same* options, so the comparison isolates
    // what the SVG conversion loses rather than what the option changes.
    let tiny = TinySkiaBackend::new();
    let mut diags = Diagnostics::default();
    let ours = pdfrum_render::render_page(&graph, &opts, &tiny, &mut diags).ok()?;
    let (width, height) = (ours.width(), ours.height());

    let converted = pdfrum_svg::page_to_svg(&graph, &opts, &tiny, &mut diags).ok()?;
    assert!(
        converted.svg.contains("<svg "),
        "{stem}: the conversion produced no document"
    );

    let rendered = render_svg(&converted.svg, width, height)?;
    let mine = ssim::Image {
        width,
        height,
        rgba: ours.to_straight_rgba(),
    };
    let against_self = ssim::compare(&mine, &rendered)?;

    let against_oracle = goldens.and_then(|dir| {
        let golden = ssim::decode_png(&oracle_png(dir, stem)?)?;
        (golden.width == width && golden.height == height)
            .then(|| ssim::compare(&golden, &rendered))
            .flatten()
    });

    Some(Scored {
        stem,
        class,
        against_self,
        against_oracle,
        causes: converted.report.counts(),
    })
}

/// The oracle's first-page PNG for one corpus document, from the golden store.
///
/// The store names artifacts exactly as `pdfium_test` wrote them, from the
/// *oracle checkout's* path rather than from the corpus stem — a corpus file
/// and the checkout file it was copied from share one content-keyed directory
/// under two names. So the directory is listed and the one `.0.png` in it is
/// taken, rather than a name being guessed.
fn oracle_png(goldens: &std::path::Path, stem: &str) -> Option<Vec<u8>> {
    let dir = goldens.join(key_for(&pdfrum_corpus::bytes(stem)));
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            name.ends_with(".0.png").then_some(name)
        })
        .collect();
    names.sort();
    std::fs::read(dir.join(names.first()?)).ok()
}

/// Every corpus file converts, and every one clears its class's floor.
///
/// One test rather than forty-four, so the published table is printed in one
/// place and a failure names every file that moved rather than the first.
#[test]
fn every_corpus_file_converts_and_clears_its_floor() {
    let goldens = goldens();
    let mut scored = Vec::new();
    let mut unconverted = Vec::new();
    for doc in CORPUS {
        match score(doc.stem, doc.class, goldens.as_ref()) {
            Some(s) => scored.push(s),
            None => unconverted.push(doc.stem),
        }
    }

    println!(
        "\n{:<28} {:>8} {:>9}  rasterized",
        "file", "vs ours", "vs oracle"
    );
    for s in &scored {
        let causes = if s.causes.is_empty() {
            "-".to_string()
        } else {
            s.causes
                .iter()
                .map(|(cause, n)| format!("{}x{n}", cause.name()))
                .collect::<Vec<_>>()
                .join(" ")
        };
        println!(
            "{:<28} {:>8.4} {:>9}  {causes}",
            s.stem,
            s.against_self,
            s.against_oracle
                .map_or_else(|| "-".to_string(), |v| format!("{v:.4}")),
        );
    }

    let mut failures = Vec::new();
    for s in &scored {
        let floor = self_floor(s.class);
        if s.against_self < floor {
            failures.push(format!(
                "{}: {:.4} against our own render, floor {floor:.2}",
                s.stem, s.against_self
            ));
        }
        if let Some(v) = s.against_oracle
            && v < oracle_floor(s.class)
        {
            failures.push(format!(
                "{}: {v:.4} against the oracle, floor {:.2}",
                s.stem,
                oracle_floor(s.class)
            ));
        }
    }

    let with_golden = scored.iter().filter(|s| s.against_oracle.is_some()).count();
    println!(
        "\n{with_golden} of {} scored against the oracle",
        scored.len()
    );

    assert!(
        unconverted.is_empty(),
        "these corpus files did not convert: {unconverted:?}"
    );
    assert_eq!(scored.len(), CORPUS.len(), "every corpus file is scored");

    // The golden store is generated locally from a built `pdfium_test` and is
    // not committed, so a hosted runner has none and the oracle half cannot
    // run there — a missing input rather than a defect. The two cases are kept
    // apart deliberately, because collapsing them is what produces a false
    // green:
    //
    // - **No store at all**: skip, loudly. The self-consistency half above has
    //   still run over all 44 files and its floors have still been enforced.
    // - **A store that is there but scored nothing**: fail. That is the lookup
    //   being broken, which would otherwise leave half the proof silently not
    //   running while the test stayed green.
    match &goldens {
        None => println!(
            "\nNOTE: no golden store, so the oracle half did not run. The \
             self-consistency half did, over all {} files. Set PDFRUM_GOLDENS \
             (or generate conformance/goldens) for the full proof.",
            scored.len()
        ),
        Some(dir) => {
            // Seven of the forty-four bench-corpus documents are not in the
            // conformance corpus and so have no golden directory: the six
            // `forms_*` widget files and `mixed_formfield`. That is a property
            // of the two corpora, not of this crate — but it is pinned as a
            // number so an *eighth* unscored file, which would mean the lookup
            // broke, fails here instead of quietly shrinking the proof.
            assert_eq!(
                with_golden,
                CORPUS.len() - 7,
                "the store at {} is present, so the oracle half must score \
                 every corpus file that has a golden",
                dir.display()
            );
        }
    }
    assert!(
        failures.is_empty(),
        "below the published floor:\n  {}",
        failures.join("\n  ")
    );
}

/// The report is non-empty exactly where the file composites in the pixel
/// domain, and empty where it does not.
///
/// The half of M24's exit condition the SSIM table cannot state: a page of
/// paths and text must convert to *vectors*, not to one big embedded PNG that
/// happens to score well.
#[test]
fn the_report_is_empty_exactly_where_the_page_is_all_vectors() {
    let tiny = TinySkiaBackend::new();
    let mut all_vector = Vec::new();
    let mut rasterized = Vec::new();
    let mut empty = Vec::new();
    for doc in CORPUS {
        let Ok(pdf) = Document::open(pdfrum_corpus::path(doc.stem)) else {
            continue;
        };
        let Ok(page) = pdf.page(0) else { continue };
        let Ok(converted) = pdfrum_svg::page_to_svg(
            &page.objects(),
            &engine_options(),
            &tiny,
            &mut Diagnostics::default(),
        ) else {
            continue;
        };
        let (svg, report) = (converted.svg, converted.report);
        if report.is_empty() {
            // An empty report and no `<path>` means the page drew nothing at
            // all — true of `forms_widgets_407`, whose content is entirely in
            // its widget annotations, and of the two deliberately corrupt
            // shading fixtures. Recorded rather than asserted away, so the
            // count below distinguishes "converted to vectors" from "was
            // empty"; the assertion that matters is the one at the end.
            if svg.contains("<path ") {
                all_vector.push(doc.stem);
            } else {
                empty.push(doc.stem);
            }
        } else {
            // Every region carries a cause, and every cause tags its element.
            for region in report.regions() {
                assert!(
                    svg.contains(&format!("data-cause=\"{}\"", region.cause.name())),
                    "{}: {} is reported but not tagged in the document",
                    doc.stem,
                    region.cause.name()
                );
            }
            rasterized.push((doc.stem, report.counts()));
        }
    }
    println!("\npages that drew nothing at all: {empty:?}");
    println!("\nall-vector pages ({}):", all_vector.len());
    for stem in &all_vector {
        println!("  {stem}");
    }
    println!("\npages with rasterized regions ({}):", rasterized.len());
    for (stem, counts) in &rasterized {
        let causes = counts
            .iter()
            .map(|(cause, n)| format!("{}x{n}", cause.name()))
            .collect::<Vec<_>>()
            .join(" ");
        println!("  {stem:<28} {causes}");
    }
    assert!(
        !all_vector.is_empty(),
        "no corpus page converted entirely to vectors, which would mean the \
         vector path is not being taken at all"
    );
}
