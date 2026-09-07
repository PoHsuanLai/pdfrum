//! The conversion itself: the repairs, in the order they must happen.
//!
//! # The shape
//!
//! Two passes over the document. The first pass **surveys**: it walks the
//! object graph deciding what each repair would be and, where the repair is a
//! compromise, whether [`Policy`] authorizes it. Nothing is written. If the
//! survey collects a [`Refusal`], the conversion stops there and the caller
//! gets every obstacle at once rather than one per run.
//!
//! Only if the survey is clean does the second pass **apply** — and by then
//! every decision has already been made, so the applying pass cannot discover
//! a compromise it has to make up its mind about mid-write. That is what makes
//! silent lossy conversion hard to write rather than merely discouraged: the
//! same survey entry produces both the edit and the [`Compromise`], so a
//! compromise that reaches the file has a report entry by construction.
//!
//! # The repairs, and which veraPDF rule each answers
//!
//! Measured over the 44-file benchmark corpus at A-2b. The count is how many of the 41 files veraPDF has an
//! opinion on fail that rule before conversion.
//!
//! | Repair | Rule | Files |
//! |---|---|---|
//! | write the XMP packet with the identification schema | 6.6.4-1 | 27 |
//! | rewrite the packet so it parses, as UTF-8 | 6.6.2.1-1/-4/-5 | 14/11/11 |
//! | write the output intent, sRGB or CMYK | 6.2.4.3-2/-3/-4 | 33/5/28 |
//! | mint a trailer `/ID` | 6.1.3-1 | 11 |
//! | re-serialize every object with legal spacing | 6.1.9-1 | 7 |
//! | strip JavaScript and the forbidden actions | 6.5.1-1, 6.4.1-1/-2 | 1, 2/2 |
//! | correct or supply annotation flags | 6.3.2-1/-2 | 4/5 |
//! | drop `/Interpolate` and `/TR` | 6.2.8-3, 6.2.5-1 | 3, 2 |
//!
//! The last two of those are had for free by writing the file at all: the
//! writer mints an `/ID` on every save and emits its own object frames, so
//! two rules are answered by re-serializing rather than by a repair.
//!
//! What this does **not** answer is the font rules — 6.2.11.4.1, 21 files —
//! except by refusing or rasterizing, and the content-stream rules the checker
//! cannot see either. §10 is that list.

use pdfrum_object::{
    Array, ByteSpan, Dict, Name, ObjRef, Object, PdfString, Resolve, Stream, StringSyntax, names,
};

use super::policy::{Compromise, Conversion, Policy, RasterCause, Refusal};
use super::xmp_write::{self, InfoFields};
use crate::{Document, PdfaLevel};

/// Actions PDF/A forbids by name (ISO 19005-2 6.5.1), `/JavaScript` included.
const FORBIDDEN_ACTIONS: &[&str] = &[
    "JavaScript",
    "Launch",
    "Sound",
    "Movie",
    "ResetForm",
    "ImportData",
    "Hide",
    "SetOCGState",
    "Rendition",
    "Trans",
    "GoTo3DView",
];

/// Annotation subtypes PDF/A does not permit.
const FORBIDDEN_ANNOTATIONS: &[&str] = &[
    "FileAttachment",
    "Sound",
    "Movie",
    "Screen",
    "3D",
    "RichMedia",
];

/// `/Print`, bit 3 of the annotation flag word (ISO 32000-1 Table 165).
const PRINT_FLAG: i64 = 1 << 2;

/// The bits that must be clear: Invisible (1), Hidden (2), `NoView` (6) and
/// `ToggleNoView` (9), as zero-based shifts of the one-based bit numbers.
const ILLEGAL_FLAGS: i64 = (1 << 0) | (1 << 1) | (1 << 5) | (1 << 8);

/// What the survey decided, carried into the applying pass.
///
/// Every entry is a decision already made and already authorized: the applying
/// pass reads this and writes, and never re-decides.
#[derive(Debug, Default)]
struct Plan {
    /// Objects to replace outright.
    ///
    /// A `Vec` rather than a map because the survey order is the report order,
    /// and because two surveys can legitimately touch one object: a form field
    /// merged with its widget is reached from the page's `/Annots` and from
    /// the `/AcroForm`. [`Plan::merged_replacements`] resolves that by folding
    /// later dictionary edits into earlier ones rather than letting one
    /// overwrite the other's repair.
    replace: Vec<(ObjRef, Object)>,
    /// Objects to delete.
    remove: Vec<ObjRef>,
    /// Catalog keys to drop.
    drop_catalog_keys: Vec<Name>,
    /// What those edits cost, in the caller's vocabulary.
    compromises: Vec<Compromise>,
}

/// Run the conversion, returning what it did and the bytes when it converted.
///
/// The bytes are `None` exactly when [`Conversion::converted`] is false. That
/// is the interlock which makes a refusal impossible to ignore: on a refusal
/// there is no file to write.
pub(crate) fn convert(
    doc: &Document,
    level: PdfaLevel,
    policy: Policy,
) -> crate::Result<(Conversion, Option<Vec<u8>>)> {
    let mut refusals = Vec::new();
    let mut plan = Plan::default();

    let catalog = doc.catalog();
    let Some(root) = doc.inner.trailer().reference(names::ROOT) else {
        // A document whose trailer names no catalog is not one we can rewrite
        // the catalog of. Nothing in the corpus is like this, and a save would
        // fail downstream anyway; refusing here says why.
        return Ok((
            Conversion {
                level,
                compromises: Vec::new(),
                refusals: vec![Refusal::ForbiddenFeature {
                    holder: ObjRef::new(0, 0),
                    kind: "no /Root in the trailer, so there is no catalog to convert".to_owned(),
                }],
            },
            None,
        ));
    };

    survey_catalog(doc, &catalog, root, level, policy, &mut plan, &mut refusals);
    survey_pages(doc, level, policy, &mut plan, &mut refusals);
    survey_stray_metadata(doc, root, &mut plan);
    survey_unrepairable(doc, level, policy, &mut refusals);

    // A refusal means nothing is written. Every obstacle is collected first —
    // a caller widening a policy wants the whole list, not its first item.
    if !refusals.is_empty() {
        return Ok((
            Conversion {
                level,
                compromises: Vec::new(),
                refusals,
            },
            None,
        ));
    }

    let bytes = apply(doc, level, root, &catalog, &plan)?;
    Ok((
        Conversion {
            level,
            compromises: plan.compromises,
            refusals: Vec::new(),
        },
        Some(bytes),
    ))
}

