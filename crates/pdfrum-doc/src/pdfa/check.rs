//! The checks themselves.
//!
//! One function per requirement group, each pushing into the same
//! [`Report`]. They run in a fixed order — document-wide properties, then the
//! page tree — so a report's violation order is stable across runs and a test
//! can pin it.
//!
//! # The walk, and its one deliberate limit
//!
//! Every check reads the object graph rather than the content streams. A
//! font's embedding, an annotation's flags, a blend mode in an `/ExtGState`,
//! a colour space in `/Resources` — all of it is in dictionaries reachable
//! from the catalog, which is why this module needs no interpreter and no
//! dependency `pdfrum-doc` did not already have.
//!
//! What that misses is the operand form: a colour set by `1 0 0 rg` rather
//! than through a named `/ColorSpace`, and a `gs`-less inline transparency.
//! §4 records this as the checker's largest known gap,
//! and it is the source of most of the clauses veraPDF reports that we do not.
//!
//! It cuts the other way in exactly one place, and that place is worth
//! knowing about: both ISO parts scope font embedding to fonts *used within*
//! the file, and telling a font a stream shows from one merely listed in
//! `/Resources` also needs the interpreter. So [`check_font`] over-reports an
//! unembedded but unused font. Everywhere else the absence of an interpreter
//! makes this checker report *fewer* violations than veraPDF, never more.

use std::collections::HashSet;

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Array, Dict, Name, ObjRef, Object, Resolve, names};

use super::report::{Clause, Level, Report, Subject, Violation};
use super::xmp;
use crate::annot::AnnotFlags;

/// Check a document against a PDF/A conformance level.
///
/// `catalog` is the document catalog (`/Root`), `trailer` the file trailer —
/// the encryption check needs `/Encrypt`, which lives there and nowhere else.
/// The report lists every requirement the document fails. An empty report
/// means the checks this engine runs all passed, which is a weaker claim than
/// ISO 19005 conformance; the module docs and say how
/// much weaker.
#[must_use]
pub fn check<R: Resolve>(
    level: Level,
    catalog: &Dict,
    trailer: &Dict,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Report {
    let mut report = Report {
        level,
        violations: Vec::new(),
    };
    let mut ctx = Ctx {
        level,
        report: &mut report,
    };

    check_encryption(&mut ctx, trailer);
    let has_output_intent = check_output_intent(&mut ctx, catalog, r);
    check_metadata(&mut ctx, catalog, trailer, r, limits, diags);
    check_catalog_features(&mut ctx, catalog, r);
    check_pages(&mut ctx, catalog, has_output_intent, r);

    report
}

/// The state every check shares: which level is being checked, and where the
/// findings go. A struct rather than two parameters threaded everywhere,
/// because every check needs both and neither is ever passed separately.
struct Ctx<'a> {
    level: Level,
    report: &'a mut Report,
}

impl Ctx<'_> {
    fn fail(&mut self, clause: Clause, subject: Subject, detail: impl Into<String>) {
        self.report.violations.push(Violation {
            clause,
            subject,
            detail: detail.into(),
        });
    }
}

/// PDF/A forbids encryption outright: an archived file must open without a
/// key, because the key is the thing most likely to be lost.
fn check_encryption(ctx: &mut Ctx<'_>, trailer: &Dict) {
    if trailer.contains_key(names::ENCRYPT) {
        ctx.fail(
            Clause::Encrypted,
            Subject::Document,
            "the trailer carries /Encrypt; PDF/A permits no encryption",
        );
    }
}

