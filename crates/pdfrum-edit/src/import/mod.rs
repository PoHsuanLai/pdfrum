//! Importing pages from one document into another (ISO 32000-1 §7.7.3).
//!
//! Three operations share one copier: importing pages as pages, imposing
//! several source pages onto one sheet (N-up), and copying viewer
//! preferences.
//!
//! # Importing is transactional
//!
//! The C++ leaves debris behind a failure: a missing source page returns
//! false *after* the destination page was created and inserted, and after
//! earlier pages of the same batch were fully exported. Every destination
//! mutation here is staged in the [`EditDoc`] overlay and committed only on
//! success, so a failed import leaves the destination exactly as it was
//! (divergence D14). The success path is byte-identical; only failure
//! differs, and no test asserts on the destination after a failed import.
//!
//! # Importing never mutates the source
//!
//! PDFium's N-up path writes `/Type /Page` into a source page dictionary that
//! lacked one, so it is not read-only with respect to what it is copying
//! from. Our source is a `&Document` over shared bytes and cannot be mutated;
//! the missing `/Type` is supplied on the *copy* (divergence D15).
//!
//! # Four keys are flattened, in this order
//!
//! An imported page is detached from its `/Parent` chain, so whatever it was
//! inheriting must be written onto it: `/MediaBox` — falling back to
//! `/CropBox`, then to US Letter — then `/Resources` — falling back to an
//! empty dictionary — then `/CropBox` and `/Rotate`, whose absence is simply
//! accepted. `/BleedBox`, `/TrimBox` and `/ArtBox` are **not** in the list
//! and are lost unless the page stated them itself.

// [oracle-bug] The four import-path defects this module fixes rather than
// ports, each verified at the line. They were ruled on as escalation E10 —
// "a departure from this program's usual rule", taken at our discretion; the
// audit's A73 relabels all four as **oracle bugs**, which PLAN.md §212-229
// makes obligatory rather than optional to fix. Each is pinned by its own
// test in `tests/import.rs`.
//
// 1. **The hardcoded destination object number.**
//    `cpdf_pageorganizer.cpp:150-153`: a cloned object whose `/Type` is
//    `Pages` returns the literal `4`, which is right only because
//    `FPDF_CreateNewDocument` happens to number its page tree node 4. Any
//    other destination gets a reference to whatever object 4 is. §7.7.3.2
//    makes `/Parent` a reference to the node's actual parent, not to a
//    number a producer guessed. Ours returns the destination's real
//    `/Root /Pages`.
// 2. **Import is not transactional.** A missing source page returns false
//    *after* the destination page was created and inserted, and after
//    earlier pages of the batch were exported, leaving a stray blank page
//    and a half-imported document. Nothing in ISO 32000-1 sanctions a failed
//    operation leaving debris; ours stages every mutation in the overlay.
// 3. **The source is mutated.** `CPDF_Page`'s constructor writes
//    `/Type /Page` into a source page dictionary that lacked one
//    (`cpdf_page.cpp:33-36`), so the N-up path is not read-only with respect
//    to what it copies from. Ours cannot be: the source is shared immutable
//    bytes, and the missing `/Type` is supplied on the copy.
// 4. **The N-up name reuse.** `cpdf_npagetooneexporter.cpp:224-228`
//    (`AddSubPage`) reuses a name from `src_page_xobject_map_`, cleared once
//    at `:171`, but the registry it must also appear in,
//    `xobject_name_to_number_map_`, is cleared **per output sheet** at
//    `:180` and written only inside `MakeXObjectFromPage` (`:284-285`),
//    which the cache hit skips. So a source page reused on a later sheet
//    emits `/Xn Do` against a `/Resources /XObject` with no `Xn` entry:
//    §8.10.1 requires the name to be in the resources, and the sub-page
//    renders blank. Ours registers the name on every sheet it appears on.
//
// pdf.js implements no page import or N-up imposition, so there is no
// independent implementation to weigh; the reading rests on the spec, and on
// three of the four producing output PDFium itself would then fail to render
// correctly. Two are annotated as bugs in the C++ source.

mod copy;
mod inherit;
mod nup;
mod range;
mod viewer;

use pdfrum_common::PageIndex;
use pdfrum_object::{Array, Dict, Name, ObjRef, Object, Resolve, names};
use pdfrum_parser::Document;

