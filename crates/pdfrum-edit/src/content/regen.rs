//! Rewriting a mutated page's `/Contents` (ISO 32000-1 §7.8.2).
//!
//! [`crate::content::emit`] turns one page object into operator bytes. This
//! module is everything around that: deciding which `//Contents` elements have
//! to be written again, framing each one, naming the resources the objects
//! refer to, and reshaping the `/Contents` entry itself when elements are
//! added or emptied.
//!
//! # A page nobody touched is not rewritten at all
//!
//! [`regenerate`] returns `None` when no stream is dirty, and the caller then
//! writes nothing — not the streams, not the resource dictionary. That is the
//! early-out the whole save path depends on: an ordinary save of an
//! unmodified document must leave every page's bytes exactly as they were, and
//! a regeneration is lossy enough that doing it speculatively would be
//! destructive.
//!
//! # Each dirty stream gets a frame, and the frame carries the transform
//!
//! A stream is rewritten whole, so it must begin from a state it can state:
//!
//! ```text
//! q
//! [ <inverse of the transform this stream inherited> cm ]
//! 0 0 0 RG 0 0 0 rg 1 w 0 J 0 j
//! /<default gs> gs
//! …objects…
//! [ EMC per still-open mark ]
//! Q
//! [ <the transform change this stream passes on> cm ]
//! ```
//!
//! The two `cm`s are the awkward part and they are not optional. A content
//! stream may leave the transform changed for the streams after it — an
//! unbalanced `q`/`cm` across an element boundary is legal and real files do
//! it — so a rewritten stream has to undo what it inherited before stating its
//! own state, and then restate what the streams after it were relying on.
//! Without the second, rewriting element 0 silently moves everything in
//! element 1.
//!
//! # An empty stream is a deletion, unless it still moves the transform
//!
//! A stream that produced no objects has its buffer cleared, and a cleared
//! buffer is the signal to drop that `/Contents` element. But a stream that
//! produced nothing and *does* change the transform keeps its whole frame:
//! deleting it would leave the streams after it drawing under the wrong
//! matrix. This is why "empty" and "removable" are two different questions.
//!
//! # Objects are visited once, in painting order, writing into several buffers
//!
//! The buffers for the dirty streams are all open at once and the single walk
//! over the page's objects appends each object to whichever one it belongs to.
//! Objects in clean streams are skipped, and so are inactive ones — an
//! inactive object's stream is regenerated *without* it, which is exactly how
//! it disappears.

use std::collections::{BTreeMap, BTreeSet};

use pdfrum_common::kurbo::Affine;
use pdfrum_object::{Dict, Name, ObjRef, Object, Resolve};
use pdfrum_page::{Page, PageObject};

use crate::content::emit::{DEFAULT_GRAPHICS, GraphicsKey, ResourceNames, default_graphics};
use crate::content::marks::{emit_mark_diff, finish_marks};
use crate::content::num::write_matrix;
use crate::content::resource::ResourceTable;

/// One regenerated `/Contents` element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Regenerated {
    /// Which element this is, or `None` for one that did not exist before.
    pub stream: Option<usize>,
    /// The bytes. **Empty means delete this element**, not "write an empty
    /// stream".
    pub bytes: String,
}

/// Everything a save has to do to a page whose objects were edited.
#[derive(Debug, Clone, PartialEq)]
pub struct PageRewrite {
    /// The elements to write, in ascending index order with the streamless
    /// one first.
    pub streams: Vec<Regenerated>,
    /// The page's `/Resources`, with the three maintained categories swept and
    /// every other key carried through.
    pub resources: Dict,
}

