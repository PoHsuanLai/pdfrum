//! A session lays its caret out with the face the *renderer* substitutes to.
//!
//! # The defect these pin
//!
//! [`FormSession`] generates the appearance a focused field draws — the glyph
//! run, the caret, the selection band — and every height in it comes from the
//! ascent and descent of the face the field's `/DA` font resolves to. That
//! resolution happens through a [`BuildContext`], and a context carries
//! substitution options: which directories are enumerated, and whether the
//! Croscore families stand in for Arial, Times and Courier.
//!
//! `FormSession::new` builds a **default** context. A caller that renders
//! through `BuildContext::with_substitution` and starts its session with
//! `new` therefore measures one document two ways: the page's `/Arial`
//! becomes Arimo — ascent 905, descent −211 — while the session's falls
//! through to the built-in base-14 Helvetica at 718 and −219. At 12pt that is
//! a caret 11.244 units tall where the page's own metrics call for 13.392,
//! which is a whole device row at 1:1.
//!
//! [`FormSession::with_context`] is the fix, and the two assertions below are
//! the two halves of it: that the constructor threads the caller's context
//! through at all, and that the number it produces under the hermetic font
//! set is the one the oracle's is.

#![allow(clippy::expect_used)]

use pdfrum::{BuildContext, Document, FormSession, Modifiers, UpdateKind, kurbo::Point};

/// `form_textfield_focused_ltr.in` expanded: one `/Tx` widget,
/// `/Rect [50 40 150 70]`, border 1, `/DA (/Arial 12 Tf 0 0 0 rg)` over a
/// `/DR` that declares `/Arial` as a bare non-embedded TrueType. Nothing is
/// embedded, so the face is entirely the substitution's choice — which is
/// what makes it the fixture for this question.
fn document() -> Document {
    Document::open("tests/fixtures/substituted_da_font.pdf")
        .expect("the substituted_da_font fixture must open")
}

/// A point inside the widget's `/Rect [50 40 150 70]`.
const INSIDE: Point = Point::new(100.0, 55.0);

/// The oracle's hermetic font set: `third_party/test_fonts`, which carries
/// Arimo, Tinos and Cousine in place of Arial, Times and Courier. PLAN.md §4
/// names it as half of the determinism recipe, alongside
/// `--croscore-font-names`.
///
/// Returns `None` when the sibling checkout is not present, which is the one
/// thing a test in this crate may not require: the workspace builds without
/// it.
///
/// # The skip is announced, and can be made an error
///
/// A test that silently returns green when its fixture is missing is a test
/// that stops being read. The absence goes to **stderr** on every run, and
/// setting `PDFRUM_REQUIRE_ORACLE_FONTS=1` turns it into a failure — which is
/// what a machine that *does* have the oracle checkout should set, so that a
/// path typo cannot quietly retire the only assertion pinning 13.392.
fn hermetic_font_dir() -> Option<std::path::PathBuf> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../pdfium-c++/third_party/test_fonts/test_fonts");
    if dir.is_dir() {
        return Some(dir);
    }
    let message = format!(
        "the oracle's hermetic font set is not at {}; the substituted-face \
         assertions are skipped",
        dir.display()
    );
    assert!(
        std::env::var_os("PDFRUM_REQUIRE_ORACLE_FONTS").is_none(),
        "PDFRUM_REQUIRE_ORACLE_FONTS is set but {message}"
    );
    eprintln!("note: {message}");
    None
}

/// Focuses the widget and returns the live appearance stream it draws.
///
/// A click is three events, and the move is not decoration: it is what tells
/// the widget the pointer is over it.
fn live_stream(session: &mut FormSession<'_>) -> Vec<u8> {
    session.mouse_move(0, INSIDE, Modifiers::NONE);
    session.mouse_down(0, INSIDE, Modifiers::NONE);
    let response = session.mouse_up(0, INSIDE, Modifiers::NONE);
    response
        .updates
        .iter()
        .find_map(|update| match &update.kind {
            UpdateKind::LiveEdit(ap) => Some(ap.stream.clone()),
            _ => None,
        })
        .expect("a focused text field draws its live editor state")
}

