//! The oracle's flatten assertions: what flattening reports, what the saved
//! document keeps, and that the flattened page draws what the annotated one
//! drew.

use pdfrum::{Document, FlattenMode, Flattened, RenderOptions, SaveOptions, VelloCpuBackend};
use std::sync::Arc;

fn open(name: &str) -> pdfrum::Result<Document> {
    Document::open(format!("tests/fixtures/{name}.pdf"))
}

fn saved(edit: &pdfrum::DocEdit<'_>) -> pdfrum::Result<(Vec<u8>, Document)> {
    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())?;
    let doc = Document::from_bytes(Arc::from(out.clone()))?;
    Ok((out, doc))
}

fn with_annotations() -> RenderOptions {
    let mut options = RenderOptions::default();
    options.annotations = true;
    options
}

/// Flattens page 0 for print and asserts the saved page, drawn without
/// annotations, is the oracle's picture `expected` to within the two
/// engines' anti-aliasing.
fn flattened_matches(name: &str, expected: &str) -> Result<(), Box<dyn std::error::Error>> {
    let doc = open(name)?;
    let mut edit = doc.edit();
    assert_eq!(edit.flatten(0, FlattenMode::Print)?, Flattened::Done);
    let (_, flat) = saved(&edit)?;
    let plain = flat.page(0)?.render(VelloCpuBackend)?;
    let (width, height, rgba) = expected_rgba(expected)?;
    assert_eq!(
        (plain.width(), plain.height()),
        (width, height),
        "{name} size"
    );
    let differing = plain
        .data()
        .iter()
        .zip(&rgba)
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        differing * 200 <= rgba.len(),
        "{name}: {differing} of {} bytes differ from {expected}",
        rgba.len()
    );
    Ok(())
}

#[test]
fn a_page_without_annotations_has_nothing_to_do() {
    let doc = open("hello_world").unwrap();
    assert_eq!(
        doc.edit().flatten(0, FlattenMode::Display).unwrap(),
        Flattened::NothingToDo
    );
}

#[test]
fn a_page_with_annotations_flattens_for_display_and_for_print() {
    let doc = open("annotiter").unwrap();
    assert_eq!(
        doc.edit().flatten(0, FlattenMode::Display).unwrap(),
        Flattened::Done
    );
    assert_eq!(
        doc.edit().flatten(0, FlattenMode::Print).unwrap(),
        Flattened::Done
    );
}

#[test]
fn a_font_with_a_bad_base_encoding_loses_its_encoding() {
    let doc = open("344775293").unwrap();
    let mut edit = doc.edit();
    assert_eq!(
        edit.flatten(0, FlattenMode::Print).unwrap(),
        Flattened::Done
    );
    let (bytes, _) = saved(&edit).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("/PDFDocEncoding"));
}

#[test]
fn a_font_with_differences_and_no_base_encoding_keeps_them() {
    let doc = open("363015187").unwrap();
    let mut edit = doc.edit();
    assert_eq!(
        edit.flatten(0, FlattenMode::Print).unwrap(),
        Flattened::Done
    );
    let (bytes, _) = saved(&edit).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("/Differences"));
}

/// The oracle's expectation PNG as RGBA.
fn expected_rgba(name: &str) -> Result<(u32, u32, Vec<u8>), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(format!("tests/fixtures/{name}.png"))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or("no buffer size")?];
    let info = reader.next_frame(&mut buffer)?;
    buffer.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgb => buffer
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        png::ColorType::Rgba => buffer,
        png::ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[v, a]| [v, v, v, a])
            .collect(),
        png::ColorType::Indexed => return Err(format!("{name}: an indexed PNG").into()),
    };
    Ok((info.width, info.height, rgba))
}

/// The pixels darker than mid-grey: where ink is, whatever its anti-aliasing.
fn dark_pixels(pixmap: &pdfrum::Pixmap) -> Vec<usize> {
    pixmap
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, p)| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) < 384)
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn the_flattened_page_draws_what_the_annotated_page_drew() {
    // The oracle's expectation for each is its render of the annotated page,
    // and it asserts the flattened save renders the same.
    for name in ["bug_890322", "bug_896366"] {
        flattened_matches(name, &format!("{name}_agg")).unwrap();
    }
}

#[test]
fn a_malformed_media_box_flattens_to_the_oracles_picture() {
    // The oracle's own expectation for this file's flattened form: the text
    // field's value, no field highlight, over a media box written with a
    // negative height.
    flattened_matches("bug_889099", "bug_889099_flattened_agg").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn a_flattened_page_is_not_blank() {
    // `[oracle-bug]` crbug.com/861842: the reference renders this page blank
    // after flattening — the check box's appearance stream has no `/BBox`,
    // so the reference skips it — and its own test says so ("This should
    // not render blank"). The flattened page keeps the ink the annotated one
    // drew; only the field highlight, which is not page content, goes.
    let doc = open("bug_861842").unwrap();
    let annotated = doc
        .page(0)
        .unwrap()
        .render_with(VelloCpuBackend, &with_annotations())
        .unwrap();
    let mut edit = doc.edit();
    assert_eq!(
        edit.flatten(0, FlattenMode::Print).unwrap(),
        Flattened::Done
    );
    let (_, flat) = saved(&edit).unwrap();
    let plain = flat.page(0).unwrap().render(VelloCpuBackend).unwrap();
    let before = dark_pixels(&annotated);
    let after = dark_pixels(&plain);
    assert!(!after.is_empty(), "the flattened page is blank");
    let shared = after.iter().filter(|p| before.contains(p)).count();
    assert!(
        shared * 10 >= before.len() * 9,
        "{shared} of {} ink pixels kept",
        before.len()
    );
    assert!(
        after.len() * 10 <= before.len() * 15,
        "{} ink pixels from {}",
        after.len(),
        before.len()
    );
}

#[test]
fn flattening_a_forms_only_page_removes_the_form() {
    let doc = open("text_form").unwrap();
    let mut edit = doc.edit();
    assert_eq!(
        edit.flatten(0, FlattenMode::Display).unwrap(),
        Flattened::Done
    );
    let (_, flat) = saved(&edit).unwrap();
    assert!(flat.form().is_none());
}

#[test]
fn a_widget_another_page_shares_keeps_the_form_until_that_page_goes_too() {
    for name in ["bug_498010830_shared_annots", "bug_498010830_shared_widget"] {
        let doc = open(name).unwrap();
        let mut edit = doc.edit();
        assert_eq!(
            edit.flatten(0, FlattenMode::Display).unwrap(),
            Flattened::Done
        );
        let (_, flat) = saved(&edit).unwrap();
        assert!(flat.form().is_some(), "{name}: the form survives page 0");
        assert_eq!(
            edit.flatten(1, FlattenMode::Display).unwrap(),
            Flattened::Done
        );
        let (_, flat) = saved(&edit).unwrap();
        assert!(flat.form().is_none(), "{name}: the form goes with page 1");
    }
}