/// Rewrite the dirty content streams of `page`, or `None` when none is dirty.
///
/// `resources` is the page's own `/Resources`, which supplies the names
/// already in use so a regenerated stream reuses them rather than minting
/// duplicates.
#[must_use]
pub fn regenerate(page: &Page, resources: &Dict, r: &impl Resolve) -> Option<PageRewrite> {
    let dirty = page.dirty_stream_set();
    if dirty.is_empty() {
        return None;
    }

    let mut table = ResourceTable::load(resources, r);
    // The prologue's `/ExtGState` is realized before anything else, so it
    // takes the lowest free name and is never swept away.
    let default_gs = table.realize_dict("ExtGState", &default_graphics());

    let mut buffers: BTreeMap<Option<usize>, StreamBuffer> = dirty
        .iter()
        .map(|stream| {
            let mut buffer = StreamBuffer::default();
            open_frame(
                &mut buffer.bytes,
                page.ctm_at_start_of_stream(*stream),
                &default_gs,
            );
            (*stream, buffer)
        })
        .collect();

    let mut used: BTreeMap<String, BTreeSet<Name>> = BTreeMap::new();
    used.entry("ExtGState".to_owned())
        .or_default()
        .insert(default_gs.clone());

    for object in &page.objects {
        if !object.is_active() {
            continue;
        }
        // Every active object's resources are recorded, not just those of the
        // objects being written: an object in a stream this run leaves alone
        // still refers to its font, and sweeping that font away would break a
        // stream nobody asked to change.
        //
        // The two cases differ in whether a *name may be minted*. An object
        // being written needs one and gets one; an object in a clean stream
        // refers to its resource by whatever name that stream already spells,
        // so it can only record the name the dictionary already holds. Minting
        // one for it would add an entry nothing refers to.
        let writing = buffers.contains_key(&object.content_stream());
        let names = realize_for(object, &mut table, &mut used, writing);
        let Some(buffer) = buffers.get_mut(&object.content_stream()) else {
            continue;
        };
        let marks = std::mem::take(&mut buffer.marks);
        let mut body = String::new();
        let open = emit_mark_diff(&mut body, &marks, object.marks(), &|_| None);
        if crate::content::emit::emit_object(&mut body, object, &names) {
            buffer.bytes.push_str(&body);
            buffer.open_marks = open;
            buffer.marks = object.marks().clone();
            buffer.wrote_something = true;
        } else {
            // The object could not be expressed, so its marks were not opened
            // either: leave the buffer's mark state where it was.
            buffer.marks = marks;
        }
    }

    let streams = buffers
        .into_iter()
        .map(|(stream, buffer)| close_frame(page, stream, buffer))
        .collect();

    table.sweep(&used);
    Some(PageRewrite {
        streams,
        resources: table.to_dict(resources),
    })
}

/// One stream's bytes plus the mark state the next object diffs against.
#[derive(Debug, Default)]
struct StreamBuffer {
    bytes: String,
    marks: pdfrum_page::ContentMarks,
    open_marks: usize,
    wrote_something: bool,
}

/// The per-stream prologue: save, undo the inherited transform, state every
/// default.
fn open_frame(out: &mut String, inherited: Affine, default_gs: &Name) {
    out.push_str("q\n");
    if inherited != Affine::IDENTITY {
        write_matrix(out, inherited.inverse());
        out.push_str(" cm\n");
    }
    out.push_str(DEFAULT_GRAPHICS);
    out.push('/');
    out.push_str(&String::from_utf8_lossy(&pdfrum_object::name_encode(
        default_gs.as_bytes(),
    )));
    out.push_str(" gs ");
}

/// The per-stream epilogue, and the decision whether the element survives.
fn close_frame(page: &Page, stream: Option<usize>, mut buffer: StreamBuffer) -> Regenerated {
    let affects_ctm = stream_affects_ctm(page, stream);

    // A stream that drew nothing and passes nothing on is deleted. One that
    // drew nothing but still moves the transform keeps its whole frame,
    // because the streams after it are relying on the move.
    if !buffer.wrote_something && !affects_ctm {
        return Regenerated {
            stream,
            bytes: String::new(),
        };
    }

    if buffer.wrote_something {
        finish_marks(&mut buffer.bytes, buffer.open_marks);
    }
    buffer.bytes.push_str("Q\n");

    // `affects_ctm` is false for a streamless element, so this only runs
    // where `stream` is a real index.
    if let Some(index) = stream.filter(|_| affects_ctm) {
        let previous = previous_ctm(page, index);
        let difference = previous.inverse() * page.ctm_at_end_of_stream(index);
        if difference != Affine::IDENTITY {
            write_matrix(&mut buffer.bytes, difference);
            buffer.bytes.push_str(" cm\n");
        }
    }

    Regenerated {
        stream,
        bytes: buffer.bytes,
    }
}