impl Plan {
    /// The replacements with each object named once.
    ///
    /// Two surveys can reach one object — a form field merged with its widget
    /// is on a page's `/Annots` *and* in the `/AcroForm` field tree — and each
    /// builds its edit by cloning the original and removing from it. Letting
    /// the later entry win would silently undo the earlier one's repair, which
    /// is exactly the class of bug this module exists to make hard.
    ///
    /// So two dictionary edits of one object are folded: a key is kept only if
    /// *both* kept it, and a value is taken from the later edit when both have
    /// it. Since every edit here either removes a key or rewrites one to a
    /// value that satisfies the same rule, that is the repair both intended.
    fn merged_replacements(&self) -> Vec<(ObjRef, Object)> {
        let mut order: Vec<ObjRef> = Vec::new();
        let mut merged: std::collections::BTreeMap<u32, (ObjRef, Object)> =
            std::collections::BTreeMap::new();

        for (reference, object) in &self.replace {
            match merged.get_mut(&reference.num) {
                None => {
                    order.push(*reference);
                    merged.insert(reference.num, (*reference, object.clone()));
                }
                Some((_, existing)) => match (&mut *existing, object) {
                    (Object::Dict(into), Object::Dict(from)) => {
                        let dropped: Vec<Name> = into
                            .iter()
                            .map(|(key, _)| key.clone())
                            .filter(|key| !from.contains_key(key))
                            .collect();
                        for key in dropped {
                            into.remove(&key);
                        }
                        for (key, value) in from.iter() {
                            if into.contains_key(key) {
                                into.insert(key.clone(), value.clone());
                            }
                        }
                    }
                    // Anything but two dictionaries is one object edited two
                    // incompatible ways, which no survey above produces. The
                    // later edit wins, and the shape is recorded rather than
                    // silently assumed impossible.
                    _ => *existing = object.clone(),
                },
            }
        }
        order
            .into_iter()
            .filter_map(|reference| merged.remove(&reference.num))
            .collect()
    }
}

/// The two obstacles this pipeline can detect and cannot yet repair.
///
/// Both repairs are not implemented yet: substituting a
/// font whose program is missing, and rasterizing a page whose content the
/// level forbids. Neither exists, so both concessions refuse — but they refuse
/// *having looked*, which is the difference between a policy field that is
/// dead and one whose repair is pending.
///
/// The detection is the checker's, not a second implementation of it: the
/// `Clause` enum is exactly the list of things wrong with the document, and
/// `Subject` carries the `ObjRef` precisely so a converter can act on one.
///
/// What each concession means here, given the repair does not exist:
///
/// - **`Refuse`** (the default) refuses, naming the font or the page. That is
///   the strict caller's answer and it is correct: the file this pipeline
///   would write does not conform, and saying so beats handing back a file
///   that quietly fails validation.
/// - **`Accept`** converts anyway and makes **no** [`Compromise`] entry,
///   because nothing was compromised — the font is exactly as unembedded as it
///   was. Every other repair still applies, which is why 10 of 44 corpus files
///   pass afterwards while 21 remain blocked on fonts.
///
/// So `Accept` here reads as "convert as far as you can", which is the
/// honest meaning until the repair lands and it becomes "substitute it".
fn survey_unrepairable(
    doc: &Document,
    level: PdfaLevel,
    policy: Policy,
    refusals: &mut Vec<Refusal>,
) {
    // Neither concession is granted: nothing below can arise.
    if policy.unembeddable_font.accepts() && policy.unrepresentable_content.accepts() {
        return;
    }
    let report = doc.check_pdfa(level);
    for violation in &report.violations {
        match violation.clause {
            crate::PdfaClause::FontNotEmbedded if !policy.unembeddable_font.accepts() => {
                let font = match violation.subject {
                    crate::PdfaSubject::Object(reference) => reference,
                    _ => ObjRef::new(0, 0),
                };
                refusals.push(Refusal::UnembeddableFont {
                    font,
                    base_name: violation.detail.clone(),
                });
            }
            // A-1b only: the checker does not run the transparency walk at
            // A-2b, which permits it, so this arm cannot fire there.
            crate::PdfaClause::Transparency if !policy.unrepresentable_content.accepts() => {
                let page = match violation.subject {
                    crate::PdfaSubject::Page(index)
                    | crate::PdfaSubject::Resource { page: index, .. } => index,
                    _ => 0,
                };
                refusals.push(Refusal::UnrepresentableContent {
                    page,
                    cause: RasterCause::Transparency,
                });
            }
            _ => {}
        }
    }
}

/// Document-wide repairs: the open action, the JavaScript name tree, optional
/// content and embedded files.
fn survey_catalog(
    doc: &Document,
    catalog: &Dict,
    root: ObjRef,
    level: PdfaLevel,
    policy: Policy,
    plan: &mut Plan,
    refusals: &mut Vec<Refusal>,
) {
    let resolve = &doc.inner;

    // `/OpenAction` is an action dictionary or a destination array; only the
    // former can carry a forbidden `/S`.
    if let Some(action) = catalog.dict(&Name::from("OpenAction"), resolve)
        && let Some(kind) = forbidden_action_kind(&action, resolve)
    {
        if policy.forbidden_feature.accepts() {
            plan.drop_catalog_keys.push(Name::from("OpenAction"));
            plan.compromises
                .push(Compromise::ActionRemoved { holder: root, kind });
        } else {
            refusals.push(Refusal::ForbiddenFeature { holder: root, kind });
        }
    }

    // The `/Names /JavaScript` name tree. Dropping the key orphans the tree,
    // and the full save's reachability sweep drops the tree itself for free.
    let names_ref = catalog.reference(names::NAMES);
    if let Some(names_dict) = catalog.dict(names::NAMES, resolve) {
        let mut edited = names_dict.clone();
        let mut removed: Vec<&str> = Vec::new();

        if edited.contains_key(&Name::from("JavaScript")) {
            if policy.forbidden_feature.accepts() {
                edited.remove(&Name::from("JavaScript"));
                removed.push("JavaScript");
                plan.compromises.push(Compromise::ActionRemoved {
                    holder: names_ref.unwrap_or(root),
                    kind: "JavaScript".to_owned(),
                });
            } else {
                refusals.push(Refusal::ForbiddenFeature {
                    holder: names_ref.unwrap_or(root),
                    kind: "a JavaScript name tree".to_owned(),
                });
            }
        }

        // A-1b forbids embedded files outright. A-2b permits one that is
        // itself PDF/A, which means recursing into it; the checker does not do
        // that (design doc §4) and neither does this, so A-2b leaves them
        // alone rather than removing files it is not entitled to remove.
        if level == PdfaLevel::A1b && edited.contains_key(names::EMBEDDED_FILES) {
            if policy.forbidden_feature.accepts() {
                edited.remove(names::EMBEDDED_FILES);
                removed.push("EmbeddedFiles");
                plan.compromises.push(Compromise::EmbeddedFileRemoved {
                    file: names_ref,
                    name: "the /EmbeddedFiles name tree".to_owned(),
                });
            } else {
                refusals.push(Refusal::ForbiddenFeature {
                    holder: names_ref.unwrap_or(root),
                    kind: "an embedded file".to_owned(),
                });
            }
        }

        if !removed.is_empty() {
            match names_ref {
                Some(reference) => plan.replace.push((reference, Object::Dict(edited))),
                // A direct `/Names` lives inside the catalog, which `apply`
                // rewrites anyway, so the edited copy goes back there.
                None => plan.replace.push((
                    root,
                    Object::Dict(with_key(catalog, names::NAMES, Object::Dict(edited))),
                )),
            }
        }
    }

    // 6.4.1-2: a *field* dictionary may carry neither `/A` nor `/AA`, and a
    // field that is not merged with its widget is not on any page's `/Annots`,
    // so the annotation walk never reaches it. veraPDF finds exactly one such
    // field in `forms_text_field`.
    if let Some(acroform) = catalog.dict(&Name::from("AcroForm"), resolve)
        && let Some(fields) = acroform.array(&Name::from("Fields"), resolve)
    {
        let mut seen = std::collections::BTreeSet::new();
        for position in 0..fields.len() {
            if let Some(Object::Ref(reference)) = fields.raw_at(position) {
                survey_field(doc, *reference, policy, plan, refusals, &mut seen, 0);
            }
        }
    }

    // A-1b forbids optional content; A-2b permits it.
    if level == PdfaLevel::A1b && catalog.contains_key(&Name::from("OCProperties")) {
        if policy.forbidden_feature.accepts() {
            plan.drop_catalog_keys.push(Name::from("OCProperties"));
            plan.compromises
                .push(Compromise::OptionalContentRemoved { catalog: root });
        } else {
            refusals.push(Refusal::ForbiddenFeature {
                holder: root,
                kind: "optional content".to_owned(),
            });
        }
    }
}

