//! `Unsupported feature: <name>.` — the oracle's notice lines, which land on
//! **stdout** and therefore inside the metadata and pageinfo dumps.
//!
//! This is the least obvious part of matching those dumps. The oracle
//! registers a handler for the unsupported-feature callback that prints on
//! stdout, so a portfolio PDF's `--show-metadata` golden opens with
//!
//! ```text
//! Unsupported feature: Portfolios_Packages.
//! Unsupported feature: Attachment.
//! Title        = Portfolio (20 bytes)
//! ```
//!
//! and a signed PDF's ends with `Unsupported feature: Digital_Signature.`
//! after the eight tags. The position is not decoration: document-level
//! notices are raised inside the load call, before anything is dumped, while
//! annotation-level ones are raised when each page's annotation list is first
//! walked, which happens before that page's own lines.
//!
//! Nothing here deduplicates, matching the C++: `two_signatures.pdf` prints
//! the signature line twice.
//!
//! Two codes the oracle can spell are unreachable in the C++ and so are not
//! implemented: `Rights_Management` has no `RaiseUnsupportedError` call site
//! anywhere, and `Unknown` is the default of a switch every live code covers.

use pdfrum_object::{Dict, Name, Resolve, names};

/// A feature notice, spelled as the oracle spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Xfa,
    PortfoliosPackages,
    Attachment,
    SharedReview,
    ThreeD,
    Movie,
    Sound,
    Screen,
    DigitalSignature,
}

impl Feature {
    /// The name inside the notice line.
    pub fn as_str(self) -> &'static str {
        match self {
            Feature::Xfa => "XFA",
            Feature::PortfoliosPackages => "Portfolios_Packages",
            Feature::Attachment => "Attachment",
            Feature::SharedReview => "Shared_Review",
            Feature::ThreeD => "3D",
            Feature::Movie => "Movie",
            Feature::Sound => "Sound",
            Feature::Screen => "Screen",
            Feature::DigitalSignature => "Digital_Signature",
        }
    }

    /// The whole line, newline included.
    pub fn line(self) -> String {
        format!("Unsupported feature: {}.\n", self.as_str())
    }
}

/// The name of the JavaScript action a shared review registers under.
const SHARED_REVIEW_ACTION: &[u8] = b"com.adobe.acrobat.SharedReview.Register";

/// Notices raised while the document loads, in the order the C++ raises them.
///
/// Order matters and is not alphabetical: `/Collection` is tested first, then
/// the two `/Names` sub-checks. All three come from one function that runs
/// inside the load call, so they precede everything the tool prints.
///
/// `Shared_Form`, which is read out of an XMP metadata stream, is not
/// detected: it needs the filter chain and an XML parser, and no corpus
/// golden contains the line.
pub fn document(catalog: &Dict, r: &impl Resolve) -> Vec<Feature> {
    let mut found = Vec::new();
    // Bare key existence, not a typed read: a `/Collection null` still counts.
    if catalog.contains_key(names::COLLECTION) {
        found.push(Feature::PortfoliosPackages);
    }
    if let Some(dict_names) = catalog.dict(names::NAMES, r) {
        if dict_names.contains_key(names::EMBEDDED_FILES) {
            found.push(Feature::Attachment);
        }
        if has_shared_review(&dict_names, r) {
            found.push(Feature::SharedReview);
        }
    }
    found
}

/// The XFA notice, which is raised from a *different* call site and therefore
/// lands in a different place in the output.
///
/// The other document-level checks run inside the load; this one runs when
/// the form-fill environment is set up, which the oracle does **after**
/// dumping the metadata. So `rectangles_multi_page_xfa.pdf`'s golden carries
/// the eight metadata lines first and `Unsupported feature: XFA.` last —
/// a nine-line file that reads as an ordering bug if the two checks are
/// merged, which is exactly how this was found.
///
/// The test is typed, unlike the two key-existence tests above: `/AcroForm`
/// must resolve to a dictionary and its `/XFA` to an array.
pub fn xfa(catalog: &Dict, r: &impl Resolve) -> Option<Feature> {
    is_xfa(catalog, r).then_some(Feature::Xfa)
}

/// Whether the JavaScript name tree registers the shared-review action.
///
/// The flat `/Names` array alternates name and action, and the C++ stringifies
/// every element rather than only the even ones; an action dictionary
/// stringifies to nothing, so the effect is the same and the loop stops at the
/// first hit.
fn has_shared_review(dict_names: &Dict, r: &impl Resolve) -> bool {
    let Some(javascript) = dict_names.dict(names::JAVA_SCRIPT, r) else {
        return false;
    };
    let Some(entries) = javascript.array(names::NAMES, r) else {
        return false;
    };
    (0..entries.len())
        .any(|index| entries.byte_string_at(index).as_deref() == Some(SHARED_REVIEW_ACTION))
}

