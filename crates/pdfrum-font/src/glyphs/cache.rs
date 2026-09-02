//! The glyph outline cache.
//!
//! Owned by a render session, never global (SPEC §6, `docs/design/pdfrum-font.md`
//! D10). The key is the interesting part: SPEC's original `(font_id, gid,
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
/// and that side is unhinted at every size — `CFX_Face::LoadGlyphPath` hints
/// only a face on FreeType's `tricky` list, which no corpus font is.
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

    fn params(self) -> GlyphParams {
        GlyphParams {
            dest_width: self.dest_width,
            weight: self.weight,
        }
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
/// where the copies were then never read (`docs/status/M12b-P2.md` §6).
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
            .or_insert_with(|| font.glyphs().outline(key.gid, key.params()).map(Arc::new))
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
    use crate::{FontCache, subst};
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
            },
        );
        let wide = source.outline(
            gid,
            GlyphParams {
                dest_width: 900,
                weight: 400,
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
            },
        );
        let heavy = source.outline(
            gid,
            GlyphParams {
                dest_width: 0,
                weight: 900,
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
            },
        );
        let b = font.glyphs().outline(
            gid,
            GlyphParams {
                dest_width: 900,
                weight: 900,
            },
        );
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }
}
