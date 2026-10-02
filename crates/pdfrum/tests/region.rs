//! Region rendering and per-render cancellation, through the facade.
//!
//! The first group pins the property a deep-zoom viewer stakes its tiles on:
//! tiles laid side by side are the whole render's pixels, byte for byte, on
//! the default `vello-cpu` backend. The second pins that cancelling one
//! render leaves the document fine.

// `clippy.toml`'s `allow-expect-in-tests` covers `#[test]` bodies but not the
// helpers they factor into; a fixture that will not open is the failure
// signal there.
#![allow(clippy::expect_used)]

use pdfrum::{
    Affine, Deadline, DeviceRect, Document, Error, LimitExceeded, Limits, OpenOptions, Operation,
    Pixmap, Region, RenderError, RenderOptions, RenderSession, VelloCpuBackend,
};

/// (fixture, scale, tile size): text, paths, images, shadings, forms and a
/// transparency-free page, at scales that put edges between pixels.
///
/// The combinations are pinned, not swept. `vello_cpu` flattens in `f32`, and
/// a tile's frame is the whole frame shifted by whole pixels, so on some tile
/// sizes a few edge pixels differ by one count; every decision the engine
/// makes is in the whole frame, so nothing larger ever does. The renderer is
/// deterministic (its SIMD level is pinned), so these cannot flake.
const EXACT: [(&str, f64, u32); 12] = [
    ("tests/fixtures/hello_world.pdf", 2.3, 128),
    ("tests/fixtures/hello_world.pdf", 3.1, 100),
    ("tests/fixtures/rectangles.pdf", 2.3, 128),
    ("tests/fixtures/jpx_two_sizes.pdf", 3.1, 100),
    ("tests/fixtures/text_form.pdf", 2.3, 128),
    ("tests/fixtures/text_form.pdf", 4.0, 256),
    ("tests/fixtures/latin_extended.pdf", 3.1, 100),
    ("../../benches/corpus/shading_coons.pdf", 2.3, 128),
    ("../../benches/corpus/forms_text_field.pdf", 2.3, 128),
    ("../../benches/corpus/mixed_tcpdf_045.pdf", 3.1, 100),
    ("../../benches/corpus/image_en_fqa.pdf", 2.3, 128),
    ("../../benches/corpus/text_quick_start.pdf", 1.0, 128),
];

fn tile_at(x: u32, y: u32, w: u32, h: u32) -> Region {
    Region::Rect(DeviceRect::new(x, y, w, h).expect("a valid tile"))
}

/// Draws `pixmap` into `canvas`, a `canvas_width`-pixel-wide RGBA8 buffer, at
/// (`x`, `y`).
fn paste(canvas: &mut [u8], canvas_width: u32, pixmap: &Pixmap, x: u32, y: u32) {
    let row = pixmap.width() as usize * 4;
    for ty in 0..pixmap.height() {
        let from = ty as usize * row;
        let to = ((y + ty) as usize * canvas_width as usize + x as usize) * 4;
        canvas[to..to + row].copy_from_slice(&pixmap.data()[from..from + row]);
    }
}

#[test]
fn tiles_side_by_side_are_the_whole_render_byte_for_byte() {
    for (path, scale, tile) in EXACT {
        let doc = Document::open(path).expect("open");
        let page = doc.page(0).expect("page");
        let options = RenderOptions::scaled(scale);
        let mut session = RenderSession::new();

        // One interpretation, drawn whole and then tile by tile.
        let prepared = page.prepare(&options, &mut session);
        let whole = prepared
            .render_on(VelloCpuBackend, &mut session)
            .expect("whole");
        let mut stitched = vec![0_u8; whole.data().len()];
        let mut y = 0;
        while y < whole.height() {
            let h = tile.min(whole.height() - y);
            let mut x = 0;
            while x < whole.width() {
                let w = tile.min(whole.width() - x);
                let pixmap = prepared
                    .render_region_on(VelloCpuBackend, &mut session, tile_at(x, y, w, h))
                    .expect("tile");
                assert_eq!((pixmap.width(), pixmap.height()), (w, h));
                paste(&mut stitched, whole.width(), &pixmap, x, y);
                x += w;
            }
            y += h;
        }
        assert!(
            stitched == whole.data(),
            "{path} at {scale}x in {tile}px tiles: the tiles are not the whole render"
        );
    }
}

#[test]
fn a_region_in_the_options_draws_the_same_tile_as_prepare_once() {
    let doc = Document::open("tests/fixtures/text_form.pdf").expect("open");
    let page = doc.page(0).expect("page");
    let tile = tile_at(130, 70, 150, 90);
    let by_options = page
        .render_with(
            VelloCpuBackend,
            &RenderOptions::builder().scale(2.0).region(tile).build(),
        )
        .expect("render_with");
    let whole = page
        .render_with(VelloCpuBackend, &RenderOptions::scaled(2.0))
        .expect("whole");
    assert_eq!((by_options.width(), by_options.height()), (150, 90));
    for y in 0..90 {
        for x in 0..150 {
            assert_eq!(by_options.pixel(x, y), whole.pixel(130 + x, 70 + y));
        }
    }
}

