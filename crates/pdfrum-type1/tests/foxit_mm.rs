//! The acceptance suite: the two Foxit Multiple-Master faces PDFium falls back
//! to when nothing else can supply a font.
//!
//! These are the crate's reason to exist,
//! and there is no C++ unit test to port — PDFium drives them through FreeType
//! and only the rendered pixels are a reference. So this file pins them two
//! ways:
//!
//! - **Structurally**, against what the font programs literally declare
//!   (`tests/fixtures/PROVENANCE.md` records the values).
//! - **Differentially**, against `read_fonts::ps::type1`, which reads Type 1
//!   containers and charstrings — including the Multiple-Master blend
//!   operators — but only ever at the weight vector the file ships with. That
//!   one point in design space is therefore an *exact* cross-check on the
//!   container walk, the eexec ciphers, the tokenizer, and every charstring
//!   operator these fonts use. What it cannot check is instantiation at any
//!   other point, which is precisely the part this crate adds.

// `clippy.toml`'s `allow-*-in-tests` covers `#[test]` bodies but not the
// helper functions an integration test factors them into; a panic in one of
// these *is* the failure signal. The numeric and indexing allows are the same
// bargain: a test asserting an exact font-unit coordinate wants
// `assert_eq!(x, 550.0)`, and one indexing a fixed-length axis list wants
// `axes[0]`. None of this relaxes anything in the library.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::manual_midpoint,
    clippy::similar_names
)]

use pdfrum_common::kurbo::{PathEl, Point, Shape};
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_type1::{AxisKind, Container, Encoding, Gid, Type1Font};

const SANS: &[u8] = include_bytes!("fixtures/FoxitSansMM.pfb");
const SERIF: &[u8] = include_bytes!("fixtures/FoxitSerifMM.pfb");

fn load(bytes: &[u8]) -> Type1Font {
    let mut diags = Diagnostics::default();
    let font = Type1Font::parse(bytes, &Limits::default(), &mut diags).expect("parses");
    assert_eq!(font.container(), Container::Pfb);
    font
}

#[test]
fn both_faces_parse_with_the_metrics_they_declare() {
    for (bytes, name, family, bbox) in [
        (
            SANS,
            "ChromeSansMM",
            "Chrome Sans MM",
            (-119.0, -257.0, 1150.0, 872.0),
        ),
        (
            SERIF,
            "ChromeSerifMM",
            "Chrome Serif MM",
            (-157.0, -257.0, 1194.0, 872.0),
        ),
    ] {
        let f = load(bytes);
        assert_eq!(f.postscript_name(), Some(name));
        assert_eq!(f.family_name(), Some(family));
        assert_eq!(f.units_per_em(), 1000);
        assert_eq!(f.num_glyphs(), 230, "{name}");
        assert!(!f.is_fixed_pitch());
        assert!(f.has_glyph_names());
        let b = f.bbox();
        assert_eq!((b.x0, b.y0, b.x1, b.y1), bbox, "{name}");
    }
}

#[test]
fn the_built_in_encoding_maps_the_latin_alphabet() {
    for bytes in [SANS, SERIF] {
        let f = load(bytes);
        // Both fonts build their own 256-entry vector rather than naming a
        // predefined one.
        assert!(matches!(f.encoding(), Encoding::Custom(_)));
        for ch in ['A', 'Z', 'a', 'z', '0', '9', ' '] {
            let code = ch as u8;
            let gid = f.code_to_gid(code).unwrap_or_else(|| panic!("{ch:?}"));
            let expected = match ch {
                ' ' => "space",
                '0' => "zero",
                '9' => "nine",
                other => return_name(other),
            };
            assert_eq!(f.glyph_name(gid), Some(expected), "{ch:?}");
            // And the synthesized Unicode charmap agrees.
            assert_eq!(f.unicode_to_gid(ch), Some(gid), "{ch:?}");
        }
        // A code the vector leaves at `.notdef`.
        assert_eq!(f.code_to_gid(0), None);
    }
}

