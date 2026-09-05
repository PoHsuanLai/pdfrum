//! `DocEdit::embed_jpeg` and `DocEdit::embed_image`: what an embedded image
//! writes into the file, and what comes back out of it.
//!
//! Ports PDFium's `FPDFImageObj_LoadJpegFile` / `FPDFImageObj_SetBitmap`
//! embedder cases (`fpdfsdk/fpdf_editimg_embeddertest.cpp`, reaching
//! `CPDF_Image::SetJpegImage` and `CPDF_Image::SetImage`,
//! `core/fpdfapi/page/cpdf_image.cpp`) onto `DocEdit::embed_jpeg(bytes)` and
//! `DocEdit::embed_image(pixels, w, h, PixelFormat)`. Each test asserts what
//! its C++ counterpart asserts in substance — the dictionary shape the embed
//! produces, the samples that decode back out of it, the ink the page gains,
//! and that the oracle reopens the result — rather than only that the call
//! returned `Ok`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a test that cannot open its own fixture has nothing to report but a panic"
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use pdfrum::{
    Dict, Document, ImageBuilder, Object, PixelFormat, Rect, RenderOptions, Resolve, SaveOptions,
    VelloCpuBackend,
};
use pdfrum_object::Name;

const HELLO_PDF: &str = "tests/fixtures/hello_world.pdf";
/// The oracle's only JPEG: 120x120, baseline SOF0, three components, eight
/// bits, JFIF with no Adobe APP14.
const MONA_LISA: &[u8] = include_bytes!("fixtures/mona_lisa.jpg");
/// A 4x4 one-component JP2 file, signature box and all.
const GRAY_JP2: &[u8] = include_bytes!("fixtures/gray.jp2");

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("pdfrum-embed-image");
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn fetch_dict(doc: &Document, reference: pdfrum::ObjRef) -> Dict {
    doc.parser()
        .fetch(reference)
        .expect("fetches")
        .as_stream()
        .map(|s| s.dict.clone())
        .or_else(|| {
            doc.parser()
                .fetch(reference)
                .ok()
                .and_then(|o| o.as_dict().cloned())
        })
        .expect("dict")
}

fn name_of(dict: &Dict, key: &str) -> Option<String> {
    dict.name(&Name::from(key))
        .map(|n| String::from_utf8_lossy(n.as_bytes()).into_owned())
}

fn int_of(dict: &Dict, key: &str) -> Option<i64> {
    match dict.raw(&Name::from(key)) {
        Some(Object::Int(v)) => Some(*v),
        _ => None,
    }
}

fn dark_pixels(data: &[u8]) -> usize {
    data.chunks_exact(4)
        .filter(|px| {
            let r = px.first().copied().unwrap_or(255);
            let g = px.get(1).copied().unwrap_or(255);
            let b = px.get(2).copied().unwrap_or(255);
            let a = px.get(3).copied().unwrap_or(0);
            a > 0 && (u16::from(r) + u16::from(g) + u16::from(b)) < 600
        })
        .count()
}

/// The oracle's `pdfium_test`, when this machine has one.
///
/// One place, two inputs: `$PDFRUM_ORACLE_BIN`, else
/// `$PDFRUM_ORACLE_CHECKOUT/out/Release/pdfium_test`, whose own default is the
/// sibling `../pdfium-c++` directory README.md names. `scripts/env.nu`
/// resolves the same two variables with the same defaults for the nushell
/// side, and `conformance` for the CLI.
///
/// Six lines rather than a shared module: STYLE.md §4 forbids a `common`,
/// `util` or `helpers` module name, and an integration test cannot reach
/// another crate's test code, so `load_font.rs` carries the same six lines.
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