/// Remove every `/Metadata` stream that is not the catalog's.
///
/// ISO 19005-2 6.6.2.3.1 applies to *all* metadata streams in the file, not
/// only the document's: `forms_push_button` carries a second packet on an
/// image `XObject` holding `tiff:` and `MicrosoftPhoto:` properties that belong
/// to no schema PDF/A predefines, and veraPDF fails the file on it even though
/// the document-level packet we wrote is clean.
///
/// Removing rather than rewriting, because a per-object packet is descriptive
/// metadata about an image, not content: nothing renders differently without
/// it. It is still a change to the file, so it is a compromise —
/// [`Compromise::EmbeddedFileRemoved`] would be the wrong name for it, so it
/// gets its own.
///
/// The scan is by object number over the cross-reference rather than a graph
/// walk, because the packet hangs off objects the conversion has no other
/// reason to visit, and the xref is the only complete list of what a file
/// holds.
fn survey_stray_metadata(doc: &Document, root: ObjRef, plan: &mut Plan) {
    let resolve = &doc.inner;
    let last = doc.inner.xref().last_object_number();
    for num in 1..=last {
        if num == root.num {
            continue;
        }
        let reference = ObjRef::new(num, 0);
        let Ok(object) = resolve.fetch(reference) else {
            continue;
        };
        let Some(dict) = object.as_dict() else {
            continue;
        };

        // A `/TR` on a graphics-state dictionary reached from a content-stream
        // operator rather than from a named page resource: `image_ccitt_transfer`
        // has exactly that, and the resource sweep cannot see it because the
        // name that reaches it is an operand. The same numeric scan finds it.
        // `/Type` is optional on a graphics-state dictionary and the corpus
        // file that carries this one omits it, so the key itself is the
        // signal. `/TR` is not a key any other PDF object type defines, so
        // there is nothing else it could be stripping.
        let type_name = dict.name(names::TYPE);
        let could_be_extgstate = type_name.is_none_or(|name| name.as_bytes() == b"ExtGState");
        let has_transfer = could_be_extgstate
            && (dict.contains_key(&Name::from("TR")) || dict.contains_key(&Name::from("TR2")));

        if !dict.contains_key(names::METADATA) && !has_transfer {
            continue;
        }

        // A stream's dictionary and a plain dictionary both carry these keys,
        // and replacing a stream with a dictionary would throw its bytes away.
        let strip = |target: &mut Dict| {
            target.remove(names::METADATA);
            target.remove(&Name::from("TR"));
            target.remove(&Name::from("TR2"));
        };
        let replacement = if let Object::Stream(stream) = object.as_ref() {
            let mut stream = stream.clone();
            strip(&mut stream.dict);
            Object::Stream(stream)
        } else {
            let mut edited = dict.clone();
            strip(&mut edited);
            Object::Dict(edited)
        };
        plan.replace.push((reference, replacement));
        // Only the metadata removal is a compromise: a transfer function is a
        // colour transform PDF/A forbids precisely because it is not
        // colour-managed, and dropping it is the repair rather than a loss.
        if dict.contains_key(names::METADATA) {
            plan.compromises
                .push(Compromise::ObjectMetadataRemoved { object: reference });
        }
    }
}

/// How deep the field tree is followed. A field tree is a tree; the guard is
/// against a document that makes it a cycle.
const MAX_FIELD_DEPTH: u32 = 32;

/// One form field and its children: strip the `/A` and `/AA` that ISO 19005-2
/// 6.4.1 forbids on a field dictionary.
///
/// Separate from [`repair_annotation`] because a field is reached from the
/// `/AcroForm` rather than from a page, and because a field that *is* merged
/// with its widget is reached both ways — hence `seen`, so the same object is
/// not planned for replacement twice with two different edits.
fn survey_field(
    doc: &Document,
    reference: ObjRef,
    policy: Policy,
    plan: &mut Plan,
    refusals: &mut Vec<Refusal>,
    seen: &mut std::collections::BTreeSet<u32>,
    depth: u32,
) {
    if depth >= MAX_FIELD_DEPTH || !seen.insert(reference.num) {
        return;
    }
    let resolve = &doc.inner;
    let Ok(object) = resolve.fetch(reference) else {
        return;
    };
    let Some(dict) = object.as_dict() else {
        return;
    };

    let mut edited = dict.clone();
    let mut changed = false;
    for key in [Name::from("A"), names::AA.clone()] {
        if !dict.contains_key(&key) {
            continue;
        }
        let kind = format!(
            "a /{} entry on a form field",
            String::from_utf8_lossy(key.as_bytes())
        );
        if policy.forbidden_feature.accepts() {
            edited.remove(&key);
            changed = true;
            plan.compromises.push(Compromise::ActionRemoved {
                holder: reference,
                kind,
            });
        } else {
            refusals.push(Refusal::ForbiddenFeature {
                holder: reference,
                kind,
            });
        }
    }
    if changed {
        plan.replace.push((reference, Object::Dict(edited)));
    }

    if let Some(kids) = dict.array(&Name::from("Kids"), resolve) {
        for position in 0..kids.len() {
            if let Some(Object::Ref(kid)) = kids.raw_at(position) {
                survey_field(doc, *kid, policy, plan, refusals, seen, depth + 1);
            }
        }
    }
}