/// The output intent, and whether one with an embedded profile was found.
///
/// The return value feeds the colour check: a device colour space is legal
/// exactly when an output intent defines what its numbers mean, so the two
/// checks cannot be independent.
///
/// # A missing intent is not itself a violation, at either level
///
/// Both parts phrase the requirement as a *condition on uncalibrated colour*
/// — ISO 19005-1 6.2.3.3, ISO 19005-2 6.2.4.3 — rather than as a standalone
/// "every file shall have an output intent". A file that uses only calibrated
/// or ICC-based colour conforms with no intent at all.
///
/// This mattered: reporting the absence on its own was an over-report at both
/// levels, and the veraPDF oracle caught it — it exempts exactly the corpus
/// files whose colour is not device colour. So the
/// absence is returned rather than reported, and only [`check_color_space`]
/// turns it into a violation, when it finds device colour that needed it.
///
/// [`Clause::OutputIntentMissing`] survives for the case that *is*
/// unconditional: an `/OutputIntents` array that exists but holds no PDF/A
/// intent, which is a malformed intent rather than an absent one.
fn check_output_intent<R: Resolve>(ctx: &mut Ctx<'_>, catalog: &Dict, r: &R) -> bool {
    let intents = catalog.array(&Name::from("OutputIntents"), r);
    let Some(intents) = intents.filter(|a| !a.is_empty()) else {
        return false;
    };

    let mut found = false;
    for index in 0..intents.len() {
        let Some(intent) = array_dict(&intents, index, r) else {
            continue;
        };
        // `GTS_PDFA1` is the subtype for every part of ISO 19005, including
        // part 2 — the `1` is in the name for historical reasons and is not a
        // version. An intent with any other subtype (`GTS_PDFX`) is a legal
        // *additional* intent but does not satisfy the requirement.
        let is_pdfa = intent
            .name(&Name::from("S"))
            .is_some_and(|s| s.as_bytes() == b"GTS_PDFA1");
        if !is_pdfa {
            continue;
        }
        found = true;
        let profile = intent.stream(&Name::from("DestOutputProfile"), r);
        if profile.is_none() {
            ctx.fail(
                Clause::OutputIntentProfileMissing,
                subject_for(&intents, index),
                "the PDF/A output intent has no /DestOutputProfile stream",
            );
        }
    }

    if !found {
        ctx.fail(
            Clause::OutputIntentMissing,
            Subject::Catalog,
            "/OutputIntents is present but has no entry with subtype /GTS_PDFA1",
        );
    }
    found
}

/// XMP: present, well-formed enough to read, carrying the identification
/// schema for this level, and agreeing with the information dictionary.
fn check_metadata<R: Resolve>(
    ctx: &mut Ctx<'_>,
    catalog: &Dict,
    trailer: &Dict,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    let Some(packet) = crate::metadata::xmp(catalog, r, limits, diags) else {
        ctx.fail(
            Clause::XmpMissing,
            Subject::Catalog,
            "the catalog has no /Metadata XMP packet",
        );
        return;
    };

    if !xmp::is_xmp(&packet) {
        ctx.fail(
            Clause::XmpMalformed,
            Subject::Catalog,
            "the /Metadata stream is not an RDF document",
        );
        return;
    }

    match xmp::identification(&packet) {
        None => ctx.fail(
            Clause::XmpIdentificationMissing,
            Subject::Catalog,
            "the XMP packet carries no pdfaid:part, so the file does not \
             identify itself as PDF/A",
        ),
        Some(id) => {
            let want = ctx.level.part();
            if id.part != want {
                let part = id.part;
                ctx.fail(
                    Clause::XmpIdentificationMismatch,
                    Subject::Catalog,
                    format!("pdfaid:part is {part}, not the {want} this level requires"),
                );
            }
            // Absent conformance is its own failure, not a pass: `part`
            // without `conformance` names no level.
            match id.conformance {
                Some('B') => {}
                Some(letter) => ctx.fail(
                    Clause::XmpIdentificationMismatch,
                    Subject::Catalog,
                    format!("pdfaid:conformance is {letter}, not the B this level requires"),
                ),
                None => ctx.fail(
                    Clause::XmpIdentificationMismatch,
                    Subject::Catalog,
                    "pdfaid:part is present but pdfaid:conformance is not",
                ),
            }
        }
    }

    check_info_agreement(ctx, trailer, &packet, r);
}

/// Each information-dictionary entry must equal the XMP property that mirrors
/// it. Only entries the document actually carries are compared: a `/Title`
/// that is absent from both is not a disagreement.
fn check_info_agreement<R: Resolve>(ctx: &mut Ctx<'_>, trailer: &Dict, packet: &[u8], r: &R) {
    let Some(info) = trailer.dict(&Name::from("Info"), r) else {
        return;
    };
    let props = xmp::info_properties(packet);

    for (key, xmp_name, value) in [
        ("Title", "dc:title", props.title.as_deref()),
        ("Author", "dc:creator", props.author.as_deref()),
        ("Creator", "xmp:CreatorTool", props.creator_tool.as_deref()),
        ("Producer", "pdf:Producer", props.producer.as_deref()),
        ("Keywords", "pdf:Keywords", props.keywords.as_deref()),
    ] {
        let Some(in_info) = info.text(&Name::from(key), r) else {
            continue;
        };
        // An empty `/Title` is what a producer writes for "no title"; it is
        // not a value to disagree about.
        if in_info.trim().is_empty() {
            continue;
        }
        if value.map(str::trim) != Some(in_info.trim()) {
            let found = value.unwrap_or("absent");
            ctx.fail(
                Clause::XmpInfoMismatch,
                Subject::Document,
                format!("/{key} is {in_info:?} but {xmp_name} is {found:?}"),
            );
        }
    }
}

