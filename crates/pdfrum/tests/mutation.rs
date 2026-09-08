//! Page mutation, over the oracle's own fixtures.
//!
//! These port the behavioral assertions of PDFium's `FPDFPage_*` /
//! `FPDFPageObj_*` embedder tests — restated over our types rather than
//! transliterated, and asserting the same *facts*: how many objects a page has
//! after an edit, which `/Contents` element each one belongs to, and what
//! survives a save and a reload.
//!
//! The three-phase shape is the tests' own and is what makes them worth
//! porting. An edit changes the in-memory graph; a save turns it into bytes;
//! a reload reads those bytes back. Each phase can be right while the next is
//! wrong, so each is asserted separately — an object count that is correct in
//! memory and wrong after a reload is the bug these tests exist to not have.

// `clippy.toml`'s `allow-expect-in-tests` covers `#[test]` bodies but not the
// `round_trip` helper these tests factor themselves into — where a save that
// fails *is* the failure signal. The indexing allow is the same bargain: the
// fixtures' object counts are asserted a line above every index.
#![allow(clippy::expect_used, clippy::indexing_slicing)]

use pdfrum::{Color, Document, PageEdit, PathBuilder, Rect, SaveOptions, Update, VelloCpuBackend};

const HELLO: &str = "tests/fixtures/hello_world.pdf";
/// Nineteen objects across three `/Contents` elements: 15 in element 0, 3 in
/// element 1, 1 in element 2.
const SPLIT: &str = "tests/fixtures/split_streams.pdf";
/// Three text objects across two `/Contents` elements: 2 in element 0, 1 in
/// element 1.
const HELLO_SPLIT: &str = "tests/fixtures/hello_world_split_streams.pdf";
/// Eight path objects on one page.
const RECTANGLES: &str = "tests/fixtures/rectangles.pdf";
/// Two pages sharing one content stream and one resource dictionary.
const TWO_PAGES: &str = "tests/fixtures/hello_world_2_pages.pdf";

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfrum-mutation-{name}"));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Save `edits` of `doc` and reopen the result.
fn round_trip(doc: &Document, edits: &[PageEdit], name: &str) -> Document {
    let out = temp_dir(name).join("out.pdf");
    doc.save_pages(&out, edits, &SaveOptions::default())
        .expect("saves");
    Document::open(&out).expect("reopens")
}

/// Which `/Contents` element each of a page's objects belongs to.
///
/// Every object on these fixtures was parsed out of a real stream, so each
/// answer is `Some`. A `None` — a created, streamless object — is reported as
/// `usize::MAX` so it is loud in a failure message rather than silently equal
/// to an index; no assertion below expects one.
fn streams_of(page: &PageEdit) -> Vec<usize> {
    page.objects()
        .iter()
        .map(pdfrum::PageObject::content_stream)
        .map(|stream| stream.unwrap_or(usize::MAX))
        .collect()
}

// -------------------------------------------------- the untouched page

// `EmptyCreation` (fpdf_edit_embeddertest.cpp:391): generating content on a
// page nobody edited must do nothing at all. The C++ asserts it against a
// byte-exact whole-file golden; we assert the same fact directly — the saved
// bytes of an unedited page are the saved bytes of no edit.
#[test]
fn an_unedited_page_is_saved_exactly_as_an_unedited_document_would_be() {
    let doc = Document::open(HELLO).expect("open");
    let page = doc.page(0).expect("page").edit();
    assert!(!page.is_modified(), "opening a page is not editing it");

    let mut edited = Vec::new();
    doc.write_pages_to(&mut edited, &[page], &SaveOptions::default())
        .expect("saves");
    let mut plain = Vec::new();
    doc.write_to(&mut plain, &SaveOptions::default())
        .expect("saves");
    // The two files are compared up to the trailer, because the trailer holds
    // the `/ID` — drawn fresh on every save by design, and the one thing two
    // saves of the same bytes are meant to disagree on. Everything before it
    // is the objects, which is what "not rewritten" is a claim about.
    // Searched over the bytes rather than a lossy `String`: the header's
    // binary comment is not UTF-8, and lossy decoding widens it, which would
    // move the index off the end of the objects.
    let body = |bytes: &[u8]| -> Vec<u8> {
        let end = bytes
            .windows(b"trailer".len())
            .position(|w| w == b"trailer")
            .unwrap_or(bytes.len());
        bytes.get(..end).unwrap_or(bytes).to_vec()
    };
    assert_eq!(
        body(&edited),
        body(&plain),
        "an unedited page must not be rewritten"
    );

    // And the page still draws what it drew: the content stream is the
    // original's, filter and all.
    let reopened = Document::open(&{
        let out = temp_dir("unedited").join("out.pdf");
        std::fs::write(&out, &edited).expect("writes");
        out
    })
    .expect("reopens");
    assert_eq!(reopened.page(0).expect("page").edit().len(), 2);
}