/// A copy of `dict` with one key set.
fn with_key(dict: &Dict, key: &Name, value: Object) -> Dict {
    let mut out = dict.clone();
    out.insert(key.clone(), value);
    out
}

/// Per-page repairs: annotations, and the image and graphics-state keys PDF/A
/// forbids.
fn survey_pages(
    doc: &Document,
    _level: PdfaLevel,
    policy: Policy,
    plan: &mut Plan,
    refusals: &mut Vec<Refusal>,
) {
    let resolve = &doc.inner;
    // Shared across pages: a form XObject reached from two pages is swept
    // once, and a document that points a form at itself terminates.
    let mut seen = std::collections::BTreeSet::new();
    for index in 0..doc.page_count() {
        let Ok(page) = doc.inner.page(index) else {
            continue;
        };

        survey_annotations(
            doc,
            &page.dict,
            page.reference,
            index,
            policy,
            plan,
            refusals,
        );
        sweep_annotation_appearances(doc, &page.dict, plan, &mut seen);

        // `/Interpolate true` (6.2.8-3) and an `/ExtGState` `/TR` (6.2.5-1)
        // are single keys on a resource that is otherwise legal, so each is
        // repaired by dropping the key rather than by touching what uses it.
        // Neither changes what a reader sees: one is an interpolation hint,
        // the other a transfer function PDF/A forbids precisely because it is
        // not colour-managed. So neither is a compromise.
        let Some(resources) = page.dict.dict(names::RESOURCES, resolve) else {
            continue;
        };
        sweep_resources(doc, &resources, plan, &mut seen, 0);
    }
}

/// Sweep the resources of every annotation appearance stream on a page.
///
/// An appearance is a form `XObject` with its own `/Resources`, and veraPDF
/// reports names and keys from inside them (`.../annots[2]/appearance[0]/...`),
/// so a sweep that stopped at the page's own resource dictionary would miss
/// them. The `/N` may be a stream or a dictionary of states, and both shapes
/// appear in the corpus.
fn sweep_annotation_appearances(
    doc: &Document,
    page: &Dict,
    plan: &mut Plan,
    seen: &mut std::collections::BTreeSet<u32>,
) {
    let resolve = &doc.inner;
    let Some(annots) = page.array(names::ANNOTS, resolve) else {
        return;
    };
    for position in 0..annots.len() {
        let Some(annot) = annots.get(position, resolve) else {
            continue;
        };
        let Some(ap) = annot.as_dict().and_then(|d| d.dict(names::AP, resolve)) else {
            continue;
        };
        let normal = Name::from("N");
        if let Some(stream) = ap.stream(&normal, resolve) {
            if let Some(resources) = stream.dict.dict(names::RESOURCES, resolve) {
                sweep_resources(doc, &resources, plan, seen, 1);
            }
        } else if let Some(states) = ap.dict(&normal, resolve) {
            // A toggle widget's `/N` is a dictionary of appearance states,
            // each of which is its own stream.
            for (_, value) in states.iter() {
                let Object::Ref(reference) = value else {
                    continue;
                };
                let Ok(object) = resolve.fetch(*reference) else {
                    continue;
                };
                if let Some(stream) = object.as_stream()
                    && let Some(resources) = stream.dict.dict(names::RESOURCES, resolve)
                {
                    sweep_resources(doc, &resources, plan, seen, 1);
                }
            }
        }
    }
}

/// How deep the resource sweep follows a form `XObject`'s own `/Resources`.
///
/// A form `XObject` carries resources, and one of those can be another form.
/// veraPDF finds an `/Interpolate` two levels down in `image_bug_898443`, so
/// the walk has to recurse; the depth and the visited set are the two guards a
/// recursive object-graph walk needs against a document that points a form at
/// itself.
const MAX_RESOURCE_DEPTH: u32 = 16;

/// Drop the keys PDF/A forbids from every resource this page can reach.
fn sweep_resources(
    doc: &Document,
    resources: &Dict,
    plan: &mut Plan,
    seen: &mut std::collections::BTreeSet<u32>,
    depth: u32,
) {
    if depth >= MAX_RESOURCE_DEPTH {
        return;
    }
    let resolve = &doc.inner;
    drop_key_from_resources(
        doc,
        resources,
        names::XOBJECT,
        &Name::from("Interpolate"),
        plan,
    );
    for key in ["TR", "TR2"] {
        drop_key_from_resources(
            doc,
            resources,
            &Name::from("ExtGState"),
            &Name::from(key),
            plan,
        );
    }

    // Recurse into every form XObject's own resource dictionary.
    let Some(xobjects) = resources.dict(names::XOBJECT, resolve) else {
        return;
    };
    for (_, value) in xobjects.iter() {
        let Object::Ref(reference) = value else {
            continue;
        };
        if !seen.insert(reference.num) {
            continue;
        }
        let Ok(object) = resolve.fetch(*reference) else {
            continue;
        };
        let Some(stream) = object.as_stream() else {
            continue;
        };
        if let Some(nested) = stream.dict.dict(names::RESOURCES, resolve) {
            sweep_resources(doc, &nested, plan, seen, depth + 1);
        }
    }
}

/// Drop `key` from every indirect member of a page's `category` resource
/// dictionary that carries it.
///
/// Only indirect members. A direct one lives inside the resource dictionary,
/// which lives inside the page, so reaching it means rewriting the resource
/// tree wholesale for a key that is rare in that position.
/// Recorded as a gap rather than left unsaid.
fn drop_key_from_resources(
    doc: &Document,
    resources: &Dict,
    category: &Name,
    key: &Name,
    plan: &mut Plan,
) {
    let resolve = &doc.inner;
    let Some(group) = resources.dict(category, resolve) else {
        return;
    };
    for (_, value) in group.iter() {
        let Object::Ref(reference) = value else {
            continue;
        };
        let Ok(object) = resolve.fetch(*reference) else {
            continue;
        };
        match object.as_ref() {
            Object::Stream(stream) if stream.dict.contains_key(key) => {
                let mut edited = stream.clone();
                edited.dict.remove(key);
                plan.replace.push((*reference, Object::Stream(edited)));
            }
            Object::Dict(dict) if dict.contains_key(key) => {
                let mut edited = dict.clone();
                edited.remove(key);
                plan.replace.push((*reference, Object::Dict(edited)));
            }
            _ => {}
        }
    }
}