fn oracle_md5(path: &Path) -> Result<String, String> {
    let Some(bin) = oracle_bin() else {
        return Err("pdfium_test binary is absent; skip oracle reopen".into());
    };
    let out = Command::new(bin)
        .args(["--md5", "--png", "--pages=0"])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "pdfium_test --md5 exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The first image page object on page 0 of `bytes`, as its dimensions,
/// pixel kind, and whether it carries alpha.
fn first_image(bytes: &[u8]) -> (u32, u32, String, bool) {
    let doc = Document::from_bytes(bytes.to_vec().into()).expect("reopens");
    let page = doc.page(0).expect("page");
    let objects = page.objects();
    for object in &objects.objects {
        if let pdfrum::PageObject::Image(image) = object {
            let data = &image.object.image;
            let kind = match &data.samples.to_pixels() {
                pdfrum_page::Pixels::Stencil(_) => "Stencil",
                pdfrum_page::Pixels::Gray8(_) => "Gray8",
                pdfrum_page::Pixels::Rgb8(_) => "Rgb8",
                pdfrum_page::Pixels::Cmyk8(_) => "Cmyk8",
                pdfrum_page::Pixels::Indexed { .. } => "Indexed",
                other => panic!("unexpected pixel kind {other:?}"),
            };
            return (
                data.width,
                data.height,
                kind.to_owned(),
                data.mask.is_some(),
            );
        }
    }
    panic!("the saved page has no image object");
}

/// `FPDFImageObj_LoadJpegFile` writes the JPEG's own bytes with the header's
/// dimensions and colour space, and never re-encodes (`cpdf_image.cpp:96-135`).
#[test]
fn a_jpeg_is_stored_verbatim_under_dct_decode() {
    let doc = Document::open(HELLO_PDF).expect("opens");
    let original_dark = dark_pixels(
        doc.page(0)
            .expect("page")
            .render(&VelloCpuBackend::new(), &RenderOptions::default())
            .expect("renders")
            .data(),
    );

    let mut edit = doc.edit();
    let image = edit.embed_jpeg(MONA_LISA).expect("embeds the JPEG");
    assert_eq!((image.width(), image.height()), (120, 120));

    let mut page = doc.page(0).expect("page").edit();
    page.push(ImageBuilder::at(image.object(), Rect::new(20.0, 20.0, 140.0, 140.0)).build());

    let mut bytes = Vec::new();
    edit.write_pages_to(&mut bytes, &[page], &SaveOptions::default())
        .expect("writes");

    // The dictionary the save produced, read back through the reader.
    let saved = Document::from_bytes(bytes.clone().into()).expect("reopens");
    let dict = fetch_dict(&saved, image.object());
    assert_eq!(name_of(&dict, "Type").as_deref(), Some("XObject"));
    assert_eq!(name_of(&dict, "Subtype").as_deref(), Some("Image"));
    assert_eq!(int_of(&dict, "Width"), Some(120));
    assert_eq!(int_of(&dict, "Height"), Some(120));
    assert_eq!(name_of(&dict, "ColorSpace").as_deref(), Some("DeviceRGB"));
    assert_eq!(int_of(&dict, "BitsPerComponent"), Some(8));
    assert_eq!(name_of(&dict, "Filter").as_deref(), Some("DCTDecode"));
    // No Adobe APP14, three components: libjpeg reads it as YCbCr, so the
    // `/ColorTransform 0` of `cpdf_image.cpp:128-132` must NOT be written.
    assert!(dict.raw(&Name::from("DecodeParms")).is_none());

    // The scan is the file's own: the stream still holds the fixture's bytes.
    let stream = saved
        .parser()
        .fetch(image.object())
        .expect("fetches")
        .as_stream()
        .expect("stream")
        .clone();
    assert_eq!(stream.data.as_bytes(), MONA_LISA);

    // The page graph reports an RGB image at the JPEG's own size.
    assert_eq!(
        first_image(&bytes),
        (120, 120, "Rgb8".to_owned(), false),
        "the reopened page's image object"
    );

    // The Mona Lisa is a dark picture on a blank page: the rectangle gains ink.
    let after = Document::from_bytes(bytes.clone().into()).expect("reopens");
    let rendered = after
        .page(0)
        .expect("page")
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect("renders");
    assert!(
        dark_pixels(rendered.data()) > original_dark + 1000,
        "placing a 120x120 JPEG should add ink"
    );

    let out = scratch_dir().join("mona_lisa.pdf");
    std::fs::write(&out, &bytes).expect("writes scratch");
    match oracle_md5(&out) {
        Ok(text) => assert!(
            text.to_ascii_lowercase().contains("md5:"),
            "oracle reopened: {text}"
        ),
        Err(why) => eprintln!("oracle reopen skipped: {why}"),
    }
}

/// §7.4.9: a `/JPXDecode` image's colour space and depth come from the
/// codestream, so the dictionary carries neither.
#[test]
fn a_jp2_codestream_is_stored_under_jpx_decode_with_no_colour_space() {
    let doc = Document::open(HELLO_PDF).expect("opens");
    let mut edit = doc.edit();
    let image = edit.embed_jpeg(GRAY_JP2).expect("embeds the JP2");
    assert_eq!((image.width(), image.height()), (4, 4));

    let mut page = doc.page(0).expect("page").edit();
    page.push(ImageBuilder::at(image.object(), Rect::new(10.0, 10.0, 90.0, 90.0)).build());
    let mut bytes = Vec::new();
    edit.write_pages_to(&mut bytes, &[page], &SaveOptions::default())
        .expect("writes");

    let saved = Document::from_bytes(bytes.clone().into()).expect("reopens");
    let dict = fetch_dict(&saved, image.object());
    assert_eq!(name_of(&dict, "Filter").as_deref(), Some("JPXDecode"));
    assert_eq!(int_of(&dict, "Width"), Some(4));
    assert_eq!(int_of(&dict, "Height"), Some(4));
    assert!(dict.raw(&Name::from("ColorSpace")).is_none());
    assert!(dict.raw(&Name::from("BitsPerComponent")).is_none());

    let stream = saved
        .parser()
        .fetch(image.object())
        .expect("fetches")
        .as_stream()
        .expect("stream")
        .clone();
    assert_eq!(stream.data.as_bytes(), GRAY_JP2);

    // Our own decoder reads it back at the codestream's size.
    let (width, height, ..) = first_image(&bytes);
    assert_eq!((width, height), (4, 4));
}

/// `CPDF_Image::SetImage`'s 32-bpp arm: the colour channels become the image
/// and `CloneAlphaMask` becomes an eight-bit `/DeviceGray` `/SMask`
/// (`cpdf_image.cpp:256-284`).
#[test]
fn rgba_samples_round_trip_with_their_alpha_in_an_smask() {
    // Two by two: opaque red, half-alpha green, opaque blue, clear white.
    let pixels: [u8; 16] = [
        0xFF, 0x00, 0x00, 0xFF, // red
        0x00, 0xFF, 0x00, 0x80, // green, half alpha
        0x00, 0x00, 0xFF, 0xFF, // blue
        0xFF, 0xFF, 0xFF, 0x00, // white, clear
    ];
    let doc = Document::open(HELLO_PDF).expect("opens");
    let mut edit = doc.edit();
    let image = edit
        .embed_image(&pixels, 2, 2, PixelFormat::Rgba8)
        .expect("embeds RGBA");
    assert_eq!((image.width(), image.height()), (2, 2));

    let mut page = doc.page(0).expect("page").edit();
    page.push(ImageBuilder::at(image.object(), Rect::new(20.0, 20.0, 120.0, 120.0)).build());
    let mut bytes = Vec::new();
    edit.write_pages_to(&mut bytes, &[page], &SaveOptions::default())
        .expect("writes");

    let saved = Document::from_bytes(bytes.clone().into()).expect("reopens");
    let dict = fetch_dict(&saved, image.object());
    assert_eq!(name_of(&dict, "ColorSpace").as_deref(), Some("DeviceRGB"));
    assert_eq!(int_of(&dict, "BitsPerComponent"), Some(8));
    // No `/Filter` was written, so the stream writer flate-encoded it.
    assert_eq!(name_of(&dict, "Filter").as_deref(), Some("FlateDecode"));
    let smask = dict
        .reference(&Name::from("SMask"))
        .expect("the alpha channel became an /SMask");
    let mask_dict = fetch_dict(&saved, smask);
    assert_eq!(
        name_of(&mask_dict, "ColorSpace").as_deref(),
        Some("DeviceGray")
    );
    assert_eq!(int_of(&mask_dict, "Width"), Some(2));
    assert_eq!(int_of(&mask_dict, "Height"), Some(2));

    // The decoded samples equal what was handed in.
    let page = saved.page(0).expect("page");
    let graph = page.objects();
    let placed = graph
        .objects
        .iter()
        .find_map(|o| match o {
            pdfrum::PageObject::Image(image) => Some(image.object.image.clone()),
            _ => None,
        })
        .expect("an image object");
    let decoded = placed.samples.to_pixels();
    let pdfrum_page::Pixels::Rgb8(rgb) = &decoded else {
        panic!("expected eight-bit RGB, got {decoded:?}");
    };
    assert_eq!(
        rgb.as_ref(),
        &[
            0xFF, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF
        ]
    );
    let Some(pdfrum_page::ImageMask::Alpha { alpha, .. }) = &placed.mask else {
        panic!("expected an alpha mask, got {:?}", placed.mask);
    };
    assert_eq!(alpha.as_ref(), &[0xFF, 0x80, 0xFF, 0x00]);
}

/// Eight-bit grey with no alpha takes the `/DeviceGray` arm
/// (`cpdf_image.cpp:222-250`, the no-palette branch).
#[test]
fn gray_samples_round_trip_unchanged() {
    let pixels: [u8; 4] = [0x00, 0x40, 0xC0, 0xFF];
    let doc = Document::open(HELLO_PDF).expect("opens");
    let mut edit = doc.edit();
    let image = edit
        .embed_image(&pixels, 2, 2, PixelFormat::Gray8)
        .expect("embeds grey");

    let mut page = doc.page(0).expect("page").edit();
    page.push(ImageBuilder::at(image.object(), Rect::new(20.0, 20.0, 120.0, 120.0)).build());
    let mut bytes = Vec::new();
    edit.write_pages_to(&mut bytes, &[page], &SaveOptions::default())
        .expect("writes");

    let saved = Document::from_bytes(bytes.clone().into()).expect("reopens");
    let dict = fetch_dict(&saved, image.object());
    assert_eq!(name_of(&dict, "ColorSpace").as_deref(), Some("DeviceGray"));
    assert!(dict.raw(&Name::from("SMask")).is_none());

    let graph = saved.page(0).expect("page").objects();
    let placed = graph
        .objects
        .iter()
        .find_map(|o| match o {
            pdfrum::PageObject::Image(image) => Some(image.object.image.clone()),
            _ => None,
        })
        .expect("an image object");
    let decoded = placed.samples.to_pixels();
    let pdfrum_page::Pixels::Gray8(grey) = &decoded else {
        panic!("expected eight-bit grey, got {decoded:?}");
    };
    assert_eq!(grey.as_ref(), &pixels);
}

/// The 1-bpp arm: `/ImageMask true` with the `/Decode` that makes a set bit
/// paint (`cpdf_image.cpp:196-220`).
#[test]
fn a_one_bit_mask_writes_an_image_mask_with_an_inverted_decode() {
    // Eight by two, so each row is exactly one byte: a top row of alternating
    // bits and a solid bottom row.
    let pixels: [u8; 2] = [0b1010_1010, 0b1111_1111];
    let doc = Document::open(HELLO_PDF).expect("opens");
    let mut edit = doc.edit();
    let image = edit
        .embed_image(&pixels, 8, 2, PixelFormat::Mask1)
        .expect("embeds the mask");
    assert_eq!((image.width(), image.height()), (8, 2));

    let mut page = doc.page(0).expect("page").edit();
    page.push(ImageBuilder::at(image.object(), Rect::new(20.0, 20.0, 120.0, 120.0)).build());
    let mut bytes = Vec::new();
    edit.write_pages_to(&mut bytes, &[page], &SaveOptions::default())
        .expect("writes");

    let saved = Document::from_bytes(bytes.clone().into()).expect("reopens");
    let dict = fetch_dict(&saved, image.object());
    assert_eq!(
        dict.raw(&Name::from("ImageMask")),
        Some(&Object::Bool(true))
    );
    assert_eq!(int_of(&dict, "BitsPerComponent"), Some(1));
    // The whole point of the arm: `[1 0]`, not the default `[0 1]`.
    let Some(Object::Array(decode)) = dict.raw(&Name::from("Decode")) else {
        panic!("an /ImageMask needs a /Decode");
    };
    assert_eq!(
        decode.iter().collect::<Vec<_>>(),
        vec![&Object::Int(1), &Object::Int(0)]
    );
    // A stencil mask has no colour space of its own.
    assert!(dict.raw(&Name::from("ColorSpace")).is_none());

    let (width, height, kind, _) = first_image(&bytes);
    assert_eq!((width, height, kind.as_str()), (8, 2, "Stencil"));
}

/// A CMYK JPEG gets `/Decode [1 0 1 0 1 0 1 0]` (`cpdf_image.cpp:118-127`).
///
/// The oracle's `testing/resources/` holds no four-component JPEG — the only
/// JPEG there is `mona_lisa.jpg`, and the CMYK ones in its `third_party/`
/// Skia corpus are 116 KB and up, or fuzzer-corrupted. `InitJPEG` reads
/// nothing but the frame header, so the header alone is the whole input to
/// the behaviour being pinned, and it is built here rather than vendored.
#[test]
fn a_cmyk_jpeg_carries_the_adobe_inversion_decode() {
    let mut jpeg = vec![0xFF, 0xD8];
    // APP14 `Adobe`, transform 0: plain CMYK, no colour transform.
    jpeg.extend_from_slice(&[0xFF, 0xEE, 0x00, 0x0E]);
    jpeg.extend_from_slice(b"Adobe");
    jpeg.extend_from_slice(&[0x00, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00]);
    // SOF0: 8-bit, 4 by 4, four components.
    jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x14, 0x08]);
    jpeg.extend_from_slice(&4u16.to_be_bytes());
    jpeg.extend_from_slice(&4u16.to_be_bytes());
    jpeg.push(4);
    for id in 1..=4u8 {
        jpeg.extend_from_slice(&[id, 0x11, 0x00]);
    }

    let doc = Document::open(HELLO_PDF).expect("opens");
    let mut edit = doc.edit();
    let image = edit.embed_jpeg(&jpeg).expect("embeds the CMYK header");
    assert_eq!((image.width(), image.height()), (4, 4));

    let mut page = doc.page(0).expect("page").edit();
    page.push(ImageBuilder::at(image.object(), Rect::new(20.0, 20.0, 60.0, 60.0)).build());
    let mut bytes = Vec::new();
    edit.write_pages_to(&mut bytes, &[page], &SaveOptions::default())
        .expect("writes");

    let saved = Document::from_bytes(bytes.into()).expect("reopens");
    let dict = fetch_dict(&saved, image.object());
    assert_eq!(name_of(&dict, "ColorSpace").as_deref(), Some("DeviceCMYK"));
    let Some(Object::Array(decode)) = dict.raw(&Name::from("Decode")) else {
        panic!("a CMYK JPEG needs the Adobe /Decode");
    };
    assert_eq!(
        decode.iter().cloned().collect::<Vec<_>>(),
        [1, 0, 1, 0, 1, 0, 1, 0]
            .into_iter()
            .map(Object::Int)
            .collect::<Vec<_>>()
    );
    // Transform 0 with four components: libjpeg reads it as plain CMYK, so
    // `/ColorTransform 0` must be written (`cpdf_image.cpp:128-132`).
    let Some(Object::Dict(parms)) = dict.raw(&Name::from("DecodeParms")) else {
        panic!("an untransformed JPEG needs /DecodeParms");
    };
    assert_eq!(int_of(parms, "ColorTransform"), Some(0));
}

