//! What the caller decides before the conversion runs, and what the
//! conversion tells them afterwards.
//!
//! # Why these are types and not a bool and a string
//!
//! The failure mode is *silent lossy conversion*. A conversion that quietly
//! drops a font, rasterizes a page or
//! deletes an annotation has produced a file that passes a validator and is
//! not the document the caller handed in. The only defence is that every
//! compromise is (a) authorized in advance and (b) reported afterwards in a
//! form a program can act on.
//!
//! So [`Policy`] is a struct of enums, one per decision the pipeline can face,
//! and [`Conversion`] carries a `Vec<Compromise>` rather than a formatted
//! summary. A caller can count them, filter them, refuse to ship a file that
//! made one of a particular kind, or map them into their own vocabulary — none
//! of which is possible against a string.

use core::fmt;

use pdfrum_object::ObjRef;

use super::Level;

/// What to do when the document cannot be converted faithfully.
///
/// One variant per *kind* of compromise the pipeline knows how to make, plus
/// [`Concession::Refuse`], which is always available and is the default. The
/// enum is deliberately not `bool`: "rasterize it" and "drop it" are different
/// answers with different consequences, and a caller who wants one and not the
/// other must be able to say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Concession {
    /// Do not convert. The conversion stops and reports why.
    ///
    /// The default at every decision point, because a caller who has not
    /// thought about a compromise has not agreed to it.
    #[default]
    Refuse,
    /// Make the compromise, and record it in [`Conversion::compromises`].
    Accept,
}

impl Concession {
    /// Whether this concession permits the compromise.
    #[must_use]
    pub const fn accepts(self) -> bool {
        matches!(self, Concession::Accept)
    }
}

/// What the conversion is permitted to do to a document it cannot convert
/// faithfully.
///
/// Every field defaults to [`Concession::Refuse`], so [`Policy::default()`] is
/// the strict policy: convert what can be converted losslessly and refuse
/// anything else. Widening it is an explicit act per axis.
///
/// The axes are separate because a caller's answers genuinely differ between
/// them — an archive may accept a substituted font (the text stays text, and
/// stays searchable) while refusing a rasterized page (the text stops being
/// text at all).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Policy {
    /// A font that is used for rendering, is not embedded, and whose program
    /// we do not have.
    ///
    /// Refusing stops the conversion and names the font, because an unembedded
    /// font is a hard PDF/A failure that no amount of other repair will fix.
    ///
    /// Accepting is meant to substitute a standard font of the same broad
    /// shape. That repair is not implemented yet, so
    /// today accepting means "convert as far as you can": every other repair
    /// applies, the font stays unembedded, and no [`Compromise`] is reported
    /// because nothing was compromised. It is the honest reading of `Accept`
    /// until the substitution lands.
    pub unembeddable_font: Concession,
    /// Content the level forbids outright and that cannot be rewritten —
    /// transparency under A-1b being the case.
    ///
    /// Refusing stops the conversion and names the page. Only A-1b can reach
    /// this: A-2b permits transparency, so nothing there is unrepresentable.
    ///
    /// Accepting is meant to rasterize the offending page — faithful to the
    /// *appearance*, and destroying the text, the vectors and the
    /// selectability. That repair is not implemented yet, so accepting reads as "convert as far as you can" exactly as
    /// [`Policy::unembeddable_font`] does, and
    /// [`Conversion::rasterized_pages`] is always empty for now.
    pub unrepresentable_content: Concession,
    /// An annotation or action of a kind PDF/A forbids.
    ///
    /// Accepting removes it — the JavaScript, the `/Launch` action, the
    /// `/Movie` annotation. Refusing stops the conversion. This is separated
    /// from the two above because it is the one most callers *do* want: a
    /// launch action in an archived file is a liability, not content, and
    /// stripping it is the point of archiving rather than a loss.
    ///
    /// It also covers one repair that *adds* rather than removes: PDF/A
    /// requires every annotation be visible and printable, so an annotation
    /// hidden by its `/F` flags has them cleared and becomes visible. That
    /// changes what the page draws, which is why it needs permission and
    /// reports [`Compromise::AnnotationFlagsChanged`].
    pub forbidden_feature: Concession,
}