/// Annotations: remove the forbidden subtypes, correct the flag words, and
/// strip the `/A` and `/AA` a widget may not carry.
fn survey_annotations(
    doc: &Document,
    page: &Dict,
    page_ref: Option<ObjRef>,
    index: u32,
    policy: Policy,
    plan: &mut Plan,
    refusals: &mut Vec<Refusal>,
) {
    let resolve = &doc.inner;
    let Some(annots) = page.array(names::ANNOTS, resolve) else {
        return;
    };
    let mut kept = Array::default();
    let mut dropped_any = false;

    for position in 0..annots.len() {
        // Unresolved on purpose: an indirect annotation is replaced as its own
        // object, a direct one only through the array, and only the raw form
        // tells the two apart.
        let Some(entry) = annots.raw_at(position) else {
            continue;
        };
        // A direct annotation dictionary is repaired in place inside the array
        // we are rebuilding; an indirect one is replaced as its own object.
        let (reference, dict) = match entry {
            Object::Ref(reference) => {
                let Ok(object) = resolve.fetch(*reference) else {
                    continue;
                };
                let Some(dict) = object.as_dict().cloned() else {
                    kept.push(entry.clone());
                    continue;
                };
                (Some(*reference), dict)
            }
            Object::Dict(dict) => (None, dict.clone()),
            other => {
                kept.push(other.clone());
                continue;
            }
        };

        let subtype = dict
            .name(names::SUBTYPE)
            .map(|n| String::from_utf8_lossy(n.as_bytes()).into_owned())
            .unwrap_or_default();
        let holder = reference.or(page_ref).unwrap_or(ObjRef::new(0, 0));

        if FORBIDDEN_ANNOTATIONS.contains(&subtype.as_str()) {
            if policy.forbidden_feature.accepts() {
                if let Some(reference) = reference {
                    plan.remove.push(reference);
                }
                plan.compromises.push(Compromise::AnnotationRemoved {
                    annotation: holder,
                    page: index,
                    subtype,
                });
                dropped_any = true;
            } else {
                refusals.push(Refusal::ForbiddenFeature {
                    holder,
                    kind: format!("a /{subtype} annotation"),
                });
                kept.push(entry.clone());
            }
            continue;
        }

        let repaired = repair_annotation(
            &dict, &subtype, resolve, policy, index, holder, plan, refusals,
        );
        match (reference, repaired) {
            (Some(reference), Some(edited)) => {
                plan.replace.push((reference, Object::Dict(edited)));
                kept.push(Object::Ref(reference));
            }
            (Some(reference), None) => kept.push(Object::Ref(reference)),
            // A direct annotation's repair only reaches the file if the array
            // it sits in is rewritten, so it forces the rewrite below.
            (None, Some(edited)) => {
                kept.push(Object::Dict(edited));
                dropped_any = true;
            }
            (None, None) => kept.push(entry.clone()),
        }
    }

    if !dropped_any {
        return;
    }
    match page.reference(names::ANNOTS) {
        Some(annots_ref) => plan.replace.push((annots_ref, Object::Array(kept))),
        // A direct `/Annots` array is rewritten with the page dictionary that
        // holds it, which needs the page to be an indirect object — it always
        // is, since the page tree names it.
        None => {
            if let Some(page_ref) = page_ref {
                plan.replace.push((
                    page_ref,
                    Object::Dict(with_key(page, names::ANNOTS, Object::Array(kept))),
                ));
            }
        }
    }
}

/// One annotation's repairs, returning the edited dictionary when anything
/// changed.
#[allow(clippy::too_many_arguments)]
fn repair_annotation<R: Resolve>(
    dict: &Dict,
    subtype: &str,
    resolve: &R,
    policy: Policy,
    index: u32,
    holder: ObjRef,
    plan: &mut Plan,
    refusals: &mut Vec<Refusal>,
) -> Option<Dict> {
    let mut edited = dict.clone();
    let mut changed = false;

    // 6.3.2-1 and -2: `/F` must be present, must set `/Print`, and must clear
    // the four bits that hide the annotation. `/Popup` is exempt from the
    // presence rule, since a popup is shown by its parent.
    if subtype != "Popup" {
        let flags = dict.int(&Name::from("F"), resolve).unwrap_or(0);
        let wanted = (flags | PRINT_FLAG) & !ILLEGAL_FLAGS;
        // Clearing a hiding bit makes an annotation that was invisible show
        // and print, which changes what the page draws — so it is a compromise
        // and needs the policy's permission. Merely *supplying* a missing `/F`
        // is neither: the PDF default for an absent flag word already prints,
        // so writing `/Print` where there was no flag word at all is a repair
        // that changes nothing a reader sees.
        let hides = flags & ILLEGAL_FLAGS != 0;
        if hides && !policy.forbidden_feature.accepts() {
            refusals.push(Refusal::ForbiddenFeature {
                holder,
                kind: "an annotation hidden by its /F flags, which PDF/A requires be \
                       visible and printable"
                    .to_owned(),
            });
        } else if wanted != flags || !dict.contains_key(&Name::from("F")) {
            edited.insert(Name::from("F"), Object::Int(wanted));
            changed = true;
            if hides {
                plan.compromises.push(Compromise::AnnotationFlagsChanged {
                    annotation: holder,
                    page: index,
                });
            }
        }
    }

    // 6.4.1-1 and -2: a widget annotation may carry neither `/A` nor `/AA`,
    // and the same rule covers the field dictionary a widget is merged with —
    // which is the same object in the merged form every corpus file uses.
    for key in [Name::from("A"), names::AA.clone()] {
        if !dict.contains_key(&key) {
            continue;
        }
        let forbidden = dict
            .dict(&key, resolve)
            .and_then(|action| forbidden_action_kind(&action, resolve));
        // On a widget the key is forbidden whatever it holds; elsewhere only
        // a forbidden action type is, because an ordinary `/GoTo` in an `/A`
        // is legal on a link and is what most `/A` keys hold.
        let is_widget = subtype == "Widget";
        let Some(kind) = forbidden.or_else(|| {
            is_widget.then(|| {
                format!(
                    "a /{} entry on a widget",
                    String::from_utf8_lossy(key.as_bytes())
                )
            })
        }) else {
            continue;
        };
        if policy.forbidden_feature.accepts() {
            edited.remove(&key);
            changed = true;
            plan.compromises
                .push(Compromise::ActionRemoved { holder, kind });
        } else {
            refusals.push(Refusal::ForbiddenFeature { holder, kind });
        }
    }

    // 6.3.3-2: an appearance dictionary may hold only `/N`.
    if let Some(ap) = dict.dict(names::AP, resolve) {
        let mut trimmed = ap.clone();
        let mut ap_changed = false;
        if ap.contains_key(&Name::from("R")) || ap.contains_key(&Name::from("D")) {
            trimmed.remove(&Name::from("R"));
            trimmed.remove(&Name::from("D"));
            ap_changed = true;
        }

        // 6.3.3-3: a button widget's `/N` must be a subdictionary keyed by
        // appearance state, not a bare stream. A pushbutton has one appearance
        // and files write it as the stream directly; the repair wraps it under
        // the widget's current `/AS`, or `/Off` when it names none, which is
        // what a reader already falls back to.
        let normal = Name::from("N");
        let is_button = field_type(dict, resolve).as_deref() == Some("Btn");
        // `Dict::dict` answers for a stream too — a stream *has* a dictionary
        // — so "is it a state subdictionary" has to be asked as "is it not a
        // stream", or the test passes on exactly the shape it is looking for.
        if is_button
            && let Some(stream_ref) = ap.reference(&normal)
            && ap.stream(&normal, resolve).is_some()
        {
            let state = dict
                .name(&Name::from("AS"))
                .map_or_else(|| Name::from("Off"), std::clone::Clone::clone);
            let mut states = Dict::default();
            states.insert(state.clone(), Object::Ref(stream_ref));
            trimmed.insert(normal, Object::Dict(states));
            // The widget must name the state it is showing, or a reader has a
            // subdictionary and no key into it.
            if !dict.contains_key(&Name::from("AS")) {
                edited.insert(Name::from("AS"), Object::Name(state));
            }
            ap_changed = true;
        }

        if ap_changed {
            edited.insert(names::AP.clone(), Object::Dict(trimmed));
            changed = true;
        }
    }

    changed.then_some(edited)
}