use crate::doc::EditDoc;
use crate::error::Error;
use crate::names as edit_names;

use copy::ObjectMap;
use inherit::inheritable;
use nup::{NupGrid, sub_page_fragment};

pub use range::PageRange;

/// US Letter, the last fallback when a page states no box and inherits none.
const LETTER: [i64; 4] = [0, 0, 612, 792];

/// How an import behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportOptions {
    /// Where in the destination's page list the imported pages go. Pages at
    /// and after this index shift up.
    pub at: PageIndex,
    /// Also copy the source catalog's `/ViewerPreferences`.
    pub viewer_preferences: bool,
}

/// How an N-up imposition behaves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NUpOptions {
    /// The sheet's size in points.
    pub sheet: (f32, f32),
    /// Columns and rows of sub-pages per sheet.
    pub grid: (u32, u32),
}

impl Default for NUpOptions {
    fn default() -> Self {
        Self {
            sheet: (612.0, 792.0),
            grid: (2, 1),
        }
    }
}

/// Prepare `dest` to be imported into, repairing its catalog as needed.
///
/// Idempotent, and every step is a repair rather than a requirement: a
/// catalog with a wrong-but-present `/Type` is left alone, a `/Pages` that
/// does not resolve to a dictionary is replaced with a fresh one, and a
/// `/Kids` that is not an array is replaced along with a force-zeroed
/// `/Count` — those two are written together, so a document with a broken
/// `/Kids` loses whatever `/Count` claimed.
///
/// Returns the destination's `/Pages` object number, which the copier needs.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when there is no catalog to repair — the
/// one hard failure in the whole import path.
pub(crate) fn init_dest(dest: &mut EditDoc<'_>) -> Result<u32, Error> {
    let catalog_ref = dest
        .base()
        .trailer()
        .reference(names::ROOT)
        .ok_or(Error::NoDestinationCatalog)?;
    let catalog = dest
        .fetch(catalog_ref)
        .ok()
        .and_then(|o| o.as_dict().cloned())
        .ok_or(Error::NoDestinationCatalog)?;

    let mut catalog = catalog;

    // An empty or missing `/Type` is repaired; a wrong one is left alone,
    // because a producer that wrote something else may have meant it.
    if catalog
        .name(names::TYPE)
        .is_none_or(|n| n.as_bytes().is_empty())
    {
        set(
            &mut catalog,
            names::TYPE,
            Object::Name(edit_names::CATALOG.clone()),
        );
    }

    // `/Pages` is accepted as a direct dictionary or as a reference. Anything
    // that does not resolve to a dictionary is replaced outright.
    let pages_ref = match catalog.raw(names::PAGES) {
        Some(Object::Ref(r)) if dest.fetch(*r).is_ok_and(|o| o.as_dict().is_some()) => *r,
        _ => {
            let fresh = fresh_pages_node(dest);
            set(&mut catalog, names::PAGES, Object::Ref(fresh));
            fresh
        }
    };

    let mut pages = dest
        .fetch(pages_ref)
        .ok()
        .and_then(|o| o.as_dict().cloned())
        .unwrap_or_default();
    if pages
        .name(names::TYPE)
        .is_none_or(|n| n.as_bytes().is_empty())
    {
        set(&mut pages, names::TYPE, Object::Name(names::PAGES.clone()));
    }
    // A non-array `/Kids` takes `/Count` down with it: both are written
    // together, so a document that lied about one loses the other.
    if !matches!(pages.raw(names::KIDS), Some(Object::Array(_))) {
        set(&mut pages, names::KIDS, Object::Array(Array::new()));
        set(&mut pages, names::COUNT, Object::Int(0));
    }

    dest.replace(pages_ref, Object::Dict(pages));
    dest.replace(catalog_ref, Object::Dict(catalog));
    Ok(pages_ref.num)
}

/// A fresh, empty `/Pages` node.
fn fresh_pages_node(dest: &mut EditDoc<'_>) -> ObjRef {
    dest.add(Object::Dict(Dict::from_pairs([
        (names::TYPE.clone(), Object::Name(names::PAGES.clone())),
        (names::COUNT.clone(), Object::Int(0)),
        (names::KIDS.clone(), Object::Array(Array::new())),
    ])))
}