// A page listed but only *read* is likewise untouched: reading objects does
// not dirty them, and only `object_mut` does.
#[test]
fn reading_a_pages_objects_does_not_dirty_it() {
    let doc = Document::open(RECTANGLES).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    let count = page.objects().len();
    assert_eq!(count, 8);
    assert!(!page.is_modified());
    // Taking a mutable reference is the edit, whether or not it is used.
    assert!(page.object_mut(0).is_some());
    assert!(page.is_modified());
}

// -------------------------------------------------- content-stream indices

// `GetContentStream` (fpdf_edit_embeddertest.cpp:2008): the baseline
// partition of `split_streams.pdf` — 15 objects in element 0, 3 in element 1,
// 1 in element 2.
#[test]
fn a_multi_stream_page_records_which_element_each_object_came_from() {
    let doc = Document::open(SPLIT).expect("open");
    let page = doc.page(0).expect("page").edit();
    assert_eq!(page.len(), 19);
    let streams = streams_of(&page);
    assert_eq!(
        streams,
        [vec![0; 15], vec![1; 3], vec![2; 1]].concat(),
        "got {streams:?}"
    );
}

// `RemoveAllFromStream` (fpdf_edit_embeddertest.cpp:2033), all three phases.
//
// Removing every object of element 1 leaves 16 objects. Before the save their
// indices still read 0..0 and 2 — the gap is preserved, because nothing has
// renumbered yet. After the save and reload the emptied element is gone and
// the object that was in element 2 reads as element 1.
#[test]
fn emptying_one_content_stream_collapses_the_indices_after_it() {
    let doc = Document::open(SPLIT).expect("open");
    let mut page = doc.page(0).expect("page").edit();

    // Backwards, so the indices of the ones still to remove do not shift.
    for index in (0..page.len()).rev() {
        if page
            .objects()
            .get(index)
            .map(pdfrum::PageObject::content_stream)
            == Some(Some(1))
        {
            assert!(page.remove(index).is_some());
        }
    }
    assert_eq!(page.len(), 16);

    // Before the save: the gap is still there.
    let streams = streams_of(&page);
    assert_eq!(
        streams,
        [vec![0; 15], vec![2; 1]].concat(),
        "got {streams:?}"
    );

    // After: the emptied element is gone and element 2 has become element 1.
    let saved = round_trip(&doc, &[page], "collapse");
    let reloaded = saved.page(0).expect("page").edit();
    assert_eq!(reloaded.len(), 16);
    let streams = streams_of(&reloaded);
    assert_eq!(
        streams,
        [vec![0; 15], vec![1; 1]].concat(),
        "got {streams:?}"
    );
}

// `RemoveFirstFromSingleStream` (:2183) and `RemoveLastFromSingleStream`
// (:2247): a single-stream page keeps index 0 whichever object goes.
#[test]
fn removing_from_a_single_stream_page_leaves_the_survivor_at_index_zero() {
    for remove in [0usize, 1] {
        let doc = Document::open(HELLO).expect("open");
        let mut page = doc.page(0).expect("page").edit();
        assert_eq!(page.len(), 2);
        assert!(page.remove(remove).is_some());
        assert_eq!(page.len(), 1);
        assert_eq!(streams_of(&page), vec![0]);

        let saved = round_trip(&doc, &[page], &format!("single-{remove}"));
        let reloaded = saved.page(0).expect("page").edit();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(streams_of(&reloaded), vec![0]);
    }
}

// `RemoveAllFromSingleStream` (:2133): emptying a single-stream page leaves it
// with nothing, and the saved file has no content at all.
#[test]
fn emptying_a_single_stream_page_leaves_it_blank() {
    let doc = Document::open(HELLO).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    while !page.is_empty() {
        assert!(page.remove(0).is_some());
    }
    assert_eq!(page.len(), 0);

    let saved = round_trip(&doc, &[page], "blank");
    assert_eq!(saved.page(0).expect("page").edit().len(), 0);
    // The page still exists and still has its size.
    assert_eq!(saved.page_count(), 1);
    assert!((saved.page(0).expect("page").width() - 200.0).abs() < 1e-9);
}