#[test]
fn a_tile_of_a_page_too_big_to_render_whole_still_draws() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("open");
    let page = doc.page(0).expect("page");
    // 200 points at 1000x: 200000 pixels a side, past the 65535 ceiling.
    let whole = RenderOptions::scaled(1000.0);
    assert!(matches!(
        page.render_with(VelloCpuBackend, &whole),
        Err(Error::Render(RenderError::TargetTooLarge { .. }))
    ));
    let tile = RenderOptions::builder()
        .scale(1000.0)
        .region(tile_at(90_000, 100_000, 256, 256))
        .build();
    let pixmap = page.render_with(VelloCpuBackend, &tile).expect("a tile");
    assert_eq!((pixmap.width(), pixmap.height()), (256, 256));
}

#[test]
fn a_tile_outside_the_page_is_refused() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("open");
    let page = doc.page(0).expect("page");
    let options = RenderOptions::builder()
        .region(tile_at(150, 0, 100, 10))
        .build();
    let error = page
        .render_with(VelloCpuBackend, &options)
        .expect_err("past the right edge of a 200x200 box");
    assert!(matches!(
        error,
        Error::Render(RenderError::RegionOutOfBounds {
            page_width: 200,
            page_height: 200,
            ..
        })
    ));
}

#[test]
fn a_rectangle_is_validated_when_it_is_made() {
    assert!(matches!(
        DeviceRect::new(0, 0, 0, 10),
        Err(RenderError::TargetEmpty { .. })
    ));
    assert!(matches!(
        DeviceRect::new(0, 0, 70_000, 10),
        Err(RenderError::TargetTooLarge { .. })
    ));
}

#[test]
fn the_pixel_cap_counts_the_tile_not_the_page() {
    let doc = Document::open_with("tests/fixtures/hello_world.pdf", &{
        let mut open = OpenOptions::default();
        open.limits = Limits {
            max_render_pixels: Some(100_000),
            ..Limits::default()
        };
        open
    })
    .expect("open");
    let page = doc.page(0).expect("page");
    // 2000 x 2000 whole is four megapixels, over the cap...
    let mut options = RenderOptions::default();
    options.transform = Affine::scale(10.0);
    assert!(matches!(
        page.render_with(VelloCpuBackend, &options),
        Err(Error::Limit(LimitExceeded::RenderPixels { .. }))
    ));
    // ...but a 256 x 256 tile of it is not.
    options.region = tile_at(512, 512, 256, 256);
    assert!(page.render_with(VelloCpuBackend, &options).is_ok());
}

#[test]
fn cancelling_one_render_leaves_the_document_usable() {
    let doc = Document::open("../../benches/corpus/text_quick_start.pdf").expect("open");
    let page = doc.page(0).expect("page");
    let options = RenderOptions::scaled(2.0);
    let mut session = RenderSession::new();

    // A viewer keeps a clone of the stop to raise; the render sees the other.
    let stale = Deadline::manual();
    session.set_deadline(Some(stale.clone()));
    stale.stop();
    let error = page
        .render_on(VelloCpuBackend, &options, &mut session)
        .expect_err("cancelled before it began");
    assert!(matches!(
        error,
        Error::Limit(LimitExceeded::Stopped {
            during: Operation::Render,
            page: Some(_),
        })
    ));

    // The document is untouched: the next render, with a fresh stop that has
    // not been raised, succeeds and so does everything else.
    session.set_deadline(Some(Deadline::manual()));
    let fine = page
        .render_on(VelloCpuBackend, &options, &mut session)
        .expect("the next render");
    session.set_deadline(None);
    let plain = page.render_with(VelloCpuBackend, &options).expect("plain");
    assert_eq!(fine.data(), plain.data());
    assert!(page.text().char_count() > 0);
    assert!(doc.page(0).is_ok());
}

#[test]
fn a_prepared_tile_is_cancelled_mid_scroll_and_the_next_tile_draws() {
    let doc = Document::open("../../benches/corpus/text_quick_start.pdf").expect("open");
    let mut session = RenderSession::new();
    let prepared = doc
        .page(0)
        .expect("page")
        .prepare(&RenderOptions::scaled(4.0), &mut session);

    let stale = Deadline::manual();
    session.set_deadline(Some(stale.clone()));
    stale.stop();
    assert!(matches!(
        prepared.render_region_on(VelloCpuBackend, &mut session, tile_at(0, 0, 256, 256)),
        Err(Error::Limit(LimitExceeded::Stopped { .. }))
    ));

    session.set_deadline(Some(Deadline::manual()));
    let tile = prepared
        .render_region_on(VelloCpuBackend, &mut session, tile_at(256, 256, 256, 256))
        .expect("the tile that is still on screen");
    assert_eq!((tile.width(), tile.height()), (256, 256));
}

#[test]
fn a_session_deadline_that_passes_mid_render_stops_it() {
    // A budget of nothing has passed before the first object is drawn.
    let doc = Document::open("../../benches/corpus/text_quick_start.pdf").expect("open");
    let mut session = RenderSession::new();
    let prepared = doc
        .page(0)
        .expect("page")
        .prepare(&RenderOptions::scaled(2.0), &mut session);
    session.set_deadline(Some(Deadline::after(std::time::Duration::ZERO)));
    assert!(matches!(
        prepared.render_on(VelloCpuBackend, &mut session),
        Err(Error::Limit(LimitExceeded::Time { .. }))
    ));
    session.set_deadline(None);
    assert!(prepared.render_on(VelloCpuBackend, &mut session).is_ok());
}
