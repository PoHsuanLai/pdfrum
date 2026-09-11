//! The owned handles: a page and a form session that hold an
//! `Arc<Document>` instead of borrowing one.
//!
//! What these pin is that the owned path **is** the borrowed path — the
//! same bytes out of a render, the same text, the same form state — and
//! that the handles do what a borrow cannot: outlive the scope the document
//! was opened in and travel to other threads.

use std::sync::Arc;

use pdfrum::{
    Document, FormSession, Key, Modifiers, OwnedFormSession, OwnedPage, Point, RenderOptions,
    RenderSession, VelloCpuBackend,
};

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
        .render_with(backend, &options)
        .expect("render");
    let owned = doc
        .page_owned(0)
        .expect("page")
        .render_with(backend, &options)
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
        .render_with(backend, &options)
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
                    page.render_on(backend, &RenderOptions::default(), &mut session)
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

// ---------------------------------------------------------------- forms

const FORM: &str = "tests/fixtures/text_form.pdf";
/// The fixture's one text field is `/Rect [100 100 200 130]`.
const INSIDE: Point = Point::new(120.0, 115.0);

fn click_owned(session: &mut OwnedFormSession, at: Point) {
    session.mouse_move(0, at, Modifiers::NONE);
    session.mouse_down(0, at, Modifiers::NONE);
    session.mouse_up(0, at, Modifiers::NONE);
}

/// A fill through the owned session — click, type, undo, blur — reaches the
/// same state the borrowed session reaches, and the session outlives the
/// scope that opened the document.
#[test]
fn an_owned_form_session_fills_a_field_like_the_borrowed_one() {
    let mut owned = {
        let doc = shared(FORM).expect("the fixture must open");
        FormSession::owned(doc)
    };
    let doc = Document::open(FORM).expect("the fixture must open");
    let mut borrowed = FormSession::new(&doc);

    assert!(owned.focused_annot().is_none());
    click_owned(&mut owned, INSIDE);
    borrowed.mouse_move(0, INSIDE, Modifiers::NONE);
    borrowed.mouse_down(0, INSIDE, Modifiers::NONE);
    borrowed.mouse_up(0, INSIDE, Modifiers::NONE);
    assert_eq!(owned.focused_annot(), borrowed.focused_annot());
    assert!(owned.focused_annot().is_some());

    for ch in "Hello".chars() {
        owned.character(ch, Modifiers::NONE);
        borrowed.character(ch, Modifiers::NONE);
    }
    assert_eq!(owned.focused_text().as_deref(), Some("Hello"));
    assert!(owned.can_undo());
    owned.key_down(Key::Z, Modifiers::CONTROL);
    borrowed.key_down(Key::Z, Modifiers::CONTROL);
    assert_eq!(owned.focused_text(), borrowed.focused_text());
    assert_eq!(owned.focused_text().as_deref(), Some("Hell"));

    let committed = owned.blur();
    let reference = borrowed.blur();
    assert!(owned.focused_annot().is_none());
    assert_eq!(committed.consumed, reference.consumed);
    assert_eq!(committed.updates.len(), reference.updates.len());
    assert!(
        committed
            .updates
            .iter()
            .any(|u| u.kind.appearance().is_some())
    );
}

/// The state survives every call: what one event read and cached is there
/// for the next, so a click followed by a keystroke on the owned session
/// costs one page read, not two.
#[test]
fn an_owned_form_session_keeps_its_state_between_calls() {
    let doc = shared(FORM).expect("the fixture must open");
    let mut session = FormSession::owned(doc);
    session.set_viewed_page(0);
    assert!(session.key_down(Key::Tab, Modifiers::NONE).consumed);
    session.character('x', Modifiers::NONE);
    assert_eq!(session.focused_text().as_deref(), Some("x"));
    assert_eq!(session.viewed_page(), 0.into());
    assert!(session.replace_selection("y"));
    assert_eq!(session.focused_text().as_deref(), Some("xy"));
}

/// The fill runs the document's own `/AA` script through the owned
/// session, and the transcript comes back the same way.
#[cfg(feature = "javascript")]
#[test]
fn an_owned_scripted_session_runs_the_documents_keystroke_script() {
    use pdfrum::ScriptConfig;

    let doc = shared("tests/fixtures/public_methods.pdf").expect("the fixture must open");
    let mut session = FormSession::owned_with_scripts(doc, &ScriptConfig::frozen_at(1_399_672_130))
        .expect("boa builds a realm on any input");
    // The field's `/Rect [100 160 200 190]`.
    click_owned(&mut session, Point::new(150.0, 175.0));
    session.character('7', Modifiers::NONE);

    let transcript = session
        .scripts()
        .expect("a scripted session")
        .transcript_text();
    assert!(
        transcript.contains("Alert: *** starting test 2 ***\n"),
        "the document's /AA /K did not run: {transcript}"
    );
    let mut diags = pdfrum::Diagnostics::default();
    assert!(session.script_failures(&mut diags).is_empty());
}