impl Policy {
    /// Refuse every compromise: convert only what converts losslessly.
    ///
    /// The same as [`Policy::default()`], named so a caller can say what they
    /// mean at the call site.
    #[must_use]
    pub const fn strict() -> Self {
        Self {
            unembeddable_font: Concession::Refuse,
            unrepresentable_content: Concession::Refuse,
            forbidden_feature: Concession::Refuse,
        }
    }

    /// Accept every compromise the pipeline knows how to make.
    ///
    /// The archivist's policy: produce a conforming file and tell me what it
    /// cost. Every compromise still lands in [`Conversion::compromises`] — the
    /// difference from [`Policy::strict`] is what stops the conversion, never
    /// what goes unreported.
    #[must_use]
    pub const fn lossy() -> Self {
        Self {
            unembeddable_font: Concession::Accept,
            unrepresentable_content: Concession::Accept,
            forbidden_feature: Concession::Accept,
        }
    }

    /// This policy with [`Policy::unembeddable_font`] set.
    ///
    /// The struct is `#[non_exhaustive]` so that a concession added later is
    /// not a breaking change for callers — which also means a caller cannot
    /// write a struct literal. These three are how a policy is built from one
    /// of the two named starting points:
    ///
    /// ```
    /// use pdfrum_doc::pdfa::{Concession, Policy};
    ///
    /// // Strip what PDF/A forbids, but never substitute a font behind my back.
    /// let policy = Policy::strict().forbidden_feature(Concession::Accept);
    /// assert!(policy.forbidden_feature.accepts());
    /// assert!(!policy.unembeddable_font.accepts());
    /// ```
    #[must_use]
    pub const fn unembeddable_font(mut self, concession: Concession) -> Self {
        self.unembeddable_font = concession;
        self
    }

    /// This policy with [`Policy::unrepresentable_content`] set.
    #[must_use]
    pub const fn unrepresentable_content(mut self, concession: Concession) -> Self {
        self.unrepresentable_content = concession;
        self
    }

    /// This policy with [`Policy::forbidden_feature`] set.
    #[must_use]
    pub const fn forbidden_feature(mut self, concession: Concession) -> Self {
        self.forbidden_feature = concession;
        self
    }
}

/// One thing the conversion did that changed the document.
///
/// Every variant carries the object or page it happened to, for the same
/// reason [`Subject`](crate::pdfa::Subject) does: a caller that wants to show the user
/// what changed needs to reach the thing, and a sentence cannot be turned back
/// into an `ObjRef`.
///
/// This is the *complete* list of ways the conversion can alter meaning. A
/// repair that changes nothing a reader would notice — minting a `/ID`,
/// writing an output intent, rewriting the XMP packet — is not a compromise
/// and does not appear here.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Compromise {
    /// A font's program was not in the file and could not be obtained, so a
    /// standard font was substituted. The glyphs the page draws change.
    ///
    /// **Never produced yet**, like [`Compromise::PageRasterized`]: the
    /// substitution behind [`Policy::unembeddable_font`] is not implemented.
    /// Both are here rather than added with their repairs because each
    /// completes a triple that *is* live — the policy field, the [`Refusal`]
    /// a caller gets today, and the compromise they will get instead. A
    /// caller writes the match arm once.
    FontSubstituted {
        /// The font dictionary that was rewritten.
        font: ObjRef,
        /// The `/BaseFont` name it had.
        base_name: String,
        /// The standard font put in its place.
        substitute: &'static str,
    },
    /// An action was removed: JavaScript, or one of the kinds the level
    /// forbids by name.
    ActionRemoved {
        /// The object the action hung off — a catalog, an annotation, a field.
        holder: ObjRef,
        /// The `/S` subtype removed, as PDF spells it.
        kind: String,
    },
    /// An annotation was removed because its subtype is not permitted.
    AnnotationRemoved {
        /// The annotation dictionary.
        annotation: ObjRef,
        /// The page it was on.
        page: u32,
        /// Its `/Subtype`.
        subtype: String,
    },
    /// An annotation was kept but its flag word was corrected, so it now
    /// prints and is visible where it was not.
    AnnotationFlagsChanged {
        /// The annotation dictionary.
        annotation: ObjRef,
        /// The page it is on.
        page: u32,
    },
    /// A page was rendered to an image and rebuilt around it. Its text is no
    /// longer text and its vectors are no longer vectors.
    ///
    /// **Never produced yet**: the rasterizing repair is not implemented.
    /// The variant is part of the vocabulary because
    /// [`Conversion::rasterized_pages`] reports which pages changed, and a
    /// caller writes that match arm once, not when the repair lands.
    PageRasterized {
        /// The page, zero-based.
        page: u32,
        /// The resolution it was rendered at, in whole dots per inch.
        ///
        /// An integer newtype rather than an `f32`: a fractional render
        /// resolution is not a thing a caller asks for, and `f32` would cost
        /// this whole report its `Eq`.
        dpi: Dpi,
        /// Why the page could not be kept as content.
        cause: RasterCause,
    },
    /// An embedded file was removed. A-1b forbids them outright.
    EmbeddedFileRemoved {
        /// The name tree entry, when the file specification is indirect.
        file: Option<ObjRef>,
        /// The file's name, as the name tree spelled it.
        name: String,
    },
    /// Optional content was removed, so what was conditionally visible is now
    /// unconditionally so. A-1b forbids `/OCProperties`.
    OptionalContentRemoved {
        /// The catalog the `/OCProperties` hung off.
        catalog: ObjRef,
    },
    /// A `/Metadata` packet on an object other than the catalog was removed.
    ///
    /// PDF/A's schema rules apply to every packet in the file, and a packet on
    /// an image describing its camera or its rights holder usually uses
    /// schemas PDF/A does not predefine. Nothing renders differently without
    /// it — it is descriptive metadata, not content — but it is information
    /// the file no longer carries, so the caller is told.
    ObjectMetadataRemoved {
        /// The object it hung off.
        object: ObjRef,
    },
}