// `RemoveAllFromMultipleStreams` (:2312): the same, across an array.
#[test]
fn emptying_every_element_of_an_array_leaves_the_page_blank() {
    let doc = Document::open(HELLO_SPLIT).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    assert_eq!(page.len(), 3);
    while !page.is_empty() {
        assert!(page.remove(0).is_some());
    }

    let saved = round_trip(&doc, &[page], "blank-array");
    assert_eq!(saved.page(0).expect("page").edit().len(), 0);
}

// `RemoveExistingPageObjectSplitStreamsNotLonely` (:1926): removing one object
// from an element that still has a sibling leaves the element in place, so the
// indices after it do not move.
#[test]
fn removing_one_of_two_objects_in_an_element_leaves_the_element_standing() {
    let doc = Document::open(HELLO_SPLIT).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    assert_eq!(streams_of(&page), vec![0, 0, 1]);
    assert!(page.remove(0).is_some());
    assert_eq!(page.len(), 2);

    let saved = round_trip(&doc, &[page], "not-lonely");
    let reloaded = saved.page(0).expect("page").edit();
    assert_eq!(reloaded.len(), 2);
    // Element 1 survives, so its object still reads as element 1.
    assert_eq!(streams_of(&reloaded), vec![0, 1]);
}

// `RemoveExistingPageObjectSplitStreamsLonely` (:1969): removing the *only*
// object of an element drops the element, so the ones after it shift down. The
// pair of tests differs only in which object goes, which is what makes them
// the array-shrink discriminator.
#[test]
fn removing_the_only_object_of_an_element_drops_the_element() {
    let doc = Document::open(HELLO_SPLIT).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    // Index 2 is the only object in element 1.
    assert!(page.remove(2).is_some());
    assert_eq!(page.len(), 2);

    let saved = round_trip(&doc, &[page], "lonely");
    let reloaded = saved.page(0).expect("page").edit();
    assert_eq!(reloaded.len(), 2);
    assert_eq!(streams_of(&reloaded), vec![0, 0]);
}

// -------------------------------------------------- inserting

// `InsertObjectAtIndex` (:2420): the pure-ordering contract, including that an
// index equal to the length is valid and one past it is not.
#[test]
fn inserting_places_objects_in_the_order_asked_for() {
    let doc = Document::open(HELLO).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    let red = || {
        PathBuilder {
            fill: Some(Color::from_rgb8(255, 0, 0)),
            ..PathBuilder::rect(Rect::new(10.0, 10.0, 30.0, 30.0))
        }
        .build()
    };

    assert!(page.insert(0, red()).is_ok());
    assert_eq!(page.len(), 3);
    assert!(matches!(page.objects()[0], pdfrum::PageObject::Path(_)));

    // One past the end is refused; the length itself appends.
    let past = page.len() + 1;
    let err = page.insert(past, red()).expect_err("index past the end");
    assert_eq!(err.index, past);
    assert_eq!(err.len, 3);
    assert_eq!(page.len(), 3);
    assert!(page.insert(3, red()).is_ok());
    assert_eq!(page.len(), 4);
    assert!(matches!(page.objects()[3], pdfrum::PageObject::Path(_)));
}

// `InsertObjectAtIndexPersistsOrder` (:2465): the z-order survives the save.
#[test]
fn an_inserted_objects_place_in_the_painting_order_survives_a_save() {
    let doc = Document::open(HELLO).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    let rect = |x: f64| {
        PathBuilder {
            fill: Some(Color::from_rgb8(0, 0, 255)),
            ..PathBuilder::rect(Rect::new(x, 10.0, x + 20.0, 30.0))
        }
        .build()
    };
    assert!(page.insert(0, rect(10.0)).is_ok());
    assert!(page.insert(2, rect(50.0)).is_ok());

    let kinds = |page: &PageEdit| -> Vec<&'static str> {
        page.objects()
            .iter()
            .map(|o| match o {
                pdfrum::PageObject::Path(_) => "path",
                pdfrum::PageObject::Text(_) => "text",
                _ => "other",
            })
            .collect()
    };
    assert_eq!(kinds(&page), vec!["path", "text", "path", "text"]);

    let saved = round_trip(&doc, &[page], "z-order");
    let reloaded = saved.page(0).expect("page").edit();
    assert_eq!(kinds(&reloaded), vec!["path", "text", "path", "text"]);
}