/// The AGL name of a single Latin letter is the letter itself.
fn return_name(ch: char) -> &'static str {
    const LETTERS: [&str; 52] = [
        "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R",
        "S", "T", "U", "V", "W", "X", "Y", "Z", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j",
        "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
    ];
    let idx = if ch.is_ascii_uppercase() {
        ch as usize - 'A' as usize
    } else {
        26 + (ch as usize - 'a' as usize)
    };
    LETTERS.get(idx).copied().unwrap_or("?")
}

#[test]
fn both_faces_report_two_axes_over_four_masters() {
    for (bytes, name, weight_range, width_range, weights) in [
        (
            SANS,
            "sans",
            (50.0f32, 1450.0f32),
            (50.0f32, 1450.0f32),
            [0.3158f32, 0.1349, 0.3849, 0.1644],
        ),
        (
            SERIF,
            "serif",
            (110.0, 790.0),
            (100.0, 900.0),
            [0.2702, 0.1048, 0.4504, 0.1746],
        ),
    ] {
        let f = load(bytes);
        let axes = f.mm_axes().unwrap_or_else(|| panic!("{name} has axes"));
        assert_eq!(axes.len(), 2, "{name}");
        assert_eq!(axes[0].kind, AxisKind::Weight, "{name}");
        assert_eq!(axes[1].kind, AxisKind::Width, "{name}");
        assert_eq!((axes[0].min, axes[0].max), weight_range, "{name}");
        assert_eq!((axes[1].min, axes[1].max), width_range, "{name}");
        for axis in axes {
            assert!(
                axis.min <= axis.default && axis.default <= axis.max,
                "{name}: {axis:?}"
            );
        }
        // Four masters, and the shipped vector is what the file says.
        assert_eq!(f.default_weight_vector().len(), 4, "{name}");
        for (got, want) in f.default_weight_vector().iter().zip(&weights) {
            assert!((got - want).abs() < 1e-4, "{name}: {got} vs {want}");
        }
    }
}

#[test]
fn instantiating_at_the_defaults_reproduces_the_shipped_weight_vector() {
    for bytes in [SANS, SERIF] {
        let f = load(bytes);
        let axes = f.mm_axes().expect("axes");
        let coords: Vec<f32> = axes.iter().map(|a| a.default).collect();
        let inst = f.instantiate(&coords).expect("instance");
        for (got, want) in inst.weight_vector().iter().zip(f.default_weight_vector()) {
            assert!((got - want).abs() < 1e-3, "{got} vs {want}");
        }
    }
}

#[test]
fn outlines_are_non_degenerate_at_the_default_instance() {
    for (bytes, name) in [(SANS, "sans"), (SERIF, "serif")] {
        let f = load(bytes);
        let gid = f.name_to_gid("A").expect("A");
        let (path, advance) = f.outline(gid).expect("outline");
        assert!(!path.is_empty(), "{name}: A has no outline");
        let bb = path.bounding_box();
        // A capital A fills most of the em box in both faces.
        assert!(bb.height() > 500.0, "{name}: {bb:?}");
        assert!(bb.width() > 300.0, "{name}: {bb:?}");
        assert!(
            f64::from(advance) > bb.width(),
            "{name}: advance {advance} vs {bb:?}"
        );
        // Every path starts with a move and every point is finite.
        assert!(matches!(path.elements().first(), Some(PathEl::MoveTo(_))));
        assert!(finite(&path), "{name}");
    }
}

