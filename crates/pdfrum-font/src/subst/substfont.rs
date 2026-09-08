//! What a substitution decided, and the synthetic adjustments that follow
//! from it.
//!
//! When a document's font is replaced by a different face, the replacement is
//! rarely the right weight or slant. PDFium compensates by shearing the
//! outline and dilating it, by amounts read from three hand-tuned tables. The
//! tables are ported verbatim — including three entries in the middle of one
//! of them that look like transcription errors and are part of the observable
//! output.

use super::charset::Charset;
#[cfg(test)]
use super::tables::{ANGLE_SKEW, WEIGHT_POW, WEIGHT_POW_11, WEIGHT_POW_SHIFT_JIS};

/// The record a substitution produces.
///
/// `weight` and `weight_cjk` are `Option` where PDFium overloads **0** to mean
/// "the face's natural weight" (D12). The sentinel is real behavior — it makes
/// the embolden level 0 and the Multiple-Master axis take its default — so the
/// mapping back to 0 happens at the two places the arithmetic needs it, not
/// silently at construction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubstFont {
    /// The family name the substitution settled on.
    pub(crate) family: String,
    /// The charset the face was chosen for.
    pub charset: Charset,
    /// The requested weight, or `None` for the face's own.
    pub(crate) weight: Option<i32>,
    /// The CJK weight, tracked separately because it has its own default.
    pub(crate) weight_cjk: Option<i32>,
    /// The synthetic italic angle, in degrees. Negative slants right.
    pub italic_angle: i32,
    /// Whether a CJK substitution happened, which switches both the weight and
    /// the skew to their CJK variants for a CID font.
    pub(crate) subst_cjk: bool,
    /// Whether the CJK substitution asked for italic.
    pub(crate) italic_cjk: bool,
    /// Whether this is one of the two built-in Multiple-Master generics, which
    /// suppresses artificial emboldening entirely — the design space handles
    /// weight properly, so dilating on top would double-count it.
    pub is_builtin_generic: bool,
}

impl SubstFont {
    /// The weight the embolden and axis arithmetic reads, mapping `None` back
    /// to PDFium's 0 sentinel.
    #[must_use]
    pub fn raw_weight(&self) -> i32 {
        self.weight.unwrap_or(0)
    }

