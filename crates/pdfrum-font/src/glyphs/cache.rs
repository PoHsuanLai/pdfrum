//! The glyph outline cache.
//!
//! Owned by a render session, never global. The key is the interesting part:
//! the obvious `(font_id, gid,
//! hint_flags)` is **insufficient**, because `dest_width` alone changes the
//! outline of a Multiple-Master face — and the Multiple-Master faces are the
//! terminal rung of the substitution ladder, so they are what draws every font
//! the system cannot supply. The corrected key is the one PDFium's own path
//! cache uses.

use super::GlyphParams;
use crate::{Font, FontId, Gid};
use pdfrum_common::kurbo::BezPath;
use std::collections::HashMap;
use std::sync::Arc;

/// What identifies one cached outline.
///
/// `hint_flags` is deliberately absent, and stayed absent when wave 7b brought
/// hinting in. This cache holds the outlines the *path* side of text fills,
/// and that side is unhinted at every size, for every face.
///
/// The hinted outline belongs to the glyph-*bitmap* side, which is a different
/// cache in a different crate (`pdfrum_render::glyph::BitmapCache`) because it
/// holds a different thing: a rasterization, which depends on the device
/// matrix, where an outline does not. That separation is why this key did not
/// need to grow a size — the key that does have one is over there.
///
/// The five fields below all genuinely vary an outline for at least one face
/// kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    /// Which font — cache entries from two fonts must never be confused even
    /// when they share a face.
    pub font: FontId,
    /// Which glyph.
    pub gid: Gid,
    /// The PDF's declared width for this character code, which solves a
    /// Multiple-Master face's width axis.
    pub dest_width: i32,
    /// The substitution weight, which drives a Multiple-Master face's weight
    /// axis. Zero means the face's own.
    pub weight: i32,
    /// The synthetic italic angle, which shears the outline.
    pub italic_angle: i32,
    /// Whether this is a vertical-writing form.
    pub vertical: bool,
}

impl GlyphKey {
    /// A key for a glyph drawn with no substitution adjustments at all.
    #[must_use]
    pub fn plain(font: FontId, gid: Gid) -> Self {
        Self {
            font,
            gid,
            dest_width: 0,
            weight: 0,
            italic_angle: 0,
            vertical: false,
        }
    }

    /// What this key asks the face for, resolved against the font it names.
    ///
    /// The first two fields go straight through; the italic angle does not,
    /// because the shear it stands for is a *table* lookup that the CJK/CID
    /// arm can override (`GetEffectiveSkew`), and the embolden level is not in
    /// the key at all — it is a function of the weight that already is.
    /// Carrying the two derived numbers rather than the raw inputs is what
    /// keeps this the only place that knows the derivation.
    ///
    /// `font` must be the font `self.font` identifies, which is the same
    /// precondition [`GlyphCache::path`] states.
    fn params(self, font: &Font) -> GlyphParams {
        let mut params = GlyphParams {
            dest_width: self.dest_width,
            weight: self.weight,
            ..GlyphParams::default()
        };
        let Some(subst) = font.subst() else {
            return params;
        };
        params.vertical = font.is_vertical();
        // The *path* side takes the plain skew, not the effective one — the
        // CJK arm is the render side's alone (`cfx_face.cpp:870` calls
        // `GetSkew()` where `:769` calls `GetEffectiveSkew()`).
        params.skew = subst.skew();
        // The load-path level is in the 26.6 units of a 64-ppem instance —
        // 64*64 per em, PDFium's own `kCoordUnit` (`cfx_face.cpp:67`) — so it
        // reaches this crate's 1000/em outlines scaled by 1000/4096.
        params.embolden = f64::from(subst.embolden_level_for_load()) * 1000.0 / 4096.0;
        params
    }
}

/// Memoized glyph outlines in 1000/em text space.
///
/// A miss is cached too: a glyph that produced no outline is stored as `None`
/// so it is not recomputed, which is what PDFium's own null-result memoization
/// does.
///
/// The outlines are held behind an `Arc` so a caller that wants one *for longer
/// than the borrow* — the renderer's per-glyph placement record, which cannot
/// hold a borrow because placing the next glyph needs the cache mutably again —
/// takes a refcount rather than a copy of the path. Copying instead was 2891
/// `BezPath` clones and 3.3 MiB per render of one corpus page, on a document
/// where the copies were then never read.
#[derive(Debug, Default)]
pub struct GlyphCache {
    entries: HashMap<GlyphKey, Option<Arc<BezPath>>>,
}

