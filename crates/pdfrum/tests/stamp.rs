//! `DocEdit::stamp_text` and `DocEdit::stamp_image`: a mark on every page,
//! appended after the page's own content, placed as the page is displayed,
//! reopened through our own parser and — when the checkout is there —
//! through the oracle's `pdfium_test --txt` and `--md5`.

#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use pdfrum::{
    Color, Document, IdSource, PageObject, PixelFormat, RenderOptions, SaveOptions, StampOptions,
    StampPosition, VelloCpuBackend,
};

const HELLO: &str = "tests/fixtures/hello_world.pdf";
/// Two pages sharing one content stream and one resource dictionary.
const TWO_PAGES: &str = "tests/fixtures/hello_world_2_pages.pdf";

fn reproducible() -> SaveOptions {
    SaveOptions {
        id_source: IdSource::Fixed([0x55; 16]),
        ..SaveOptions::default()
    }
}

fn saved(edit: &pdfrum::DocEdit<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    edit.write_to(&mut out, &reproducible()).unwrap();
    out
}

fn reopen(bytes: Vec<u8>) -> Document {
    Document::from_bytes(Arc::from(bytes)).unwrap()
}

fn stamped(path: &str, text: &str, options: &StampOptions) -> Document {
    let doc = Document::open(path).unwrap();
    let mut edit = doc.edit();
    edit.stamp_text(text, options).unwrap();
    reopen(saved(&edit))
}

/// The one word of `page` spelled `text`, as its box in page space.
fn word_box(page: &pdfrum::Page<'_>, text: &str) -> pdfrum::Rect {
    let Some(word) = page.words().into_iter().find(|word| word.text == text) else {
        panic!("no word {text:?} on page {}", page.index());
    };
    word.rect
}

fn pixels(doc: &Document, index: u32) -> Vec<u8> {
    doc.page(index)
        .unwrap()
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .unwrap()
        .data()
        .to_vec()
}

#[test]
fn a_text_stamp_lands_on_every_page_after_the_content_it_had() {
    let original = Document::open(TWO_PAGES).unwrap();
    let doc = stamped(TWO_PAGES, "DRAFT", &StampOptions::default());
    for index in 0..2 {
        let before = original.page(index).unwrap().edit();
        let after = doc.page(index).unwrap().edit();
        assert_eq!(after.len(), before.len() + 1, "page {index}");
        // The original objects come first, unchanged in kind and order; the
        // stamp is the last thing painted.
        for (i, object) in before.objects().iter().enumerate() {
            assert_eq!(
                std::mem::discriminant(&after.objects()[i]),
                std::mem::discriminant(object)
            );
        }
        let PageObject::Text(text) = after.objects().last().unwrap() else {
            panic!("the stamp is text");
        };
        assert_eq!(&*text.object.segments[0].codes, b"DRAFT");
        assert!(
            doc.page(index)
                .unwrap()
                .text()
                .to_string()
                .contains("DRAFT")
        );
        assert_ne!(
            pixels(&doc, index),
            pixels(&original, index),
            "page {index} changed"
        );
    }
}

#[test]
fn the_original_content_stream_is_carried_through_untouched() {
    let original = Document::open(HELLO).unwrap();
    let original_contents = original
        .parser()
        .page(pdfrum::PageIndex::from(0u32))
        .unwrap()
        .dict
        .reference(&pdfrum::Name::from("Contents"))
        .expect("the fixture's page names one stream");

    let doc = stamped(HELLO, "DRAFT", &StampOptions::default());
    let page = doc.parser().page(pdfrum::PageIndex::from(0u32)).unwrap();
    // Two streams now: the fixture's own — the same object it always was,
    // its bytes copied through — and the stamp's after it.
    let contents = page
        .dict
        .array(&pdfrum::Name::from("Contents"), doc.parser())
        .expect("an array of two");
    assert_eq!(contents.len(), 2);
    assert_eq!(contents.reference_at(0), Some(original_contents));

    // The fixture's two objects are in stream 0, as they were and where they
    // were; the stamp is in stream 1. (The save flate-compresses the
    // fixture's stream, so its bytes are compared decoded, through the
    // objects they draw.)
    let objects = doc.page(0).unwrap().edit();
    let streams: Vec<Option<usize>> = objects
        .objects()
        .iter()
        .map(pdfrum::PageObject::content_stream)
        .collect();
    assert_eq!(streams, [Some(0), Some(0), Some(1)]);
    let was = original.page(0).unwrap().edit();
    for (before, after) in was.objects().iter().zip(objects.objects()) {
        let (PageObject::Text(before), PageObject::Text(after)) = (before, after) else {
            panic!("the fixture draws text");
        };
        assert_eq!(before.object.segments, after.object.segments);
        assert_eq!(before.object.position, after.object.position);
        assert_eq!(before.object.matrix, after.object.matrix);
    }
}