/// The transform in force where `stream` began, as the epilogue reckons it.
fn previous_ctm(page: &Page, stream: usize) -> Affine {
    if stream == 0 {
        Affine::IDENTITY
    } else {
        page.ctm_at_end_of_stream(stream.saturating_sub(1))
    }
}

/// Whether rewriting `stream` changes what the streams after it inherit.
///
/// A streamless element is appended after everything, so nothing follows it
/// and it can never affect anything.
fn stream_affects_ctm(page: &Page, stream: Option<usize>) -> bool {
    let Some(stream) = stream else {
        return false;
    };
    previous_ctm(page, stream) != page.ctm_at_end_of_stream(stream)
}

/// Record — and, when `writing`, allocate — the resource names one object
/// needs.
fn realize_for(
    object: &PageObject,
    table: &mut ResourceTable,
    used: &mut BTreeMap<String, BTreeSet<Name>>,
    writing: bool,
) -> ResourceNames {
    let mut names = ResourceNames::default();

    if let Some(key) = GraphicsKey::of(object.state()) {
        let dict = key.to_dict();
        let name = if writing {
            Some(table.realize_dict("ExtGState", &dict))
        } else {
            table.name_of_dict("ExtGState", &dict)
        };
        names.ext_gstate = record(used, "ExtGState", name);
    }

    match object {
        PageObject::Text(text) => {
            if let Some(source) = text.object.font_source {
                let name = named(table, "Font", source, writing);
                names.font = record(used, "Font", name);
            }
        }
        PageObject::Image(image) => {
            if let Some(source) = image.object.source {
                let name = named(table, "XObject", source, writing);
                names.xobject = record(used, "XObject", name);
            }
        }
        PageObject::Form(form) => {
            if let Some(source) = form.object.source {
                let name = named(table, "XObject", source, writing);
                names.xobject = record(used, "XObject", name);
            }
        }
        // A path needs no named resource beyond its graphics state, and a
        // shading object is never written at all.
        PageObject::Path(_) | PageObject::Shading(_) => {}
    }
    names
}

/// The name `source` goes by in `category`, minting one only when the stream
/// naming it is being written.
fn named(table: &mut ResourceTable, category: &str, source: ObjRef, writing: bool) -> Option<Name> {
    if writing {
        Some(table.realize(category, source))
    } else {
        table.name_of(category, source)
    }
}

/// Note `name` as still in use, and hand it back.
fn record(
    used: &mut BTreeMap<String, BTreeSet<Name>>,
    category: &str,
    name: Option<Name>,
) -> Option<Name> {
    let name = name?;
    used.entry(category.to_owned())
        .or_default()
        .insert(name.clone());
    Some(name)
}

/// Where a regenerated element lands in the page's `/Contents`.
///
/// The `/Contents` entry is a stream, an array of streams, or absent, and
/// adding or removing an element moves it between those shapes. The rules are
/// not symmetric, which is the point of naming them:
///
/// - **absent** gaining an element becomes a lone stream at index 0;
/// - **a lone stream** gaining a second becomes an array `[old new]`, and the
///   new one is index 1;
/// - **an array** gaining one appends, at `len - 1`;
/// - **a lone stream** losing index 0 loses the `/Contents` key entirely;
/// - **an array** losing elements keeps being an array — *even down to one
///   element*, and even down to none. Collapsing a one-element array back to a
///   bare stream would be tidier and is deliberately not done: a second stream
///   may well be added next, and every object's recorded index would have to
///   move again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentsShape {
    /// No `/Contents` at all.
    Absent,
    /// One stream, reached through `/Contents` directly.
    Single(ObjRef),
    /// An array of stream references.
    Array(Vec<ObjRef>),
}