// `InsertObjectAtIndexAcrossContentStreams` (:2520): an object inserted next
// to one that lives in an element adopts that element, which is what makes its
// position survive. Without that it would be appended to a fresh element and
// drawn last whatever its index said.
#[test]
fn an_inserted_object_adopts_the_element_of_the_object_it_lands_before() {
    let doc = Document::open(HELLO_SPLIT).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    assert_eq!(streams_of(&page), vec![0, 0, 1]);

    let rect = |color: Color| {
        PathBuilder {
            fill: Some(color),
            ..PathBuilder::rect(Rect::new(10.0, 10.0, 30.0, 30.0))
        }
        .build()
    };
    assert!(page.insert(0, rect(Color::from_rgb8(255, 0, 0))).is_ok());
    assert!(page.insert(3, rect(Color::from_rgb8(0, 255, 0))).is_ok());

    assert_eq!(page.len(), 5);
    assert_eq!(streams_of(&page), vec![0, 0, 0, 1, 1]);

    // And the partition survives the round trip.
    let saved = round_trip(&doc, &[page], "adopt");
    let reloaded = saved.page(0).expect("page").edit();
    assert_eq!(reloaded.len(), 5);
    assert_eq!(streams_of(&reloaded), vec![0, 0, 0, 1, 1]);
}

// An appended object with no neighbour is streamless, so it becomes a new
// `/Contents` element — and a page that had one stream now has two.
#[test]
fn an_appended_object_becomes_a_new_content_element() {
    let doc = Document::open(HELLO).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        PathBuilder {
            fill: Some(Color::new([0.0, 0.5, 0.0, 1.0])),
            ..PathBuilder::rect(Rect::new(20.0, 20.0, 80.0, 60.0))
        }
        .build(),
    );

    let saved = round_trip(&doc, &[page], "append");
    let reloaded = saved.page(0).expect("page").edit();
    assert_eq!(reloaded.len(), 3);
    // Two elements now: the original text in 0, the new path in 1.
    assert_eq!(streams_of(&reloaded), vec![0, 0, 1]);
}

// -------------------------------------------------- visibility

// `PageObjectActiveState` (fpdf_editpage_embeddertest.cpp:490): a hidden
// object stays in the in-memory list — the count does not change — but is
// absent from the saved file.
#[test]
fn a_hidden_object_stays_in_memory_and_vanishes_from_the_saved_file() {
    let doc = Document::open(RECTANGLES).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    assert_eq!(page.len(), 8);

    assert!(page.hide(4).is_ok());
    assert_eq!(page.is_visible(4), Some(false));
    assert_eq!(page.len(), 8, "still eight in memory");
    assert!(page.is_modified());

    let saved = round_trip(&doc, &[page], "inactive");
    assert_eq!(
        saved.page(0).expect("page").edit().len(),
        7,
        "one fewer on disk"
    );
}

// `Bug378120423` (fpdf_editpage_embeddertest.cpp:563): hiding the *only*
// object empties the page's content entirely, and showing it again brings it
// back. The full cycle, because each direction is a different code path.
#[test]
fn hiding_the_only_object_empties_the_page_and_showing_it_restores_it() {
    let doc = Document::open(HELLO).expect("open");

    let mut page = doc.page(0).expect("page").edit();
    while page.len() > 1 {
        assert!(page.remove(1).is_some());
    }
    let one_object = round_trip(&doc, &[page], "cycle-a");
    assert_eq!(one_object.page(0).expect("page").edit().len(), 1);

    // Hide it: nothing is left on disk.
    let mut page = one_object.page(0).expect("page").edit();
    assert!(page.hide(0).is_ok());
    let hidden = round_trip(&one_object, &[page], "cycle-b");
    assert_eq!(hidden.page(0).expect("page").edit().len(), 0);

    // The document that still has the object can show it again.
    let mut page = one_object.page(0).expect("page").edit();
    assert!(page.hide(0).is_ok());
    assert!(page.show(0).is_ok());
    // Two flips leave it visible; the second dirties it again.
    assert!(page.is_modified());
    let shown = round_trip(&one_object, &[page], "cycle-c");
    assert_eq!(shown.page(0).expect("page").edit().len(), 1);
}