/// Import `pages` from `src` into `dest`.
///
/// The pages land as a contiguous run at `opts.at`, in the order the range
/// names them, with duplicates producing duplicate destination pages.
///
/// Objects shared between two imported pages — a font, an image `XObject`, a
/// `/Resources` dictionary — are copied **once**, and both destination pages
/// point at the one copy.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the destination has no catalog, and
/// [`Error::PageIndexOutOfRange`] when the range names a page the source does
/// not have. The destination is untouched on either.
pub fn import_pages(
    dest: &mut EditDoc<'_>,
    src: &Document,
    pages: &PageRange,
    opts: &ImportOptions,
) -> Result<(), Error> {
    // Check the whole range before mutating anything: an import either
    // happens or it does not.
    for index in pages.indices() {
        if index.get() >= src.page_count() {
            return Err(Error::PageIndexOutOfRange(*index));
        }
    }

    let pages_node = init_dest(dest)?;
    let mut map = ObjectMap::new();
    let mut created = Vec::new();

    for index in pages.indices() {
        let page = src
            .page(*index)
            .map_err(|_| Error::PageIndexOutOfRange(*index))?;
        let dest_page = dest.add(Object::Null);

        // Register the page's own mapping *first*, so a self-reference from
        // inside it — an annotation's `/P` back-pointer — lands on the new
        // page rather than being pruned by the copier's cross-page rule.
        if let Some(source_ref) = page.reference {
            map.record(source_ref.num, dest_page.num);
        }

        let built = build_page(dest, src, &page.dict, pages_node, &mut map, dest_page);
        dest.replace(dest_page, Object::Dict(built));
        created.push(dest_page);
    }

    insert_into_tree(dest, pages_node, &created, opts.at.get());

    if opts.viewer_preferences
        && let Ok(catalog) = src.catalog()
        && let Some(prefs) = viewer::filtered(&catalog, src)
    {
        copy_viewer_preferences(dest, prefs);
    }
    Ok(())
}

/// Build one destination page dictionary from a source page.
fn build_page(
    dest: &mut EditDoc<'_>,
    src: &Document,
    source: &Dict,
    pages_node: u32,
    map: &mut ObjectMap,
    self_ref: ObjRef,
) -> Dict {
    let mut out = Dict::from_pairs([
        (names::TYPE.clone(), Object::Name(names::PAGE.clone())),
        (
            names::PARENT.clone(),
            Object::Ref(ObjRef::new(pages_node, 0)),
        ),
    ]);

    // Every source key except the two just written: `/Type` is already
    // correct and `/Parent` must name the *destination's* tree.
    for (key, value) in source.iter() {
        if key == names::TYPE || key == names::PARENT {
            continue;
        }
        let mut value = value.clone();
        if rewrite_value(dest, src, &mut value, map, pages_node) {
            out.push(key.clone(), value);
        }
    }

    flatten_inherited(dest, src, source, &mut out, map, pages_node);
    let _ = self_ref;
    out
}

/// Write the four inheritable keys onto the copy, with their fallbacks.
fn flatten_inherited(
    dest: &mut EditDoc<'_>,
    src: &Document,
    source: &Dict,
    out: &mut Dict,
    map: &mut ObjectMap,
    pages_node: u32,
) {
    // `/MediaBox`, falling back to `/CropBox`, then to US Letter. A page with
    // no box at all is still a page, and Letter is what it becomes.
    if !copy_inherited(dest, src, source, out, names::MEDIA_BOX, map, pages_node)
        && !copy_inherited(dest, src, source, out, names::CROP_BOX, map, pages_node)
    {
        // Written as `left bottom right top`, the order a box array uses.
        set(
            out,
            names::MEDIA_BOX,
            Object::Array(Array::of(LETTER.map(Object::Int))),
        );
    } else if !out.contains_key(names::MEDIA_BOX) {
        // The `/CropBox` fallback fired: it becomes the `/MediaBox`.
        if let Some(crop) = out.raw(names::CROP_BOX).cloned() {
            set(out, names::MEDIA_BOX, crop);
        }
    }

    // `/Resources`, falling back to an empty dictionary.
    if !copy_inherited(dest, src, source, out, names::RESOURCES, map, pages_node) {
        set(out, names::RESOURCES, Object::Dict(Dict::new()));
    }

    // `/CropBox` and `/Rotate`: taken when inheritable, absent otherwise.
    // `/Rotate` is not normalized — a value of 450 travels as 450, and only
    // the renderer reduces it.
    copy_inherited(dest, src, source, out, names::CROP_BOX, map, pages_node);
    copy_inherited(dest, src, source, out, names::ROTATE, map, pages_node);
}