/// How deep a `/Next` chain is followed.
///
/// A malformed document can point an action's `/Next` back at itself, and the
/// walk below is recursive. The depth is the guard: an action chain longer
/// than this in a real document does not exist, and one that is longer is
/// damage rather than content.
const MAX_ACTION_DEPTH: u32 = 32;

/// A field's `/FT`, which is inheritable through `/Parent`.
///
/// A widget merged with its field carries `/FT` directly; one whose field is a
/// separate object, or which is a kid of a parent that holds the type, has to
/// walk up. The depth guard is [`MAX_FIELD_DEPTH`], against a `/Parent` cycle.
fn field_type<R: Resolve>(dict: &Dict, resolve: &R) -> Option<String> {
    let mut current = dict.clone();
    for _ in 0..MAX_FIELD_DEPTH {
        if let Some(ft) = current.name(&Name::from("FT")) {
            return Some(String::from_utf8_lossy(ft.as_bytes()).into_owned());
        }
        current = current.dict(&Name::from("Parent"), resolve)?;
    }
    None
}

/// The `/S` of an action dictionary when it names a kind PDF/A forbids,
/// following the `/Next` chain.
fn forbidden_action_kind<R: Resolve>(action: &Dict, resolve: &R) -> Option<String> {
    forbidden_action_at(action, resolve, 0)
}

fn forbidden_action_at<R: Resolve>(action: &Dict, resolve: &R, depth: u32) -> Option<String> {
    if depth >= MAX_ACTION_DEPTH {
        return None;
    }
    if let Some(kind) = action.name(&Name::from("S")) {
        let kind = String::from_utf8_lossy(kind.as_bytes()).into_owned();
        if FORBIDDEN_ACTIONS.contains(&kind.as_str()) {
            return Some(kind);
        }
    }
    // A forbidden action reached through `/Next` is as forbidden as one
    // reached directly, and `/Next` is either one action or an array of them.
    let next = Name::from("Next");
    if let Some(array) = action.array(&next, resolve) {
        return (0..array.len())
            .filter_map(|i| array.get(i, resolve))
            .filter_map(|entry| entry.as_dict().cloned())
            .find_map(|entry| forbidden_action_at(&entry, resolve, depth + 1));
    }
    action
        .dict(&next, resolve)
        .and_then(|entry| forbidden_action_at(&entry, resolve, depth + 1))
}

/// The second pass: write the plan, the metadata and the output intent, then
/// serialize.
fn apply(
    doc: &Document,
    level: PdfaLevel,
    root: ObjRef,
    catalog: &Dict,
    plan: &Plan,
) -> crate::Result<Vec<u8>> {
    let mut edit = pdfrum_edit::EditDoc::new(&doc.inner);

    // The catalog may itself be a planned replacement (a direct `/Names` that
    // had a key dropped), so the plan is applied first and the catalog is then
    // built on whatever the plan left there.
    let mut catalog = catalog.clone();
    for (reference, object) in &plan.merged_replacements() {
        if *reference == root
            && let Object::Dict(dict) = object
        {
            catalog = dict.clone();
            continue;
        }
        edit.replace(*reference, object.clone());
    }
    for reference in &plan.remove {
        edit.remove(*reference);
    }
    for key in &plan.drop_catalog_keys {
        catalog.remove(key);
    }

    // The XMP packet, generated rather than patched — `xmp_write`'s module
    // docs say why. It replaces whatever was there, because the packets that
    // fail are exactly the ones that cannot be edited into shape.
    let mut metadata_dict = Dict::default();
    metadata_dict.insert(names::TYPE.clone(), Object::Name(names::METADATA.clone()));
    metadata_dict.insert(names::SUBTYPE.clone(), Object::Name(Name::from("XML")));
    let metadata = edit.add(Object::Stream(Box::new(Stream::new(
        metadata_dict,
        ByteSpan::from(xmp_write::packet(level, &info_fields(doc))),
    ))));
    catalog.insert(names::METADATA.clone(), Object::Ref(metadata));

    // The output intent, which is what makes the file's device colour spaces
    // legal: 33 of 41 corpus files fail 6.2.4.3-2 without one. Which profile
    // it carries follows from what the document paints in — see
    // [`intent_profile`], and note that a file gets exactly one.
    if let Some(intents) = output_intent(&mut edit, intent_profile(doc)) {
        catalog.insert(Name::from("OutputIntents"), Object::Array(intents));
    }

    edit.replace(root, Object::Dict(catalog));

    let options = pdfrum_edit::SaveOptions {
        // A rewrite, never an increment: an incremental save leaves the
        // original bytes in front, and those bytes are what fails 6.1.9-1
        // (object spacing) and carry the `/Encrypt` the level forbids.
        mode: pdfrum_edit::SaveMode::Full,
        // PDF/A forbids encryption outright, and this is the only way to drop
        // the trailer's `/Encrypt` — the trailer is the writer's to build.
        remove_security: true,
        ..pdfrum_edit::SaveOptions::default()
    };
    let mut out = Vec::new();
    pdfrum_edit::save(&edit, &options, &mut out)?;
    Ok(out)
}

/// Which destination profile the file's output intent carries.
///
/// A file has **one** — ISO 19005-2 6.2.4.2 requires every `/OutputIntents`
/// entry to share a `/DestOutputProfile`, so this is a choice and not a set —
/// and the profile answers for exactly one family of device space. 6.2.4.3
/// spells that out per space: `-2` wants an RGB destination profile for
/// `/DeviceRGB`, `-3` a CMYK one for `/DeviceCMYK`, `-4` either for
/// `/DeviceGray`. sRGB satisfies RGB and Gray; nothing but a CMYK profile
/// satisfies CMYK.
///
/// An enum rather than a `bool` because the two arms differ in three
/// coordinated values — the profile bytes, `/N`, and the condition string —
/// and a boolean would have each of them written as a separate conditional
/// that a later edit could get out of step with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntentProfile {
    /// sRGB, which answers `/DeviceRGB` and `/DeviceGray`.
    Srgb,
    /// CMYK, which answers `/DeviceCMYK` and nothing else.
    Cmyk,
}