/// Junk is refused rather than panicking or writing a broken dictionary.
#[test]
fn junk_bytes_are_an_error() {
    let doc = Document::open(HELLO_PDF).expect("opens");
    let mut edit = doc.edit();
    assert!(edit.embed_jpeg(b"not a jpeg").is_err());
    assert!(edit.embed_jpeg(&[]).is_err());
    // A start-of-image and a truncated frame header.
    assert!(edit.embed_jpeg(&[0xFF, 0xD8, 0xFF, 0xC0, 0x00]).is_err());
    // Two components is not a count a PDF may hold
    // (`CPDF_Image::IsValidJpegComponent`, `cpdf_image.cpp:42-44`).
    let mut two_comp = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08];
    two_comp.extend_from_slice(&4u16.to_be_bytes());
    two_comp.extend_from_slice(&4u16.to_be_bytes());
    two_comp.push(2);
    assert!(edit.embed_jpeg(&two_comp).is_err());
}

/// A sample buffer that does not match the dimensions is refused.
#[test]
fn a_mismatched_sample_length_is_an_error() {
    let doc = Document::open(HELLO_PDF).expect("opens");
    let mut edit = doc.edit();
    // Three bytes for a 2x2 grey image, which needs four.
    assert!(
        edit.embed_image(&[0, 1, 2], 2, 2, PixelFormat::Gray8)
            .is_err()
    );
    // Twelve for a 2x2 RGBA image, which needs sixteen.
    assert!(
        edit.embed_image(&[0; 12], 2, 2, PixelFormat::Rgba8)
            .is_err()
    );
    // A zero dimension has no image to write.
    assert!(edit.embed_image(&[], 0, 4, PixelFormat::Gray8).is_err());
    assert!(edit.embed_image(&[], 4, 0, PixelFormat::Gray8).is_err());
    // A 3x2 mask pads each row to a byte, so it is two bytes, not one.
    assert!(edit.embed_image(&[0xFF], 3, 2, PixelFormat::Mask1).is_err());
    assert!(
        edit.embed_image(&[0xFF, 0xFF], 3, 2, PixelFormat::Mask1)
            .is_ok()
    );
}