/// The catalog-level features PDF/A restricts: JavaScript and embedded files
/// in the name tree, optional content, the open action, and the additional
/// actions dictionary.
fn check_catalog_features<R: Resolve>(ctx: &mut Ctx<'_>, catalog: &Dict, r: &R) {
    if let Some(tree) = catalog.dict(names::NAMES, r) {
        if tree.contains_key(&Name::from("JavaScript")) {
            ctx.fail(
                Clause::JavaScript,
                Subject::Catalog,
                "/Names has a /JavaScript name tree",
            );
        }
        // A-2 permits embedded files if they are themselves PDF/A, which this
        // checker cannot verify without recursing into them; A-1 forbids them
        // outright, which it can. Reporting only the case we can decide is
        // the honest choice — see.
        if ctx.level == Level::A1b && tree.contains_key(names::EMBEDDED_FILES) {
            ctx.fail(
                Clause::EmbeddedFile,
                Subject::Catalog,
                "/Names has an /EmbeddedFiles tree; PDF/A-1 forbids embedded files",
            );
        }
    }

    // Optional content is PDF 1.5, above A-1's PDF 1.4 base. A-2 permits it.
    if ctx.level == Level::A1b && catalog.contains_key(&Name::from("OCProperties")) {
        ctx.fail(
            Clause::OptionalContent,
            Subject::Catalog,
            "/OCProperties; PDF/A-1 forbids optional content",
        );
    }

    if let Some(action) = catalog.dict(&Name::from("OpenAction"), r) {
        check_action(ctx, &action, Subject::Catalog, r, &mut HashSet::new());
    }
    if let Some(aa) = catalog.dict(names::AA, r) {
        check_action_dict(ctx, &aa, &Subject::Catalog, r);
    }
    // The AcroForm's own additional actions and its field tree's, which is
    // where a form's JavaScript usually lives.
    if let Some(fields) = catalog
        .dict(&Name::from("AcroForm"), r)
        .and_then(|form| form.array(&Name::from("Fields"), r))
    {
        for index in 0..fields.len() {
            if let Some(aa) = array_dict(&fields, index, r).and_then(|f| f.dict(names::AA, r)) {
                check_action_dict(ctx, &aa, &subject_for(&fields, index), r);
            }
        }
    }
}

/// Every value of an additional-actions dictionary is an action.
fn check_action_dict<R: Resolve>(ctx: &mut Ctx<'_>, aa: &Dict, subject: &Subject, r: &R) {
    let mut seen = HashSet::new();
    for (key, _) in aa.iter() {
        if let Some(action) = aa.dict(key, r) {
            check_action(ctx, &action, subject.clone(), r, &mut seen);
        }
    }
}

/// One action and everything in its `/Next` chain.
///
/// `seen` guards the chain: `/Next` may point back at an earlier action, and
/// a malformed file is exactly where that happens.
fn check_action<R: Resolve>(
    ctx: &mut Ctx<'_>,
    action: &Dict,
    subject: Subject,
    r: &R,
    seen: &mut HashSet<ObjRef>,
) {
    if let Some(kind) = action.name(&Name::from("S")) {
        let kind = kind.as_bytes();
        if kind == b"JavaScript" {
            ctx.fail(
                Clause::JavaScript,
                subject.clone(),
                "a /JavaScript action; PDF/A permits no script",
            );
        } else if matches!(
            kind,
            b"Launch" | b"Sound" | b"Movie" | b"ResetForm" | b"ImportData"
        ) {
            let name = String::from_utf8_lossy(kind).into_owned();
            ctx.fail(
                Clause::ForbiddenAction,
                subject.clone(),
                format!("a /{name} action; PDF/A permits no action of this type"),
            );
        }
    }

    // `/Next` is one action or an array of them.
    match action.raw(&Name::from("Next")) {
        Some(Object::Array(next)) => {
            for index in 0..next.len() {
                if let Some(Object::Ref(reference)) = next.raw_at(index)
                    && !seen.insert(*reference)
                {
                    continue;
                }
                if let Some(dict) = array_dict(next, index, r) {
                    check_action(ctx, &dict, subject.clone(), r, seen);
                }
            }
        }
        Some(_) => {
            if let Some(reference) = action.reference(&Name::from("Next"))
                && !seen.insert(reference)
            {
                return;
            }
            if let Some(next) = action.dict(&Name::from("Next"), r) {
                check_action(ctx, &next, subject, r, seen);
            }
        }
        None => {}
    }
}

