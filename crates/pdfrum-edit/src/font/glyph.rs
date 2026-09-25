//! Faces for glyph runs: fonts a layout engine has already shaped with, drawn
//! by glyph ID rather than by character.
//!
//! [`EditDoc::embed_font`] turns *text* into codes through the program's own
//! cmap, which cannot reach a glyph the cmap does not name — a ligature, a
//! contextual form, a glyph two code points share. A shaper hands over glyph
//! IDs, so a glyph font takes those directly. Each glyph is given a CID the
//! first time it is drawn (CID 0 is `.notdef`), and the file's font objects
//! are written from what was drawn once a drawing call returns:
//!
//! - the program, **subset to the drawn glyphs in CID order**, so the
//!   subsetter's new glyph IDs *are* the CIDs and no `/CIDToGIDMap` is needed
//!   (a CFF result is a `CIDFontType0`, where CID is GID by definition);
//! - at the face's **variable instance**, which the subsetter bakes into the
//!   program (feature `variable-fonts`);
//! - `/W` from the advances the runs were positioned with, so a viewer's pen
//!   and the layout's agree to the unit;
//! - `/ToUnicode` from the **text each glyph was drawn for**, as the caller
//!   gave it — a ligature maps to all of its letters; a glyph drawn for two
//!   different texts keeps its first, and the other run carries
//!   `/ActualText` instead (see [`Canvas::glyphs`](crate::Canvas::glyphs)).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use pdfrum_font::GlyphSource;
use pdfrum_object::{ByteSpan, Dict, Name, ObjRef, Object, Stream};
use sha2::{Digest as _, Sha256};
use skrifa::instance::{LocationRef, Size};
use skrifa::raw::TableProvider as _;
use skrifa::{FontRef, GlyphId, MetadataProvider as _, Tag};

use crate::doc::EditDoc;
use crate::error::Error;
use crate::font::embed::{
    MAX_BF_ENTRIES, ProgramKind, TO_UNICODE_END, TO_UNICODE_START, add_charcode, cid_font_dict,
    create_widths_array, load_font_desc, type0_font_dict,
};
use crate::font::instance::{self, FontInstance};
use crate::font::{is_opentype_cff, subset_name, subset_tag};
use crate::names;
use crate::write::id::IdSource;

/// A face embedded for glyph runs, ready to draw with
/// [`Canvas::glyphs`](crate::Canvas::glyphs).
///
/// A handle into the session that embedded it: the font objects are written
/// from what that session drew, each time a drawing call returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphFont {
    slot: usize,
    font: ObjRef,
}

impl GlyphFont {
    /// The `/Type0` font dictionary a page's resources name.
    #[must_use]
    pub fn object(&self) -> ObjRef {
        self.font
    }
}

/// How a glyph's text stands in the font's `/ToUnicode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Claim {
    /// The map says this text for the glyph (or the text is empty and says
    /// nothing).
    Mapped,
    /// The map already says something else: the run must carry the text as
    /// `/ActualText`.
    Taken,
}

/// One glyph font's state in a session: the face, and what has been drawn.
#[derive(Debug, Clone)]
pub(crate) struct GlyphFace {
    program: ByteSpan,
    index: u32,
    instance: FontInstance,
    /// The `/Type0` dictionary, reserved at embedding and written by
    /// [`finish`].
    font: ObjRef,
    /// The face's PostScript name, untagged.
    name: Vec<u8>,
    /// The glyph each CID draws; CID 0 is `.notdef`.
    gids: Vec<u16>,
    cids: HashMap<u16, u16>,
    /// Each CID's advance in thousandths of an em, at the instance.
    widths: Vec<u32>,
    /// Each CID's text, once a run has claimed it.
    texts: Vec<Option<String>>,
    /// Whether anything changed since the objects were last written.
    stale: bool,
}