/// A render resolution, in whole dots per inch.
///
/// A newtype because asks for one on a unit-bearing scalar, and
/// because it is what lets [`Conversion`] keep `Eq` — a report a caller cannot
/// compare for equality is a report they cannot write a test against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Dpi(pub u16);

impl fmt::Display for Dpi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} dpi", self.0)
    }
}

/// Why a page had to be rasterized rather than repaired.
///
/// A separate enum rather than a string on [`Compromise::PageRasterized`],
/// because the two causes are acted on differently: a caller converting to
/// A-1b who sees [`RasterCause::Transparency`] can reasonably retry at A-2b
/// and keep the vectors, and a program should be able to notice that without
/// matching on prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum RasterCause {
    /// The page uses transparency and the level forbids it. A-1b only: A-2b
    /// permits transparency, so this cause cannot arise there.
    Transparency,
    /// The page draws with a font whose program is not available and which
    /// could not be substituted.
    UnembeddableFont,
}

impl fmt::Display for RasterCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RasterCause::Transparency => "the page uses transparency, which this level forbids",
            RasterCause::UnembeddableFont => "the page draws with a font that cannot be embedded",
        })
    }
}

impl fmt::Display for Compromise {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Compromise::FontSubstituted {
                base_name,
                substitute,
                ..
            } => write!(
                f,
                "substituted {substitute} for the unembedded /{base_name}"
            ),
            Compromise::ActionRemoved { kind, .. } => write!(f, "removed a /{kind} action"),
            Compromise::AnnotationRemoved { page, subtype, .. } => {
                write!(f, "removed a /{subtype} annotation from page {}", page + 1)
            }
            Compromise::AnnotationFlagsChanged { page, .. } => {
                write!(f, "corrected an annotation's flags on page {}", page + 1)
            }
            Compromise::PageRasterized { page, dpi, cause } => {
                write!(f, "rasterized page {} at {dpi}: {cause}", page + 1)
            }
            Compromise::EmbeddedFileRemoved { name, .. } => {
                write!(f, "removed the embedded file {name}")
            }
            Compromise::OptionalContentRemoved { .. } => {
                f.write_str("removed the optional-content configuration")
            }
            Compromise::ObjectMetadataRemoved { object } => write!(
                f,
                "removed the XMP packet on object {} {}",
                object.num, object.generation
            ),
        }
    }
}