/// Walk the page tree, checking each page's annotations and resources.
fn check_pages<R: Resolve>(ctx: &mut Ctx<'_>, catalog: &Dict, has_output_intent: bool, r: &R) {
    let Some(root) = catalog.dict(names::PAGES, r) else {
        return;
    };
    let mut index = 0u32;
    let mut visited = HashSet::new();
    walk(ctx, &root, &mut index, &mut visited, has_output_intent, r);
}

/// One node of the page tree.
///
/// `visited` guards against a `/Kids` cycle, which a malformed file can carry
/// and which would otherwise be an infinite walk in a library that must never
/// hang on untrusted input.
fn walk<R: Resolve>(
    ctx: &mut Ctx<'_>,
    node: &Dict,
    index: &mut u32,
    visited: &mut HashSet<ObjRef>,
    has_output_intent: bool,
    r: &R,
) {
    let Some(kids) = node.array(names::KIDS, r) else {
        check_page(ctx, node, *index, has_output_intent, r);
        *index += 1;
        return;
    };
    for i in 0..kids.len() {
        if let Some(Object::Ref(reference)) = kids.raw_at(i)
            && !visited.insert(*reference)
        {
            continue;
        }
        if let Some(kid) = array_dict(&kids, i, r) {
            walk(ctx, &kid, index, visited, has_output_intent, r);
        }
    }
}

/// One page: its annotations, and the resources it names.
fn check_page<R: Resolve>(
    ctx: &mut Ctx<'_>,
    page: &Dict,
    index: u32,
    has_output_intent: bool,
    r: &R,
) {
    if let Some(annots) = page.array(names::ANNOTS, r) {
        for i in 0..annots.len() {
            if let Some(annot) = array_dict(&annots, i, r) {
                check_annotation(ctx, &annot, &subject_for(&annots, i), r);
            }
        }
    }
    if let Some(resources) = page.dict(names::RESOURCES, r) {
        check_resources(ctx, &resources, index, has_output_intent, r);
    }
}

/// One annotation: its subtype, its flags, and its appearance.
fn check_annotation<R: Resolve>(ctx: &mut Ctx<'_>, annot: &Dict, subject: &Subject, r: &R) {
    let subtype = annot.name(names::SUBTYPE).map_or(&b""[..], Name::as_bytes);

    // `/Movie` and `/Sound` carry the multimedia PDF/A forbids; the rest are
    // simply not in either part's permitted table.
    if matches!(subtype, b"Movie" | b"Sound" | b"Screen") {
        let name = String::from_utf8_lossy(subtype).into_owned();
        ctx.fail(
            Clause::EmbeddedMultimedia,
            subject.clone(),
            format!("a /{name} annotation embeds audio or video"),
        );
    } else if matches!(subtype, b"FileAttachment" | b"3D" | b"RichMedia") {
        let name = String::from_utf8_lossy(subtype).into_owned();
        ctx.fail(
            Clause::AnnotationSubtypeForbidden,
            subject.clone(),
            format!("a /{name} annotation; PDF/A does not permit this subtype"),
        );
    }

    let flags = AnnotFlags::from_bits(annot.int(names::F, r).unwrap_or(0));
    // A `/Popup` is drawn only when its parent is open, so the flag rules
    // that govern printable annotations do not apply to it.
    if subtype != b"Popup" {
        let mut illegal: Vec<&str> = Vec::new();
        if flags.contains(AnnotFlags::HIDDEN) {
            illegal.push("/Hidden set");
        }
        if flags.contains(AnnotFlags::INVISIBLE) {
            illegal.push("/Invisible set");
        }
        if flags.contains(AnnotFlags::NO_VIEW) {
            illegal.push("/NoView set");
        }
        if !flags.contains(AnnotFlags::PRINT) {
            illegal.push("/Print clear");
        }
        if !illegal.is_empty() {
            let list = illegal.join(", ");
            ctx.fail(
                Clause::AnnotationFlagsIllegal,
                subject.clone(),
                format!("annotation flags: {list}"),
            );
        }

        // A `/Link` has no appearance of its own and is not required to; a
        // `/Popup` is excluded above. Everything else must carry `/AP /N`.
        if subtype != b"Link"
            && annot
                .dict(&Name::from("AP"), r)
                .is_none_or(|ap| !ap.contains_key(&Name::from("N")))
        {
            let name = String::from_utf8_lossy(subtype).into_owned();
            ctx.fail(
                Clause::AnnotationAppearanceMissing,
                subject.clone(),
                format!("a /{name} annotation has no /AP /N appearance stream"),
            );
        }
    }

    if let Some(action) = annot.dict(&Name::from("A"), r) {
        check_action(ctx, &action, subject.clone(), r, &mut HashSet::new());
    }
    if let Some(aa) = annot.dict(names::AA, r) {
        check_action_dict(ctx, &aa, subject, r);
    }
}