    /// The weight in effect, which for a CID font in a CJK substitution is the
    /// separately-tracked CJK weight (`GetEffectiveWeight`).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn effective_weight(&self, is_cid_font: bool) -> i32 {
        if self.subst_cjk && is_cid_font {
            self.weight_cjk.unwrap_or(0)
        } else {
            self.raw_weight()
        }
    }

    /// The synthetic shear, as hundredths of a unit of x per unit of y.
    ///
    /// A table lookup by `-italic_angle`, saturating at **-58** for a positive
    /// angle or one past the table's 30 entries.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn skew(&self) -> i32 {
        skew_from_angle(self.italic_angle)
    }

    /// The CJK shear: a fixed -15° when the CJK substitution asked for italic,
    /// and none otherwise.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn skew_cjk(&self) -> i32 {
        skew_from_angle(if self.italic_cjk { -15 } else { 0 })
    }

    /// The shear actually applied, which for a CID font in a CJK substitution
    /// is the CJK one.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn effective_skew(&self, is_cid_font: bool) -> i32 {
        if self.subst_cjk && is_cid_font {
            self.skew_cjk()
        } else {
            self.skew()
        }
    }

    /// How much to dilate an outline when *rendering*, given the text matrix's
    /// two horizontal components.
    ///
    /// Returns `None` where the C++ returns -1 and its caller abandons the
    /// glyph: a weight index at or past 100, i.e. a weight of 1400 or more.
    /// The intermediate is 64-bit deliberately — a large matrix overflows
    /// 32 bits and the oracle's own unittest pins the wide result.
    // `xx` and `xy` are the matrix components' own names; renaming either to
    // please the lint would make the pair harder to read, not easier.
    #[allow(clippy::similar_names)]
    #[must_use]
    #[cfg(test)]
    pub(crate) fn embolden_level_for_render(
        &self,
        is_cid_font: bool,
        matrix_xx: i32,
        matrix_xy: i32,
    ) -> Option<i32> {
        if self.is_builtin_generic {
            return Some(0);
        }
        let w = self.effective_weight(is_cid_font);
        if w <= 400 {
            return Some(0);
        }
        let index = usize::try_from((w - 400) / 10).ok()?;
        let level = weight_level(index, self.charset == Charset::ShiftJis)?;
        let scaled =
            i64::from(level) * (i64::from(matrix_xx).abs() + i64::from(matrix_xy).abs()) / 36655;
        Some(i32::try_from(scaled).unwrap_or(0))
    }

    /// How much to dilate when *loading* a glyph, which reads a different
    /// table and — unlike the render path — **clamps** the index rather than
    /// failing past 99.
    ///
    /// Note it also reads the plain weight, not the effective one.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn embolden_level_for_load(&self) -> i32 {
        if self.is_builtin_generic {
            return 0;
        }
        let w = self.raw_weight();
        if w <= 400 {
            return 0;
        }
        let Ok(index) = usize::try_from((w - 400) / 10) else {
            return 0;
        };
        weight_level_for_load(index.min(99), self.charset == Charset::ShiftJis)
    }

    /// The stem thickness implied by the weight.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn estimated_stem_v(&self) -> i32 {
        self.raw_weight() / 5
    }

    /// Whether a base font name names *this* face.
    ///
    /// A **prefix** test over the lowercased family with all spaces removed,
    /// which is loose enough to be wrong — the C++'s own comment notes that a
    /// family called `Book` would match `Bookman`. Ported as-is because the
    /// glyph-spacing heuristic of the former working note turns on it.
    ///
    /// `base_name` is a `/BaseFont` name, so it arrives as bytes and **the
    /// caller lowercases it** — the two sides are lowered separately upstream
    /// and a caller that has already lowered it for other tests should not
    /// pay for it twice. An empty family never matches: a substitution that
    /// named no family did not load the document's font.
    #[must_use]
    pub(crate) fn is_actual_font_loaded(&self, base_name: &[u8]) -> bool {
        let normalized: String = self
            .family
            .chars()
            .filter(|c| *c != ' ')
            .flat_map(char::to_lowercase)
            .collect();
        if normalized.is_empty() {
            return false;
        }
        base_name.starts_with(normalized.as_bytes())
    }

    /// Apply the adjustments `ConfigureExternalSubst` makes when a *system*
    /// face was chosen.
    ///
    /// Two sentinels live here. The weight is left at `None` — PDFium's 0 —
    /// when the request already matches the face's own weight, which is what
    /// makes the embolden level 0 for a face that needs no help. And the
    /// italic angle is nudged: an unslanted request against an upright face
    /// becomes -12°, while an angle already within 5° of upright is zeroed as
    /// not worth synthesizing.
    // The parameter list is the ported one: each argument is read by a distinct
    // rung of the adjustment above, and grouping them into a struct would only
    // move the same eight values behind a name that means nothing on its own.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn configure_external(
        &mut self,
        face_name: String,
        charset: Charset,
        weight: i32,
        is_italic: bool,
        mut italic_angle: i32,
        face_is_bold: bool,
        face_is_italic: bool,
    ) {
        self.family = face_name;
        self.charset = charset;
        let face_weight = if face_is_bold { 700 } else { 400 };
        if weight != face_weight {
            self.weight = Some(weight);
        }
        if is_italic && !face_is_italic {
            if italic_angle == 0 {
                italic_angle = -12;
            } else if italic_angle.abs() < 5 {
                italic_angle = 0;
            }
            self.italic_angle = italic_angle;
        }
    }

    /// Mark this as the built-in serif generic, which also scales the weight
    /// down by a fifth (`UseChromeSerif`).
    pub(crate) fn use_chrome_serif(&mut self) {
        "Chrome Serif".clone_into(&mut self.family);
        if let Some(w) = self.weight {
            self.weight = Some(w * 4 / 5);
        }
    }
}

/// The shear for an italic angle (`GetSkewFromAngle`).
#[cfg(test)]
#[must_use]
pub fn skew_from_angle(angle: i32) -> i32 {
    // A positive angle, the `i32::MIN` whose negation overflows, and anything
    // past the table all take the terminal value.
    if angle > 0 || angle == i32::MIN {
        return -58;
    }
    let index = angle.unsigned_abs() as usize;
    ANGLE_SKEW.get(index).map_or(-58, |&s| i32::from(s))
}

