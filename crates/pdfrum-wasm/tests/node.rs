//! The binding, exercised on `wasm32-unknown-unknown` under Node.
//!
//! These are the only tests that prove the crate at all: it compiles for the
//! host as an `rlib` so clippy and rustdoc have something to look at, but a
//! `#[wasm_bindgen]` export does not exist until the module is instantiated by
//! a JavaScript runtime. So every test here runs where the product runs.
//!
//! The fixtures are embedded with `include_bytes!` rather than read from disk,
//! because there is no filesystem on this target — which is also the shape a
//! real caller has, handing the module a `Uint8Array` it got from a fetch or a
//! file input.
//!
//! Run with `cargo test -p pdfrum-wasm --target wasm32-unknown-unknown`; the
//! runner is `wasm-bindgen-test-runner`, named in this crate's
//! `.cargo/config.toml`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pdfrum_wasm::{Cancel, Document, Form, OpenOptions, version};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::wasm_bindgen_test;

/// Two pages of text, the same fixture the C test opens.
const HELLO: &[u8] = include_bytes!("../../pdfrum-cli/tests/fixtures/hello_world_2_pages.pdf");
/// One text field, for the fill-save-reopen round trip.
const TEXT_FORM: &[u8] = include_bytes!("../../pdfrum-cli/tests/fixtures/text_form.pdf");
/// Password-protected, for the error contract.
const ENCRYPTED: &[u8] = include_bytes!("../../pdfrum-cli/tests/fixtures/encrypted.pdf");

/// The JavaScript `Error` a failure throws as.
///
/// A Rust caller of the `rlib` gets the crate's own `Failure`; a JavaScript
/// caller gets what `From<Failure> for JsValue` makes of it. The tests assert
/// on the *JavaScript* value, because that is the contract — so every one of
/// them converts first, through the same conversion the binding uses.
fn thrown<T>(result: Result<T, impl Into<JsValue>>) -> JsValue {
    match result {
        Ok(_) => panic!("the call was expected to fail"),
        Err(failure) => failure.into(),
    }
}

/// Asserts that a thrown `Error` carries the expected `.code`.
///
/// The whole error contract in one helper, so "throws with code N" is checked
/// the same way each time rather than by six variations on reflection.
/// JavaScript has one number type, so the code arrives as an `f64` and the
/// comparison is made there — every code this crate sets is a small whole
/// number, which an `f64` represents exactly, so there is no rounding for a
/// cast to hide.
#[track_caller]
fn assert_code(error: &JsValue, expected: u32) {
    let code = js_sys::Reflect::get(error, &JsValue::from_str("code"))
        .ok()
        .and_then(|code| code.as_f64());
    assert_eq!(
        code,
        Some(f64::from(expected)),
        "expected .code {expected}, message was {:?}",
        message_of(error),
    );
}

/// The message a thrown `Error` carries.
fn message_of(error: &JsValue) -> String {
    js_sys::Reflect::get(error, &JsValue::from_str("message"))
        .ok()
        .and_then(|message| message.as_string())
        .unwrap_or_default()
}

#[wasm_bindgen_test]
fn version_is_the_crate_version() {
    assert_eq!(version(), env!("CARGO_PKG_VERSION"));
}

#[wasm_bindgen_test]
fn opens_and_counts_pages() {
    let doc = Document::open(HELLO, None).expect("hello_world_2_pages.pdf opens");
    assert_eq!(doc.page_count(), 2);
}

#[wasm_bindgen_test]
fn renders_a_page_with_ink_on_it() {
    let doc = Document::open(HELLO, None).expect("opens");
    let page = doc.page(0).expect("page 0");
    let rendered = page.render(1.0, None).expect("renders");

    assert!(rendered.width > 0 && rendered.height > 0);
    assert_eq!(
        rendered.data.len(),
        rendered.width as usize * rendered.height as usize * 4,
        "the buffer is exactly width * height * 4 RGBA bytes",
    );

    // A page with text on it must have at least one pixel that is not the
    // white background. Asserting only the size would pass on a render that
    // drew nothing at all, which is the failure this test exists to catch.
    let inked = rendered
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[0] < 250 || pixel[1] < 250 || pixel[2] < 250);
    assert!(inked, "the rendered page has a non-white pixel");
}

#[wasm_bindgen_test]
fn extracts_text_and_words() {
    let doc = Document::open(HELLO, None).expect("opens");
    let page = doc.page(0).expect("page 0");

    let text = page.text();
    assert!(text.contains("Hello, world!"), "page 0's text was {text:?}");

    let words = page.words();
    assert!(!words.is_empty(), "page 0 has words");
    // The words are indices into the same string `text()` returned, which is
    // the promise the `Word` documentation makes.
    let first = &words[0];
    assert!(first.end > first.start);
    assert!(!first.text.is_empty());
}

