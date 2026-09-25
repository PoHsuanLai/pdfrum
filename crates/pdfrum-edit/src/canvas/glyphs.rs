//! Shaped glyph runs on a canvas: glyph IDs at the positions a layout engine
//! chose, with the text each was shaped from.
//!
//! [`Canvas::text`] shapes nothing and places one string; a run from a real
//! layout engine arrives already shaped, as glyph IDs with origins, and only
//! the engine knows which characters each glyph stands for. A run is written
//! as `TJ` arrays in a [`GlyphFont`], its positions exact: each glyph's
//! advance is the one `/W` will carry, and the difference to where the layout
//! put the next glyph goes into the array. The text goes into the font's
//! `/ToUnicode` when the glyph can carry it there, and into `/ActualText`
//! around the glyphs when it cannot — a cluster of several glyphs, or a glyph
//! already mapped to other text.

use std::fmt::Write as _;
use std::ops::Range;

use kurbo::Affine;
use pdfrum_object::{Object, names as pdf_names};

use super::{Canvas, Paint, write_f64};
use crate::Error;
use crate::font::glyph::{Claim, GlyphFont};
use crate::write_matrix;

/// One glyph of a run: which glyph, where its origin sits, and which bytes
/// of the run's text it was shaped from.
#[derive(Debug, Clone, PartialEq)]
pub struct RunGlyph {
    /// The glyph ID in the face.
    pub id: u16,
    /// The origin across, in run space: points at the run's size.
    pub x: f64,
    /// The origin up, in run space (y up, as text space is).
    pub y: f64,
    /// The cluster this glyph belongs to, as a byte range of
    /// [`GlyphRun::text`]. Glyphs of one cluster share the range; a ligature
    /// is one glyph whose range covers every letter it joins.
    pub text: Range<usize>,
}

/// A shaped run, ready to draw with [`Canvas::glyphs`].
#[derive(Debug, Clone)]
pub struct GlyphRun<'a> {
    /// The face, embedded by [`EditDoc::embed_glyph_font`](crate::EditDoc::embed_glyph_font)
    /// in this session.
    pub font: &'a GlyphFont,
    /// The font size, in run-space units.
    pub size: f64,
    /// Run space to canvas space. Run space is text space: y up, glyphs
    /// upright; a caller in a y-down space flips here.
    pub transform: Affine,
    /// The glyphs, in drawing order.
    pub glyphs: &'a [RunGlyph],
    /// The text the run was shaped from, which the glyphs' ranges index.
    pub text: &'a str,
    /// How the glyphs are painted: filled, stroked (text render mode 1), or
    /// both (mode 2, a synthetic bold).
    pub paint: Paint,
}

/// How one stretch of a run carries its text.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Voice {
    /// Through the font's `/ToUnicode`.
    Mapped,
    /// As `/ActualText` around the stretch.
    Actual(String),
}

/// Consecutive glyphs written with one `Tm` and one `TJ`.
#[derive(Debug)]
struct Stretch {
    glyphs: Range<usize>,
    voice: Voice,
}

impl Canvas<'_, '_> {
    /// Draw a shaped glyph run.
    ///
    /// Each glyph is drawn exactly at its origin; the text a reader copies
    /// is the run's own, taken from each glyph's cluster range. See the
    /// module documentation for how that text is written.
    ///
    /// # Errors
    ///
    /// [`Error::ForeignGlyphFont`] for a font another session embedded, and
    /// [`Error::TooManyGlyphs`] past 65 535 distinct glyphs in one font, both
    /// recorded and returned by the drawing call as for [`Canvas::text`].
    ///
    /// ```
    /// use pdfrum_edit::{EditDoc, FontInstance, GlyphRun, Paint, RunGlyph, Size, blank_document};
    /// use pdfrum_common::Limits;
    /// use pdfrum_object::ByteSpan;
    /// use kurbo::Affine;
    /// use peniko::Color;
    ///
    /// let base = blank_document(&[Size::new(200.0, 100.0)])?;
    /// let mut edit = EditDoc::new(&base);
    /// let program = ByteSpan::from(include_bytes!("../../tests/files/tiny.ttf").to_vec());
    /// let font = edit.embed_glyph_font(program, 0, FontInstance::Default)?;
    /// let glyphs = [
    ///     RunGlyph { id: 1, x: 0.0, y: 0.0, text: 0..1 },
    ///     RunGlyph { id: 2, x: 12.0, y: 0.0, text: 1..2 },
    /// ];
    /// edit.draw_page(0, &Limits::default(), |c| {
    ///     c.glyphs(&GlyphRun {
    ///         font: &font,
    ///         size: 20.0,
    ///         transform: Affine::translate((20.0, 40.0)),
    ///         glyphs: &glyphs,
    ///         text: "ab",
    ///         paint: Paint::Fill(Color::BLACK),
    ///     });
    /// })?;
    /// # Ok::<(), pdfrum_edit::Error>(())
    /// ```
    pub fn glyphs(&mut self, run: &GlyphRun<'_>) {
        if run.glyphs.is_empty() || !(run.size.is_finite() && run.size > 0.0) {
            return;
        }
        let Some(face) = self.edit.glyph_face(run.font) else {
            return self.fail(Error::ForeignGlyphFont);
        };
        let ids: Vec<u16> = run.glyphs.iter().map(|glyph| glyph.id).collect();
        let cids = match face.cids(&ids) {
            Ok(cids) => cids,
            Err(error) => return self.fail(error),
        };
        let stretches = stretches(run, |cid, text| face.claim(cid, text), &cids);
        let name = self.realize(pdf_names::FONT, Object::Ref(run.font.object()));
        let mode = match run.paint {
            Paint::Fill(_) => None,
            Paint::Stroke(_) => Some(1),
            Paint::FillStroke(..) => Some(2),
        };
        self.out.push_str("q\n");
        self.set_paint(run.paint.clone());
        self.out.push_str("BT\n/");
        self.push_name(&name);
        self.out.push(' ');
        write_f64(&mut self.out, run.size);
        self.out.push_str(" Tf\n");
        if let Some(mode) = mode {
            let _ = writeln!(self.out, "{mode} Tr");
        }
        for stretch in &stretches {
            self.stretch(run, &cids, stretch);
        }
        self.out.push_str("ET\nQ\n");
    }