impl GlyphFace {
    /// The CIDs of `gids` and their advances, giving a CID to each glyph not
    /// drawn before.
    ///
    /// # Errors
    ///
    /// [`Error::TooManyGlyphs`] past 65 535 distinct glyphs, which is all an
    /// Identity-H code can name.
    pub(crate) fn cids(&mut self, gids: &[u16]) -> Result<Vec<(u16, u32)>, Error> {
        let fresh: Vec<u16> = gids
            .iter()
            .copied()
            .filter(|gid| !self.cids.contains_key(gid))
            .collect();
        if !fresh.is_empty() {
            let advances = self.advances(&fresh);
            for (gid, width) in fresh.into_iter().zip(advances) {
                if self.cids.contains_key(&gid) {
                    continue;
                }
                let cid = u16::try_from(self.gids.len()).map_err(|_| Error::TooManyGlyphs)?;
                self.gids.push(gid);
                self.cids.insert(gid, cid);
                self.widths.push(width);
                self.texts.push(None);
                self.stale = true;
            }
        }
        Ok(gids
            .iter()
            .map(|gid| {
                let cid = self.cids.get(gid).copied().unwrap_or(0);
                let width = self.widths.get(usize::from(cid)).copied().unwrap_or(0);
                (cid, width)
            })
            .collect())
    }

    /// Record that `cid` was drawn for `text`.
    pub(crate) fn claim(&mut self, cid: u16, text: &str) -> Claim {
        let Some(slot) = self.texts.get_mut(usize::from(cid)) else {
            return Claim::Taken;
        };
        match slot {
            _ if text.is_empty() => Claim::Mapped,
            Some(held) if held == text => Claim::Mapped,
            Some(_) => Claim::Taken,
            None => {
                *slot = Some(text.to_owned());
                self.stale = true;
                Claim::Mapped
            }
        }
    }

    /// Whether `font` names this face.
    pub(crate) fn is(&self, font: &GlyphFont) -> bool {
        self.font == font.font
    }

    /// Advances of `gids` at the instance, in thousandths of an em, rounded:
    /// the numbers `/W` will carry, so a run positioned with them lands where
    /// a viewer's pen does.
    fn advances(&self, gids: &[u16]) -> Vec<u32> {
        let Ok(font) = FontRef::from_index(&self.program, self.index) else {
            return vec![500; gids.len()];
        };
        let per_em = f32::from(font.head().map_or(1000, |head| head.units_per_em()).max(1));
        let location = instance::location(&font, &self.instance);
        let metrics = font.glyph_metrics(Size::unscaled(), LocationRef::from(&location));
        gids.iter()
            .map(|&gid| {
                let advance = metrics
                    .advance_width(GlyphId::new(u32::from(gid)))
                    .unwrap_or(0.0);
                thousandths(advance * 1000.0 / per_em)
            })
            .collect()
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "an advance in thousandths of an em, clamped to what /W holds"
)]
fn thousandths(value: f32) -> u32 {
    value.round().clamp(0.0, 65_535.0) as u32
}

impl EditDoc<'_> {
    /// Embed face `face` of `program` at `instance`, to draw shaped glyph
    /// runs with.
    ///
    /// `program` is a TrueType or OpenType font or a collection (`.ttc`), of
    /// which `face` picks one (0 for a single font). It is shared, not copied:
    /// a 30 MB collection costs nothing until a save writes the subset of it
    /// the pages drew. Nothing of the font reaches the file until a drawing
    /// call that used it returns.
    ///
    /// # Errors
    ///
    /// [`Error::UnrecognisedFontProgram`] when `face` of `program` cannot be
    /// read, [`Error::EmptyFontProgram`] when it declares no glyphs, and
    /// [`Error::VariableFontsDisabled`] for a CFF2 face or a non-default
    /// instance without the `variable-fonts` feature.
    ///
    /// ```
    /// use pdfrum_edit::{EditDoc, FontInstance, Size, blank_document};
    /// use pdfrum_object::ByteSpan;
    ///
    /// let base = blank_document(&[Size::new(200.0, 100.0)])?;
    /// let mut edit = EditDoc::new(&base);
    /// let program = ByteSpan::from(include_bytes!("../../tests/files/tiny.ttf").to_vec());
    /// let font = edit.embed_glyph_font(program, 0, FontInstance::Default)?;
    /// assert_ne!(font.object().num, 0);
    /// # Ok::<(), pdfrum_edit::Error>(())
    /// ```
    pub fn embed_glyph_font(
        &mut self,
        program: ByteSpan,
        face: u32,
        instance: FontInstance,
    ) -> Result<GlyphFont, Error> {
        let parsed =
            FontRef::from_index(&program, face).map_err(|_| Error::UnrecognisedFontProgram)?;
        if parsed.maxp().map_or(0, |maxp| maxp.num_glyphs()) == 0 {
            return Err(Error::EmptyFontProgram);
        }
        let needs_instancing = parsed.table_data(Tag::new(b"CFF2")).is_some()
            || !instance::user_values(&parsed, &instance).is_empty();
        if needs_instancing && !cfg!(feature = "variable-fonts") {
            return Err(Error::VariableFontsDisabled);
        }
        let name = postscript_name(&parsed);
        let font = self.add(Object::Null);
        let slot = self.glyph_faces.len();
        self.glyph_faces.push(GlyphFace {
            program,
            index: face,
            instance,
            font,
            name,
            gids: vec![0],
            cids: HashMap::from([(0, 0)]),
            widths: vec![0],
            texts: vec![None],
            stale: false,
        });
        Ok(GlyphFont { slot, font })
    }

    /// The state of the glyph font `font` names, if this session embedded it.
    pub(crate) fn glyph_face(&mut self, font: &GlyphFont) -> Option<&mut GlyphFace> {
        self.glyph_faces
            .get_mut(font.slot)
            .filter(|face| face.is(font))
    }
}

