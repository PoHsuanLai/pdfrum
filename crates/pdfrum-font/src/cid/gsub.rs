//! Vertical glyph substitution through the OpenType `GSUB` table.
//!
//! A vertical-writing CJK font draws different glyphs for brackets, dashes and
//! punctuation than it does horizontally, and says so through the `vert` and
//! `vrt2` features. `read-fonts` parses the table; what is ported here is
//! PDFium's **selection policy**, which is not what the OpenType specification
//! prescribes (`docs/design/pdfrum-font.md` §1.10.6).

use crate::GlyphSource;
use pdfrum_common::{DiagKind, Diagnostics, Severity};
use read_fonts::TableProvider;
use read_fonts::tables::gsub::{Gsub, SingleSubst, SubstitutionLookup};
use read_fonts::types::Tag;
use std::collections::HashMap;

/// The two feature tags that mean "the vertical form of this glyph".
const VERT: Tag = Tag::new(b"vert");
const VRT2: Tag = Tag::new(b"vrt2");

/// The single substitutions the vertical features declare, flattened.
///
/// A map rather than a live view of the table: the substitutions are a small
/// closed set and resolving one is on the per-glyph path, so paying the parse
/// once is right. It also keeps the font `Send + Sync` without a lock.
#[derive(Debug, Clone, Default)]
pub(super) struct VerticalSubst {
    map: HashMap<u16, u16>,
}

impl VerticalSubst {
    /// No substitutions at all.
    pub(super) fn none() -> Self {
        Self::default()
    }

    /// Read the vertical features out of a face's `GSUB` table.
    ///
    /// Never fails: a malformed table yields "no vertical substitution" and a
    /// diagnostic, never a panic. The C++ reads attacker-controlled offsets
    /// with unchecked spans; `read-fonts` is bounds-checked throughout, so
    /// this is the same behavior with the crashes removed.
    pub(super) fn parse(glyphs: &GlyphSource, diags: &mut Diagnostics) -> Self {
        let Some(face) = super::face_of(glyphs) else {
            return Self::none();
        };
        let Ok(font) = skrifa::FontRef::from_index(face.bytes(), face.index()) else {
            return Self::none();
        };
        let Ok(gsub) = font.gsub() else {
            // No `GSUB` at all is ordinary, not damage.
            return Self::none();
        };
        if let Some(map) = collect(&gsub) {
            Self { map }
        } else {
            diags.record(Severity::Suspicious, DiagKind::GsubUnreadable, None);
            Self::none()
        }
    }

    /// The vertical form of a glyph, when one is declared.
    pub(super) fn vertical_glyph(&self, gid: u16) -> Option<u16> {
        // A substitution to glyph 0 is "no substitution" in the C++, which
        // tests the result against zero rather than checking presence.
        self.map.get(&gid).copied().filter(|g| *g != 0)
    }

    /// How many substitutions were found.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.map.len()
    }
}