// Setting visibility to what it already is changes nothing, so the page is not
// rewritten — the idempotent call must not cost a regeneration.
#[test]
fn setting_visibility_to_its_current_value_does_not_dirty_the_page() {
    let doc = Document::open(RECTANGLES).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    assert!(page.show(0).is_ok());
    assert!(!page.is_modified());
}

// -------------------------------------------------- transforming

// `ModifyFormObject` (:3491) and `GetAndSetMatrixForPath`
// (fpdf_editpath_embeddertest.cpp:83): a transform alone marks the object
// changed — no insert or remove needed — and the moved geometry survives the
// save.
#[test]
fn transforming_an_object_dirties_it_and_the_move_survives_a_save() {
    let doc = Document::open(RECTANGLES).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    assert!(!page.is_modified());

    let before = match &page.objects()[0] {
        pdfrum::PageObject::Path(p) => p.object.path.clone(),
        other => panic!("expected a path, got {other:?}"),
    };
    assert!(
        page.transform(0, pdfrum::Affine::translate((25.0, 0.0)))
            .is_ok()
    );
    assert!(page.is_modified());

    let saved = round_trip(&doc, &[page], "transform");
    let reloaded = saved.page(0).expect("page").edit();
    assert_eq!(reloaded.len(), 8);
    let after = match &reloaded.objects()[0] {
        pdfrum::PageObject::Path(p) => {
            // The regenerated stream folds the matrix into a `cm`, so compare
            // the placed geometry rather than the raw path.
            p.object.matrix * p.object.path.clone()
        }
        other => panic!("expected a path, got {other:?}"),
    };
    // `kurbo::Shape` is named through `kurbo` and not through `pdfrum`,
    // because § narrowed the facade's re-export to the five types its own
    // signatures speak and `Shape` is not one of them. A caller wanting it
    // adds `kurbo` — which is what this test's manifest already does, and
    // what the ordinary Rust rule says.
    let moved = kurbo::Shape::bounding_box(&after);
    let original = kurbo::Shape::bounding_box(&before);
    assert!(
        (moved.x0 - original.x0 - 25.0).abs() < 0.5,
        "expected a 25pt shift: {original:?} -> {moved:?}"
    );
}

// An index that names no object is an error, not a silent no-op and not a
// panic: the page is left as it was.
#[test]
fn show_hide_and_transform_refuse_an_index_that_is_not_on_the_page() {
    let doc = Document::open(RECTANGLES).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    let len = page.len();
    assert_eq!(len, 8);

    let missing = len;
    let show = page.show(missing).expect_err("no such object");
    assert_eq!(show.index, missing);
    assert_eq!(show.len, len);
    let hide = page.hide(missing).expect_err("no such object");
    assert_eq!(hide.index, missing);
    assert_eq!(hide.len, len);
    let transform = page
        .transform(missing, pdfrum::Affine::translate((1.0, 0.0)))
        .expect_err("no such object");
    assert_eq!(transform.index, missing);
    assert_eq!(transform.len, len);

    assert_eq!(page.len(), 8);
    assert!(!page.is_modified());
}

// -------------------------------------------------- sharing

// `RemoveTextObjectWithTwoPagesSharingContentStreamAndResources` (:1337): the
// other page must be untouched at every stage. Editing one page of a pair that
// shares a content stream copies the stream rather than rewriting it.
#[test]
fn editing_one_of_two_pages_sharing_a_stream_leaves_the_other_alone() {
    let doc = Document::open(TWO_PAGES).expect("open");
    assert_eq!(doc.page_count(), 2);
    let before = doc.page(1).expect("page").edit().len();

    let mut page = doc.page(0).expect("page").edit();
    assert!(page.remove(0).is_some());

    let saved = round_trip(&doc, &[page], "shared");
    assert_eq!(saved.page_count(), 2);
    assert_eq!(
        saved.page(1).expect("page").edit().len(),
        before,
        "the page that was not edited must be untouched"
    );
    assert_eq!(saved.page(0).expect("page").edit().len(), before - 1);
}