#[test]
fn the_centre_position_centres_the_word_on_the_crop_box() {
    let doc = stamped(HELLO, "MIDDLE", &StampOptions::default());
    let page = doc.page(0).unwrap();
    let crop = page.crop_box();
    let word = word_box(&page, "MIDDLE");
    assert!(
        (word.center().x - crop.center().x).abs() < 2.0,
        "{word:?} vs {crop:?}"
    );
    assert!(
        (word.center().y - crop.center().y).abs() < 4.0,
        "{word:?} vs {crop:?}"
    );
}

#[test]
fn a_corner_position_sits_inside_the_margin() {
    let options = StampOptions {
        position: StampPosition::BottomRight,
        margin: 20.0,
        font_size: 12.0,
        ..StampOptions::default()
    };
    let doc = stamped(HELLO, "CORNER", &options);
    let page = doc.page(0).unwrap();
    let crop = page.crop_box();
    let word = word_box(&page, "CORNER");
    assert!(
        (word.x1 - (crop.x1 - 20.0)).abs() < 2.0,
        "{word:?} vs {crop:?}"
    );
    assert!(
        (word.y0 - (crop.y0 + 20.0)).abs() < 4.0,
        "{word:?} vs {crop:?}"
    );

    let options = StampOptions {
        position: StampPosition::TopLeft,
        ..options
    };
    let doc = stamped(HELLO, "CORNER", &options);
    let page = doc.page(0).unwrap();
    let word = word_box(&page, "CORNER");
    assert!(
        (word.x0 - (crop.x0 + 20.0)).abs() < 2.0,
        "{word:?} vs {crop:?}"
    );
    assert!(
        (word.y1 - (crop.y1 - 20.0)).abs() < 4.0,
        "{word:?} vs {crop:?}"
    );
}

#[test]
fn an_angle_turns_the_text_matrix_about_the_centre() {
    let doc = stamped(
        HELLO,
        "TILT",
        &StampOptions {
            angle: 90.0,
            ..StampOptions::default()
        },
    );
    let page = doc.page(0).unwrap();
    let objects = page.objects();
    let PageObject::Text(text) = objects.objects.last().unwrap() else {
        panic!("the stamp is text");
    };
    let [a, b, c, d, _, _] = text.object.matrix.as_coeffs();
    // A quarter turn: (a b c d) = (0 s -s 0) up to the size.
    assert!(a.abs() < 1e-3 && d.abs() < 1e-3, "{:?}", text.object.matrix);
    assert!(b > 0.99 && (b + c).abs() < 1e-3, "{:?}", text.object.matrix);
    // Still centred: a turn about the box's own centre moves nothing.
    let word = word_box(&page, "TILT");
    let crop = page.crop_box();
    assert!((word.center().x - crop.center().x).abs() < 4.0, "{word:?}");
    assert!((word.center().y - crop.center().y).abs() < 4.0, "{word:?}");
}

#[test]
fn opacity_and_colour_reach_the_saved_state() {
    let doc = stamped(
        HELLO,
        "FAINT",
        &StampOptions {
            opacity: 0.5,
            color: Color::from_rgb8(255, 0, 0),
            ..StampOptions::default()
        },
    );
    let page = doc.page(0).unwrap();
    let objects = page.objects();
    let PageObject::Text(text) = objects.objects.last().unwrap() else {
        panic!("the stamp is text");
    };
    assert!((text.state.general.fill_alpha - 0.5).abs() < 1e-6);
    let rgb = text.state.fill.to_rgb().unwrap();
    assert!(rgb.r > 0.99 && rgb.g < 0.01 && rgb.b < 0.01, "{rgb:?}");
}