impl ContentsShape {
    /// Read the shape out of a page dictionary.
    ///
    /// Anything that is neither a stream nor an array of streams — a dangling
    /// reference, a number, a name — reads as [`ContentsShape::Absent`], which
    /// is how a page with unusable contents behaves everywhere else.
    #[must_use]
    pub fn read(page_dict: &Dict, r: &impl Resolve) -> Self {
        let Some(contents) = page_dict.raw(pdfrum_object::names::CONTENTS) else {
            return Self::Absent;
        };
        // An array may be written inline or reached through a reference.
        let direct = match contents {
            Object::Ref(reference) => match r.fetch(*reference) {
                Ok(object) => (*object).clone(),
                Err(_) => return Self::Absent,
            },
            other => other.clone(),
        };
        match direct {
            Object::Stream(_) => match contents {
                Object::Ref(reference) => Self::Single(*reference),
                // A stream written inline in the page dictionary is not
                // something a PDF can express, so there is nothing to name.
                _ => Self::Absent,
            },
            Object::Array(array) => {
                Self::Array(array.iter().filter_map(Object::as_ref_id).collect())
            }
            _ => Self::Absent,
        }
    }

    /// The index a newly added element takes, and the shape afterwards.
    #[must_use]
    pub fn with_added(&self, added: ObjRef) -> (usize, Self) {
        match self {
            Self::Absent => (0, Self::Single(added)),
            Self::Single(existing) => (1, Self::Array(vec![*existing, added])),
            Self::Array(elements) => {
                let mut next = elements.clone();
                next.push(added);
                (next.len().saturating_sub(1), Self::Array(next))
            }
        }
    }

    /// The shape after `removed` elements are dropped, and the map from each
    /// surviving element's old index to its new one.
    ///
    /// Every object whose index is *not* in the map — one whose own element
    /// was removed, and one that was still streamless — collapses to index 0.
    /// That is the C++'s default-inserting map read literally, and it is
    /// deliberate: those objects were not written by this regeneration and
    /// their recorded index has to point somewhere.
    ///
    /// Both halves of the map are `usize`: a `/Contents` index is a position
    /// in an array, and there is no negative sentinel.
    #[must_use]
    pub fn with_removed(&self, removed: &BTreeSet<usize>) -> (Self, BTreeMap<usize, usize>) {
        match self {
            Self::Absent => (Self::Absent, BTreeMap::new()),
            Self::Single(_) => {
                let shape = if removed.contains(&0) {
                    // The whole key goes, rather than becoming an empty
                    // stream.
                    Self::Absent
                } else {
                    self.clone()
                };
                (shape, BTreeMap::new())
            }
            Self::Array(elements) => {
                let mut mapping = BTreeMap::new();
                let mut kept = Vec::new();
                for (old, element) in elements.iter().enumerate() {
                    if removed.contains(&old) {
                        continue;
                    }
                    let new = kept.len();
                    kept.push(*element);
                    mapping.insert(old, new);
                }
                // Still an array, whatever is left of it.
                (Self::Array(kept), mapping)
            }
        }
    }

    /// The `/Contents` value this shape writes, given the object number a
    /// fresh array would take.
    ///
    /// `None` for [`ContentsShape::Absent`], which removes the key.
    #[must_use]
    pub fn to_object(&self, array_ref: Option<ObjRef>) -> Option<Object> {
        match self {
            Self::Absent => None,
            Self::Single(reference) => Some(Object::Ref(*reference)),
            Self::Array(elements) => {
                let array = pdfrum_object::Array::of(elements.iter().map(|e| Object::Ref(*e)));
                Some(match array_ref {
                    Some(reference) => {
                        let _ = &array;
                        Object::Ref(reference)
                    }
                    None => Object::Array(array),
                })
            }
        }
    }

    /// The element references, in order.
    #[must_use]
    pub fn elements(&self) -> Vec<ObjRef> {
        match self {
            Self::Absent => Vec::new(),
            Self::Single(reference) => vec![*reference],
            Self::Array(elements) => elements.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "test fixtures index collections whose length the fixture fixes"
    )]