/// Copy one inheritable key onto the destination page, renumbering any
/// references it carries.
fn copy_inherited(
    dest: &mut EditDoc<'_>,
    src: &Document,
    source: &Dict,
    out: &mut Dict,
    key: &Name,
    map: &mut ObjectMap,
    pages_node: u32,
) -> bool {
    if out.contains_key(key) {
        return true;
    }
    let Some(mut value) = inheritable(source, key, src) else {
        return false;
    };
    if !rewrite_value(dest, src, &mut value, map, pages_node) {
        return false;
    }
    out.push(key.clone(), value);
    true
}

/// Rewrite one value's references into the destination's numbering.
fn rewrite_value(
    dest: &mut EditDoc<'_>,
    src: &Document,
    value: &mut Object,
    map: &mut ObjectMap,
    pages_node: u32,
) -> bool {
    copy::rewrite_in_place(dest, src, value, map, pages_node)
}

/// Insert the created pages into the destination's `/Kids` at `at`.
fn insert_into_tree(dest: &mut EditDoc<'_>, pages_node: u32, created: &[ObjRef], at: u32) {
    let node_ref = ObjRef::new(pages_node, 0);
    let mut node = dest
        .fetch(node_ref)
        .ok()
        .and_then(|o| o.as_dict().cloned())
        .unwrap_or_default();

    let existing: Vec<Object> = node
        .raw(names::KIDS)
        .and_then(Object::as_array)
        .map(|a| a.iter().cloned().collect())
        .unwrap_or_default();

    // Past the end lands at the end, which is what "append" means to every
    // caller that passes a large index.
    let split = (at as usize).min(existing.len());
    let mut kids = Array::new();
    for value in existing.get(..split).unwrap_or_default() {
        kids.push(value.clone());
    }
    for page in created {
        kids.push(Object::Ref(*page));
    }
    for value in existing.get(split..).unwrap_or_default() {
        kids.push(value.clone());
    }

    let count = i64::try_from(kids.len()).unwrap_or(i64::MAX);
    set(&mut node, names::KIDS, Object::Array(kids));
    set(&mut node, names::COUNT, Object::Int(count));
    dest.replace(node_ref, Object::Dict(node));
}

/// Write a filtered `/ViewerPreferences` onto the destination catalog,
/// replacing whatever was there.
fn copy_viewer_preferences(dest: &mut EditDoc<'_>, prefs: Dict) {
    let Some(catalog_ref) = dest.base().trailer().reference(names::ROOT) else {
        return;
    };
    let Some(mut catalog) = dest
        .fetch(catalog_ref)
        .ok()
        .and_then(|o| o.as_dict().cloned())
    else {
        return;
    };
    // Written as a **direct** dictionary, and replacing unconditionally.
    set(&mut catalog, names::VIEWER_PREFERENCES, Object::Dict(prefs));
    dest.replace(catalog_ref, Object::Dict(catalog));
}