/// The render-path dilation table lookup. `None` past the table, where the
/// C++ returns -1 and its caller abandons the glyph.
#[must_use]
#[cfg(test)]
fn weight_level(index: usize, shift_jis: bool) -> Option<i32> {
    if index >= 100 {
        return None;
    }
    let table = if shift_jis {
        &WEIGHT_POW_SHIFT_JIS
    } else {
        &WEIGHT_POW_11
    };
    table.get(index).map(|&v| i32::from(v))
}

/// The load-path dilation table lookup, whose Shift-JIS arm is additionally
/// rescaled by `65536 / 36655`.
#[must_use]
#[cfg(test)]
fn weight_level_for_load(index: usize, shift_jis: bool) -> i32 {
    if shift_jis {
        WEIGHT_POW_SHIFT_JIS
            .get(index)
            .map_or(0, |&v| i32::from(v) * 65536 / 36655)
    } else {
        WEIGHT_POW.get(index).map_or(0, |&v| i32::from(v))
    }
}

/// The facts a font offers the glyph-spacing gate of the former working note.
///
/// A borrowed view rather than owned state: the answer is a property of a
/// loaded font, and pulling the five inputs out makes each of the gate's
/// refusals sayable on its own.
#[derive(Debug, Clone, Copy)]
pub struct GlyphSpacingGate<'a> {
    /// Whether the font writes vertically — a `-V` CMap.
    pub vertical: bool,
    /// Whether the document shipped a usable font program.
    pub embedded: bool,
    /// Whether the PDF declared its own advance widths (`HasFontWidths`).
    pub declared_widths: bool,
    /// The `/BaseFont` name, subset prefix already stripped, in any case.
    pub base_font_name: &'a [u8],
    /// What substitution settled on, or `None` when none ran.
    pub subst: Option<&'a SubstFont>,
}

/// Whether a font's glyphs take the glyph-spacing correction of the former working note.
///
/// The correction exists for one situation: a PDF that declares its own
/// advance widths, does **not** ship the font program, and got substituted
/// onto a face that draws its glyphs at some other width. The document's
/// widths are then the only truth about how wide the text should look, and the
/// face disagrees with it. Five conditions each say instead "the widths and
/// the outlines already agree, leave the glyph alone":
///
/// - **vertical writing** — the correction is horizontal, while a `-V` CMap's
///   advances run down the page;
/// - **an embedded font** — the program in the file *is* the font, so its
///   glyph widths are the document's own and cannot disagree with `/Widths`;
/// - **no declared widths** — a simple font with no `/Widths` reads its
///   advances off the face, so the comparison is a number against itself;
/// - **a standard-14 `/BaseFont` name** — Helvetica landing on Arial is a
///   sanctioned alias rather than a failed match, and the two families were
///   designed to share metrics;
/// - **a built-in generic** — the Multiple-Master fallbacks solve their own
///   width axis to the declared width, so their outlines already come out at
///   it and correcting again would double-count.
///
/// What survives is a substitution onto some *named* face, and the last
/// question is whether that face is the one the document asked for:
/// [`SubstFont::is_actual_font_loaded`] answers no, and only then does the
/// correction run. A font with no substitution record at all has no
/// substituted face to disagree with, and declines.
#[must_use]
pub fn applies_glyph_spacing(gate: &GlyphSpacingGate<'_>) -> bool {
    if gate.vertical || gate.embedded || !gate.declared_widths {
        return false;
    }
    // Both remaining tests are asked in lower case, so the name is lowered
    // once for the two of them.
    let lower = gate.base_font_name.to_ascii_lowercase();
    if super::standard_font_index(&lower).is_some() {
        return false;
    }
    let Some(subst) = gate.subst else {
        return false;
    };
    !subst.is_builtin_generic && !subst.is_actual_font_loaded(&lower)
}

#[cfg(test)]
mod tests {
    // Test fixtures are fixed-size arrays with known contents.
    #![allow(clippy::indexing_slicing)]
    use super::*;