/// A page's resource dictionary: the fonts, the graphics states, the colour
/// spaces and the `XObject`s it names.
fn check_resources<R: Resolve>(
    ctx: &mut Ctx<'_>,
    resources: &Dict,
    page: u32,
    has_output_intent: bool,
    r: &R,
) {
    if let Some(fonts) = resources.dict(names::FONT, r) {
        for (name, _) in fonts.iter() {
            if let Some(font) = fonts.dict(name, r) {
                check_font(ctx, &font, resource_subject(page, name), r);
            }
        }
    }

    if ctx.level.forbids_transparency()
        && let Some(states) = resources.dict(names::EXT_G_STATE, r)
    {
        for (name, _) in states.iter() {
            if let Some(state) = states.dict(name, r) {
                check_graphics_state(ctx, &state, &resource_subject(page, name), r);
            }
        }
    }

    if !has_output_intent && let Some(spaces) = resources.dict(names::COLOR_SPACE, r) {
        for (name, _) in spaces.iter() {
            check_color_space(
                ctx,
                spaces.get(name, r).as_deref(),
                resource_subject(page, name),
            );
        }
    }

    if let Some(xobjects) = resources.dict(names::XOBJECT, r) {
        for (name, _) in xobjects.iter() {
            let Some(xobject) = xobjects.stream(name, r) else {
                continue;
            };
            let dict = &xobject.dict;
            let subject = resource_subject(page, name);

            // A reference XObject names content in another file, which is the
            // external-reference case the specification forbids by name.
            if dict.contains_key(&Name::from("Ref")) {
                ctx.fail(
                    Clause::ExternalContentReference,
                    subject.clone(),
                    "a reference XObject points at content in another file",
                );
            }
            // `/F` on a *stream* is an external file holding its data, which
            // is the same prohibition seen from the other side.
            if dict.contains_key(names::F) {
                ctx.fail(
                    Clause::ExternalContentReference,
                    subject.clone(),
                    "the stream's data lives in an external file (/F)",
                );
            }
            if ctx.level.forbids_transparency() {
                if dict.contains_key(&Name::from("Group")) {
                    ctx.fail(
                        Clause::Transparency,
                        subject.clone(),
                        "a transparency group; PDF/A-1 permits no transparency",
                    );
                }
                if dict.raw(names::SMASK).is_some_and(|s| !is_none_name(s)) {
                    ctx.fail(
                        Clause::Transparency,
                        subject.clone(),
                        "an image soft mask; PDF/A-1 permits no transparency",
                    );
                }
            }
            check_filters(ctx, dict, &subject);

            // A form XObject carries its own resources, and a page's fonts
            // are routinely reached only through one.
            if let Some(nested) = dict.dict(names::RESOURCES, r) {
                check_resources(ctx, &nested, page, has_output_intent, r);
            }
        }
    }
}