/// The face's PostScript name (`name` ID 6), or `Untitled`.
fn postscript_name(font: &FontRef<'_>) -> Vec<u8> {
    font.localized_strings(skrifa::string::StringId::POSTSCRIPT_NAME)
        .english_or_first()
        .map(|name| {
            name.chars()
                .filter(|c| c.is_ascii_graphic() && !"[](){}<>/%#".contains(*c))
                .collect::<String>()
        })
        .filter(|name| !name.is_empty())
        .map_or_else(|| b"Untitled".to_vec(), String::into_bytes)
}

/// Write every glyph font that changed: subset program, descriptor, `/W`,
/// `/ToUnicode` and the descendant, under the `/Type0` reserved for it.
///
/// # Errors
///
/// [`Error::Subset`] when the subsetter refuses the face.
pub(crate) fn finish(doc: &mut EditDoc<'_>) -> Result<(), Error> {
    let stale: Vec<GlyphFace> = doc
        .glyph_faces
        .iter()
        .filter(|face| face.stale && face.gids.len() > 1)
        .cloned()
        .collect();
    for face in stale {
        write(doc, &face)?;
        if let Some(held) = doc
            .glyph_faces
            .iter_mut()
            .find(|held| held.font == face.font)
        {
            held.stale = false;
        }
    }
    Ok(())
}

fn write(doc: &mut EditDoc<'_>, face: &GlyphFace) -> Result<(), Error> {
    let program = subset_program(face)?;
    let kind = if is_opentype_cff(&program) {
        ProgramKind::OpenTypeCff
    } else {
        ProgramKind::TrueType
    };
    let glyphs =
        GlyphSource::from_bytes(program.as_slice()).ok_or(Error::UnrecognisedFontProgram)?;
    let base = instance_base_name(face);
    let name = subset_name(&base, subset_tag(IdSource::Fixed(fingerprint(face))));
    let descriptor = load_font_desc(doc, &name, &program, kind, &glyphs);
    let widths: BTreeMap<u32, u32> = face
        .widths
        .iter()
        .enumerate()
        .filter_map(|(cid, width)| Some((u32::try_from(cid).ok()?, *width)))
        .collect();
    let w = doc.add(Object::Array(create_widths_array(&widths)));
    let to_unicode = doc.add(Object::Stream(Box::new(Stream::new(
        Dict::new(),
        ByteSpan::from(to_unicode_cmap(&face.texts)),
    ))));
    let subtype = match kind {
        ProgramKind::OpenTypeCff | ProgramKind::Type1 => names::CID_FONT_TYPE0.clone(),
        ProgramKind::TrueType => names::CID_FONT_TYPE2.clone(),
    };
    let mut descendant = cid_font_dict(doc, &name, subtype, descriptor, w);
    if kind == ProgramKind::TrueType {
        descendant.push(
            names::CID_TO_GID_MAP.clone(),
            Object::Name(Name::from("Identity")),
        );
    }
    let descendant = doc.add(Object::Dict(descendant));
    doc.replace(
        face.font,
        Object::Dict(type0_font_dict(&name, descendant, to_unicode)),
    );
    Ok(())
}

