//! The ceilings a host sets through `OpenOptions::limits`: the render pixel
//! cap. Off by default, so nothing here changes what a default `Limits` does.

// `clippy.toml`'s `allow-expect-in-tests` covers `#[test]` bodies but not the
// `open_with` and `capped` helpers; a fixture that will not open is the
// failure signal there.
#![allow(clippy::expect_used)]

use std::time::{Duration, Instant};

use pdfrum::{
    Affine, Document, Error, LimitExceeded, Limits, OpenOptions, RenderOptions, RenderSession,
    VelloCpuBackend,
};

/// 200 x 200 points, so scale 100 asks for 20000 x 20000 = 400 megapixels.
const HELLO: &str = "tests/fixtures/hello_world.pdf";

fn open_with(limits: Limits) -> Document {
    Document::open_with(
        HELLO,
        &OpenOptions {
            limits,
            ..OpenOptions::default()
        },
    )
    .expect("open")
}

fn capped(max_render_pixels: u64) -> Document {
    open_with(Limits {
        max_render_pixels: Some(max_render_pixels),
        ..Limits::default()
    })
}

fn huge() -> RenderOptions {
    RenderOptions {
        transform: Affine::scale(100.0),
        ..RenderOptions::default()
    }
}

#[test]
fn a_render_above_the_pixel_cap_is_refused_before_anything_is_allocated() {
    let doc = capped(100_000_000);
    let page = doc.page(0).expect("page");
    let started = Instant::now();
    let error = page
        .render(&VelloCpuBackend::new(), &huge())
        .expect_err("400 megapixels is above a 100-megapixel cap");
    // A 20000 x 20000 target is 1.6 GB of pixmap; refusing it takes no time
    // at all, and the bound here is generous enough for a loaded machine.
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the refusal took {:?}: something was allocated or decoded first",
        started.elapsed()
    );
    match error {
        Error::Limit(LimitExceeded::RenderPixels {
            width,
            height,
            allowed,
        }) => {
            assert_eq!((width, height, allowed), (20_000, 20_000, 100_000_000));
        }
        other => panic!("expected the pixel cap, got {other:?}"),
    }
    let message = error.to_string();
    assert!(
        message.contains("20000 x 20000 px") && message.contains("cap of 100 megapixels"),
        "a person can act on the message: {message}"
    );
}

#[test]
fn a_prepared_page_is_refused_at_the_draw() {
    let doc = capped(100_000_000);
    let page = doc.page(0).expect("page");
    let mut session = RenderSession::new();
    // `prepare` is infallible; the cap is applied when the page is drawn.
    let prepared = page.prepare(&huge(), &mut session);
    assert!(matches!(
        prepared.render_on(&VelloCpuBackend::new(), &mut session),
        Err(Error::Limit(LimitExceeded::RenderPixels { .. }))
    ));
    assert!(matches!(
        prepared.render(&VelloCpuBackend::new()),
        Err(Error::Limit(LimitExceeded::RenderPixels { .. }))
    ));
}

#[test]
fn a_render_under_the_cap_succeeds() {
    let doc = capped(100_000_000);
    let page = doc.page(0).expect("page");
    let pixmap = page
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect("40000 px is under the cap");
    assert_eq!((pixmap.width(), pixmap.height()), (200, 200));
    // Exactly at the cap is under it: the cap is the most a render may have.
    let doc = capped(40_000);
    let page = doc.page(0).expect("page");
    assert!(
        page.render(&VelloCpuBackend::new(), &RenderOptions::default())
            .is_ok()
    );
    let doc = capped(39_999);
    let page = doc.page(0).expect("page");
    assert!(matches!(
        page.render(&VelloCpuBackend::new(), &RenderOptions::default()),
        Err(Error::Limit(LimitExceeded::RenderPixels {
            width: 200,
            height: 200,
            allowed: 39_999
        }))
    ));
}

#[test]
fn the_default_limits_cap_nothing() {
    let doc = open_with(Limits::default());
    let page = doc.page(0).expect("page");
    let pixmap = page
        .render(&VelloCpuBackend::new(), &RenderOptions::scaled(4.0))
        .expect("no cap by default");
    assert_eq!((pixmap.width(), pixmap.height()), (800, 800));
}