impl IntentProfile {
    /// The encoded profile, its component count, and its condition name.
    ///
    /// `None` when the profile will not encode, which neither does; the caller
    /// then writes no intent rather than one whose `/DestOutputProfile` is
    /// empty, since that is a worse PDF/A failure than having none.
    fn parts(self) -> Option<(Vec<u8>, i64, &'static [u8])> {
        match self {
            Self::Srgb => Some((pdfrum_page::srgb_profile_bytes()?, 3, b"sRGB IEC61966-2.1")),
            // The registered ICC characterization name for CGATS TR 001, which
            // is the condition the vendored profile describes. PDF/A wants
            // `/OutputConditionIdentifier` to name a real condition rather
            // than to be free text.
            Self::Cmyk => Some((pdfrum_page::cmyk_profile_bytes()?, 4, b"CGATS TR 001")),
        }
    }
}

/// The profile the document's own device colour usage calls for.
///
/// **CMYK only when the document paints in CMYK and in nothing else.** Mixing
/// is not a case this can serve: a file that uses `/DeviceCMYK` *and*
/// `/DeviceRGB` needs two destination profiles and is allowed one, so whatever
/// is chosen it fails 6.2.4.3 on the other space. Preferring sRGB there is the
/// smaller loss, because sRGB covers two of the three device spaces.
///
/// The scan is the shallow one, over every object's colour-space keys and
/// array heads — the same no-interpreter limit §4
/// records for the checker. A `/DeviceCMYK` named only as a content-stream
/// operand is invisible here, and the file keeps the sRGB intent it would have
/// had anyway, so the gap costs a repair rather than causing a wrong one.
fn intent_profile(doc: &Document) -> IntentProfile {
    let resolve = &doc.inner;
    let mut cmyk = false;
    let mut other = false;

    let last = doc.inner.xref().last_object_number();
    for num in 1..=last {
        let Ok(object) = resolve.fetch(ObjRef::new(num, 0)) else {
            continue;
        };
        note_device_spaces(object.as_ref(), &mut cmyk, &mut other);
    }

    if cmyk && !other {
        IntentProfile::Cmyk
    } else {
        IntentProfile::Srgb
    }
}

/// How deep a colour-space array is followed looking for a device space.
///
/// `/Indexed` over `/Separation` over `/DeviceCMYK` is three, and a document
/// that nests further is describing something this scan does not need to
/// resolve exactly — it only has to decide which of two profiles to write.
const MAX_COLOR_SPACE_DEPTH: u32 = 8;

/// Record any device colour space `object` names, at one level of the graph.
///
/// It looks at the keys that hold a colour space (`/ColorSpace` on an image or
/// a shading, `/CS` on a group or a shading's abbreviation) and at the
/// `/ColorSpace` resource dictionary, whose every value is one. That reaches
/// an image's space, a shading's space and a named resource, which is where
/// the corpus's CMYK lives.
fn note_device_spaces(object: &Object, cmyk: &mut bool, other: &mut bool) {
    let Some(dict) = object.as_dict() else {
        return;
    };
    for key in [names::COLOR_SPACE, &Name::from("CS")] {
        if let Some(space) = dict.raw(key) {
            note_one_space(space, cmyk, other, 0);
        }
    }
    // A `/ColorSpace` *resource* dictionary maps names to spaces, so its
    // values are spaces rather than its `/ColorSpace` key. Reached here as a
    // sibling of `/Font` and `/XObject` under a `/Resources`.
    if let Some(Object::Dict(resources)) = dict.raw(names::RESOURCES)
        && let Some(Object::Dict(spaces)) = resources.raw(names::COLOR_SPACE)
    {
        for (_, space) in spaces.iter() {
            note_one_space(space, cmyk, other, 0);
        }
    }
}

/// One colour space: a device name, or an array whose head names its base.
fn note_one_space(space: &Object, cmyk: &mut bool, other: &mut bool, depth: u32) {
    match space {
        Object::Name(name) => match name.as_bytes() {
            b"DeviceCMYK" => *cmyk = true,
            b"DeviceRGB" | b"DeviceGray" => *other = true,
            _ => {}
        },
        // `/Indexed`, `/Separation` and `/DeviceN` all carry the space they
        // are built over, and a device space underneath is the same problem
        // one level down — 6.2.4.3 is about what the file finally paints in.
        Object::Array(array) if depth < MAX_COLOR_SPACE_DEPTH => {
            for i in 0..array.len() {
                if let Some(element) = array.raw_at(i) {
                    note_one_space(element, cmyk, other, depth + 1);
                }
            }
        }
        _ => {}
    }
}

/// The `/OutputIntents` array with `profile` behind it.
///
/// `None` when the profile will not encode; see [`IntentProfile::parts`].
fn output_intent(edit: &mut pdfrum_edit::EditDoc<'_>, profile: IntentProfile) -> Option<Array> {
    let (profile_bytes, components, condition) = profile.parts()?;
    let mut profile_dict = Dict::default();
    profile_dict.insert(Name::from("N"), Object::Int(components));
    let profile = edit.add(Object::Stream(Box::new(Stream::new(
        profile_dict,
        ByteSpan::from(profile_bytes),
    ))));

    let mut intent = Dict::default();
    intent.insert(
        names::TYPE.clone(),
        Object::Name(Name::from("OutputIntent")),
    );
    // `GTS_PDFA1` is the subtype for every part of ISO 19005 including part 2:
    // the `1` is historical and is not a version number.
    intent.insert(Name::from("S"), Object::Name(Name::from("GTS_PDFA1")));
    let condition = PdfString::new(condition, StringSyntax::Literal);
    intent.insert(
        Name::from("OutputConditionIdentifier"),
        Object::Str(condition.clone()),
    );
    intent.insert(Name::from("Info"), Object::Str(condition));
    intent.insert(Name::from("DestOutputProfile"), Object::Ref(profile));

    let intent_ref = edit.add(Object::Dict(intent));
    let mut array = Array::default();
    array.push(Object::Ref(intent_ref));
    Some(array)
}

/// The information-dictionary fields the XMP packet mirrors.
fn info_fields(doc: &Document) -> InfoFields {
    let metadata = doc.metadata();
    InfoFields {
        title: metadata.title.clone(),
        author: metadata.author.clone(),
        creator: metadata.creator.clone(),
        producer: metadata.producer.clone(),
        keywords: metadata.keywords.clone(),
        create_date: metadata.creation_date.as_deref().map(pdf_date_to_iso8601),
        modify_date: metadata
            .modification_date
            .as_deref()
            .map(pdf_date_to_iso8601),
    }
}

