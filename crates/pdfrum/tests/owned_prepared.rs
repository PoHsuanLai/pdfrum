//! A stop from a flag the caller owns, and a prepared page that owns its
//! document: what a viewer's worker keeps between tiles.

#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pdfrum::{
    Deadline, DeviceRect, Document, Error, LimitExceeded, OwnedPreparedPage, Region, RenderOptions,
    RenderSession, VelloCpuBackend,
};

const FIXTURE: &str = "../../benches/corpus/text_quick_start.pdf";

fn tile(x: u32, y: u32) -> Region {
    Region::Rect(DeviceRect::new(x, y, 256, 256).expect("tile"))
}

#[test]
fn the_owned_prepared_page_is_send_sync_and_borrows_nothing() {
    fn owned<T: Send + Sync + 'static>() {}
    owned::<OwnedPreparedPage>();
}

#[test]
fn an_owned_prepared_page_draws_the_bytes_the_borrowed_one_does() {
    let doc = Arc::new(Document::open(FIXTURE).expect("open"));
    let options = RenderOptions::scaled(2.3);
    let mut session = RenderSession::new();
    let borrowed = doc.page(0).expect("page").prepare(&options, &mut session);
    let owned = doc
        .page_owned(0)
        .expect("page")
        .prepare(&options, &mut session);

    let whole_b = borrowed
        .render_on(VelloCpuBackend, &mut session)
        .expect("b");
    let whole_o = owned.render_on(VelloCpuBackend, &mut session).expect("o");
    assert_eq!(whole_b.data(), whole_o.data());
    assert_eq!(
        owned.render(VelloCpuBackend).expect("plain").data(),
        whole_b.data()
    );

    // Reused across many tiles, each equal to the borrowed one's.
    for (x, y) in [(0, 0), (256, 0), (0, 256), (256, 256), (100, 37)] {
        let b = borrowed
            .render_region_on(VelloCpuBackend, &mut session, tile(x, y))
            .expect("b tile");
        let o = owned
            .render_region_on(VelloCpuBackend, &mut session, tile(x, y))
            .expect("o tile");
        assert_eq!((o.width(), o.height()), (256, 256));
        assert_eq!(b.data(), o.data(), "tile at {x},{y}");
    }
}

#[test]
fn an_owned_prepared_page_outlives_every_other_handle_and_moves_to_a_thread() {
    let doc = Arc::new(Document::open(FIXTURE).expect("open"));
    let mut session = RenderSession::new();
    let owned = doc
        .page_owned(0)
        .expect("page")
        .prepare(&RenderOptions::scaled(2.0), &mut session);
    assert!(Arc::ptr_eq(owned.document(), &doc));
    drop(doc);
    let owned = Arc::new(owned);
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let owned = Arc::clone(&owned);
            std::thread::spawn(move || {
                owned
                    .render_region_on(VelloCpuBackend, &mut RenderSession::new(), tile(i * 10, 0))
                    .expect("tile")
            })
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().expect("worker").width(), 256);
    }
}

#[test]
fn a_flag_raised_from_another_thread_stops_a_render_in_progress() {
    let doc = Arc::new(Document::open(FIXTURE).expect("open"));
    let mut session = RenderSession::new();
    let prepared = doc
        .page_owned(0)
        .expect("page")
        .prepare(&RenderOptions::scaled(2.0), &mut session);

    let flag = Arc::new(AtomicBool::new(false));
    session.set_deadline(Some(Deadline::from_flag(Arc::clone(&flag))));
    let raiser = {
        let flag = Arc::clone(&flag);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            flag.store(true, Ordering::Relaxed);
        })
    };
    // Draw until the flag lands: one of the draws is cut off, whichever one
    // the raise falls in, and every one after it is refused.
    let error = loop {
        match prepared.render_on(VelloCpuBackend, &mut session) {
            Ok(_) => {}
            Err(error) => break error,
        }
    };
    raiser.join().expect("raiser");
    assert!(matches!(
        error,
        Error::Limit(LimitExceeded::Stopped { page: Some(_), .. })
    ));
    assert!(flag.load(Ordering::Relaxed));

    // The prepared page, the session and the document are all still usable.
    session.set_deadline(Some(Deadline::from_flag(Arc::new(AtomicBool::new(false)))));
    let again = prepared
        .render_on(VelloCpuBackend, &mut session)
        .expect("a fresh flag");
    session.set_deadline(None);
    assert_eq!(
        again.data(),
        prepared.render(VelloCpuBackend).expect("plain").data()
    );
    assert!(doc.page(0).expect("page").text().char_count() > 0);
}