/// Impose `pages` from `src` onto sheets in `dest`.
///
/// Each source page becomes a Form `XObject` carrying only its `/Resources` —
/// `/Annots`, `/Group`, `/CropBox` and everything else is dropped — and each
/// sheet's content stream invokes the forms in slot order.
///
/// A source page used on two different sheets produces **one** form and two
/// invocations, and the name is registered in both sheets' resources
/// (divergence D16: the C++ registers it only on the first, so the sub-page
/// silently renders blank on every later sheet).
///
/// # Errors
///
/// [`Error::BadNupParams`] for a zero grid dimension or a zero sheet
/// dimension, [`Error::NoDestinationCatalog`], and
/// [`Error::PageIndexOutOfRange`].
pub fn n_page_to_one(
    dest: &mut EditDoc<'_>,
    src: &Document,
    pages: &PageRange,
    opts: &NUpOptions,
) -> Result<(), Error> {
    let (x, y) = opts.grid;
    let (width, height) = opts.sheet;
    if x == 0 || y == 0 || width <= 0.0 || height <= 0.0 {
        return Err(Error::BadNupParams);
    }
    for index in pages.indices() {
        if index.get() >= src.page_count() {
            return Err(Error::PageIndexOutOfRange(*index));
        }
    }

    let pages_node = init_dest(dest)?;
    let grid = NupGrid {
        sheet_width: width,
        sheet_height: height,
        x,
        y,
    };

    let mut map = ObjectMap::new();
    // Source page number to the form that was made from it, so a page used
    // twice makes one form.
    let mut forms: Vec<(u32, ObjRef)> = Vec::new();
    let mut created = Vec::new();

    for chunk in pages.indices().chunks(grid.per_sheet().max(1) as usize) {
        let mut content = String::new();
        let mut xobjects = Dict::new();

        for (slot, index) in chunk.iter().enumerate() {
            let page = src
                .page(*index)
                .map_err(|_| Error::PageIndexOutOfRange(*index))?;
            let key = page.reference.map_or(u32::MAX - index.get(), |r| r.num);

            let form = if let Some((_, existing)) = forms.iter().find(|(k, _)| *k == key) {
                *existing
            } else {
                let made = make_form(dest, src, &page.dict, pages_node, &mut map);
                forms.push((key, made));
                made
            };

            let (page_w, page_h) = page_size(&page.dict, src);
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a slot index is bounded by the grid, which is a u32"
            )]
            let edit = grid.edit(slot as u32, page_w, page_h);
            let name = format!("X{}", xobjects.len() + 1);
            content.push_str(&sub_page_fragment(&name, edit));
            // Registered on *every* sheet the form appears on, not just the
            // first — D16's fix.
            xobjects.push(Name::from(name.as_str()), Object::Ref(form));
        }

        created.push(make_sheet(
            dest, pages_node, &content, xobjects, width, height,
        ));
    }

    insert_into_tree(dest, pages_node, &created, 0);
    Ok(())
}

/// One source page as a Form `XObject`.
fn make_form(
    dest: &mut EditDoc<'_>,
    src: &Document,
    page: &Dict,
    pages_node: u32,
    map: &mut ObjectMap,
) -> ObjRef {
    // Content is **decoded** here — the one place in the import path that
    // re-encodes — because an array-valued `/Contents` may split a token
    // across elements, and the join needs a separator between every pair.
    let content = assemble_content(page, src);

    let mut resources = Dict::new();
    let mut carrier = Dict::new();
    if copy_inherited(
        dest,
        src,
        page,
        &mut carrier,
        names::RESOURCES,
        map,
        pages_node,
    ) && let Some(found) = carrier.raw(names::RESOURCES).and_then(Object::as_dict)
    {
        resources = found.clone();
    }

    let (media, crop) = boxes(page, src);
    let bbox = intersect(media, crop);

    // `/Type`, `/Subtype` and `/FormType` are written **after** everything
    // else, so the reference walk above saw a `/Type`-less dictionary and did
    // not trip the copier's Pages/Page special cases.
    let dict = Dict::from_pairs([
        (names::RESOURCES.clone(), Object::Dict(resources)),
        (
            edit_names::BBOX.clone(),
            Object::Array(Array::of(bbox.map(Object::Real))),
        ),
        (
            edit_names::MATRIX.clone(),
            Object::Array(Array::of(
                [1.0, 0.0, 0.0, 1.0, -bbox[0], -bbox[1]].map(Object::Real),
            )),
        ),
        (
            names::TYPE.clone(),
            Object::Name(edit_names::XOBJECT.clone()),
        ),
        (
            names::SUBTYPE.clone(),
            Object::Name(edit_names::FORM.clone()),
        ),
        (edit_names::FORM_TYPE.clone(), Object::Int(1)),
        (
            names::LENGTH.clone(),
            Object::Int(i64::try_from(content.len()).unwrap_or(0)),
        ),
    ]);

    dest.add(Object::Stream(Box::new(pdfrum_object::Stream::new(
        dict,
        pdfrum_object::ByteSpan::from(content),
    ))))
}