#[test]
fn a_rotated_page_places_the_stamp_as_displayed() {
    // A page turned a quarter clockwise: its displayed bottom-right corner
    // is the page-space top-right one, and upright text there is drawn
    // turned a quarter counter-clockwise in page space.
    let doc = Document::open(HELLO).unwrap();
    let mut edit = doc.edit();
    edit.set_rotation(0, 90).unwrap();
    let turned = reopen(saved(&edit));
    assert_eq!(turned.page(0).unwrap().rotation().degrees(), 90);

    let mut edit = turned.edit();
    edit.stamp_text(
        "UP",
        &StampOptions {
            position: StampPosition::BottomRight,
            margin: 10.0,
            font_size: 12.0,
            ..StampOptions::default()
        },
    )
    .unwrap();
    let doc = reopen(saved(&edit));
    let page = doc.page(0).unwrap();
    let crop = page.crop_box();
    let word = word_box(&page, "UP");
    assert!(
        (word.x1 - (crop.x1 - 10.0)).abs() < 4.0,
        "{word:?} vs {crop:?}"
    );
    assert!(
        (word.y1 - (crop.y1 - 10.0)).abs() < 4.0,
        "{word:?} vs {crop:?}"
    );
    let objects = page.objects();
    let PageObject::Text(text) = objects.objects.last().unwrap() else {
        panic!("the stamp is text");
    };
    let [a, b, _, _, _, _] = text.object.matrix.as_coeffs();
    assert!(a.abs() < 1e-3 && b > 0.99, "{:?}", text.object.matrix);
}

#[test]
fn two_stamps_accumulate_and_each_keeps_its_font() {
    let doc = Document::open(TWO_PAGES).unwrap();
    let mut edit = doc.edit();
    edit.stamp_text("ONE", &StampOptions::default()).unwrap();
    edit.stamp_text(
        "TWO",
        &StampOptions {
            position: StampPosition::TopLeft,
            ..StampOptions::default()
        },
    )
    .unwrap();
    let saved = reopen(saved(&edit));
    for index in 0..2 {
        let page = saved.page(index).unwrap();
        let text = page.text().to_string();
        assert!(
            text.contains("ONE") && text.contains("TWO"),
            "page {index}: {text}"
        );
        let edit = page.edit();
        assert_eq!(edit.len(), doc.page(index).unwrap().edit().len() + 2);
        // Each stamp still finds its font: the second did not sweep the
        // first's resource name away.
        assert!(edit.font_of(edit.len() - 1).is_some(), "TWO's font");
        assert!(edit.font_of(edit.len() - 2).is_some(), "ONE's font");
    }
}

#[test]
fn an_image_stamp_keeps_its_aspect_and_carries_its_opacity() {
    let doc = Document::open(TWO_PAGES).unwrap();
    let mut edit = doc.edit();
    // Four by two, red.
    let image = edit
        .embed_image(&[255, 0, 0].repeat(8), 4, 2, PixelFormat::Rgb8)
        .unwrap();
    edit.stamp_image(
        &image,
        200.0,
        &StampOptions {
            position: StampPosition::TopRight,
            margin: 10.0,
            opacity: 0.25,
            ..StampOptions::default()
        },
    )
    .unwrap();
    let saved = reopen(saved(&edit));
    for index in 0..2 {
        let page = saved.page(index).unwrap();
        let objects = page.objects();
        let PageObject::Image(placed) = objects.objects.last().unwrap() else {
            panic!("the stamp is an image");
        };
        let [scale_x, skew_y, skew_x, scale_y, origin_x, origin_y] =
            placed.object.matrix.as_coeffs();
        assert!(
            (scale_x - 200.0).abs() < 1e-3 && (scale_y - 100.0).abs() < 1e-3,
            "{scale_x} {scale_y}"
        );
        assert!(skew_y.abs() < 1e-6 && skew_x.abs() < 1e-6);
        let crop = page.crop_box();
        assert!((origin_x + 200.0 - (crop.x1 - 10.0)).abs() < 1e-3);
        assert!((origin_y + 100.0 - (crop.y1 - 10.0)).abs() < 1e-3);
        assert!((placed.state.general.fill_alpha - 0.25).abs() < 1e-6);
        assert_ne!(pixels(&saved, index), pixels(&doc, index));
    }
}

