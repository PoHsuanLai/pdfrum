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

/// What identifies one cached outline.
///
/// `hint_flags` is deliberately absent: we draw filled unhinted outlines
/// unconditionally (OQ-4), so it would be a constant. The other five fields
/// all genuinely vary an outline for at least one face kind.
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
#[derive(Debug, Default)]
pub struct GlyphCache {
    entries: HashMap<GlyphKey, Option<BezPath>>,
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
    pub fn path(&mut self, font: &Font, key: GlyphKey) -> Option<&BezPath> {
        self.entries
            .entry(key)
            .or_insert_with(|| font.glyphs().outline(key.gid, &key.params()))
            .as_ref()
    }

    /// How many outlines — hits and misses alike — are memoized.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether anything has been drawn yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Forget everything.
    pub fn clear(&mut self) {
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
        let dict = Dict::from_pairs([
            (
                crate::names::SUBTYPE.clone(),
                Object::Name(Name::from("Type1")),
            ),
            (
                crate::names::BASE_FONT.clone(),
                Object::Name(Name::from("Helvetica")),
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
            &GlyphParams {
                dest_width: 200,
                weight: 400,
            },
        );
        let wide = source.outline(
            gid,
            &GlyphParams {
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
            &GlyphParams {
                dest_width: 0,
                weight: 100,
            },
        );
        let heavy = source.outline(
            gid,
            &GlyphParams {
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
            &GlyphParams {
                dest_width: 100,
                weight: 100,
            },
        );
        let b = font.glyphs().outline(
            gid,
            &GlyphParams {
                dest_width: 900,
                weight: 900,
            },
        );
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }
}