#[wasm_bindgen_test]
fn fills_a_form_saves_it_and_reads_the_value_back() {
    let doc = Document::open(TEXT_FORM, None).expect("text_form.pdf opens");
    let mut form = Form::open(&doc).expect("the document has a form");

    let fields = form.fields().expect("fields");
    assert!(!fields.is_empty(), "text_form.pdf has fields");
    let name = fields[0].name.clone();

    form.set(&name, "pdfrum on the web".to_owned())
        .expect("the field takes a value");
    let saved = form.save(None).expect("saves");

    // The round trip is the test: a save that wrote the value into a place
    // nothing reads back would pass every assertion above it.
    let reopened = Document::open(&saved, None).expect("the saved bytes are a PDF");
    let form = Form::open(&reopened).expect("the saved document still has a form");
    let value = form
        .fields()
        .expect("fields")
        .into_iter()
        .find(|field| field.name == name)
        .expect("the field survived the save")
        .value;
    assert_eq!(value, "pdfrum on the web");
}

#[wasm_bindgen_test]
fn a_wrong_password_throws_code_three() {
    let error = thrown(Document::open(
        ENCRYPTED,
        Some("not the password".to_owned()),
    ));
    // WrongPassword is 3, the facade's own number.
    assert_code(&error, 3);
    assert!(
        !message_of(&error).is_empty(),
        "the facade's message travels beside the code",
    );
}

#[wasm_bindgen_test]
fn a_pixel_limit_throws_code_nine() {
    let mut options = OpenOptions::new();
    // Far below what a 612x792 page needs at scale 1, so the render is
    // refused rather than attempted.
    options.max_render_pixels = Some(1000.0);
    let doc = Document::open_with(HELLO, None, Some(options)).expect("the open itself is fine");
    let page = doc.page(0).expect("page 0");

    let error = thrown(page.render(1.0, None));
    // Limit is 9.
    assert_code(&error, 9);
}

#[wasm_bindgen_test]
fn a_raised_cancel_flag_stops_a_render() {
    let cancel = Cancel::new();
    let mut options = OpenOptions::new();
    options.cancel = Some(cancel.clone());
    let doc = Document::open_with(HELLO, None, Some(options)).expect("opens");
    let page = doc.page(0).expect("page 0");

    // Raised before the render rather than during one: this target has one
    // thread, so "during" is not a thing a test can arrange. What is being
    // asserted is that the flag reaches the render at all, which is the half
    // a host's own `setTimeout` depends on.
    cancel.stop();
    let error = thrown(page.render(1.0, None));
    // A cancelled render reports Limit, the same code a pixel cap does.
    assert_code(&error, 9);
}

#[wasm_bindgen_test]
fn a_bad_scale_is_rejected_as_an_argument() {
    let doc = Document::open(HELLO, None).expect("opens");
    let page = doc.page(0).expect("page 0");
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let error = thrown(page.render(scale, None));
        // An argument failure is 100.
        assert_code(&error, 100);
    }
}

#[wasm_bindgen_test]
fn an_index_past_the_end_throws() {
    let doc = Document::open(HELLO, None).expect("opens");
    let error = thrown(doc.page(99));
    // Read is 4 — the facade reports an index past the end as a failure to
    // read the document ("no page at index 99"), not as a malformed one. The
    // number is asserted rather than merely "some code", because a caller
    // branching on it needs it to be the same one every time.
    assert_code(&error, 4);
}

#[wasm_bindgen_test]
fn search_and_markdown_and_metadata_answer() {
    let doc = Document::open(HELLO, None).expect("opens");
    let page = doc.page(0).expect("page 0");

    let hits = page.search("Hello", None);
    assert!(!hits.is_empty(), "the needle is on the page");
    let text = page.text();
    let matched: String = text
        .chars()
        .skip(hits[0].start)
        .take(hits[0].end - hits[0].start)
        .collect();
    assert_eq!(matched, "Hello", "a hit indexes into text()");

    assert!(!page.markdown().is_empty(), "markdown is not empty");
    // An empty needle is an empty result, not a hit at every position.
    assert!(page.search("", None).is_empty());

    // These three answer for any document; the point is that they answer
    // rather than throw, since a document need carry none of them.
    let _ = doc.metadata();
    let _ = doc.bookmarks();
    let _ = doc.attachments();
    let _ = page.links();
    let _ = page.images();
}

#[wasm_bindgen_test]
fn a_document_with_no_form_is_refused_at_the_open() {
    let doc = Document::open(HELLO, None).expect("opens");
    let error = thrown(Form::open(&doc));
    assert_code(&error, 100);
}

#[wasm_bindgen_test]
fn a_page_outlives_its_document() {
    let page = {
        let doc = Document::open(HELLO, None).expect("opens");
        let page = doc.page(0).expect("page 0");
        // `free()` in JavaScript; `drop` is the same thing from Rust, and
        // this test is written in Rust.
        drop(doc);
        page
    };
    // The handle holds its own reference to the document, so the bytes are
    // still there. This is the memory rule the module documentation states,
    // and the one a caller is most likely to get wrong.
    assert!(page.text().contains("Hello, world!"));
    drop(page);
}