    /// Write one stretch: its `/ActualText` if any, its `Tm`, its `TJ`.
    fn stretch(&mut self, run: &GlyphRun<'_>, cids: &[(u16, u32)], stretch: &Stretch) {
        let (Some(first), Some(codes)) = (
            run.glyphs.get(stretch.glyphs.start),
            cids.get(stretch.glyphs.clone()),
        ) else {
            return;
        };
        if let Voice::Actual(text) = &stretch.voice {
            self.out.push_str("/Span <</ActualText <FEFF");
            for unit in text.encode_utf16() {
                let _ = write!(self.out, "{unit:04X}");
            }
            self.out.push_str(">>> BDC\n");
        }
        write_matrix(
            &mut self.out,
            run.transform * Affine::translate((first.x, first.y)),
        );
        self.out.push_str(" Tm\n[");
        let glyphs = run.glyphs.get(stretch.glyphs.clone()).unwrap_or_default();
        for (at, ((cid, width), glyph)) in codes.iter().zip(glyphs).enumerate() {
            let _ = write!(self.out, "<{cid:04X}>");
            if let Some(next) = glyphs.get(at + 1) {
                // TJ moves the pen back by the number, in thousandths of the
                // size: the advance `/W` gives, less where the next glyph is.
                let adjustment = f64::from(*width) - (next.x - glyph.x) * 1000.0 / run.size;
                if adjustment.abs() > 1e-3 {
                    write_f64(&mut self.out, adjustment);
                }
            }
        }
        self.out.push_str("] TJ\n");
        if matches!(stretch.voice, Voice::Actual(_)) {
            self.out.push_str("EMC\n");
        }
    }
}

/// The run cut into stretches: one per change of baseline, and one per
/// cluster that must carry its text as `/ActualText`. `claim` records a
/// single glyph's text in the font and answers whether the font can say it.
fn stretches(
    run: &GlyphRun<'_>,
    mut claim: impl FnMut(u16, &str) -> Claim,
    cids: &[(u16, u32)],
) -> Vec<Stretch> {
    let mut out: Vec<Stretch> = Vec::new();
    let mut start = 0;
    while start < run.glyphs.len() {
        let Some(range) = run.glyphs.get(start).map(|glyph| glyph.text.clone()) else {
            break;
        };
        let end = run
            .glyphs
            .iter()
            .skip(start)
            .position(|glyph| glyph.text != range)
            .map_or(run.glyphs.len(), |offset| start + offset);
        let text = run.text.get(range).unwrap_or_default();
        let voice = match (end - start, cids.get(start)) {
            (1, Some((cid, _))) => match claim(*cid, text) {
                Claim::Mapped => Voice::Mapped,
                Claim::Taken => Voice::Actual(text.to_owned()),
            },
            _ if text.is_empty() => Voice::Mapped,
            _ => Voice::Actual(text.to_owned()),
        };
        let same_line = |a: usize, b: usize| match (run.glyphs.get(a), run.glyphs.get(b)) {
            (Some(a), Some(b)) => (a.y - b.y).abs() < 1e-9,
            _ => false,
        };
        match out.last_mut() {
            Some(last)
                if voice == Voice::Mapped
                    && last.voice == Voice::Mapped
                    && same_line(last.glyphs.start, start)
                    && (start..end).all(|at| same_line(start, at)) =>
            {
                last.glyphs.end = end;
            }
            _ => {
                // A stretch never spans a change of baseline: split this
                // cluster's glyphs wherever `y` moves.
                let mut from = start;
                for at in start + 1..=end {
                    if at == end || !same_line(from, at) {
                        out.push(Stretch {
                            glyphs: from..at,
                            voice: voice.clone(),
                        });
                        from = at;
                    }
                }
            }
        }
        start = end;
    }
    out
}