    use super::{ContentsShape, Regenerated, regenerate};
    use pdfrum_common::kurbo::{Affine, BezPath};
    use pdfrum_object::{Dict, Name, NoResolve, ObjRef, Object};
    use pdfrum_page::{Content, FillRule, Page, PageObject, PathObject};
    use pdfrum_page::{ContentMarks, GraphicsState};
    use std::collections::{BTreeMap, BTreeSet};

    fn path(stream: usize, dirty: bool) -> PageObject {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((1.0, 0.0));
        p.line_to((1.0, 1.0));
        p.close_path();
        PageObject::Path(Box::new(Content {
            object: PathObject {
                path: p,
                matrix: Affine::IDENTITY,
                fill_rule: FillRule::Winding,
                stroke: false,
            },
            state: GraphicsState::default(),
            marks: ContentMarks::new(),
            content_stream: Some(stream),
            dirty,
            active: true,
        }))
    }

    fn page_of(objects: Vec<PageObject>) -> Page {
        Page {
            objects,
            ..Page::empty()
        }
    }

    // `GenerateContent` (:336-338) — the early-out. An untouched page is not
    // rewritten at all, which is what keeps an ordinary save byte-identical.
    #[test]
    fn an_untouched_page_regenerates_nothing() {
        let page = page_of(vec![path(0, false), path(0, false)]);
        assert!(regenerate(&page, &Dict::new(), &NoResolve).is_none());
    }

    // The per-stream frame, verbatim.
    #[test]
    fn a_dirty_stream_gets_the_whole_frame() {
        let page = page_of(vec![path(0, true)]);
        let rewrite = regenerate(&page, &Dict::new(), &NoResolve).expect("dirty");
        assert_eq!(rewrite.streams.len(), 1);
        let bytes = &rewrite.streams[0].bytes;
        assert!(
            bytes.starts_with("q\n0 0 0 RG 0 0 0 rg 1 w 0 J 0 j\n/FXE1 gs "),
            "got {bytes}"
        );
        assert!(bytes.ends_with("Q\n"), "got {bytes}");
        // The default graphics state is in the resources, under the name the
        // prologue used.
        let Some(Object::Dict(gs)) = rewrite.resources.raw(&Name::from("ExtGState")) else {
            panic!("no ExtGState");
        };
        assert!(gs.contains_key(&Name::from("FXE1")));
    }

    // A clean stream beside a dirty one is left alone entirely.
    #[test]
    fn only_dirty_streams_are_written() {
        let page = page_of(vec![path(0, false), path(1, true)]);
        let rewrite = regenerate(&page, &Dict::new(), &NoResolve).expect("dirty");
        assert_eq!(rewrite.streams.len(), 1);
        assert_eq!(rewrite.streams[0].stream, Some(1));
    }

    // An object in a dirty stream is written even though it is itself clean —
    // the stream is rewritten whole, so everything in it has to be restated.
    #[test]
    fn a_clean_object_in_a_dirty_stream_is_still_written() {
        let mut page = page_of(vec![path(0, false), path(0, true)]);
        page.objects[0].mark_clean();
        let rewrite = regenerate(&page, &Dict::new(), &NoResolve).expect("dirty");
        let bytes = &rewrite.streams[0].bytes;
        assert_eq!(bytes.matches(" f Q\n").count(), 2, "got {bytes}");
    }

    // `SetIsActive` (fpdf_editpage_embeddertest.cpp:490): an inactive object's
    // stream is regenerated *without* it, which is how it disappears.
    #[test]
    fn an_inactive_object_is_left_out_of_its_regenerated_stream() {
        let mut page = page_of(vec![path(0, false), path(0, false)]);
        page.objects[1].set_active(false);
        let rewrite = regenerate(&page, &Dict::new(), &NoResolve).expect("dirty");
        let bytes = &rewrite.streams[0].bytes;
        assert_eq!(bytes.matches(" f Q\n").count(), 1, "got {bytes}");
    }

    // `Bug378120423` (fpdf_editpage_embeddertest.cpp:563): deactivating the
    // only object empties the stream, and an empty stream is a deletion.
    #[test]
    fn a_stream_left_with_nothing_comes_back_empty() {
        let mut page = page_of(vec![path(0, false)]);
        page.objects[0].set_active(false);
        let rewrite = regenerate(&page, &Dict::new(), &NoResolve).expect("dirty");
        assert_eq!(
            rewrite.streams,
            vec![Regenerated {
                stream: Some(0),
                bytes: String::new()
            }]
        );
    }