/// The face's own name, or the instance's when the subset is not the
/// default one: `/BaseFont` and `/FontName` then say the weight the glyphs
/// were cut at, not the default instance's.
fn instance_base_name(face: &GlyphFace) -> Vec<u8> {
    FontRef::from_index(&face.program, face.index)
        .ok()
        .and_then(|font| {
            let values = instance::user_values(&font, &face.instance);
            crate::font::instance_name::instance_name(&font, &values)
        })
        .unwrap_or_else(|| face.name.clone())
}

/// The program subset to the drawn glyphs in CID order, at the instance.
fn subset_program(face: &GlyphFace) -> Result<Vec<u8>, Error> {
    let mut remapper = subsetter::GlyphRemapper::new();
    for gid in &face.gids {
        remapper.remap(*gid);
    }
    subset_at_instance(face, &remapper).map_err(|error| Error::Subset(error.to_string()))
}

#[cfg(feature = "variable-fonts")]
fn subset_at_instance(
    face: &GlyphFace,
    remapper: &subsetter::GlyphRemapper,
) -> Result<Vec<u8>, subsetter::Error> {
    let values = FontRef::from_index(&face.program, face.index)
        .map(|font| instance::user_values(&font, &face.instance))
        .unwrap_or_default();
    let coordinates: Vec<(subsetter::Tag, f32)> = values
        .iter()
        .map(|axis| (subsetter::Tag::new(&axis.tag), axis.value))
        .collect();
    subsetter::subset_with_variations(&face.program, face.index, &coordinates, remapper)
}

#[cfg(not(feature = "variable-fonts"))]
fn subset_at_instance(
    face: &GlyphFace,
    remapper: &subsetter::GlyphRemapper,
) -> Result<Vec<u8>, subsetter::Error> {
    subsetter::subset(&face.program, face.index, remapper)
}

/// Sixteen bytes that change whenever the subset does: the name and the
/// drawn glyphs, so a fixed input gives a fixed subset tag.
fn fingerprint(face: &GlyphFace) -> [u8; 16] {
    let mut hash = Sha256::new();
    hash.update(&face.name);
    for gid in &face.gids {
        hash.update(gid.to_be_bytes());
    }
    let digest = hash.finalize();
    let mut out = [0u8; 16];
    out.iter_mut()
        .zip(digest.iter())
        .for_each(|(slot, byte)| *slot = *byte);
    out
}

/// The `/ToUnicode` CMap: one `bfchar` per CID with text, the text as
/// UTF-16BE of any length (a ligature's letters, a grapheme's marks).
fn to_unicode_cmap(texts: &[Option<String>]) -> Vec<u8> {
    let entries: Vec<(u32, &str)> = texts
        .iter()
        .enumerate()
        .filter_map(|(cid, text)| {
            let text = text.as_deref().filter(|text| !text.is_empty())?;
            Some((u32::try_from(cid).ok()?, text))
        })
        .collect();
    let mut buf = String::from(TO_UNICODE_START);
    for chunk in entries.chunks(MAX_BF_ENTRIES) {
        let _ = writeln!(buf, "{} beginbfchar", chunk.len());
        for (cid, text) in chunk {
            add_charcode(&mut buf, *cid);
            buf.push_str(" <");
            for unit in text.encode_utf16() {
                let _ = write!(buf, "{unit:04X}");
            }
            buf.push_str(">\n");
        }
        buf.push_str("endbfchar\n");
    }
    buf.push_str(TO_UNICODE_END);
    buf.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::to_unicode_cmap;

    #[test]
    fn a_ligature_maps_to_all_its_letters() {
        let cmap = String::from_utf8(to_unicode_cmap(&[
            None,
            Some("fi".into()),
            Some("一".into()),
        ]))
        .expect("ASCII");
        assert!(cmap.contains("<0001> <00660069>"), "{cmap}");
        assert!(cmap.contains("<0002> <4E00>"), "{cmap}");
        assert!(cmap.contains("2 beginbfchar"), "notdef has no text: {cmap}");
    }

    #[test]
    fn a_cid_without_text_is_left_out() {
        let cmap =
            String::from_utf8(to_unicode_cmap(&[None, None, Some(String::new())])).expect("ASCII");
        assert!(!cmap.contains("beginbfchar"), "{cmap}");
    }
}
