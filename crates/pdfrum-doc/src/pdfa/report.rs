//! The vocabulary a check speaks: the level asked for, the clause broken,
//! the object that broke it, and the report holding the lot.

use core::fmt;

use pdfrum_object::ObjRef;

/// Which PDF/A conformance level to check against.
///
/// An enum rather than a string or a pair of numbers, because the two levels
/// differ in what they *forbid* and a caller who passes the wrong spelling of
/// a string gets no answer at all. Only the two `b` (basic) levels are here;
/// the module docs say why the `a` levels are absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// PDF/A-1b, ISO 19005-1:2005. Built on PDF 1.4: no transparency at all,
    /// no optional content, no embedded files, no JPEG 2000.
    A1b,
    /// PDF/A-2b, ISO 19005-2:2011. Built on PDF 1.7: transparency is allowed
    /// when a blending colour space is defined, JPEG 2000 is allowed, and
    /// embedded files are allowed if they are themselves PDF/A.
    A2b,
}

impl Level {
    /// The level's name as ISO 19005 spells it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Level::A1b => "PDF/A-1b",
            Level::A2b => "PDF/A-2b",
        }
    }

    /// The `pdfaid:part` an XMP identification schema must carry for this
    /// level: 1 for A-1, 2 for A-2.
    #[must_use]
    pub const fn part(self) -> u8 {
        match self {
            Level::A1b => 1,
            Level::A2b => 2,
        }
    }

    /// Whether transparency is forbidden outright.
    ///
    /// True for A-1 only. A-2 permits transparency, so the check that walks
    /// for it is skipped rather than run-and-ignored — a check that produces
    /// findings nobody reads is how a report grows noise.
    #[must_use]
    pub const fn forbids_transparency(self) -> bool {
        matches!(self, Level::A1b)
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The requirement a [`Violation`] breaks.
///
/// One variant per rule this checker enforces, named for what the rule *is*
/// rather than for the section number, because the section numbers differ
/// between ISO 19005-1 and -2 for the same requirement and a caller matching
/// on `Clause::FontNotEmbedded` should not have to care which part it is
/// reading. [`Clause::iso`] gives the citation for a report that wants to
/// print one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Clause {
    /// A font used for rendering has no embedded font program.
    FontNotEmbedded,
    /// An embedded font program is a subset (its name carries the six-letter
    /// tag) but does not declare the character set it covers.
    FontSubsetIncomplete,
    /// A symbolic TrueType font has no usable `cmap`, or a non-symbolic one
    /// lacks the encoding PDF/A requires.
    FontEncodingInvalid,
    /// The file is encrypted. PDF/A forbids it outright: an archived file
    /// must be readable without a key that may be lost.
    Encrypted,
    /// The document carries JavaScript, in the name tree or in an action.
    JavaScript,
    /// A forbidden action type: `/Launch`, `/Sound`, `/Movie`, `/ResetForm`,
    /// `/ImportData` or `/JavaScript`.
    ForbiddenAction,
    /// An embedded sound or movie annotation, or a `/Sound` or `/Movie` entry
    /// referencing multimedia content.
    EmbeddedMultimedia,
    /// The catalog carries no `/Metadata` XMP packet.
    XmpMissing,
    /// The XMP packet is present but is not well-formed enough to read the
    /// identification schema from.
    XmpMalformed,
    /// The XMP packet carries no PDF/A identification schema (`pdfaid:part`).
    XmpIdentificationMissing,
    /// The XMP identification schema names a different part or conformance
    /// letter than the level being checked.
    XmpIdentificationMismatch,
    /// A document information dictionary entry disagrees with the XMP
    /// property that mirrors it.
    XmpInfoMismatch,
    /// The catalog has no `/OutputIntents`, or none with a
    /// `GTS_PDFA1` subtype.
    OutputIntentMissing,
    /// An output intent is present but its `/DestOutputProfile` is missing or
    /// is not an embeddable ICC stream.
    OutputIntentProfileMissing,
    /// An annotation's flag word is illegal: `/Hidden`, `/NoView` or
    /// `/Invisible` set, or `/Print` clear.
    AnnotationFlagsIllegal,
    /// An annotation of a subtype PDF/A does not allow.
    AnnotationSubtypeForbidden,
    /// A non-widget annotation has no normal appearance stream.
    AnnotationAppearanceMissing,
    /// A stream, file specification or reference `XObject` points at content
    /// outside the file.
    ExternalContentReference,
    /// Transparency: a soft mask, a non-`Normal` blend mode, a non-opaque
    /// constant alpha, or a transparency group. A-1 only.
    Transparency,
    /// A device colour space is used with no output intent to define what it
    /// means.
    DeviceColorWithoutOutputIntent,
    /// Optional content (`/OCProperties`, `/OCG`, `/OCMD`). A-1 only; A-2
    /// permits it.
    OptionalContent,
    /// An embedded file. A-1 forbids embedded files outright.
    EmbeddedFile,
    /// A `/LZWDecode` filter, which both parts forbid for patent reasons.
    ///
    /// Separate from [`Clause::JpxFilter`] because they are separate
    /// requirements with separate citations — folding them into one clause
    /// made a JPEG 2000 stream cite the LZW rule, which the veraPDF oracle
    /// caught.
    LzwFilter,
    /// A `/JPXDecode` filter under A-1, whose PDF 1.4 base does not define
    /// JPEG 2000. A-2 permits it, so this never fires there.
    JpxFilter,
}