/// One output sheet.
fn make_sheet(
    dest: &mut EditDoc<'_>,
    pages_node: u32,
    content: &str,
    xobjects: Dict,
    width: f32,
    height: f32,
) -> ObjRef {
    let stream = dest.add(Object::Stream(Box::new(pdfrum_object::Stream::new(
        Dict::from_pairs([(
            names::LENGTH.clone(),
            Object::Int(i64::try_from(content.len()).unwrap_or(0)),
        )]),
        pdfrum_object::ByteSpan::from(content.as_bytes().to_vec()),
    ))));

    dest.add(Object::Dict(Dict::from_pairs([
        (names::TYPE.clone(), Object::Name(names::PAGE.clone())),
        (
            names::PARENT.clone(),
            Object::Ref(ObjRef::new(pages_node, 0)),
        ),
        (
            names::MEDIA_BOX.clone(),
            Object::Array(Array::of([
                Object::Int(0),
                Object::Int(0),
                Object::Real(width),
                Object::Real(height),
            ])),
        ),
        (
            names::RESOURCES.clone(),
            Object::Dict(Dict::from_pairs([(
                edit_names::XOBJECT.clone(),
                Object::Dict(xobjects),
            )])),
        ),
        (names::CONTENTS.clone(), Object::Ref(stream)),
    ])))
}

/// A page's `/Contents`, decoded and joined with a newline after **every**
/// element — including the last, which is what stops a token split across two
/// array elements running into whatever follows.
fn assemble_content(page: &Dict, src: &Document) -> Vec<u8> {
    let limits = pdfrum_common::Limits::default();
    let mut diags = pdfrum_common::Diagnostics::default();
    let mut out = Vec::new();

    match page.get(names::CONTENTS, src).as_deref() {
        Some(Object::Stream(s)) => {
            out = pdfrum_parser::decoded_stream(s, src, &limits, &mut diags);
        }
        Some(Object::Array(a)) => {
            for i in 0..a.len() {
                let Some(s) = a.stream_at(i, src) else {
                    continue;
                };
                out.extend_from_slice(&pdfrum_parser::decoded_stream(&s, src, &limits, &mut diags));
                out.push(b'\n');
            }
        }
        // No contents at all is an empty form, not a failure.
        _ => {}
    }
    out
}

/// A page's media and crop boxes, defaulted and normalized.
fn boxes(page: &Dict, src: &Document) -> ([f32; 4], Option<[f32; 4]>) {
    let read = |key: &Name| -> Option<[f32; 4]> {
        let a = match inheritable(page, key, src) {
            Some(Object::Array(a)) => a,
            _ => page.array(key, src)?,
        };
        if a.len() < 4 {
            return None;
        }
        Some(normalize([
            a.number_at_or_zero(0),
            a.number_at_or_zero(1),
            a.number_at_or_zero(2),
            a.number_at_or_zero(3),
        ]))
    };
    let media = read(names::MEDIA_BOX).unwrap_or([0.0, 0.0, 612.0, 792.0]);
    (media, read(names::CROP_BOX))
}

/// A box with its corners in ascending order.
fn normalize(b: [f32; 4]) -> [f32; 4] {
    [
        b[0].min(b[2]),
        b[1].min(b[3]),
        b[0].max(b[2]),
        b[1].max(b[3]),
    ]
}

/// The crop box intersected with the media box, or the media box alone.
fn intersect(media: [f32; 4], crop: Option<[f32; 4]>) -> [f32; 4] {
    let Some(crop) = crop else {
        return media;
    };
    [
        media[0].max(crop[0]),
        media[1].max(crop[1]),
        media[2].min(crop[2]),
        media[3].min(crop[3]),
    ]
}

/// A page's visible size, which is what an N-up slot scales to fit.
fn page_size(page: &Dict, src: &Document) -> (f32, f32) {
    let (media, crop) = boxes(page, src);
    let b = intersect(media, crop);
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    // A quarter turn swaps the visible dimensions.
    let quarter = matches!(
        inheritable(page, names::ROTATE, src)
            .or_else(|| page.raw(names::ROTATE).cloned())
            .and_then(|o| o.as_int())
            .map(|v| ((v / 90) % 4 + 4) % 4),
        Some(1 | 3)
    );
    if quarter { (h, w) } else { (w, h) }
}

/// Set a key, replacing in place so the emitted key order does not shuffle.
fn set(dict: &mut Dict, key: &Name, value: Object) {
    if dict.contains_key(key) {
        *dict = Dict::from_pairs(dict.iter().map(|(k, v)| {
            if k == key {
                (k.clone(), value.clone())
            } else {
                (k.clone(), v.clone())
            }
        }));
    } else {
        dict.push(key.clone(), value);
    }
}