/// A font is conforming when its program is embedded.
///
/// A Type 0 font's program hangs off its descendant, so the descriptor is
/// looked for one level down as well as on the font itself. A Type 3 font has
/// no program at all — its glyphs are content streams — and is conforming
/// without one.
///
/// # "Used for rendering"
///
/// Both parts scope the rule to fonts *used within* the file. This check has
/// no interpreter, so it cannot tell a font a stream actually shows from one
/// merely listed in `/Resources`, and it reports every unembedded font in the
/// resource dictionary. veraPDF makes the distinction, which is why a corpus
/// file with a listed-but-unused unembedded font is a **known over-report**
/// on our side rather than a disagreement about the font — recorded in
/// §6 as the one place the checker is not strictly
/// under-reporting.
fn check_font<R: Resolve>(ctx: &mut Ctx<'_>, font: &Dict, subject: Subject, r: &R) {
    let subtype = font.name(names::SUBTYPE).map_or(&b""[..], Name::as_bytes);
    if subtype == b"Type3" {
        return;
    }

    let descriptor = font.dict(&Name::from("FontDescriptor"), r).or_else(|| {
        font.array(&Name::from("DescendantFonts"), r)
            .and_then(|kids| array_dict(&kids, 0, r))
            .and_then(|kid| kid.dict(&Name::from("FontDescriptor"), r))
    });

    let base = font.name(&Name::from("BaseFont")).map_or_else(
        || "(unnamed)".to_owned(),
        |n| String::from_utf8_lossy(n.as_bytes()).into_owned(),
    );

    let Some(descriptor) = descriptor else {
        ctx.fail(
            Clause::FontNotEmbedded,
            subject,
            format!("font {base} has no /FontDescriptor, so no embedded program"),
        );
        return;
    };

    let embedded = ["FontFile", "FontFile2", "FontFile3"]
        .iter()
        .any(|key| descriptor.contains_key(&Name::from(*key)));
    if !embedded {
        ctx.fail(
            Clause::FontNotEmbedded,
            subject.clone(),
            format!("font {base} has no /FontFile, /FontFile2 or /FontFile3"),
        );
        return;
    }

    // A subset font's base name carries a six-uppercase-letter tag and a `+`.
    //
    // **A-1 only.** ISO 19005-1 6.3.5 requires a CIDFont subset to carry a
    // `/CIDSet` declaring its coverage. ISO 19005-2 dropped that: its 6.2.11.4.2
    // is *conditional* — "if the FontDescriptor contains a CIDSet, then it
    // shall identify all CIDs present" — so an A-2 file with no `/CIDSet` at
    // all conforms. Reporting one at A-2 was an over-report the veraPDF oracle
    // caught.
    //
    // We check for the key's presence, never its completeness: verifying that
    // a `/CIDSet` lists exactly the CIDs in the program means parsing the
    // embedded font, which this checker does not parse. So A-2's conditional
    // rule has no check here at all rather than a check that would guess.
    if ctx.level != Level::A1b {
        return;
    }
    let is_subset = base.len() > 7
        && base.get(6..7) == Some("+")
        && base
            .get(..6)
            .is_some_and(|tag| tag.bytes().all(|b| b.is_ascii_uppercase()));
    // Only a CIDFont subset is covered: 6.3.5 names CIDFonts, and a simple
    // Type 1 subset's `/CharSet` is a PDF recommendation rather than a PDF/A
    // requirement.
    if is_subset
        && font.contains_key(&Name::from("DescendantFonts"))
        && !descriptor.contains_key(&Name::from("CIDSet"))
    {
        ctx.fail(
            Clause::FontSubsetIncomplete,
            subject,
            format!("CIDFont subset {base} declares no /CIDSet"),
        );
    }
}

/// A graphics state that turns transparency on. A-1 only — the caller does
/// not run this for A-2, which permits every one of these.
fn check_graphics_state<R: Resolve>(ctx: &mut Ctx<'_>, state: &Dict, subject: &Subject, r: &R) {
    if state.raw(names::SMASK).is_some_and(|s| !is_none_name(s)) {
        ctx.fail(
            Clause::Transparency,
            subject.clone(),
            "/SMask in an /ExtGState; PDF/A-1 permits no transparency",
        );
    }
    // A blend mode is a name or a one-element array of names. Anything but
    // `Normal` and its `Compatible` alias composites.
    let blend = state
        .name(&Name::from("BM"))
        .map(|n| n.as_bytes().to_vec())
        .or_else(|| {
            state
                .array(&Name::from("BM"), r)
                .and_then(|a| a.name_at(0).map(|n| n.as_bytes().to_vec()))
        });
    if let Some(blend) = blend
        && !matches!(blend.as_slice(), b"Normal" | b"Compatible")
    {
        let name = String::from_utf8_lossy(&blend).into_owned();
        ctx.fail(
            Clause::Transparency,
            subject.clone(),
            format!("blend mode /{name}; PDF/A-1 permits only /Normal"),
        );
    }
    // `/CA` and `/ca` are deliberately **not** checked.
    //
    // "PDF/A-1 forbids transparency" is the popular summary, but the clause
    // that carries it (6.4) is narrower than the summary: it bans a `/SMask`
    // other than `/None` and the blend modes above, and says nothing about
    // constant alpha. veraPDF agrees — it passes a corpus file carrying
    // `/CA 0.498` — and treating the summary as the rule made us report a
    // violation that is not one.
}