    // The removal case: nothing is left to name the stream, so the page's own
    // set is what says it must be written.
    #[test]
    fn a_removal_regenerates_the_stream_it_emptied() {
        let mut page = page_of(vec![path(0, false), path(1, false)]);
        assert!(page.remove_object(1).is_some());
        let rewrite = regenerate(&page, &Dict::new(), &NoResolve).expect("dirty");
        assert_eq!(rewrite.streams.len(), 1);
        assert_eq!(rewrite.streams[0].stream, Some(1));
        assert!(rewrite.streams[0].bytes.is_empty());
    }

    // A stream inheriting a transform undoes it, and restates what it passes
    // on. Both `cm`s, or element 1 draws in the wrong place.
    #[test]
    fn an_inherited_transform_is_undone_and_restated() {
        let mut page = page_of(vec![path(1, true)]);
        page.stream_ctms.insert(0, Affine::scale(2.0));
        page.stream_ctms.insert(1, Affine::scale(2.0));
        let rewrite = regenerate(&page, &Dict::new(), &NoResolve).expect("dirty");
        let bytes = &rewrite.streams[0].bytes;
        // The inverse of the scale, undone at the top.
        assert!(bytes.starts_with("q\n.5 0 0 .5 0 0 cm\n"), "got {bytes}");
        // Stream 1 ends where it began, so it passes nothing on.
        assert!(bytes.ends_with("Q\n"), "got {bytes}");
    }

    #[test]
    fn a_stream_that_moves_the_transform_restates_the_move() {
        let mut page = page_of(vec![path(0, true)]);
        page.stream_ctms.insert(0, Affine::scale(3.0));
        let rewrite = regenerate(&page, &Dict::new(), &NoResolve).expect("dirty");
        let bytes = &rewrite.streams[0].bytes;
        assert!(
            bytes.starts_with("q\n"),
            "no inverse: stream 0 inherits none"
        );
        assert!(bytes.ends_with("Q\n3 0 0 3 0 0 cm\n"), "got {bytes}");
    }

    // An empty stream that still moves the transform keeps its frame: deleting
    // it would move everything after it.
    #[test]
    fn an_empty_stream_that_moves_the_transform_survives() {
        let mut page = page_of(vec![path(1, true)]);
        page.dirty_streams.insert(Some(0));
        page.stream_ctms.insert(0, Affine::scale(2.0));
        let rewrite = regenerate(&page, &Dict::new(), &NoResolve).expect("dirty");
        let zero = rewrite
            .streams
            .iter()
            .find(|s| s.stream == Some(0))
            .expect("stream 0");
        assert!(!zero.bytes.is_empty(), "it moves the transform");
        assert!(
            zero.bytes.ends_with("2 0 0 2 0 0 cm\n"),
            "got {}",
            zero.bytes
        );
    }

    // A streamless object sorts first (`None < Some(0)`), so a brand-new
    // object is written before any existing stream is rewritten.
    #[test]
    fn a_streamless_object_is_written_first() {
        let mut page = page_of(vec![path(0, true)]);
        page.push_object(path(0, false));
        let rewrite = regenerate(&page, &Dict::new(), &NoResolve).expect("dirty");
        let order: Vec<Option<usize>> = rewrite.streams.iter().map(|s| s.stream).collect();
        assert_eq!(order, vec![None, Some(0)]);
    }

    // ---- `/Contents` shape transitions (cpdf_pagecontentmanager.cpp) ----

    #[test]
    fn nothing_gaining_a_stream_becomes_a_lone_stream_at_zero() {
        let (index, shape) = ContentsShape::Absent.with_added(ObjRef::new(5, 0));
        assert_eq!(index, 0);
        assert_eq!(shape, ContentsShape::Single(ObjRef::new(5, 0)));
    }

