//! The owned handles: a page and a form session that hold an
//! `Arc<Document>` instead of borrowing one.
//!
//! What these pin is that the owned path **is** the borrowed path — the
//! same bytes out of a render, the same text, the same form state — and
//! that the handles do what a borrow cannot: outlive the scope the document
//! was opened in and travel to other threads.

use std::sync::Arc;

use pdfrum::{Document, OwnedPage, RenderOptions, RenderSession, VelloCpuBackend};

const HELLO: &str = "tests/fixtures/hello_world.pdf";
const TWO_PAGES: &str = "tests/fixtures/hello_world_2_pages.pdf";

/// The fixture, shared — or the reason it did not open, for the test to
/// fail on.
fn shared(path: &str) -> Result<Arc<Document>, pdfrum::Error> {
    Document::open(path).map(Arc::new)
}

// ------------------------------------------------------------------ pages

/// The one test the whole design rests on: an owned page renders the same
/// pixels as the borrowed page it wraps.
#[test]
fn an_owned_page_renders_byte_identically_to_the_borrowed_one() {
    let doc = shared(HELLO).expect("the fixture must open");
    let backend = VelloCpuBackend::new();
    let options = RenderOptions::default();

    let borrowed = doc
        .page(0)
        .expect("page")
        .render(&backend, &options)
        .expect("render");
    let owned = doc
        .page_owned(0)
        .expect("page")
        .render(&backend, &options)
        .expect("render");

    assert_eq!(
        (owned.width(), owned.height()),
        (borrowed.width(), borrowed.height())
    );
    assert_eq!(owned.data(), borrowed.data());
}

/// The rest of the read-only surface agrees with the borrowed page too.
#[test]
fn an_owned_page_reads_what_the_borrowed_one_reads() {
    let doc = shared(HELLO).expect("the fixture must open");
    let borrowed = doc.page(0).expect("page");
    let owned = doc.page_owned(0).expect("page");

    assert_eq!(owned.index(), borrowed.index());
    assert_eq!(owned.width().to_bits(), borrowed.width().to_bits());
    assert_eq!(owned.height().to_bits(), borrowed.height().to_bits());
    assert_eq!(owned.media_box(), borrowed.media_box());
    assert_eq!(owned.crop_box(), borrowed.crop_box());
    assert_eq!(owned.bleed_box(), borrowed.bleed_box());
    assert_eq!(owned.trim_box(), borrowed.trim_box());
    assert_eq!(owned.art_box(), borrowed.art_box());
    assert_eq!(owned.rotation(), borrowed.rotation());
    assert_eq!(owned.text().to_string(), borrowed.text().to_string());
    assert_eq!(owned.words(), borrowed.words());
    assert_eq!(owned.links().len(), borrowed.links().len());
    assert_eq!(owned.page_links(), borrowed.page_links());
    assert_eq!(owned.annotations().len(), borrowed.annotations().len());
    assert_eq!(owned.images().len(), borrowed.images().len());
    assert_eq!(owned.structure().is_some(), borrowed.structure().is_some());
}

/// The point of the handle: the page outlives every other reference to
/// the document.
#[test]
fn an_owned_page_keeps_its_document_alive() {
    let page = {
        let doc = shared(HELLO).expect("the fixture must open");
        doc.page_owned(0).expect("page")
    };
    assert_eq!(Arc::strong_count(page.document()), 1);
    assert!(page.text().to_string().contains("Hello, world!"));
}

/// Eight handles over one document, rendered on eight threads at once,
/// all producing the reference bytes. A scoped thread needs `Send`, which
/// a page borrowing a stack-local document would also have; the difference
/// is that these were made *before* the threads and could equally have been
/// made after the scope that opened the document closed.
#[test]
fn eight_owned_pages_render_concurrently() {
    let doc = shared(HELLO).expect("the fixture must open");
    let backend = VelloCpuBackend::new();
    let options = RenderOptions::default();
    let reference = doc
        .page(0)
        .expect("page")
        .render(&backend, &options)
        .expect("render");

    let pages: Vec<OwnedPage> = (0..8).map(|_| doc.page_owned(0).expect("page")).collect();
    drop(doc);

    std::thread::scope(|scope| {
        let handles: Vec<_> = pages
            .iter()
            .map(|page| {
                scope.spawn(|| {
                    let backend = VelloCpuBackend::new();
                    let mut session = RenderSession::new();
                    page.render_on(&backend, &RenderOptions::default(), &mut session)
                        .expect("render")
                })
            })
            .collect();
        for handle in handles {
            let pixmap = handle.join().expect("a render thread must not panic");
            assert_eq!(pixmap.data(), reference.data());
        }
    });
}

/// `pages_owned` yields every page as an owned handle, in order, and the
/// iterator holds the document itself.
#[test]
fn pages_owned_walks_every_page_and_outlives_the_arc() {
    let iter = {
        let doc = shared(TWO_PAGES).expect("the fixture must open");
        doc.pages_owned()
    };
    let pages: Vec<OwnedPage> = iter.collect::<Result<_, _>>().expect("every page loads");
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].index(), 0.into());
    assert_eq!(pages[1].index(), 1.into());
    assert!(Arc::ptr_eq(pages[0].document(), pages[1].document()));
}

/// A page past the end is an error from `page_owned`, as from `page`.
#[test]
fn page_owned_refuses_an_index_past_the_end() {
    let doc = shared(HELLO).expect("the fixture must open");
    assert!(matches!(doc.page_owned(1), Err(pdfrum::Error::Read(_))));
}