// Two pages edited in one save both take effect, and neither undoes the other.
#[test]
fn two_pages_edited_in_one_save_both_take_effect() {
    let doc = Document::open(TWO_PAGES).expect("open");
    let mut first = doc.page(0).expect("page").edit();
    let mut second = doc.page(1).expect("page").edit();
    let before = second.len();
    assert!(first.remove(0).is_some());
    second.push(
        PathBuilder {
            fill: Some(Color::from_rgb8(255, 0, 0)),
            ..PathBuilder::rect(Rect::new(5.0, 5.0, 45.0, 25.0))
        }
        .build(),
    );

    let saved = round_trip(&doc, &[first, second], "both");
    assert_eq!(saved.page(0).expect("page").edit().len(), before - 1);
    assert_eq!(saved.page(1).expect("page").edit().len(), before + 1);
}

// -------------------------------------------------- saving again

// `InsertAndRemoveLargeFile` (:2587) and `EditOverExistingContent` (:2834):
// a saved document can be reopened and edited again. This is the chain that
// catches a save which produces something only *we* can read back.
#[test]
fn a_saved_edit_can_be_reopened_and_edited_again() {
    let doc = Document::open(RECTANGLES).expect("open");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        PathBuilder {
            fill: Some(Color::from_rgb8(0, 0, 255)),
            ..PathBuilder::rect(Rect::new(10.0, 10.0, 50.0, 30.0))
        }
        .build(),
    );
    let once = round_trip(&doc, &[page], "chain-a");
    assert_eq!(once.page(0).expect("page").edit().len(), 9);

    let mut page = once.page(0).expect("page").edit();
    assert!(page.remove(0).is_some());
    let twice = round_trip(&once, &[page], "chain-b");
    assert_eq!(twice.page(0).expect("page").edit().len(), 8);
}

// `IncrementalSaveWithModifications` (fpdf_save_embeddertest.cpp:244): an
// incremental save of an edited page keeps the original bytes as its prefix
// and still reflects the edit.
#[test]
fn an_incremental_save_of_an_edited_page_appends_and_still_reflects_the_edit() {
    let doc = Document::open(RECTANGLES).expect("open");
    let original = std::fs::read(RECTANGLES).expect("read");

    let mut page = doc.page(0).expect("page").edit();
    assert!(page.remove(0).is_some());

    let out = temp_dir("incremental").join("out.pdf");
    doc.save_pages(&out, &[page], &{
        let mut __o = SaveOptions::default();
        __o.update = Update::Incremental;
        __o
    })
    .expect("saves");

    let written = std::fs::read(&out).expect("read");
    assert!(
        written.starts_with(&original),
        "an incremental save keeps the original bytes as its prefix"
    );
    let reopened = Document::open(&out).expect("reopens");
    assert_eq!(reopened.page(0).expect("page").edit().len(), 7);
}

// -------------------------------------------------- rendering the result

// The end of the whole chain: an edited page renders, and renders differently
// from the one it was edited from. A save that produced bytes nothing draws
// would pass every count assertion above.
#[test]
fn an_edited_page_renders_and_renders_differently() {
    use pdfrum::RenderOptions;

    let doc = Document::open(RECTANGLES).expect("open");
    let before = doc
        .page(0)
        .expect("page")
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect("renders");

    let mut page = doc.page(0).expect("page").edit();
    while page.len() > 4 {
        assert!(page.remove(page.len() - 1).is_some());
    }
    let saved = round_trip(&doc, &[page], "render");
    let after = saved
        .page(0)
        .expect("page")
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect("renders");

    assert_eq!(
        (before.width(), before.height()),
        (after.width(), after.height())
    );
    let differing = (0..before.height())
        .flat_map(|y| (0..before.width()).map(move |x| (x, y)))
        .filter(|(x, y)| before.pixel(*x, *y) != after.pixel(*x, *y))
        .count();
    assert!(
        differing > 0,
        "removing half the objects must change pixels"
    );
}

// The other half of the same guard: a page whose edits were only *reads*
// renders identically to the unedited one.
#[test]
fn a_saved_but_unedited_page_renders_identically() {
    use pdfrum::RenderOptions;

    let doc = Document::open(RECTANGLES).expect("open");
    let before = doc
        .page(0)
        .expect("page")
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect("renders");

    let page = doc.page(0).expect("page").edit();
    let saved = round_trip(&doc, &[page], "unedited-render");
    let after = saved
        .page(0)
        .expect("page")
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect("renders");

    let differing = (0..before.height())
        .flat_map(|y| (0..before.width()).map(move |x| (x, y)))
        .filter(|(x, y)| before.pixel(*x, *y) != after.pixel(*x, *y))
        .count();
    assert_eq!(differing, 0, "an unedited page must render unchanged");
}
