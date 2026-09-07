//! The faces an ingested SVG's `<text>` is set in.
//!
//! roadmap item 3, and the reason it is a feature of its own. An SVG
//! names fonts by *family* — `font-family: Inter, sans-serif` — and a PDF
//! carries font *programs*. Nothing bridges those two without a font
//! database, so the caller supplies one: [`SvgFonts`] is a set of face
//! programs, each registered under the family its own name table declares.
//!
//! # Why the caller supplies them
//!
//! `usvg`'s own defaults would scan the host — `system-fonts` and
//! `memmap-fonts` — and pdfrum does not take a dependency's defaults.
//! More to the point, a document whose appearance depends on which
//! fonts a build machine happens to have installed is not reproducible, and
//! reproducibility is the whole reason `SaveOptions::id_source` exists. A
//! caller who *wants* the host's fonts reads them and registers them, and
//! that is then a decision in their code rather than an accident in ours.
//!
//! # What text becomes
//!
//! **Outlines.** `usvg` lays the text out and flattens each span to filled
//! paths, and the ingestion walk draws those paths like any others — so text
//! goes into the page as vectors with no font embedded and no encoding to get
//! wrong, and it renders identically everywhere. The roadmap's "embedded
//! fonts on request" half is not this pass's; §6
//! records why outlines are the honest default and what embedding would need.
//!
//! An SVG whose `<text>` names a family this set does not carry draws
//! nothing, and that is reported as [`Unsupported::Text`](crate::Unsupported)
//! exactly as it was before the feature existed — a missing face is a
//! reported gap, never a silent one.

use std::sync::Arc;

/// The font faces an ingested SVG's `<text>` may be set in.
///
/// Each face is registered under the family names its own `name` table
/// declares, which is how an SVG's `font-family` finds it. Empty by default:
/// a session that registers nothing renders no text, and says so in the
/// report.
///
/// Cheap to clone — the registered faces are shared, not copied — so one set
/// built at start-up serves every document.
///
/// ```no_run
/// use pdfrum::{Document, SvgFonts};
///
/// let mut fonts = SvgFonts::new();
/// fonts.register(std::fs::read("Inter.ttf")?);
/// assert_eq!(fonts.families(), ["Inter"]);
///
/// let doc = Document::open("in.pdf")?;
/// let mut edit = doc.edit();
/// edit.set_svg_fonts(fonts);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Default)]
pub struct SvgFonts {
    /// `usvg`'s database, behind an `Arc` because that is the shape
    /// [`usvg::Options`] wants and it makes a clone free.
    db: Arc<usvg::fontdb::Database>,
    /// The family every `<text>` with no `font-family` of its own is set in,
    /// and the one `usvg` falls back to for a family it does not know. The
    /// first registered face's, because that is the answer in every case a
    /// second knob would have been set to.
    default_family: String,
}

impl SvgFonts {
    /// An empty set: no faces, so no text draws.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one font program — a TTF, OTF or TTC — under the families its
    /// own name table declares.
    ///
    /// Returns whether the face was usable. A `false` is a font this build
    /// cannot parse, not an error: registering a directory of faces should
    /// not fail on the one that is a README, and a family that never arrives
    /// shows up as a reported [`Unsupported::Text`](crate::Unsupported) when
    /// a document asks for it.
    ///
    /// The **first** face registered also becomes the default family, so the
    /// common case — one face, text with no `font-family` — needs no second
    /// call.
    pub fn register(&mut self, program: impl Into<Vec<u8>>) -> bool {
        let db = Arc::make_mut(&mut self.db);
        let before = db.len();
        db.load_font_data(program.into());
        if db.len() == before {
            return false;
        }
        if self.default_family.is_empty() {
            // The last face is the one just loaded: `load_font_data` appends,
            // and this branch only runs when the set was empty before.
            self.default_family = db
                .faces()
                .last()
                .and_then(|face| face.families.first().map(|(name, _)| name.clone()))
                .unwrap_or_default();
        }
        true
    }

    /// Every family this set can serve, sorted and without duplicates.
    ///
    /// What an SVG's `font-family` is matched against, so it is also the
    /// answer to "why did that text not draw".
    #[must_use]
    pub fn families(&self) -> Vec<String> {
        let mut families: Vec<String> = self
            .db
            .faces()
            .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
            .collect();
        families.sort_unstable();
        families.dedup();
        families
    }

    /// Whether any face is registered.
    ///
    /// `true` means every `<text>` in every document is reported rather than
    /// drawn, which is what this crate did before the feature existed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.db.is_empty()
    }

    /// The database and default family, for the parse options.
    pub(crate) fn parts(&self) -> (Arc<usvg::fontdb::Database>, &str) {
        (Arc::clone(&self.db), &self.default_family)
    }
}