/// Every glyph in both faces draws without producing a non-finite coordinate,
/// at three points across design space.
#[test]
fn every_glyph_draws_finitely_at_every_probe() {
    for (bytes, name) in [(SANS, "sans"), (SERIF, "serif")] {
        let f = load(bytes);
        let axes = f.mm_axes().expect("axes");
        let probes = [
            vec![axes[0].min, axes[1].min],
            vec![axes[0].default, axes[1].default],
            vec![axes[0].max, axes[1].max],
        ];
        let mut drawn = 0usize;
        for gid in 0..f.num_glyphs() {
            let gid = Gid(gid as u16);
            let mut diags = Diagnostics::default();
            let (path, _) = f
                .outline_with_diagnostics(gid, &mut diags)
                .unwrap_or_else(|| panic!("{name}: glyph {gid:?} missing"));
            assert!(
                diags.is_empty(),
                "{name}: glyph {:?} ({:?}) aborted: {:?}",
                gid,
                f.glyph_name(gid),
                diags.entries()
            );
            if !path.is_empty() {
                drawn += 1;
                assert!(finite(&path), "{name}: {gid:?}");
            }
            for probe in &probes {
                let inst = f.instantiate(probe).expect("instance");
                let (p, adv) = inst.outline(gid).expect("instance outline");
                assert!(finite(&p), "{name}: {gid:?} at {probe:?}");
                assert!(adv.is_finite() && adv >= 0.0, "{name}: {gid:?} adv {adv}");
            }
        }
        // Nearly all 230 glyphs have contours; `space` and `.notdef` do not.
        assert!(drawn >= 225, "{name}: only {drawn} glyphs drew anything");
    }
}

/// The interpolation actually moves the outline, which is what makes the
/// terminal substitution rung usable — a static default would render every
/// unresolvable font at one weight.
#[test]
fn the_weight_axis_changes_the_outline() {
    for (bytes, name) in [(SANS, "sans"), (SERIF, "serif")] {
        let f = load(bytes);
        let axes = f.mm_axes().expect("axes");
        let gid = f.name_to_gid("A").expect("A");
        let mid_width = (axes[1].min + axes[1].max) / 2.0;

        let light = f
            .instantiate(&[axes[0].min, mid_width])
            .expect("light")
            .outline(gid)
            .expect("light outline");
        let heavy = f
            .instantiate(&[axes[0].max, mid_width])
            .expect("heavy")
            .outline(gid)
            .expect("heavy outline");

        assert_ne!(light.0, heavy.0, "{name}: weight axis did nothing");
        // A heavier master is drawn with thicker strokes, so it covers more
        // area at the same nominal size.
        let (a_light, a_heavy) = (area(&light.0), area(&heavy.0));
        assert!(
            a_heavy > a_light * 1.2,
            "{name}: heavy {a_heavy} vs light {a_light}"
        );
    }
}

/// `AdjustVariationParams`'s bisection needs the width axis to move the
/// *advance width*, not merely the outline. This is the property the C++
/// probes for with two `FT_Load_Glyph` calls.
#[test]
fn the_width_axis_changes_the_advance() {
    for (bytes, name) in [(SANS, "sans"), (SERIF, "serif")] {
        let f = load(bytes);
        let axes = f.mm_axes().expect("axes");
        let gid = f.name_to_gid("A").expect("A");
        let mid_weight = (axes[0].min + axes[0].max) / 2.0;

        let narrow = f
            .instantiate(&[mid_weight, axes[1].min])
            .expect("narrow")
            .advance(gid)
            .expect("narrow advance");
        let wide = f
            .instantiate(&[mid_weight, axes[1].max])
            .expect("wide")
            .advance(gid)
            .expect("wide advance");

        assert!(
            (wide - narrow).abs() > 1.0,
            "{name}: advances {narrow} and {wide} are indistinguishable"
        );
        // Monotone in between, which is what makes the C++'s linear
        // interpolation between the two probes meaningful.
        let mid = f
            .instantiate(&[mid_weight, (axes[1].min + axes[1].max) / 2.0])
            .expect("mid")
            .advance(gid)
            .expect("mid advance");
        let (lo, hi) = if narrow < wide {
            (narrow, wide)
        } else {
            (wide, narrow)
        };
        assert!(lo <= mid && mid <= hi, "{name}: {lo} <= {mid} <= {hi}");
    }
}