impl GlyphCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The outline for `key`, drawing it if this is the first request.
    ///
    /// `font` must be the font `key.font` identifies; passing a different one
    /// returns that font's glyph under the wrong key, which is why the key
    /// carries the id at all.
    #[cfg(test)]
    pub(crate) fn path(&mut self, font: &Font, key: GlyphKey) -> Option<&BezPath> {
        self.entry(font, key).map(AsRef::as_ref)
    }

    /// The same outline, as a handle that outlives the borrow.
    ///
    /// For a caller that needs the outline *after* asking the cache for the next
    /// glyph. Cloning the returned `Arc` is a refcount bump; cloning a borrowed
    /// path copies every element.
    pub fn shared(&mut self, font: &Font, key: GlyphKey) -> Option<Arc<BezPath>> {
        self.entry(font, key).map(Arc::clone)
    }

    /// The stored entry, drawn on first request.
    fn entry(&mut self, font: &Font, key: GlyphKey) -> Option<&Arc<BezPath>> {
        self.entries
            .entry(key)
            .or_insert_with(|| {
                font.glyphs()
                    .outline(key.gid, key.params(font))
                    .map(Arc::new)
            })
            .as_ref()
    }

    /// How many outlines — hits and misses alike — are memoized.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether anything has been drawn yet.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Forget everything.
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontCache, SubstFont, subst};
    use pdfrum_common::kurbo::Shape;
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Dict, Name, NoResolve, Object};

    fn helvetica() -> Font {
        named("Helvetica")
    }

    /// A non-embedded Type 1 by name. An unrecognised name falls all the way
    /// to the built-in Multiple-Master generic, which is the face the width
    /// solve applies to; a base-14 name resolves to a Foxit blob instead.
    fn named(base_font: &str) -> Font {
        let dict = Dict::from_pairs([
            (
                crate::names::SUBTYPE.clone(),
                Object::Name(Name::from("Type1")),
            ),
            (
                crate::names::BASE_FONT.clone(),
                Object::Name(Name::from(base_font)),
            ),
        ]);
        crate::load(
            &dict,
            &NoResolve,
            &FontCache::new(),
            &Limits::default(),
            &mut Diagnostics::default(),
        )
        .expect("a simple font always constructs")
    }

    #[test]
    fn the_key_separates_every_field_it_declares() {
        let base = GlyphKey::plain(FontId(1), Gid(5));
        let variants = [
            GlyphKey {
                font: FontId(2),
                ..base
            },
            GlyphKey {
                gid: Gid(6),
                ..base
            },
            GlyphKey {
                dest_width: 700,
                ..base
            },
            GlyphKey {
                weight: 700,
                ..base
            },
            GlyphKey {
                italic_angle: -12,
                ..base
            },
            GlyphKey {
                vertical: true,
                ..base
            },
        ];
        for v in variants {
            assert_ne!(base, v, "these must be distinct cache entries");
        }
    }

    #[test]
    fn a_repeated_request_is_served_from_the_cache() {
        let font = helvetica();
        let mut cache = GlyphCache::new();
        let gid = font.glyphs().name_index(b"A");
        let key = GlyphKey::plain(font.id(), Gid(gid));

        assert!(cache.is_empty());
        let first = cache.path(&font, key).cloned();
        assert_eq!(cache.len(), 1);
        let second = cache.path(&font, key).cloned();
        assert_eq!(cache.len(), 1, "no second entry was created");
        assert_eq!(first, second);
    }

    #[test]
    fn a_glyph_with_no_outline_is_memoized_as_a_miss() {
        let font = helvetica();
        let mut cache = GlyphCache::new();
        // A glyph index far past the face's own count.
        let key = GlyphKey::plain(font.id(), Gid(60_000));
        assert!(cache.path(&font, key).is_none());
        assert_eq!(cache.len(), 1, "the miss itself is cached");
        assert!(cache.path(&font, key).is_none());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_dest_width_solves_the_multiple_master_width_axis() {
        // Burn-down wave 5's font defect, in the form that outlives the file
        // that exposed it. `AGaramond` is not embedded and is not a base-14
        // name, so it substitutes onto the built-in generic — a
        // Multiple-Master Type 1 face — and PDFium then solves that face's
        // width axis until the glyph's own advance equals the PDF's declared
        // `/Widths` value
        // (`AdjustVariationParams`, `cfx_face.cpp:1561-1605`, reached from
        // `cpdf_font.cpp:440-444`).
        //
        // Leaving `dest_width` at zero draws the axis default instead. On
        // `5.5_simple_font.pdf`, whose `/Widths` say `a = 800` against a face
        // whose own is 452, the glyphs overran their advances and piled into
        // each other — which read as dropped characters on the `/Type_1_F`
        // band and as a "too narrow" `/Type_1_MM_F` band. Both were this.
        let font = named("AGaramond");
        assert!(
            font.subst().is_some_and(|s| s.is_builtin_generic),
            "the fixture must actually reach the Multiple-Master generic"
        );
        let gid = Gid(font.glyphs().name_index(b"a"));
        let params = |w: i32| GlyphParams {
            dest_width: w,
            weight: 0,
            ..GlyphParams::default()
        };
        let at = |w: i32| font.glyphs().advance(gid, params(w));

        let default = at(0);
        let narrow = at(300);
        let wide = at(900);
        assert!(default > 0, "the substitute face has a real glyph");
        assert!(
            narrow < default && default < wide,
            "the axis solve tracks dest_width: {narrow} < {default} < {wide}"
        );
        // The solve is an interpolation onto the requested advance, so it
        // lands on it rather than merely moving toward it.
        // Inside the axis the solve is an *interpolation onto the requested
        // advance*, so it lands on it exactly rather than merely moving
        // toward it. This is the assertion the defect would have failed:
        // before the fix every one of these returned the default, 556.
        for want in [300, 400, 500, 600, 700] {
            assert_eq!(at(want), want, "dest_width {want} must be solved for");
        }
        // Outside it the advance saturates, because the interpolated design
        // *coordinate* is unclamped (`AdjustVariationParams` deliberately
        // extrapolates) but the blend then clamps it to the axis range, which
        // is what `FT_Set_MM_Design_Coordinates` does. So an extreme
        // `/Widths` gets the widest or narrowest the face can draw, not a
        // degenerate outline.
        assert_eq!(at(900), at(1500), "the wide end saturates");
        assert_eq!(at(50), at(1), "and so does the narrow end");
        assert!(at(50) < at(300) && at(700) < at(900));
    }

    #[test]
    fn a_dest_width_and_the_default_are_separate_cache_entries() {
        // The whole reason `dest_width` is in the key: the two draw different
        // outlines from the same face and glyph.
        let font = named("AGaramond");
        let mut cache = GlyphCache::new();
        let gid = Gid(font.glyphs().name_index(b"a"));
        let plain = GlyphKey::plain(font.id(), gid);
        let sized = GlyphKey {
            dest_width: 300,
            ..plain
        };
        let a = cache.path(&font, plain).cloned();
        let b = cache.path(&font, sized).cloned();
        assert_eq!(cache.len(), 2, "two entries, not one");
        assert_ne!(a, b, "a solved width draws a different outline");
    }

    #[test]
    fn clearing_empties_the_cache() {
        let font = helvetica();
        let mut cache = GlyphCache::new();
        cache.path(&font, GlyphKey::plain(font.id(), Gid(1)));
        assert!(!cache.is_empty());
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn dest_width_changes_a_multiple_master_outline() {
        // The reason the SPEC key had to grow. The two generic fallback faces
        // are Multiple Master, and the width axis is solved from `dest_width`
        // — so two requests differing only in that field must not share an
        // entry, and must not produce the same outline either.
        let (source, _) = subst::builtin_generic(false);
        let gid = Gid(source.name_index(b"A"));
        assert_ne!(gid.0, 0, "the fallback face has an `A`");

        let narrow = source.outline(
            gid,
            GlyphParams {
                dest_width: 200,
                weight: 400,
                ..GlyphParams::default()
            },
        );
        let wide = source.outline(
            gid,
            GlyphParams {
                dest_width: 900,
                weight: 400,
                ..GlyphParams::default()
            },
        );
        let (Some(narrow), Some(wide)) = (narrow, wide) else {
            panic!("both instantiations must draw");
        };
        assert_ne!(
            format!("{narrow:?}"),
            format!("{wide:?}"),
            "the width axis must actually move the outline"
        );
    }

    #[test]
    fn weight_changes_a_multiple_master_outline() {
        let (source, _) = subst::builtin_generic(false);
        let gid = Gid(source.name_index(b"A"));
        let light = source.outline(
            gid,
            GlyphParams {
                dest_width: 0,
                weight: 100,
                ..GlyphParams::default()
            },
        );
        let heavy = source.outline(
            gid,
            GlyphParams {
                dest_width: 0,
                weight: 900,
                ..GlyphParams::default()
            },
        );
        let (Some(light), Some(heavy)) = (light, heavy) else {
            panic!("both instantiations must draw");
        };
        assert_ne!(format!("{light:?}"), format!("{heavy:?}"));
    }

    #[test]
    fn a_base14_face_ignores_the_variation_fields() {
        // A bare CFF has no design space, so the extra key fields are inert
        // for it — which is exactly why they were easy to leave out and wrong
        // to leave out.
        let font = helvetica();
        let gid = Gid(font.glyphs().name_index(b"A"));
        let a = font.glyphs().outline(
            gid,
            GlyphParams {
                dest_width: 100,
                weight: 100,
                ..GlyphParams::default()
            },
        );
        let b = font.glyphs().outline(
            gid,
            GlyphParams {
                dest_width: 900,
                weight: 900,
                ..GlyphParams::default()
            },
        );
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    /// A non-embedded font with a descriptor complete enough to earn
    /// `USE_EXTERN_ATTR` — without that flag substitution throws the caller's
    /// weight and slant away, and the synthetic adjustments never arise.
    fn synthesized(base_font: &str, italic_angle: i64, weight: i64) -> Font {
        let desc = Dict::from_pairs([
            (
                crate::names::ITALIC_ANGLE.clone(),
                Object::Int(italic_angle),
            ),
            (crate::names::FONT_WEIGHT.clone(), Object::Int(weight)),
            (crate::names::ASCENT.clone(), Object::Int(700)),
            (crate::names::DESCENT.clone(), Object::Int(-200)),
            (crate::names::CAP_HEIGHT.clone(), Object::Int(700)),
            (crate::names::STEM_V.clone(), Object::Int(80)),
        ]);
        let dict = Dict::from_pairs([
            (
                crate::names::SUBTYPE.clone(),
                Object::Name(Name::from("TrueType")),
            ),
            (
                crate::names::BASE_FONT.clone(),
                Object::Name(Name::from(base_font)),
            ),
            (crate::names::FONT_DESCRIPTOR.clone(), Object::Dict(desc)),
        ]);
        crate::load(
            &dict,
            &NoResolve,
            &FontCache::new(),
            &Limits::default(),
            &mut Diagnostics::default(),
        )
        .expect("a simple font always constructs")
    }

    /// The defect this cache had for its whole life: `GlyphKey` carried the
    /// italic angle, `params` dropped it on the floor, and nothing anywhere
    /// sheared an outline — so a document asking for an italic it did not
    /// embed got upright glyphs (`CFX_Face::LoadGlyphPath`,
    /// `cfx_face.cpp:869-876`).
    #[test]
    fn a_synthetic_italic_leans_the_outline_the_document_asked_for() {
        let upright = synthesized("SomeFontNobodyHas", 0, 400);
        let italic = synthesized("SomeFontNobodyHas", -20, 400);
        assert_eq!(upright.subst().map(|s| s.italic_angle), Some(0));
        assert_eq!(
            italic.subst().map(|s| s.italic_angle),
            Some(-20),
            "the fixture must actually reach a synthetic slant"
        );

        let gid = Gid(italic.glyphs().name_index(b"A"));
        let mut cache = GlyphCache::new();
        let straight = cache
            .path(&upright, GlyphKey::plain(upright.id(), gid))
            .cloned()
            .expect("the fallback face draws an A");
        let leaning = cache
            .path(
                &italic,
                GlyphKey {
                    italic_angle: -20,
                    ..GlyphKey::plain(italic.id(), gid)
                },
            )
            .cloned()
            .expect("and draws it slanted too");
        assert_ne!(
            format!("{straight:?}"),
            format!("{leaning:?}"),
            "the shear must reach the outline"
        );
        // A shear about the baseline leaves the bottom where it was and pushes
        // the top to the right, so the slanted glyph reaches further right
        // without reaching further left.
        let (a, b) = (straight.bounding_box(), leaning.bounding_box());
        assert!(b.x1 > a.x1, "the top must lean right: {a:?} vs {b:?}");
        assert!(b.y0 >= a.y0 - 1.0 && b.y1 <= a.y1 + 1.0, "y is untouched");
    }

    /// The other half, and the one that reads a different table on each of the
    /// oracle's two call sites: a weight the face cannot supply is dilated
    /// into the outline (`FT_Outline_Embolden`, `cfx_face.cpp:886-892`).
    /// The other half, and the one that reads a different table on each of the
    /// oracle's two call sites: a weight the face cannot supply is dilated into
    /// the outline (`FT_Outline_Embolden`, `cfx_face.cpp:886-892`).
    ///
    /// Driven through [`GlyphParams`] rather than through a substitution,
    /// because the only face a test can reach without a system font database
    /// is the Multiple-Master generic — and that one deliberately suppresses
    /// the dilation, solving weight on its own axis instead. The derivation
    /// from a weight to a level is [`SubstFont::embolden_level_for_load`]'s,
    /// and is pinned beside it; what is proven here is that the level reaches
    /// the outline at all, which is what it never used to do.
    #[test]
    fn a_synthetic_bold_dilates_the_outline_the_document_asked_for() {
        let font = helvetica();
        let gid = Gid(font.glyphs().name_index(b"o"));
        let at = |embolden: f64| {
            font.glyphs()
                .outline(
                    gid,
                    GlyphParams {
                        embolden,
                        ..GlyphParams::default()
                    },
                )
                .expect("Helvetica draws an o")
        };
        let thin = at(0.0);
        // Weight 700 is level 70 on the load table, which is 70/4096 of an em.
        let fat = at(f64::from(70) * 1000.0 / 4096.0);
        assert_ne!(format!("{thin:?}"), format!("{fat:?}"));
        assert!(
            fat.area().abs() > thin.area().abs(),
            "the dilation must add ink: {} vs {}",
            fat.area().abs(),
            thin.area().abs()
        );
        // ...and the level really is the one a 700-weight substitution asks
        // for, so the number above is not a magic constant.
        let bold = SubstFont {
            weight: Some(700),
            ..SubstFont::default()
        };
        assert_eq!(bold.embolden_level_for_load(), 70);
    }

    /// The bitmap side's own resolution, which is not the path side's: its
    /// embolden level scales with the device matrix (`GetEmboldenLevelForRender`,
    /// `cfx_face.cpp:806-816`) where the path side's does not, and it reads the
    /// effective skew rather than the plain one (`:769` against `:870`).
    #[test]
    fn the_bitmap_side_scales_its_dilation_with_the_device_matrix() {
        let italic = synthesized("SomeFontNobodyHas", -20, 400);
        // 12 pt at 1:1, in the oracle's 16.16 `matrix.a / 64 * 65536`.
        let ft = |em_per_px: f64| (em_per_px / 64.0 * 65536.0) as i32;
        let small = italic.render_synth(ft(12.0), 0).expect("12 pt draws");
        let large = italic.render_synth(ft(48.0), 0).expect("48 pt draws too");
        // The generic suppresses the dilation, so what the size must move here
        // is nothing — but the skew is a pure table lookup and must not move
        // with the size either way.
        assert_eq!(small.skew, large.skew);
        // `kAngleSkew[20]`, the value `GetSkewFromAngle` returns for -20°.
        assert_eq!(small.skew, -36, "the render side reads the same table");
        assert!(!small.vertical, "a simple font is never vertical");

        // A face that does dilate: the level is proportional to the matrix, so
        // four times the size is four times the strength.
        let bold = SubstFont {
            weight: Some(700),
            ..SubstFont::default()
        };
        let level = |px: f64| {
            bold.embolden_level_for_render(false, ft(px), 0)
                .expect("700 is inside the table")
        };
        assert!(
            level(48.0) > level(12.0) * 3,
            "{} {}",
            level(12.0),
            level(48.0)
        );
    }

    /// The oracle abandons the glyph outright when the level comes back
    /// negative — a substitution weight of 1400 or more is past the render
    /// table, and `RenderGlyph` returns a null bitmap (`cfx_face.cpp:809-811`).
    #[test]
    fn a_weight_past_the_render_table_abandons_the_glyph() {
        let heavy = SubstFont {
            weight: Some(1400),
            ..SubstFont::default()
        };
        assert_eq!(heavy.embolden_level_for_render(false, 1024, 0), None);
        // ...and the load side clamps instead of failing, which is why the two
        // sides cannot share one answer.
        assert!(heavy.embolden_level_for_load() > 0);
    }

    /// An embedded font is the document's own program, so neither adjustment
    /// applies — and the outline must come back byte-for-byte as the face drew
    /// it, not through a shear of zero that a floating-point matrix would
    /// perturb.
    #[test]
    fn a_font_with_no_substitution_takes_neither_adjustment() {
        let font = helvetica();
        assert!(font.subst().is_none_or(|s| s.italic_angle == 0));
        let gid = Gid(font.glyphs().name_index(b"A"));
        let key = GlyphKey::plain(font.id(), gid);
        let mut cache = GlyphCache::new();
        let cached = cache.path(&font, key).cloned().expect("Helvetica draws");
        let raw = font
            .glyphs()
            .outline(gid, GlyphParams::default())
            .expect("and draws unadjusted");
        assert_eq!(format!("{cached:?}"), format!("{raw:?}"));
    }
}
