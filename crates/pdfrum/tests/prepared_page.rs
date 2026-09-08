//! A prepared page draws the bytes the page draws, as many times as asked.

use pdfrum::{Affine, Document, RenderOptions, RenderSession, VelloCpuBackend};

const FIXTURES: [&str; 4] = [
    "tests/fixtures/hello_world.pdf",
    "tests/fixtures/rectangles.pdf",
    "tests/fixtures/jpx_two_sizes.pdf",
    "tests/fixtures/text_form.pdf",
];

fn with_annotations() -> RenderOptions {
    let mut options = RenderOptions::default();
    options.annotations = true;
    options
}

#[test]
fn a_prepared_page_draws_the_bytes_the_page_draws() {
    let backend = VelloCpuBackend::new();
    for path in FIXTURES {
        let doc = Document::open(path).unwrap();
        let page = doc.page(0).unwrap();
        let options = with_annotations();

        let direct = page
            .render_on(&backend, &options, &mut RenderSession::new())
            .unwrap();

        let mut session = RenderSession::new();
        let prepared = page.prepare(&options, &mut session);
        let first = prepared.render_on(&backend, &mut session).unwrap();
        let second = prepared.render_on(&backend, &mut session).unwrap();

        assert_eq!(
            direct.data(),
            first.data(),
            "{path}: prepared differs from direct"
        );
        assert_eq!(
            first.data(),
            second.data(),
            "{path}: second draw differs from first"
        );
    }
}

#[test]
fn a_prepared_page_is_prepared_for_its_options() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").unwrap();
    let page = doc.page(0).unwrap();
    let mut session = RenderSession::new();

    let one_to_one = page.prepare(&RenderOptions::default(), &mut session);
    let doubled = page.prepare(
        &{
            let mut __o = RenderOptions::default();
            __o.transform = Affine::scale(2.0);
            __o
        },
        &mut session,
    );

    let small = one_to_one.render(&VelloCpuBackend::new()).unwrap();
    let big = doubled.render(&VelloCpuBackend::new()).unwrap();
    assert_eq!(
        (big.width(), big.height()),
        (small.width() * 2, small.height() * 2)
    );
}

#[test]
fn render_on_and_prepare_warm_the_same_session() {
    let backend = VelloCpuBackend::new();
    let doc = Document::open("tests/fixtures/text_form.pdf").unwrap();
    let page = doc.page(0).unwrap();
    let options = with_annotations();
    let mut session = RenderSession::new();

    let cold = page.render_on(&backend, &options, &mut session).unwrap();
    let warm = page
        .prepare(&options, &mut session)
        .render_on(&backend, &mut session)
        .unwrap();
    assert_eq!(cold.data(), warm.data());
}
