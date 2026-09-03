//! The oracle's thumbnail assertions: the raw and decoded byte counts of
//! each `/Thumb`, and the decoded picture against the oracle's own PNG.

use pdfrum::{Document, Pixmap};

fn open(name: &str) -> pdfrum::Result<Document> {
    Document::open(format!("tests/fixtures/{name}.pdf"))
}

/// The oracle's expectation PNG as RGBA rows, whatever its colour type.
fn expected_rgba(name: &str) -> Result<(u32, u32, Vec<u8>), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(format!("tests/fixtures/{name}.png"))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or("no buffer size")?];
    let info = reader.next_frame(&mut buffer)?;
    buffer.truncate(info.buffer_size());
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Rgb => buffer
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Rgba => buffer,
        png::ColorType::Indexed => return Err(format!("{name}: an indexed PNG").into()),
    };
    Ok((info.width, info.height, rgba))
}

/// The worst per-channel gap between the picture and the oracle's, and how
/// many bytes differ at all.
fn gap(pixmap: &Pixmap, expected: &(u32, u32, Vec<u8>)) -> (u8, usize) {
    let (width, height, rgba) = expected;
    assert_eq!((pixmap.width(), pixmap.height()), (*width, *height), "size");
    assert_eq!(pixmap.data().len(), rgba.len(), "byte count");
    pixmap
        .data()
        .iter()
        .zip(rgba)
        .map(|(ours, theirs)| ours.abs_diff(*theirs))
        .fold((0, 0), |(worst, differing), d| {
            (worst.max(d), differing + usize::from(d > 0))
        })
}

#[test]
fn a_filtered_thumbnail_reports_its_raw_and_decoded_sizes() {
    let doc = open("simple_thumbnail").unwrap();
    let first = doc.page(0).unwrap();
    assert_eq!(first.thumbnail_raw().unwrap().len(), 1851);
    assert_eq!(first.thumbnail_data().unwrap().len(), 1138);
    let second = doc.page(1).unwrap();
    assert_eq!(second.thumbnail_raw().unwrap().len(), 1792);
    assert_eq!(second.thumbnail_data().unwrap().len(), 1110);
}

#[test]
fn an_unfiltered_thumbnail_is_the_same_bytes_raw_and_decoded() {
    let doc = open("thumbnail_with_no_filters").unwrap();
    let page = doc.page(0).unwrap();
    let raw = page.thumbnail_raw().unwrap();
    assert_eq!(raw.len(), 301);
    assert_eq!(page.thumbnail_data().unwrap(), raw);
}

#[test]
fn a_page_without_a_thumbnail_answers_nothing() {
    let doc = open("hello_world").unwrap();
    let page = doc.page(0).unwrap();
    assert!(page.thumbnail_raw().is_none());
    assert!(page.thumbnail_data().is_none());
    assert!(page.thumbnail().is_none());
}

#[test]
fn the_unfiltered_thumbnail_is_the_oracles_picture_exactly() {
    let doc = open("thumbnail_with_no_filters").unwrap();
    let pixmap = doc.page(0).unwrap().thumbnail().unwrap();
    let expected = expected_rgba("thumbnail_with_no_filters").unwrap();
    assert_eq!(gap(&pixmap, &expected), (0, 0));
}

#[test]
fn the_jpeg_thumbnails_are_the_oracles_pictures_to_within_a_count() {
    // The thumbnails are DCT-encoded, and two JPEG decoders' inverse
    // transforms round a few samples one count apart; the oracle's md5 is
    // its own decoder's answer, so the picture is pinned to within that.
    let doc = open("simple_thumbnail").unwrap();
    for (page, name) in [(0, "simple_thumbnail0"), (1, "simple_thumbnail1")] {
        let pixmap = doc.page(page).unwrap().thumbnail().unwrap();
        let expected = expected_rgba(name).unwrap();
        let (worst, differing) = gap(&pixmap, &expected);
        assert!(worst <= 1, "{name}: a sample {worst} counts off");
        assert!(differing <= 20, "{name}: {differing} bytes differ");
    }
}

#[test]
fn a_thumbnail_that_is_not_a_stream_is_no_thumbnail() {
    // The fixture's `/Thumb` is a dictionary with a filter list and no data.
    let doc = open("thumbnail_with_empty_stream").unwrap();
    let page = doc.page(0).unwrap();
    assert!(page.thumbnail_raw().is_none());
    assert!(page.thumbnail().is_none());
}

#[test]
fn reading_the_thumbnail_does_not_alter_the_page() {
    let doc = open("simple_thumbnail").unwrap();
    let page = doc.page(0).unwrap();
    let before = page.thumbnail_raw().unwrap();
    assert!(page.thumbnail().is_some());
    assert_eq!(page.thumbnail_raw().unwrap(), before);
}