#[test]
fn an_image_stamp_with_no_width_is_refused() {
    let doc = Document::open(HELLO).unwrap();
    let mut edit = doc.edit();
    let image = edit
        .embed_image(&[0, 0, 0], 1, 1, PixelFormat::Rgb8)
        .unwrap();
    assert!(
        edit.stamp_image(&image, 0.0, &StampOptions::default())
            .is_err()
    );
}

#[test]
fn a_reproducible_stamped_save_is_byte_identical() {
    let doc = Document::open(HELLO).unwrap();
    let mut edit = doc.edit();
    edit.stamp_text("SAME", &StampOptions::default()).unwrap();
    assert_eq!(saved(&edit), saved(&edit));
}

// ---- the oracle

/// The oracle's `pdfium_test`, when this machine has one: `$PDFRUM_ORACLE_BIN`,
/// else `$PDFRUM_ORACLE_CHECKOUT/out/Release/pdfium_test`, else the sibling
/// `../pdfium-c++` checkout. The same six lines as `embed_image.rs`, for the
/// same reason.
fn oracle_bin() -> Option<PathBuf> {
    let bin = std::env::var_os("PDFRUM_ORACLE_BIN").map_or_else(
        || {
            let checkout = std::env::var_os("PDFRUM_ORACLE_CHECKOUT").map_or_else(
                || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pdfium-c++"),
                PathBuf::from,
            );
            checkout.join("out/Release/pdfium_test")
        },
        PathBuf::from,
    );
    bin.is_file().then_some(bin)
}

/// `pdfium_test --md5 --png` and then `--txt` over `name` in `dir` — the two
/// outputs cannot be asked for in one run: the md5 line, and the page-0
/// text it wrote (UTF-32LE, decoded).
fn oracle_md5_and_text(bin: &Path, dir: &Path, name: &str) -> (String, String) {
    let run = |args: &[&str]| {
        let out = Command::new(bin)
            .args(args)
            .arg(name)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let stdout = run(&["--md5", "--png", "--pages=0"]);
    run(&["--txt", "--pages=0"]);
    let md5 = stdout
        .lines()
        .find(|line| line.contains("MD5:"))
        .unwrap_or_else(|| panic!("no MD5 line in {stdout}"))
        .rsplit(':')
        .next()
        .unwrap()
        .trim()
        .to_owned();
    let raw = std::fs::read(dir.join(format!("{name}.0.txt"))).unwrap();
    let text: String = raw
        .chunks_exact(4)
        .map(|unit| u32::from_le_bytes([unit[0], unit[1], unit[2], unit[3]]))
        .map(|cp| char::from_u32(cp).unwrap_or('\u{FFFD}'))
        .collect();
    (md5, text)
}

#[test]
fn the_oracle_extracts_the_stamp_and_renders_it() {
    let Some(bin) = oracle_bin() else {
        eprintln!("pdfium_test is absent; skipping the oracle round trip");
        return;
    };
    let dir = std::env::temp_dir().join("pdfrum-stamp-oracle");
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(HELLO, dir.join("plain.pdf")).unwrap();

    let doc = Document::open(HELLO).unwrap();
    let mut edit = doc.edit();
    edit.stamp_text(
        "CONFIDENTIAL",
        &StampOptions {
            angle: 30.0,
            opacity: 0.4,
            font_size: 48.0,
            color: Color::from_rgb8(200, 0, 0),
            ..StampOptions::default()
        },
    )
    .unwrap();
    std::fs::write(dir.join("stamped.pdf"), saved(&edit)).unwrap();

    let (plain_md5, plain_text) = oracle_md5_and_text(&bin, &dir, "plain.pdf");
    let (stamped_md5, stamped_text) = oracle_md5_and_text(&bin, &dir, "stamped.pdf");
    assert!(!plain_text.contains("CONFIDENTIAL"));
    assert!(stamped_text.contains("CONFIDENTIAL"), "{stamped_text}");
    assert!(
        stamped_text.contains("Hello, world!"),
        "the page's own text is still there: {stamped_text}"
    );
    assert_ne!(plain_md5, stamped_md5, "the oracle painted the stamp");
}