/// The height of the caret rectangle in a live appearance stream.
///
/// The stream draws two rectangles and the caret is the **second**: the
/// widget's `/MK /BG` background comes first, outside the `/Tx BMC` marked
/// content, and it is the field's whole 100x30 client box rather than
/// anything the metrics decide. So this takes the last `re`'s fourth operand,
/// and it is deliberately a literal parse of the emitted bytes — a test that
/// re-derived the geometry could agree with a wrong generator.
fn caret_height(stream: &[u8]) -> f32 {
    let text = String::from_utf8_lossy(stream);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let re = tokens
        .iter()
        .rposition(|token| *token == "re")
        .expect("a focused field strokes a caret, which is a `re`");
    tokens[re - 1]
        .parse()
        .expect("the caret's height is the last operand before `re`")
}

/// The plumbing, with no font set required: the constructor uses the context
/// it is handed.
///
/// Both sessions here resolve `/Arial` the same way, because both contexts
/// are default ones — so this cannot assert a *difference*. What it asserts
/// is that `with_context` produces a working session at all and that its
/// answer is the one `new` gives when the two contexts agree, which is the
/// property a caller relies on when it does not substitute.
#[test]
fn a_session_built_on_a_default_context_agrees_with_the_default_constructor() {
    let doc = document();

    let mut plain = FormSession::new(&doc);
    let mut ctx = BuildContext::new();
    let mut threaded = FormSession::with_context(&doc, &mut ctx);

    let (plain, threaded) = (
        caret_height(&live_stream(&mut plain)),
        caret_height(&live_stream(&mut threaded)),
    );
    assert!(
        (plain - threaded).abs() < f32::EPSILON,
        "two default contexts must substitute one document one way: {plain} vs {threaded}"
    );
}

/// The defect itself, measured.
///
/// Under the hermetic set, `/Arial` resolves to Arimo — 905 up, 211 down over
/// 1000 units — so a 12pt caret is `(905 + 211) * 12 / 1000` = **13.392**.
/// The default context has no directory to enumerate, finds no Arimo, and
/// falls through to the built-in base-14 Helvetica at 718 and −219, giving
/// `(718 + 219) * 12 / 1000` = **11.244**.
///
/// Those two numbers are the bug: the second is what the session used to
/// produce for a document the renderer was drawing with the first.
#[test]
fn the_hermetic_set_gives_the_caret_the_substituted_faces_height() {
    let Some(font_dir) = hermetic_font_dir() else {
        // The sibling oracle checkout is optional. The assertion above still
        // covers the plumbing without it.
        return;
    };
    let doc = document();

    let mut ctx = BuildContext::with_substitution(pdfrum::SubstitutionOptions {
        font_dirs: vec![font_dir],
        croscore_font_names: true,
        ..pdfrum::SubstitutionOptions::default()
    });
    let mut substituting = FormSession::with_context(&doc, &mut ctx);
    let substituted = caret_height(&live_stream(&mut substituting));

    let mut base14 = FormSession::new(&doc);
    let defaulted = caret_height(&live_stream(&mut base14));

    assert!(
        (substituted - 13.392).abs() < 1e-3,
        "Arimo's 905/-211 at 12pt is 13.392, got {substituted}"
    );
    assert!(
        (defaulted - 11.244).abs() < 1e-3,
        "base-14 Helvetica's 718/-219 at 12pt is 11.244, got {defaulted}"
    );
}