    /// A gate that passes, which each refusal test then breaks one way.
    ///
    /// A document asking for `Verdana`, substituted onto a `Nimbus Sans`
    /// that is plainly a different family.
    fn passing_gate(subst: &SubstFont) -> GlyphSpacingGate<'_> {
        GlyphSpacingGate {
            vertical: false,
            embedded: false,
            declared_widths: true,
            base_font_name: b"Verdana",
            subst: Some(subst),
        }
    }

    fn nimbus() -> SubstFont {
        SubstFont {
            family: "Nimbus Sans".to_owned(),
            ..SubstFont::default()
        }
    }

    #[test]
    fn a_substitution_onto_a_different_family_takes_the_correction() {
        let subst = nimbus();
        assert!(applies_glyph_spacing(&passing_gate(&subst)));
    }

    #[test]
    fn vertical_writing_declines_the_correction() {
        let subst = nimbus();
        let gate = GlyphSpacingGate {
            vertical: true,
            ..passing_gate(&subst)
        };
        assert!(!applies_glyph_spacing(&gate));
    }

    #[test]
    fn an_embedded_program_declines_the_correction() {
        let subst = nimbus();
        let gate = GlyphSpacingGate {
            embedded: true,
            ..passing_gate(&subst)
        };
        assert!(!applies_glyph_spacing(&gate));
    }

    #[test]
    fn a_font_without_declared_widths_declines_the_correction() {
        let subst = nimbus();
        let gate = GlyphSpacingGate {
            declared_widths: false,
            ..passing_gate(&subst)
        };
        assert!(!applies_glyph_spacing(&gate));
    }

    /// The standard-14 test goes through the **alias** table, so a name that
    /// is not one of the fourteen canonical spellings still refuses.
    #[test]
    fn a_standard_fourteen_base_font_name_declines_the_correction() {
        let subst = nimbus();
        for name in [&b"Helvetica"[..], b"ArialMT", b"arial,bold", b"CourierNew"] {
            let gate = GlyphSpacingGate {
                base_font_name: name,
                ..passing_gate(&subst)
            };
            assert!(
                !applies_glyph_spacing(&gate),
                "{}",
                String::from_utf8_lossy(name)
            );
        }
    }

    #[test]
    fn a_built_in_generic_declines_the_correction() {
        let subst = SubstFont {
            family: "Chrome Sans".to_owned(),
            is_builtin_generic: true,
            ..SubstFont::default()
        };
        assert!(!applies_glyph_spacing(&passing_gate(&subst)));
    }

    /// The sixth refusal, and the one the gate ends on: the face that was
    /// loaded *is* the one the document named, so nothing needs correcting.
    #[test]
    fn loading_the_document_s_own_face_declines_the_correction() {
        let subst = SubstFont {
            family: "Verdana".to_owned(),
            ..SubstFont::default()
        };
        assert!(!applies_glyph_spacing(&passing_gate(&subst)));
        // And the prefix test is case-insensitive on the document's side,
        // because the gate lowers the `/BaseFont` name before asking.
        let bold = GlyphSpacingGate {
            base_font_name: b"Verdana,Bold",
            ..passing_gate(&subst)
        };
        assert!(!applies_glyph_spacing(&bold));
    }

    #[test]
    fn a_font_that_was_never_substituted_declines_the_correction() {
        let subst = nimbus();
        let gate = GlyphSpacingGate {
            subst: None,
            ..passing_gate(&subst)
        };
        assert!(!applies_glyph_spacing(&gate));
    }

    /// `cfx_substfont_unittest.cpp`'s `EffectiveSkew`.
    #[test]
    fn effective_skew_matches_the_oracle() {
        let mut s = SubstFont {
            italic_angle: -12,
            ..SubstFont::default()
        };
        assert_eq!(s.effective_skew(false), -21);
        s.subst_cjk = true;
        s.italic_cjk = true;
        assert_eq!(s.effective_skew(true), -27);
        // Not a CID font, so the CJK arm does not apply.
        assert_eq!(s.effective_skew(false), -21);
    }

    #[test]
    fn the_skew_table_saturates_outside_its_range() {
        assert_eq!(skew_from_angle(0), 0);
        assert_eq!(skew_from_angle(-1), -2);
        assert_eq!(skew_from_angle(-29), -55);
        // Past the table's 30 entries.
        assert_eq!(skew_from_angle(-30), -58);
        assert_eq!(skew_from_angle(-1000), -58);
        // Any positive angle.
        assert_eq!(skew_from_angle(1), -58);
        assert_eq!(skew_from_angle(i32::MAX), -58);
        // And the value whose negation overflows.
        assert_eq!(skew_from_angle(i32::MIN), -58);
    }

    #[test]
    fn the_cjk_skew_is_a_fixed_fifteen_degrees_or_none() {
        let mut s = SubstFont::default();
        assert_eq!(s.skew_cjk(), 0);
        s.italic_cjk = true;
        assert_eq!(s.skew_cjk(), -27);
    }

    /// `cfx_substfont_unittest.cpp`'s `EffectiveWeight`.
    #[test]
    fn effective_weight_switches_only_for_a_cid_font() {
        let s = SubstFont {
            weight: Some(700),
            weight_cjk: Some(400),
            subst_cjk: true,
            ..SubstFont::default()
        };
        assert_eq!(s.effective_weight(true), 400);
        assert_eq!(s.effective_weight(false), 700);
        // Without a CJK substitution the CJK weight is never consulted.
        let s = SubstFont {
            weight: Some(700),
            weight_cjk: Some(400),
            ..SubstFont::default()
        };
        assert_eq!(s.effective_weight(true), 700);
    }

    /// `cfx_substfont_unittest.cpp`'s `EmboldenLevels`, all five assertions.
    #[test]
    fn embolden_levels_match_the_oracle() {
        let mut s = SubstFont {
            weight: Some(700),
            ..SubstFont::default()
        };
        // Weight 700 is index 30, whose render value is 39.
        assert_eq!(s.embolden_level_for_render(false, 1024, 0), Some(1));
        // And whose load value is 70, from the *other* table.
        assert_eq!(s.embolden_level_for_load(), 70);
        // The i64 intermediate: 39 * 60_000_000 overflows an i32 before the
        // division, so a 32-bit intermediate would give the wrong answer.
        assert_eq!(
            s.embolden_level_for_render(false, 30_000_000, 30_000_000),
            Some(63838)
        );
        // A built-in generic zeroes both, because its design space already
        // carries the weight.
        s.is_builtin_generic = true;
        assert_eq!(s.embolden_level_for_render(false, 1024, 0), Some(0));
        assert_eq!(s.embolden_level_for_load(), 0);
    }

    #[test]
    fn a_weight_at_or_below_four_hundred_needs_no_emboldening() {
        for w in [0, 100, 400] {
            let s = SubstFont {
                weight: Some(w),
                ..SubstFont::default()
            };
            assert_eq!(s.embolden_level_for_render(false, 1024, 0), Some(0));
            assert_eq!(s.embolden_level_for_load(), 0);
        }
    }

    #[test]
    fn the_render_path_fails_past_the_table_while_the_load_path_clamps() {
        // Index 100 is weight 1400.
        let s = SubstFont {
            weight: Some(1400),
            ..SubstFont::default()
        };
        assert_eq!(
            s.embolden_level_for_render(false, 1024, 0),
            None,
            "the render path abandons the glyph"
        );
        assert_eq!(
            s.embolden_level_for_load(),
            i32::from(WEIGHT_POW[99]),
            "the load path clamps to the last entry"
        );
    }

    /// `cfx_substfont_unittest.cpp`'s `EstimatedStemV`.
    #[test]
    fn the_stem_estimate_is_a_fifth_of_the_weight() {
        let s = SubstFont {
            weight: Some(700),
            ..SubstFont::default()
        };
        assert_eq!(s.estimated_stem_v(), 140);
    }

    /// `cfx_substfont_unittest.cpp`'s `IsActualFontLoaded`.
    #[test]
    fn is_actual_font_loaded_is_a_loose_prefix_test() {
        let s = SubstFont {
            family: "Times New Roman".to_owned(),
            ..SubstFont::default()
        };
        assert!(s.is_actual_font_loaded(b"timesnewroman,bold"));
        assert!(s.is_actual_font_loaded(b"timesnewromanps-bold"));
        assert!(!s.is_actual_font_loaded(b"arial,bold"));
        // The looseness the C++ comment acknowledges.
        let book = SubstFont {
            family: "Book".to_owned(),
            ..SubstFont::default()
        };
        assert!(book.is_actual_font_loaded(b"bookman"));
        // An empty family matches nothing rather than everything.
        assert!(!SubstFont::default().is_actual_font_loaded(b"anything"));
    }

    #[test]
    fn the_weight_sentinel_survives_a_matching_face() {
        let mut s = SubstFont::default();
        // A 400-weight request against an upright face leaves the weight
        // unset, which is what keeps the embolden level at 0.
        s.configure_external(
            "Arial".to_owned(),
            Charset::Ansi,
            400,
            false,
            0,
            false,
            false,
        );
        assert_eq!(s.weight, None);
        assert_eq!(s.raw_weight(), 0);
        assert_eq!(s.embolden_level_for_load(), 0);

        // A 700-weight request against the same upright face does set it.
        let mut s = SubstFont::default();
        s.configure_external(
            "Arial".to_owned(),
            Charset::Ansi,
            700,
            false,
            0,
            false,
            false,
        );
        assert_eq!(s.weight, Some(700));

        // ...and a 700-weight request against a *bold* face does not.
        let mut s = SubstFont::default();
        s.configure_external(
            "Arial".to_owned(),
            Charset::Ansi,
            700,
            false,
            0,
            true,
            false,
        );
        assert_eq!(s.weight, None);
    }

    #[test]
    fn the_three_italic_angle_cases() {
        // Zero against an upright face becomes -12.
        let mut s = SubstFont::default();
        s.configure_external("F".to_owned(), Charset::Ansi, 400, true, 0, false, false);
        assert_eq!(s.italic_angle, -12);

        // An angle within 5 degrees of upright is not worth synthesizing.
        for angle in [-4, 4, 1] {
            let mut s = SubstFont::default();
            s.configure_external(
                "F".to_owned(),
                Charset::Ansi,
                400,
                true,
                angle,
                false,
                false,
            );
            assert_eq!(s.italic_angle, 0, "angle {angle}");
        }

        // Anything larger is kept.
        let mut s = SubstFont::default();
        s.configure_external("F".to_owned(), Charset::Ansi, 400, true, -20, false, false);
        assert_eq!(s.italic_angle, -20);

        // And a face that is already italic needs no synthesis at all.
        let mut s = SubstFont::default();
        s.configure_external("F".to_owned(), Charset::Ansi, 400, true, 0, false, true);
        assert_eq!(s.italic_angle, 0);
    }

    #[test]
    fn chrome_serif_scales_the_weight_by_four_fifths() {
        let mut s = SubstFont {
            weight: Some(500),
            ..SubstFont::default()
        };
        s.use_chrome_serif();
        assert_eq!(s.family, "Chrome Serif");
        assert_eq!(s.weight, Some(400));
        // With no weight set, the sentinel survives.
        let mut s = SubstFont::default();
        s.use_chrome_serif();
        assert_eq!(s.weight, None);
    }

    #[test]
    fn the_weight_pow_11_table_has_three_non_monotonic_entries() {
        // These look like transcription errors frozen into PDFium's output.
        // They are part of the observable behavior and must not be "fixed".
        assert_eq!(WEIGHT_POW_11[52], 43, "dips below the 47 before it");
        assert_eq!(WEIGHT_POW_11[51], 46);
        assert_eq!(WEIGHT_POW_11[59], 45, "dips below the 48 before it");
        assert_eq!(WEIGHT_POW_11[58], 48);
        assert_eq!(WEIGHT_POW_11[63], 46, "dips below the 50 before it");
        assert_eq!(WEIGHT_POW_11[62], 50);

        // ...and they are the *only* three descents in the whole ramp.
        let descents = WEIGHT_POW_11.windows(2).filter(|w| w[1] < w[0]).count();
        assert_eq!(descents, 3);
    }

    #[test]
    fn the_other_two_weight_tables_are_monotonic() {
        for (name, table) in [
            ("kWeightPow", &WEIGHT_POW),
            ("kWeightPowShiftJis", &WEIGHT_POW_SHIFT_JIS),
        ] {
            assert!(
                table.windows(2).all(|w| w[1] >= w[0]),
                "{name} should be non-decreasing"
            );
        }
    }

    #[test]
    fn the_shift_jis_arm_reads_a_different_table() {
        let s = SubstFont {
            weight: Some(700),
            charset: Charset::ShiftJis,
            ..SubstFont::default()
        };
        // Index 30 in the Shift-JIS table is 96, not 39 or 70.
        assert_eq!(s.embolden_level_for_render(false, 36655, 0), Some(96));
        // And the load path rescales it.
        assert_eq!(s.embolden_level_for_load(), 96 * 65536 / 36655);
    }
}