/// Why a conversion refused.
///
/// The mirror of [`Compromise`]: each variant is a compromise the pipeline
/// would have had to make and the policy did not authorize. A caller that gets
/// one of these knows exactly which [`Policy`] field to widen, which is why
/// the refusal names the concession rather than describing the problem.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// A font is used for rendering, is not embedded, and its program was not
    /// found. Widen [`Policy::unembeddable_font`].
    UnembeddableFont {
        /// The font dictionary.
        font: ObjRef,
        /// Its `/BaseFont` name.
        base_name: String,
    },
    /// A page carries content the level forbids and that cannot be rewritten.
    /// Widen [`Policy::unrepresentable_content`].
    UnrepresentableContent {
        /// The page, zero-based.
        page: u32,
        /// What would have forced the rasterization.
        cause: RasterCause,
    },
    /// The document carries a feature PDF/A forbids — JavaScript, a `/Launch`
    /// action, a `/Movie` annotation. Widen [`Policy::forbidden_feature`].
    ForbiddenFeature {
        /// The object carrying it.
        holder: ObjRef,
        /// What it is, as PDF spells it.
        kind: String,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::UnembeddableFont { base_name, .. } => write!(
                f,
                "/{base_name} is not embedded and its program was not found; \
                 set Policy::unembeddable_font to accept a substitute"
            ),
            Refusal::UnrepresentableContent { page, cause } => write!(
                f,
                "page {}: {cause}; set Policy::unrepresentable_content to \
                 accept a rasterized page",
                page + 1
            ),
            Refusal::ForbiddenFeature { kind, .. } => write!(
                f,
                "the document carries {kind}, which PDF/A forbids; set \
                 Policy::forbidden_feature to accept its removal"
            ),
        }
    }
}

/// What a conversion produced, and what it cost.
///
/// Returned by the facade's `Document::to_pdfa` whether or not the conversion
/// succeeded: [`Conversion::refusals`] non-empty means no bytes were written
/// and the file is unchanged, and an empty `refusals` with a non-empty
/// [`Conversion::compromises`] means a file was written that is not the
/// document that went in.
///
/// The three states are distinguishable without inspecting the byte count,
/// which is the point — see [`Conversion::converted`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversion {
    /// The level converted to.
    pub level: Level,
    /// Everything the conversion did that changed the document's meaning, in
    /// the order the pipeline did it.
    ///
    /// Empty means the conversion was faithful: the file that came out draws
    /// what the file that went in drew.
    pub compromises: Vec<Compromise>,
    /// Every compromise the policy did not authorize.
    ///
    /// Non-empty means **nothing was written**. The conversion collects all of
    /// them rather than stopping at the first, so a caller can widen the
    /// policy once instead of discovering the obstacles one run at a time.
    pub refusals: Vec<Refusal>,
}

impl Conversion {
    /// Whether a file was written.
    ///
    /// False exactly when [`Conversion::refusals`] is non-empty.
    #[must_use]
    pub fn converted(&self) -> bool {
        self.refusals.is_empty()
    }

    /// Whether a file was written and it draws what the input drew.
    #[must_use]
    pub fn faithful(&self) -> bool {
        self.converted() && self.compromises.is_empty()
    }

    /// The pages that were rendered to images, in page order.
    ///
    /// The one compromise whose *extent* a caller almost always wants
    /// separately from the list, because it is the one that changes what a
    /// page fundamentally is.
    #[must_use]
    pub fn rasterized_pages(&self) -> Vec<u32> {
        let mut pages: Vec<u32> = self
            .compromises
            .iter()
            .filter_map(|c| match c {
                Compromise::PageRasterized { page, .. } => Some(*page),
                _ => None,
            })
            .collect();
        pages.sort_unstable();
        pages.dedup();
        pages
    }
}

impl fmt::Display for Conversion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.converted() {
            write!(f, "{} refused: ", self.level)?;
            for (i, refusal) in self.refusals.iter().enumerate() {
                if i > 0 {
                    f.write_str("; ")?;
                }
                write!(f, "{refusal}")?;
            }
            return Ok(());
        }
        if self.compromises.is_empty() {
            return write!(f, "{} conversion, faithful", self.level);
        }
        write!(
            f,
            "{} conversion with {} compromise(s)",
            self.level,
            self.compromises.len()
        )
    }
}