/// The whole `AdjustVariationParams` procedure, reproduced against our API:
/// probe both ends of the width axis, then interpolate to a target advance.
#[test]
fn a_target_advance_can_be_hit_by_bisecting_the_width_axis() {
    let f = load(SANS);
    let axes = f.mm_axes().expect("axes");
    let gid = f.name_to_gid("A").expect("A");
    let weight = 400.0f32;

    let min_width = f
        .instantiate(&[weight, axes[1].min])
        .expect("min")
        .advance(gid)
        .expect("min advance");
    let max_width = f
        .instantiate(&[weight, axes[1].max])
        .expect("max")
        .advance(gid)
        .expect("max advance");
    assert!((max_width - min_width).abs() > 1.0);

    // Ask for the midpoint advance and solve for the coordinate, exactly as
    // the C++ does.
    let target = (min_width + max_width) / 2.0;
    let coord =
        axes[1].min + (axes[1].max - axes[1].min) * (target - min_width) / (max_width - min_width);
    let got = f
        .instantiate(&[weight, coord])
        .expect("solved")
        .advance(gid)
        .expect("solved advance");
    // The advance is piecewise-linear in the coordinate for a two-master axis,
    // so the linear solve is close to exact.
    assert!(
        (got - target).abs() < (max_width - min_width).abs() * 0.05,
        "wanted {target}, got {got} (range {min_width}..{max_width})"
    );
}

/// The differential oracle: at the *file's own* weight vector, `read-fonts`
/// interprets the same charstrings, and our outlines must match its own.
///
/// This is a whole-font check over both faces — every glyph, every operator
/// these fonts use, both containers, both ciphers, the Multiple-Master blend
/// at the default point, and the glyph ordering. A disagreement anywhere shows
/// up here.
#[test]
fn outlines_match_read_fonts_at_the_file_weight_vector() {
    use read_fonts::types::GlyphId;

    for (bytes, name) in [(SANS, "sans"), (SERIF, "serif")] {
        let ours = load(bytes);
        let theirs = read_fonts::ps::type1::Type1Font::new(bytes).expect("read-fonts parses");

        assert_eq!(
            ours.num_glyphs(),
            theirs.num_glyphs(),
            "{name}: glyph count"
        );
        assert_eq!(
            ours.postscript_name(),
            theirs.name(),
            "{name}: PostScript name"
        );
        assert_eq!(u32::from(ours.units_per_em()), theirs.upem() as u32);

        let mut compared = 0usize;
        for gid in 0..ours.num_glyphs() {
            // read-fonts renumbers `.notdef` to index 0 when the font declares
            // it elsewhere; ask it for the glyph *we* mean.
            let their_gid = theirs.remapped_gid(GlyphId::new(gid));
            let their_name = theirs.glyph_name(their_gid);
            assert_eq!(
                ours.glyph_name(Gid(gid as u16)),
                their_name,
                "{name}: glyph {gid} name"
            );

            let mut pen = Collector::default();
            let their_advance = theirs
                .draw(their_gid, None, &mut pen)
                .unwrap_or_else(|e| panic!("{name}: glyph {gid}: {e:?}"));
            let (our_path, our_advance) = ours.outline(Gid(gid as u16)).expect("our outline");

            // `draw` with no ppem applies the font matrix's *scale* only, and
            // both fonts are 1000/em, so the coordinates are directly
            // comparable — once the degenerate segments FreeType suppresses
            // are dropped from ours too (see `without_degenerates`).
            let ours_filtered = without_degenerates(our_path.elements());
            assert_eq!(
                ours_filtered.len(),
                pen.0.len(),
                "{name}: glyph {gid} ({their_name:?}) segment count"
            );
            for (i, (a, b)) in ours_filtered.iter().zip(&pen.0).enumerate() {
                assert!(
                    same(a, b),
                    "{name}: glyph {gid} ({their_name:?}) element {i}: {a:?} vs {b:?}"
                );
                // The stronger statement: their coordinate is ours put through
                // FreeType's truncation, so the divergence is exactly the
                // quantum and never an accumulating drift.
                assert!(
                    truncates_to(a, b),
                    "{name}: glyph {gid} ({their_name:?}) element {i} is not a \
                     truncation of ours: {a:?} vs {b:?}"
                );
            }
            if let Some(theirs) = their_advance {
                // `read-fonts` rounds the advance through its 16.16 metric
                // transform before handing it back; we keep the charstring's
                // blended value. Agreeing to within a font unit is the most
                // that comparison can assert.
                assert!(
                    (our_advance - theirs).abs() < 1.0,
                    "{name}: glyph {gid} advance {our_advance} vs {theirs}"
                );
            }
            compared += 1;
        }
        assert_eq!(compared, 230, "{name}");
    }
}