/// A device colour space with no output intent to define it.
///
/// `/DeviceGray`, `/DeviceRGB` and `/DeviceCMYK` say "these numbers, in
/// whatever the consumer's device does" — which is the opposite of what an
/// archive needs. They are legal only when an output intent says what device.
fn check_color_space(ctx: &mut Ctx<'_>, space: Option<&Object>, subject: Subject) {
    let name = match space {
        Some(Object::Name(name)) => name.as_bytes(),
        // An array space (`/ICCBased`, `/Separation`, `/Indexed`) names its
        // base in element 0; a `/DeviceN` or `/Separation` over a device
        // alternate is the same problem one level down, which this checker
        // does not chase — recorded as a gap in.
        _ => return,
    };
    if matches!(name, b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK") {
        let name = String::from_utf8_lossy(name).into_owned();
        ctx.fail(
            Clause::DeviceColorWithoutOutputIntent,
            subject,
            format!("/{name} with no output intent to define what its values mean"),
        );
    }
}

/// The filters PDF/A forbids: `/LZWDecode` in both parts, and `/JPXDecode`
/// under A-1, whose PDF 1.4 base does not define it.
fn check_filters(ctx: &mut Ctx<'_>, dict: &Dict, subject: &Subject) {
    let check_one = |filter: &[u8], ctx: &mut Ctx<'_>| {
        if filter == b"LZWDecode" {
            ctx.fail(
                Clause::LzwFilter,
                subject.clone(),
                "/LZWDecode; PDF/A permits no LZW-compressed stream",
            );
        } else if filter == b"JPXDecode" && ctx.level == Level::A1b {
            ctx.fail(
                Clause::JpxFilter,
                subject.clone(),
                "/JPXDecode; PDF/A-1 is built on PDF 1.4, which has no JPEG 2000",
            );
        }
    };
    match dict.raw(names::FILTER) {
        Some(Object::Name(name)) => check_one(name.as_bytes(), ctx),
        Some(Object::Array(filters)) => {
            for i in 0..filters.len() {
                if let Some(Object::Name(name)) = filters.raw_at(i) {
                    check_one(name.as_bytes(), ctx);
                }
            }
        }
        _ => {}
    }
}

/// `/None` is how a `/SMask` says "no mask", and is not transparency.
fn is_none_name(object: &Object) -> bool {
    matches!(object, Object::Name(name) if name.as_bytes() == b"None")
}

/// The dictionary at one array index, whether it is direct or a reference.
fn array_dict<R: Resolve>(array: &Array, index: usize, r: &R) -> Option<Dict> {
    match array.raw_at(index)? {
        Object::Dict(dict) => Some(dict.clone()),
        Object::Stream(stream) => Some(stream.dict.clone()),
        Object::Ref(reference) => match &*r.fetch(*reference).ok()? {
            Object::Dict(dict) => Some(dict.clone()),
            Object::Stream(stream) => Some(stream.dict.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Name the object at an array index when it is a reference, and the
/// containing document when it is a direct value. A direct object has no
/// `ObjRef` to give, and inventing one would be worse than saying so.
fn subject_for(array: &Array, index: usize) -> Subject {
    match array.raw_at(index) {
        Some(Object::Ref(reference)) => Subject::Object(*reference),
        _ => Subject::Document,
    }
}

/// A named resource on a page.
fn resource_subject(page: u32, name: &Name) -> Subject {
    Subject::Resource {
        page,
        name: String::from_utf8_lossy(name.as_bytes()).into_owned(),
    }
}
