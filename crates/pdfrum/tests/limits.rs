//! The ceilings a host sets through `OpenOptions::limits`: the render pixel
//! cap and the deadline. Both off by default, so nothing here changes what a
//! default `Limits` does.

// `clippy.toml`'s `allow-expect-in-tests` covers `#[test]` bodies but not the
// `open_with` and `capped` helpers; a fixture that will not open is the
// failure signal there.
#![allow(clippy::expect_used)]

use std::time::{Duration, Instant};

use pdfrum::{
    Affine, Deadline, DiagKind, Document, Error, LimitExceeded, Limits, OpenOptions, Operation,
    PageIndex, RenderOptions, RenderSession, VelloCpuBackend,
};

/// 200 x 200 points, so scale 100 asks for 20000 x 20000 = 400 megapixels.
const HELLO: &str = "tests/fixtures/hello_world.pdf";
/// The corpus guide: real text on a real page, for the deadline.
const GUIDE: &str = "../../benches/corpus/text_quick_start.pdf";

fn open_with(limits: Limits) -> Document {
    Document::open_with(HELLO, &{
        let mut __o = OpenOptions::default();
        __o.limits = limits;
        __o
    })
    .expect("open")
}

fn open_guide(deadline: Deadline) -> pdfrum::Result<Document> {
    Document::open_with(GUIDE, &{
        let mut __o = OpenOptions::default();
        __o.limits = Limits {
            deadline: Some(deadline),
            ..Limits::default()
        };
        __o
    })
}

fn capped(max_render_pixels: u64) -> Document {
    open_with(Limits {
        max_render_pixels: Some(max_render_pixels),
        ..Limits::default()
    })
}

fn huge() -> RenderOptions {
    let mut options = RenderOptions::default();
    options.transform = Affine::scale(100.0);
    options
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
fn a_deadline_already_passed_refuses_the_open() {
    let error = open_guide(Deadline::after(Duration::ZERO)).expect_err("no time at all");
    assert!(matches!(
        error,
        Error::Limit(LimitExceeded::Time {
            during: Operation::Open,
            page: None,
            ..
        })
    ));
    assert_eq!(
        error.to_string(),
        "time limit of 0 s exceeded while opening the document; allow more time or do less"
    );
    // An `Instant` already reached says the same.
    assert!(matches!(
        open_guide(Deadline::at(Instant::now())),
        Err(Error::Limit(LimitExceeded::Time {
            during: Operation::Open,
            ..
        }))
    ));
}

#[test]
fn a_generous_deadline_changes_nothing() {
    let doc = open_guide(Deadline::after(Duration::from_hours(1))).expect("an hour is plenty");
    let plain = Document::open(GUIDE).expect("open");
    let (page, reference) = (doc.page(0).expect("page"), plain.page(0).expect("page"));
    let backend = VelloCpuBackend::new();
    let options = RenderOptions::default();
    assert_eq!(
        page.render(&backend, &options).expect("render").data(),
        reference.render(&backend, &options).expect("render").data()
    );
    assert_eq!(page.text().to_string(), reference.text().to_string());
    assert!(!page.text().to_string().is_empty());
    assert!(!doc.all_diagnostics().contains(&DiagKind::TimeLimitReached));
}

#[test]
fn a_deadline_that_passes_after_the_open_stops_every_later_operation() {
    let doc = open_guide(Deadline::after(Duration::from_millis(200))).expect("opens in time");
    let page = doc.page(0).expect("loads in time");
    std::thread::sleep(Duration::from_millis(250));

    // A render fails at the engine's entry, naming the page.
    let error = page
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect_err("out of time");
    assert!(matches!(
        error,
        Error::Limit(LimitExceeded::Time {
            during: Operation::Render,
            page: Some(index),
            ..
        }) if index == PageIndex::FIRST
    ));
    assert_eq!(
        error.to_string(),
        "time limit of 200 ms exceeded while rendering page 0; allow more time or do less"
    );

    // Extraction cannot fail: the page comes back empty and the record says why.
    assert_eq!(page.text().char_count(), 0);
    assert!(doc.all_diagnostics().contains(&DiagKind::TimeLimitReached));

    // The next page is refused at the boundary, and the walk ends there.
    assert!(matches!(
        doc.page(0),
        Err(Error::Limit(LimitExceeded::Time {
            during: Operation::PageLoad,
            page: Some(index),
            ..
        })) if index == PageIndex::FIRST
    ));
    assert_eq!(doc.pages().count(), 0);
}

#[test]
fn a_caller_can_stop_the_work_by_hand() {
    // The mechanism under the clock: a flag, which is all a `wasm32` host has.
    let stop = Deadline::manual();
    let doc = open_guide(stop.clone()).expect("open");
    let page = doc.page(0).expect("page");
    assert!(
        page.render(&VelloCpuBackend::new(), &RenderOptions::default())
            .is_ok(),
        "nothing is raised yet"
    );
    stop.stop();
    let error = page
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect_err("stopped");
    assert!(matches!(
        error,
        Error::Limit(LimitExceeded::Stopped {
            during: Operation::Render,
            page: Some(index),
        }) if index == PageIndex::FIRST
    ));
    assert_eq!(
        error.to_string(),
        "stopped by the caller while rendering page 0"
    );
    assert_eq!(page.text().char_count(), 0);
    assert!(matches!(
        doc.page(0),
        Err(Error::Limit(LimitExceeded::Stopped {
            during: Operation::PageLoad,
            ..
        }))
    ));
}

#[test]
fn a_caller_can_stop_the_work_from_another_thread() {
    let stop = Deadline::manual();
    let doc = open_guide(stop.clone()).expect("open");
    let page = doc.page(0).expect("page");
    let raiser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        stop.stop();
    });
    // Render until the flag lands: the loop is bounded by the timer above,
    // and how many renders finish before it is the machine's business.
    let backend = VelloCpuBackend::new();
    let error = loop {
        if let Err(error) = page.render(&backend, &RenderOptions::default()) {
            break error;
        }
    };
    raiser.join().expect("the raiser exits");
    assert!(matches!(
        error,
        Error::Limit(LimitExceeded::Stopped {
            during: Operation::Render,
            ..
        })
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