fn collect(gsub: &Gsub<'_>) -> Option<HashMap<u16, u16>> {
    let feature_list = gsub.feature_list().ok()?;
    let lookup_list = gsub.lookup_list().ok()?;

    // **Script-reachable features first.** Only features some script's
    // language system actually references are considered...
    let mut wanted: Vec<u16> = Vec::new();
    if let Ok(scripts) = gsub.script_list() {
        for script in scripts.script_records() {
            let Ok(s) = script.script(scripts.offset_data()) else {
                continue;
            };
            let mut lang_systems: Vec<_> = s
                .lang_sys_records()
                .iter()
                .filter_map(|r| r.lang_sys(s.offset_data()).ok())
                .collect();
            if let Some(Ok(default)) = s.default_lang_sys() {
                lang_systems.push(default);
            }
            for lang in lang_systems {
                for &idx in lang.feature_indices() {
                    let i = idx.get();
                    if is_vertical_feature(&feature_list, i) && !wanted.contains(&i) {
                        wanted.push(i);
                    }
                }
            }
        }
    }

    // ...and **only if that finds nothing** does the whole feature list get
    // scanned, taking every vertical feature regardless of reachability. A
    // font whose `vert` feature no script references still applies it.
    if wanted.is_empty() {
        for (i, rec) in feature_list.feature_records().iter().enumerate() {
            if is_vertical_tag(rec.feature_tag())
                && let Ok(i) = u16::try_from(i)
            {
                wanted.push(i);
            }
        }
    }

    let mut map = HashMap::new();
    for feature_index in wanted {
        let Some(rec) = feature_list
            .feature_records()
            .get(usize::from(feature_index))
        else {
            continue;
        };
        let Ok(feature) = rec.feature(feature_list.offset_data()) else {
            continue;
        };
        for &lookup_index in feature.lookup_list_indices() {
            let Ok(lookup) = lookup_list.lookups().get(usize::from(lookup_index.get())) else {
                continue;
            };
            // **Only single substitutions.** A `vert` feature implemented as
            // an alternate or ligature lookup is ignored entirely.
            let SubstitutionLookup::Single(single) = lookup else {
                continue;
            };
            for subtable in single.subtables().iter().flatten() {
                collect_single(&subtable, &mut map);
            }
        }
    }
    Some(map)
}

/// Flatten one single-substitution subtable into the map.
///
/// Format 1 is a signed delta applied to every covered glyph — and the delta
/// **wraps**, which is how a font expresses a substitution near the end of the
/// glyph range. Format 2 is an explicit array indexed by coverage position.
fn collect_single(subtable: &SingleSubst<'_>, map: &mut HashMap<u16, u16>) {
    match subtable {
        SingleSubst::Format1(t) => {
            let Ok(coverage) = t.coverage() else { return };
            let delta = t.delta_glyph_id();
            for gid in coverage.iter() {
                let from = gid.to_u16();
                map.entry(from)
                    .or_insert_with(|| from.wrapping_add(delta as u16));
            }
        }
        SingleSubst::Format2(t) => {
            let Ok(coverage) = t.coverage() else { return };
            let substitutes = t.substitute_glyph_ids();
            for (i, gid) in coverage.iter().enumerate() {
                let Some(to) = substitutes.get(i) else {
                    // A coverage table longer than the substitute array is
                    // malformed; the C++ reads past the end, we stop.
                    break;
                };
                map.entry(gid.to_u16()).or_insert_with(|| to.get().to_u16());
            }
        }
    }
}

fn is_vertical_feature(list: &read_fonts::tables::layout::FeatureList<'_>, index: u16) -> bool {
    list.feature_records()
        .get(usize::from(index))
        .is_some_and(|r| is_vertical_tag(r.feature_tag()))
}

fn is_vertical_tag(tag: Tag) -> bool {
    tag == VERT || tag == VRT2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_vertical_tags_are_vert_and_vrt2() {
        assert!(is_vertical_tag(VERT));
        assert!(is_vertical_tag(VRT2));
        assert!(!is_vertical_tag(Tag::new(b"liga")));
        assert!(!is_vertical_tag(Tag::new(b"vkrn")));
    }

    #[test]
    fn a_face_with_no_gsub_substitutes_nothing() {
        let mut diags = Diagnostics::default();
        let (glyphs, _) = crate::subst::builtin_generic(false);
        let v = VerticalSubst::parse(&glyphs, &mut diags);
        assert_eq!(v.len(), 0);
        assert_eq!(v.vertical_glyph(1), None);
        // No `GSUB` is ordinary, not damage.
        assert!(!diags.contains(&DiagKind::GsubUnreadable));
    }

    #[test]
    fn the_empty_substitution_answers_nothing() {
        let v = VerticalSubst::none();
        assert_eq!(v.vertical_glyph(0), None);
        assert_eq!(v.vertical_glyph(u16::MAX), None);
    }
}