impl Clause {
    /// The ISO 19005 citation for this requirement, at the given level.
    ///
    /// The two parts number the same requirement differently, so the level is
    /// a parameter rather than the clause carrying one number.
    #[must_use]
    // Several distinct clauses share a citation because ISO puts several
    // requirements in one numbered paragraph: 6.6.1 covers JavaScript, the
    // forbidden action list and multimedia together. Merging the arms would
    // hide which requirement each clause *is*, which is the whole point of
    // the enum, so the repetition is the readable form here.
    #[allow(clippy::match_same_arms)]
    pub const fn iso(self, level: Level) -> &'static str {
        match (self, level) {
            (Clause::FontNotEmbedded, Level::A1b) => "ISO 19005-1:2005, 6.3.4",
            (Clause::FontNotEmbedded, Level::A2b) => "ISO 19005-2:2011, 6.2.11.4.1",
            (Clause::FontSubsetIncomplete, Level::A1b) => "ISO 19005-1:2005, 6.3.5",
            (Clause::FontSubsetIncomplete, Level::A2b) => "ISO 19005-2:2011, 6.2.11.4.2",
            (Clause::FontEncodingInvalid, Level::A1b) => "ISO 19005-1:2005, 6.3.6",
            (Clause::FontEncodingInvalid, Level::A2b) => "ISO 19005-2:2011, 6.2.11.5",
            (Clause::Encrypted, Level::A1b) => "ISO 19005-1:2005, 6.1.3",
            (Clause::Encrypted, Level::A2b) => "ISO 19005-2:2011, 6.1.3",
            (Clause::JavaScript, Level::A1b) => "ISO 19005-1:2005, 6.6.1",
            (Clause::JavaScript, Level::A2b) => "ISO 19005-2:2011, 6.5.1",
            (Clause::ForbiddenAction, Level::A1b) => "ISO 19005-1:2005, 6.6.1",
            (Clause::ForbiddenAction, Level::A2b) => "ISO 19005-2:2011, 6.5.1",
            (Clause::EmbeddedMultimedia, Level::A1b) => "ISO 19005-1:2005, 6.6.1",
            (Clause::EmbeddedMultimedia, Level::A2b) => "ISO 19005-2:2011, 6.5.1",
            (Clause::XmpMissing, Level::A1b) => "ISO 19005-1:2005, 6.7.2",
            (Clause::XmpMissing, Level::A2b) => "ISO 19005-2:2011, 6.6.2.1",
            (Clause::XmpMalformed, Level::A1b) => "ISO 19005-1:2005, 6.7.2",
            (Clause::XmpMalformed, Level::A2b) => "ISO 19005-2:2011, 6.6.2.1",
            (Clause::XmpIdentificationMissing, Level::A1b) => "ISO 19005-1:2005, 6.7.11",
            (Clause::XmpIdentificationMissing, Level::A2b) => "ISO 19005-2:2011, 6.6.4",
            (Clause::XmpIdentificationMismatch, Level::A1b) => "ISO 19005-1:2005, 6.7.11",
            (Clause::XmpIdentificationMismatch, Level::A2b) => "ISO 19005-2:2011, 6.6.4",
            (Clause::XmpInfoMismatch, Level::A1b) => "ISO 19005-1:2005, 6.7.3",
            (Clause::XmpInfoMismatch, Level::A2b) => "ISO 19005-2:2011, 6.6.2.3.1",
            // A-1 folds the output intent into the uncalibrated-colour rule:
            // both "no intent" and "device colour without one" are 6.2.3.3.
            // A-2 separates them — 6.2.10 is the intent, 6.2.4.3 the colour.
            (Clause::OutputIntentMissing, Level::A1b) => "ISO 19005-1:2005, 6.2.3.3",
            (Clause::OutputIntentMissing, Level::A2b) => "ISO 19005-2:2011, 6.2.10",
            (Clause::OutputIntentProfileMissing, Level::A1b) => "ISO 19005-1:2005, 6.2.3.3",
            (Clause::OutputIntentProfileMissing, Level::A2b) => "ISO 19005-2:2011, 6.2.10",
            (Clause::AnnotationFlagsIllegal, Level::A1b) => "ISO 19005-1:2005, 6.5.3",
            (Clause::AnnotationFlagsIllegal, Level::A2b) => "ISO 19005-2:2011, 6.3.2",
            (Clause::AnnotationSubtypeForbidden, Level::A1b) => "ISO 19005-1:2005, 6.5.2",
            (Clause::AnnotationSubtypeForbidden, Level::A2b) => "ISO 19005-2:2011, 6.3.1",
            (Clause::AnnotationAppearanceMissing, Level::A1b) => "ISO 19005-1:2005, 6.5.3",
            (Clause::AnnotationAppearanceMissing, Level::A2b) => "ISO 19005-2:2011, 6.3.3",
            (Clause::ExternalContentReference, Level::A1b) => "ISO 19005-1:2005, 6.1.6",
            (Clause::ExternalContentReference, Level::A2b) => "ISO 19005-2:2011, 6.2.2",
            (Clause::Transparency, Level::A1b) => "ISO 19005-1:2005, 6.4",
            // A-2 permits transparency; the check never runs, so this arm is
            // unreachable in practice and cites the clause that governs it.
            (Clause::Transparency, Level::A2b) => "ISO 19005-2:2011, 6.2.4.3",
            (Clause::DeviceColorWithoutOutputIntent, Level::A1b) => "ISO 19005-1:2005, 6.2.3.3",
            (Clause::DeviceColorWithoutOutputIntent, Level::A2b) => "ISO 19005-2:2011, 6.2.4.3",
            (Clause::OptionalContent, Level::A1b) => "ISO 19005-1:2005, 6.1.13",
            (Clause::OptionalContent, Level::A2b) => "ISO 19005-2:2011, 6.1.13",
            (Clause::EmbeddedFile, Level::A1b) => "ISO 19005-1:2005, 6.1.11",
            (Clause::EmbeddedFile, Level::A2b) => "ISO 19005-2:2011, 6.8",
            (Clause::LzwFilter, Level::A1b) => "ISO 19005-1:2005, 6.1.10",
            (Clause::LzwFilter, Level::A2b) => "ISO 19005-2:2011, 6.1.7.2",
            // A-1's JPEG 2000 prohibition is a consequence of its PDF 1.4
            // base, which 6.1.3 fixes; A-2 permits JPX, so the check never
            // runs there and this arm cites the same base clause.
            (Clause::JpxFilter, Level::A1b) => "ISO 19005-1:2005, 6.1.3",
            (Clause::JpxFilter, Level::A2b) => "ISO 19005-2:2011, 6.1.3",
        }
    }
}