/// A PDF date string (`D:YYYYMMDDHHmmSSOHH'mm'`) as XMP's ISO 8601.
///
/// A value with less than a four-digit year comes back unchanged and
/// `xmp_write` then drops it: this converts what it recognises rather than
/// guessing at what it does not, because a wrong date written confidently is
/// worse than an absent optional property.
fn pdf_date_to_iso8601(pdf: &str) -> String {
    let body = pdf.trim_start_matches("D:");
    let digits: Vec<char> = body.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return pdf.to_owned();
    }
    let at = |range: std::ops::Range<usize>| -> Option<String> {
        (digits.len() >= range.end).then(|| digits[range].iter().collect())
    };
    let mut out: String = digits[0..4].iter().collect();
    let Some(month) = at(4..6) else { return out };
    out.push('-');
    out.push_str(&month);
    let Some(day) = at(6..8) else { return out };
    out.push('-');
    out.push_str(&day);
    let (Some(hour), Some(minute)) = (at(8..10), at(10..12)) else {
        return out;
    };
    out.push('T');
    out.push_str(&hour);
    out.push(':');
    out.push_str(&minute);
    // Seconds are optional in the PDF form; XMP wants a complete time once it
    // has one at all, so an absent seconds field becomes `00`.
    out.push(':');
    out.push_str(&at(12..14).unwrap_or_else(|| "00".to_owned()));

    // The zone. A PDF date's offset is `O HH ' mm '`; anything unreadable
    // becomes `Z`, which is what a date with no zone means to XMP anyway.
    let rest: String = body.chars().skip(digits.len()).collect();
    let sign = rest.chars().next();
    let zone: Vec<char> = rest.chars().filter(char::is_ascii_digit).collect();
    if matches!(sign, Some('+' | '-')) && zone.len() >= 4 {
        out.push(sign.unwrap_or('Z'));
        out.extend(&zone[0..2]);
        out.push(':');
        out.extend(&zone[2..4]);
    } else {
        out.push('Z');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{FORBIDDEN_ACTIONS, ILLEGAL_FLAGS, PRINT_FLAG, pdf_date_to_iso8601};

    #[test]
    fn a_pdf_date_becomes_iso8601() {
        assert_eq!(
            pdf_date_to_iso8601("D:20260907120000Z"),
            "2026-09-07T12:00:00Z"
        );
        assert_eq!(
            pdf_date_to_iso8601("D:20260907120000+01'00'"),
            "2026-09-07T12:00:00+01:00"
        );
        // Truncated forms stop where the input does rather than inventing
        // digits: a fabricated day is a wrong date.
        assert_eq!(pdf_date_to_iso8601("D:2026"), "2026");
        assert_eq!(pdf_date_to_iso8601("D:202609"), "2026-09");
        assert_eq!(pdf_date_to_iso8601("D:20260907"), "2026-09-07");
    }

    // Unrecognisable input comes back unchanged and `xmp_write` declines to
    // write it. The two halves together keep a malformed date out of the
    // packet rather than failing 6.6.2.3.1-2 on it.
    #[test]
    fn an_unreadable_date_is_returned_for_the_writer_to_drop() {
        assert_eq!(pdf_date_to_iso8601("garbage"), "garbage");
        let packet = super::xmp_write::packet(
            crate::PdfaLevel::A2b,
            &super::InfoFields {
                create_date: Some(pdf_date_to_iso8601("garbage")),
                ..super::InfoFields::default()
            },
        );
        assert!(!packet.windows(10).any(|w| w == b"CreateDate"));
    }

    #[test]
    fn the_flag_repair_sets_print_and_clears_the_hiding_bits() {
        // The 6.3.2-2 message veraPDF prints on the corpus is `F = 6`, which
        // is Print and Hidden both set.
        assert_eq!((6 | PRINT_FLAG) & !ILLEGAL_FLAGS, PRINT_FLAG);
        // An annotation with no flags at all gains Print, which is 6.3.2-1.
        assert_eq!(PRINT_FLAG & !ILLEGAL_FLAGS, 4);
        // A flag word that is already legal is left exactly as it stands.
        assert_eq!((PRINT_FLAG | 16) | PRINT_FLAG & !ILLEGAL_FLAGS, 20);
    }

    /// What [`super::note_one_space`] makes of one colour space, as the pair
    /// the intent choice is actually made on.
    fn spaces_in(space: &pdfrum_object::Object) -> (bool, bool) {
        let (mut cmyk, mut other) = (false, false);
        super::note_one_space(space, &mut cmyk, &mut other, 0);
        (cmyk, other)
    }

    #[test]
    fn a_device_space_is_recognised_by_name() {
        use pdfrum_object::{Name, Object};
        assert_eq!(
            spaces_in(&Object::Name(Name::from("DeviceCMYK"))),
            (true, false)
        );
        assert_eq!(
            spaces_in(&Object::Name(Name::from("DeviceRGB"))),
            (false, true)
        );
        assert_eq!(
            spaces_in(&Object::Name(Name::from("DeviceGray"))),
            (false, true)
        );
        // An ICC or Lab space is already device-independent, so it neither
        // needs nor constrains the intent.
        assert_eq!(
            spaces_in(&Object::Name(Name::from("Pattern"))),
            (false, false)
        );
    }

    // `/Separation` and `/Indexed` name the space they are built over, and a
    // device space underneath is what the file finally paints in — 6.2.4.3
    // is about that, not about the wrapper.
    #[test]
    fn a_device_space_under_a_wrapper_is_found() {
        use pdfrum_object::{Array, Name, Object};
        let mut separation = Array::default();
        separation.push(Object::Name(Name::from("Separation")));
        separation.push(Object::Name(Name::from("Spot")));
        separation.push(Object::Name(Name::from("DeviceCMYK")));
        assert_eq!(spaces_in(&Object::Array(separation)), (true, false));
    }

    // The mixed case, which is the one the choice turns on: a file painting
    // in both gets sRGB, because one intent cannot answer for both spaces and
    // sRGB covers two of the three device spaces.
    #[test]
    fn a_document_painting_in_both_is_not_a_cmyk_document() {
        use pdfrum_object::{Array, Name, Object};
        let mut both = Array::default();
        both.push(Object::Name(Name::from("DeviceCMYK")));
        both.push(Object::Name(Name::from("DeviceRGB")));
        // Both are seen, which is what makes it the mixed case — and the
        // choice is `cmyk && !other`, so seeing the other one is what sends
        // this file to sRGB.
        assert_eq!(spaces_in(&Object::Array(both)), (true, true));
    }

    #[test]
    fn javascript_is_in_the_forbidden_action_list() {
        assert!(FORBIDDEN_ACTIONS.contains(&"JavaScript"));
        assert!(FORBIDDEN_ACTIONS.contains(&"Launch"));
        // And an ordinary navigation action is not, or every link in every
        // document would be stripped.
        assert!(!FORBIDDEN_ACTIONS.contains(&"GoTo"));
        assert!(!FORBIDDEN_ACTIONS.contains(&"URI"));
    }
}