/// Collects `read-fonts` pen calls as `kurbo` path elements so the two
/// outlines can be compared elementwise.
#[derive(Default)]
struct Collector(Vec<PathEl>);

impl read_fonts::model::pen::OutlinePen for Collector {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(PathEl::MoveTo(Point::new(x.into(), y.into())));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(PathEl::LineTo(Point::new(x.into(), y.into())));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.0.push(PathEl::QuadTo(
            Point::new(cx.into(), cy.into()),
            Point::new(x.into(), y.into()),
        ));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.push(PathEl::CurveTo(
            Point::new(cx0.into(), cy0.into()),
            Point::new(cx1.into(), cy1.into()),
            Point::new(x.into(), y.into()),
        ));
    }
    fn close(&mut self) {
        self.0.push(PathEl::ClosePath);
    }
}

/// Elementwise comparison with a one-font-unit tolerance.
///
/// The tolerance is not slop, it is a **known and deliberate divergence**
/// (recorded in the internal working notes). `read-fonts` reproduces
/// FreeType's fixed-point pipeline bit for bit: in the unscaled case it
/// multiplies by 1/64, discards the low 10 bits, and shifts back — which
/// quantizes every unscaled coordinate to a whole font unit. We keep the
/// charstring's arithmetic in `f64` and hand back the unrounded value, because
/// the outline is about to be multiplied by a text matrix and rasterized at
/// device resolution, where throwing away sub-unit precision is pure loss.
/// The two therefore agree to within the quantum FreeType introduces, and
/// nowhere near it for glyphs whose coordinates are already integral.
fn same(a: &PathEl, b: &PathEl) -> bool {
    const TOL: f64 = 1.0;
    let close = |p: Point, q: Point| (p.x - q.x).abs() < TOL && (p.y - q.y).abs() < TOL;
    match (a, b) {
        (PathEl::MoveTo(p), PathEl::MoveTo(q)) | (PathEl::LineTo(p), PathEl::LineTo(q)) => {
            close(*p, *q)
        }
        (PathEl::QuadTo(a0, a1), PathEl::QuadTo(b0, b1)) => close(*a0, *b0) && close(*a1, *b1),
        (PathEl::CurveTo(a0, a1, a2), PathEl::CurveTo(b0, b1, b2)) => {
            close(*a0, *b0) && close(*a1, *b1) && close(*a2, *b2)
        }
        (PathEl::ClosePath, PathEl::ClosePath) => true,
        _ => false,
    }
}

/// Drop zero-length lines.
///
/// FreeType suppresses degenerate moves and lines so that stem darkening does
/// not produce artifacts, and `read-fonts` reproduces that filter; both Foxit
/// faces contain a handful of genuinely coincident consecutive points (glyph
/// 137, `sterling`, is one). We emit them, because a zero-length segment is
/// harmless to a fill rasterizer and dropping it is the rasterizer's business
/// rather than the font parser's. Filtering here rather than in the library
/// keeps that judgement in one place.
fn without_degenerates(elements: &[PathEl]) -> Vec<PathEl> {
    let mut out: Vec<PathEl> = Vec::with_capacity(elements.len());
    let mut at: Option<Point> = None;
    for el in elements {
        match *el {
            PathEl::LineTo(p) if at == Some(p) => continue,
            PathEl::MoveTo(p)
            | PathEl::LineTo(p)
            | PathEl::QuadTo(_, p)
            | PathEl::CurveTo(_, _, p) => at = Some(p),
            PathEl::ClosePath => {}
        }
        out.push(*el);
    }
    out
}