    #[test]
    fn a_lone_stream_gaining_a_second_becomes_an_array_and_the_new_one_is_one() {
        let shape = ContentsShape::Single(ObjRef::new(4, 0));
        let (index, next) = shape.with_added(ObjRef::new(9, 0));
        assert_eq!(index, 1);
        assert_eq!(
            next,
            ContentsShape::Array(vec![ObjRef::new(4, 0), ObjRef::new(9, 0)])
        );
    }

    #[test]
    fn an_array_gaining_one_appends_at_the_end() {
        let shape = ContentsShape::Array(vec![ObjRef::new(1, 0), ObjRef::new(2, 0)]);
        let (index, next) = shape.with_added(ObjRef::new(3, 0));
        assert_eq!(index, 2);
        assert_eq!(next.elements().len(), 3);
    }

    // A single stream losing index 0 loses the whole key.
    #[test]
    fn a_lone_stream_removed_leaves_no_contents_key_at_all() {
        let shape = ContentsShape::Single(ObjRef::new(4, 0));
        let (next, mapping) = shape.with_removed(&[0].into_iter().collect());
        assert_eq!(next, ContentsShape::Absent);
        assert!(mapping.is_empty());
        assert_eq!(next.to_object(None), None);
    }

    // `RemoveAllFromStream` (fpdf_edit_embeddertest.cpp:2033): removing
    // element 1 of three shifts element 2 down to 1.
    #[test]
    fn removing_an_array_element_shifts_the_ones_after_it_down() {
        let shape = ContentsShape::Array(vec![
            ObjRef::new(1, 0),
            ObjRef::new(2, 0),
            ObjRef::new(3, 0),
        ]);
        let (next, mapping) = shape.with_removed(&[1].into_iter().collect());
        assert_eq!(
            next,
            ContentsShape::Array(vec![ObjRef::new(1, 0), ObjRef::new(3, 0)])
        );
        assert_eq!(mapping, BTreeMap::from([(0, 0), (2, 1)]));
    }

    // An array stays an array even at one element, and even at none.
    #[test]
    fn an_array_is_never_collapsed_back_to_a_bare_stream() {
        let shape = ContentsShape::Array(vec![ObjRef::new(1, 0), ObjRef::new(2, 0)]);
        let (next, _) = shape.with_removed(&[1].into_iter().collect());
        assert!(matches!(next, ContentsShape::Array(ref e) if e.len() == 1));
        let (empty, _) = next.with_removed(&[0].into_iter().collect());
        assert_eq!(empty, ContentsShape::Array(Vec::new()));
    }

    #[test]
    fn removing_several_elements_renumbers_the_survivors_in_one_pass() {
        let shape = ContentsShape::Array((1..=5).map(|n| ObjRef::new(n, 0)).collect());
        let removed: BTreeSet<usize> = [0, 3].into_iter().collect();
        let (next, mapping) = shape.with_removed(&removed);
        assert_eq!(next.elements().len(), 3);
        assert_eq!(mapping, BTreeMap::from([(1, 0), (2, 1), (4, 2)]));
    }

    // Reading the shape out of a page dictionary.
    #[test]
    fn a_page_with_no_contents_reads_as_absent() {
        assert_eq!(
            ContentsShape::read(&Dict::new(), &NoResolve),
            ContentsShape::Absent
        );
        // A `/Contents` naming something that is neither stream nor array is
        // absent too.
        let odd = Dict::from_pairs([(pdfrum_object::names::CONTENTS.clone(), Object::Int(7))]);
        assert_eq!(ContentsShape::read(&odd, &NoResolve), ContentsShape::Absent);
    }

    #[test]
    fn an_inline_array_of_references_reads_as_an_array() {
        let array = pdfrum_object::Array::of([
            Object::Ref(ObjRef::new(2, 0)),
            Object::Ref(ObjRef::new(3, 0)),
        ]);
        let dict =
            Dict::from_pairs([(pdfrum_object::names::CONTENTS.clone(), Object::Array(array))]);
        assert_eq!(
            ContentsShape::read(&dict, &NoResolve),
            ContentsShape::Array(vec![ObjRef::new(2, 0), ObjRef::new(3, 0)])
        );
    }
}