/// A Hebrew live edit is set in the **second face**, not in the `/DA` font's
/// low bytes.
///
/// # The two defects this closes, and why they are one test
///
/// The field's `/DA` names `/Arial`, whose charset is Ansi and which cannot
/// write a single Hebrew code point. `CPDF_BAFontMap::GetWordFontIndex`
/// (`core/fpdfdoc/cpdf_bafontmap.cpp:116-151`) answers that per character by
/// adding a face for the character's charset, and the question it asks knows
/// nothing about whether the character was **typed** or **stored**.
///
/// Ours used to. The stored `/V` path reached the second face; the typed path
/// forwarded `None` and wrote the low byte of each code point through Arial,
/// so `בחר` came out as Latin mojibake — and, worse than looking wrong, it
/// *measured* wrong: the Latin glyphs are wider than the Hebrew ones, so the
/// selection band ended ten units short of the run it was covering.
///
/// The assertion is therefore about the emitted bytes rather than about
/// pixels: the run's `Tf` must name the Hebrew substitute alias `/_B1`
/// (`font_map::substitute_alias(Charset::Hebrew)`), and the appearance's own
/// `/Resources /Font` must carry that key so the name resolves when the
/// stream is drawn.
#[test]
fn a_hebrew_live_edit_sets_its_text_in_the_second_face() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    session.mouse_move(0, INSIDE, Modifiers::NONE);
    session.mouse_down(0, INSIDE, Modifiers::NONE);
    session.mouse_up(0, INSIDE, Modifiers::NONE);

    // Typed, not stored: this is the path that used to forward no substitute.
    let mut last = None;
    for ch in "בחר".chars() {
        let response = session.character(ch, Modifiers::NONE);
        if let Some(update) = response
            .updates
            .iter()
            .find(|update| matches!(update.kind, UpdateKind::LiveEdit(_)))
        {
            last = Some(update.kind.clone());
        }
    }
    let UpdateKind::LiveEdit(ap) = last.expect("typing redraws the focused field") else {
        unreachable!("filtered to LiveEdit above")
    };

    let stream = String::from_utf8_lossy(&ap.stream).into_owned();
    assert!(
        stream.contains("/_B1"),
        "the Hebrew run must be set in the second face, got:\n{stream}"
    );
    // The alias has to resolve when the stream is drawn, so the appearance
    // carries the face in its own `/Resources /Font` rather than relying on
    // the form's `/DR`. Asserted through `Debug` because the facade does not
    // re-export the object model, and the key is the whole claim.
    let resources = format!("{:?}", ap.resources);
    assert!(
        resources.contains("_B1"),
        "the second face must be declared in the appearance's own resources, got:\n{resources}"
    );

    // And the run is **laid out** in that face, not merely encoded through it.
    //
    // The two are separable and only the second is what the pixels see: an
    // implementation that puts the substitute on `LiveInput` and leaves the
    // width closure on the `/DA` font emits exactly the `/_B1` and the
    // resource entry asserted above, and then advances every character by
    // Arial's table. That is the F2 residue this commit closed —
    // `form_textfield_selected_rtl`'s band ending at device column 101 with
    // its glyphs running to 111.
    //
    // The number is the substitute face's own advance for U+05D1: 250/1000 of
    // an em at 12 points is **3.0 units**, where the `/DA` fallback's 722
    // would be 8.664. Right-to-left, so the `Td` steps are negative.
    let advances: Vec<f32> = stream
        .lines()
        .filter_map(|line| line.strip_suffix(" Td"))
        .filter_map(|operands| {
            let mut parts = operands.split_whitespace();
            let x: f32 = parts.next()?.parse().ok()?;
            parts.next().filter(|y| *y == "0")?;
            Some(x)
        })
        .collect();
    assert!(
        !advances.is_empty(),
        "a three-character run steps between its characters, got:\n{stream}"
    );
    for advance in advances {
        assert!(
            (advance.abs() - 3.0).abs() < 1e-3,
            "each Hebrew character advances by the second face's 250/1000 at \
             12pt, which is 3.0 and not the /DA font's 8.664; got {advance}"
        );
    }
}