/// What in the file broke a requirement.
///
/// Deliberately not a string. A converter that wants to repair a violation
/// needs the `ObjRef` to reach the object; a caller that wants to tell a user
/// which page to look at needs the index. Formatting either into a sentence
/// throws that away and cannot be recovered.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Subject {
    /// The document as a whole — the trailer, the header, or a property with
    /// no single object behind it.
    Document,
    /// The document catalog.
    Catalog,
    /// A page, by zero-based index.
    Page(u32),
    /// A specific indirect object.
    Object(ObjRef),
    /// A named resource on a page: the page index and the resource's name.
    Resource {
        /// The page the resource is reached from.
        page: u32,
        /// The name it has in that page's resource dictionary.
        name: String,
    },
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Subject::Document => f.write_str("document"),
            Subject::Catalog => f.write_str("catalog"),
            Subject::Page(index) => write!(f, "page {}", index + 1),
            Subject::Object(reference) => {
                write!(f, "object {} {}", reference.num, reference.generation)
            }
            Subject::Resource { page, name } => write!(f, "page {} resource /{name}", page + 1),
        }
    }
}

/// One requirement the document fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The requirement broken.
    pub clause: Clause,
    /// What broke it.
    pub subject: Subject,
    /// A sentence naming the specific thing found, for a report a person
    /// reads. The machine-readable answer is `clause` and `subject`; this is
    /// the detail neither of them can carry, such as the font's base name.
    pub detail: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.subject, self.detail)
    }
}

/// What [`check`](super::check) found.
///
/// [`Report::conforms`] is the one-bit answer; the violations are the reason.
/// A report is always complete for the checks this engine implements, which
/// is not the same as complete for ISO 19005 — see the design note. A caller
/// that needs the distinction should treat a passing report as "we found
/// nothing", not as a certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The level checked against.
    pub level: Level,
    /// Every requirement failed, in the order the checks ran: document-wide
    /// properties first, then per-page walks in page order.
    pub violations: Vec<Violation>,
}

impl Report {
    /// Whether the document passed every check this engine runs.
    #[must_use]
    pub fn conforms(&self) -> bool {
        self.violations.is_empty()
    }

    /// Every violation of one clause.
    pub fn by_clause(&self, clause: Clause) -> impl Iterator<Item = &Violation> {
        self.violations.iter().filter(move |v| v.clause == clause)
    }

    /// The distinct clauses failed, in first-seen order.
    ///
    /// The summary a caller usually wants: a file with two hundred
    /// non-embedded fonts fails one requirement two hundred times, and the
    /// interesting number is the one.
    #[must_use]
    pub fn clauses(&self) -> Vec<Clause> {
        let mut out: Vec<Clause> = Vec::new();
        for violation in &self.violations {
            if !out.contains(&violation.clause) {
                out.push(violation.clause);
            }
        }
        out
    }
}