/// Whether `theirs` is `ours` with the low fractional bits discarded, which is
/// what FreeType's `>> 10` on a 16.16 value amounts to for an unscaled outline.
/// Allows a one-unit slack in the truncation direction only, so a genuine
/// disagreement (which would go either way, and by more) still fails.
fn truncates_to(ours: &PathEl, theirs: &PathEl) -> bool {
    let ok = |p: Point, q: Point| {
        let trunc = |v: f64, w: f64| (v - w) >= -0.001 && (v - w) < 1.0;
        trunc(p.x, q.x) && trunc(p.y, q.y)
    };
    match (ours, theirs) {
        (PathEl::MoveTo(p), PathEl::MoveTo(q)) | (PathEl::LineTo(p), PathEl::LineTo(q)) => {
            ok(*p, *q)
        }
        (PathEl::QuadTo(a0, a1), PathEl::QuadTo(b0, b1)) => ok(*a0, *b0) && ok(*a1, *b1),
        (PathEl::CurveTo(a0, a1, a2), PathEl::CurveTo(b0, b1, b2)) => {
            ok(*a0, *b0) && ok(*a1, *b1) && ok(*a2, *b2)
        }
        (PathEl::ClosePath, PathEl::ClosePath) => true,
        _ => false,
    }
}

fn finite(path: &pdfrum_common::kurbo::BezPath) -> bool {
    path.elements().iter().all(|el| match *el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => p.is_finite(),
        PathEl::QuadTo(a, b) => a.is_finite() && b.is_finite(),
        PathEl::CurveTo(a, b, c) => a.is_finite() && b.is_finite() && c.is_finite(),
        PathEl::ClosePath => true,
    })
}

/// Absolute enclosed area — a proxy for ink coverage that is insensitive to
/// contour direction.
fn area(path: &pdfrum_common::kurbo::BezPath) -> f64 {
    path.area().abs()
}

/// The never-panic sweep the acceptance target demands, run against the real
/// fonts rather than synthetic ones: every prefix and a spread of single-byte
/// corruptions of both faces.
#[test]
fn corrupted_real_fonts_never_panic() {
    let limits = Limits::default();
    for bytes in [SANS, SERIF] {
        for len in (0..bytes.len()).step_by(4099) {
            let mut d = Diagnostics::with_limit(4);
            drive(bytes.get(..len).unwrap_or_default(), &limits, &mut d);
        }
        for at in (0..bytes.len()).step_by(2731) {
            let mut c = bytes.to_vec();
            if let Some(b) = c.get_mut(at) {
                *b = b.wrapping_add(0x9D);
            }
            let mut d = Diagnostics::with_limit(4);
            drive(&c, &limits, &mut d);
        }
    }
}

/// Parse and then exercise every accessor, so a corruption that survives
/// parsing is still caught by a later panic.
fn drive(bytes: &[u8], limits: &Limits, diags: &mut Diagnostics) {
    let Ok(f) = Type1Font::parse(bytes, limits, diags) else {
        return;
    };
    let n = f.num_glyphs().min(300) as u16;
    for g in (0..n).chain([n, u16::MAX]) {
        let _ = f.outline(Gid(g));
        let _ = f.glyph_bounds(Gid(g));
        let _ = f.glyph_name(Gid(g));
    }
    for code in [0u8, 32, 65, 255] {
        let _ = f.code_to_gid(code);
    }
    if let Some(axes) = f.mm_axes() {
        let extremes: Vec<f32> = axes.iter().map(|a| a.max).collect();
        for probe in [extremes, vec![f32::NAN, f32::NAN], Vec::new()] {
            if let Some(inst) = f.instantiate(&probe) {
                for g in 0..n.min(16) {
                    let _ = inst.outline(Gid(g));
                }
            }
        }
    }
}