/// Whether the catalog carries an XFA form.
fn is_xfa(catalog: &Dict, r: &impl Resolve) -> bool {
    catalog
        .dict(names::ACRO_FORM, r)
        .is_some_and(|form| form.array(names::XFA, r).is_some())
}

/// The notice one annotation raises, if any.
///
/// Both conditional cases read a key straight off the annotation dictionary
/// with no inheritance: a widget whose `/FT` lives on a parent field node is
/// **not** reported as a signature, and a screen annotation without `/IT` is
/// reported because a missing key reads as the empty string.
pub fn annotation(annot: &Dict) -> Option<Feature> {
    let subtype = annot.name(names::SUBTYPE)?.as_str()?;
    match subtype {
        "FileAttachment" => Some(Feature::Attachment),
        "Movie" => Some(Feature::Movie),
        "Sound" => Some(Feature::Sound),
        "3D" => Some(Feature::ThreeD),
        "RichMedia" => Some(Feature::Screen),
        "Screen" => {
            (annot.name(names::IT).and_then(Name::as_str) != Some("Img")).then_some(Feature::Screen)
        }
        "Widget" => {
            (annot.name(names::FT) == Some(names::SIG)).then_some(Feature::DigitalSignature)
        }
        _ => None,
    }
}

/// Every notice a page's annotations raise, in `/Annots` order.
///
/// Array entries that are not dictionaries are skipped, as are `/Popup`
/// annotations, which the C++ drops before the walk and which raise nothing
/// anyway.
pub fn page_annotations(page: &Dict, r: &impl Resolve) -> Vec<Feature> {
    let Some(annots) = page.array(names::ANNOTS, r) else {
        return Vec::new();
    };
    (0..annots.len())
        .filter_map(|index| annots.dict_at(index, r))
        .filter(|annot| annot.name(names::SUBTYPE).and_then(Name::as_str) != Some("Popup"))
        .filter_map(|annot| annotation(&annot))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfrum_object::{Array, NoResolve, Object, PdfString};

    fn dict(pairs: impl IntoIterator<Item = (&'static str, Object)>) -> Dict {
        Dict::from_pairs(pairs.into_iter().map(|(k, v)| (Name::from(k), v)))
    }

    fn annot_of(subtype: &'static str, extra: &[(&'static str, Object)]) -> Dict {
        let mut pairs: Vec<(&'static str, Object)> =
            vec![("Subtype", Object::Name(Name::from(subtype)))];
        pairs.extend_from_slice(extra);
        dict(pairs)
    }

    #[test]
    fn the_line_is_spelled_exactly_as_the_golden_store_holds_it() {
        assert_eq!(
            Feature::PortfoliosPackages.line(),
            "Unsupported feature: Portfolios_Packages.\n"
        );
        assert_eq!(
            Feature::DigitalSignature.line(),
            "Unsupported feature: Digital_Signature.\n"
        );
        assert_eq!(Feature::ThreeD.line(), "Unsupported feature: 3D.\n");
    }

    #[test]
    fn a_collection_key_is_a_portfolio_whatever_its_value() {
        assert_eq!(
            document(&dict([("Collection", Object::Null)]), &NoResolve),
            [Feature::PortfoliosPackages]
        );
    }

    #[test]
    fn embedded_files_under_names_is_an_attachment() {
        let catalog = dict([(
            "Names",
            Object::Dict(dict([("EmbeddedFiles", Object::Dict(Dict::new()))])),
        )]);
        assert_eq!(document(&catalog, &NoResolve), [Feature::Attachment]);
    }

    #[test]
    fn a_portfolio_reports_both_lines_in_the_oracles_order() {
        // 2520881570aa69dd/metadata.txt opens with exactly these two lines.
        let catalog = dict([
            (
                "Names",
                Object::Dict(dict([("EmbeddedFiles", Object::Dict(Dict::new()))])),
            ),
            ("Collection", Object::Dict(Dict::new())),
        ]);
        assert_eq!(
            document(&catalog, &NoResolve),
            [Feature::PortfoliosPackages, Feature::Attachment]
        );
    }

    #[test]
    fn a_shared_review_action_name_is_found_in_the_flat_names_array() {
        let catalog = dict([(
            "Names",
            Object::Dict(dict([(
                "JavaScript",
                Object::Dict(dict([(
                    "Names",
                    Object::Array(Array::of([
                        Object::Str(PdfString::literal(SHARED_REVIEW_ACTION)),
                        Object::Dict(Dict::new()),
                    ])),
                )])),
            )])),
        )]);
        assert_eq!(document(&catalog, &NoResolve), [Feature::SharedReview]);
    }

    #[test]
    fn xfa_needs_an_acroform_dictionary_holding_an_xfa_array() {
        let with_array = dict([(
            "AcroForm",
            Object::Dict(dict([("XFA", Object::Array(Array::new()))])),
        )]);
        assert_eq!(xfa(&with_array, &NoResolve), Some(Feature::Xfa));

        // A non-array /XFA is not one this check accepts, matching
        // `GetArrayFor`.
        let with_number = dict([("AcroForm", Object::Dict(dict([("XFA", Object::Int(1))])))]);
        assert_eq!(xfa(&with_number, &NoResolve), None);
        // An AcroForm without /XFA is an ordinary form.
        assert_eq!(
            xfa(&dict([("AcroForm", Object::Dict(Dict::new()))]), &NoResolve),
            None
        );
    }

    #[test]
    fn xfa_is_not_one_of_the_load_time_notices() {
        // It comes from the form-fill setup, which happens after the metadata
        // dump; keeping it out of `document` is what puts the line last.
        let catalog = dict([(
            "AcroForm",
            Object::Dict(dict([("XFA", Object::Array(Array::new()))])),
        )]);
        assert!(document(&catalog, &NoResolve).is_empty());
    }

    #[test]
    fn a_signature_widget_is_recognized_by_its_own_field_type() {
        assert_eq!(
            annotation(&annot_of(
                "Widget",
                &[("FT", Object::Name(Name::from("Sig")))]
            )),
            Some(Feature::DigitalSignature)
        );
        // No /FT of its own: inherited field types are deliberately not
        // followed, so this widget raises nothing.
        assert_eq!(annotation(&annot_of("Widget", &[])), None);
        assert_eq!(
            annotation(&annot_of(
                "Widget",
                &[("FT", Object::Name(Name::from("Tx")))]
            )),
            None
        );
    }

    #[test]
    fn a_screen_annotation_is_reported_unless_it_is_an_image() {
        assert_eq!(annotation(&annot_of("Screen", &[])), Some(Feature::Screen));
        assert_eq!(
            annotation(&annot_of(
                "Screen",
                &[("IT", Object::Name(Name::from("Img")))]
            )),
            None
        );
        assert_eq!(
            annotation(&annot_of("RichMedia", &[])),
            Some(Feature::Screen)
        );
    }

    #[test]
    fn the_media_subtypes_map_to_their_own_names() {
        assert_eq!(annotation(&annot_of("Movie", &[])), Some(Feature::Movie));
        assert_eq!(annotation(&annot_of("Sound", &[])), Some(Feature::Sound));
        assert_eq!(annotation(&annot_of("3D", &[])), Some(Feature::ThreeD));
        assert_eq!(
            annotation(&annot_of("FileAttachment", &[])),
            Some(Feature::Attachment)
        );
    }

    #[test]
    fn ordinary_annotations_raise_nothing() {
        for subtype in ["Link", "Text", "Highlight", "Popup", "Redact"] {
            assert_eq!(annotation(&annot_of(subtype, &[])), None, "{subtype}");
        }
    }

    #[test]
    fn two_signatures_report_twice() {
        // resources/two_signatures.pdf: the golden holds the line twice,
        // because the C++ has no dedup at all.
        let sig = || {
            Object::Dict(annot_of(
                "Widget",
                &[("FT", Object::Name(Name::from("Sig")))],
            ))
        };
        let page = dict([("Annots", Object::Array(Array::of([sig(), sig()])))]);
        assert_eq!(
            page_annotations(&page, &NoResolve),
            [Feature::DigitalSignature, Feature::DigitalSignature]
        );
    }

    #[test]
    fn non_dictionary_annotation_entries_are_skipped() {
        let page = dict([(
            "Annots",
            Object::Array(Array::of([
                Object::Int(3),
                Object::Dict(annot_of("Sound", &[])),
                Object::Null,
            ])),
        )]);
        assert_eq!(page_annotations(&page, &NoResolve), [Feature::Sound]);
    }

    #[test]
    fn a_page_without_annots_raises_nothing() {
        assert!(page_annotations(&Dict::new(), &NoResolve).is_empty());
    }
}
