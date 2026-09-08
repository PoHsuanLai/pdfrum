//! `pdfrum pages …`: merge, split, slice, delete, rotate, reorder, create,
//! nup, booklet.
//!
//! Every command here writes a new file and never touches the input. A
//! rewrite drops whatever nothing points at, so the output of `split` and
//! `slice` carries only what its pages use.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use pdfrum::{DocEdit, Document, EmbeddedImage, IdSource, PageBox, Rect, Rotation, SaveOptions};

use crate::out::outln;
use crate::term::{Style, Term};
use crate::{out, pages};

/// Save options shared by every writing command.
pub fn save_options(deterministic: bool, input: &[u8]) -> SaveOptions {
    let mut options = SaveOptions::default();
    options.id_source = if deterministic {
        IdSource::Fixed(seed(input))
    } else {
        IdSource::Random
    };
    options
}

/// Sixteen bytes that depend on the input's content and nothing else, so
/// `--deterministic` gives the same `/ID` for the same input on any machine.
/// Two FNV-1a passes with different offsets; identity, not cryptography.
fn seed(input: &[u8]) -> [u8; 16] {
    let fnv = |offset: u64| {
        input.iter().fold(offset, |hash, &byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    };
    let mut seed = [0u8; 16];
    seed[..8].copy_from_slice(&fnv(0xcbf2_9ce4_8422_2325).to_le_bytes());
    seed[8..].copy_from_slice(&fnv(0x8422_2325_cbf2_9ce4).to_le_bytes());
    seed
}

/// A `WxH` size in points, e.g. `612x792`, or the two paper names everyone
/// types.
pub fn parse_size(text: &str) -> Result<(f64, f64)> {
    match text.to_ascii_lowercase().as_str() {
        "letter" => return Ok((612.0, 792.0)),
        "a4" => return Ok((595.276, 841.89)),
        _ => {}
    }
    let (w, h) = text
        .split_once('x')
        .with_context(|| format!("{text:?} is not WIDTHxHEIGHT in points, `letter` or `a4`"))?;
    let (w, h): (f64, f64) = (
        w.trim()
            .parse()
            .with_context(|| format!("bad width {w:?}"))?,
        h.trim()
            .parse()
            .with_context(|| format!("bad height {h:?}"))?,
    );
    if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
        bail!("a page size must be positive");
    }
    Ok((w, h))
}

/// A `CxR` grid, e.g. `2x2`.
pub fn parse_grid(text: &str) -> Result<(u32, u32)> {
    let (c, r) = text
        .split_once('x')
        .with_context(|| format!("{text:?} is not COLUMNSxROWS, e.g. 2x2"))?;
    let (c, r): (u32, u32) = (
        c.trim()
            .parse()
            .with_context(|| format!("bad column count {c:?}"))?,
        r.trim()
            .parse()
            .with_context(|| format!("bad row count {r:?}"))?,
    );
    if c == 0 || r == 0 {
        bail!("a grid needs at least one column and one row");
    }
    Ok((c, r))
}

/// `x0,y0,x1,y1` in points.
pub fn parse_rect(text: &str) -> Result<Rect> {
    let parts: Vec<f64> = text
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("{text:?} is not x0,y0,x1,y1"))?;
    match parts.as_slice() {
        [x0, y0, x1, y1] => Ok(Rect::new(*x0, *y0, *x1, *y1)),
        _ => bail!("a box takes exactly four numbers: x0,y0,x1,y1"),
    }
}

// ---- merge ----------------------------------------------------------------

/// `base` with every page of `others` appended, in order, as the bytes of
/// a saved file, and how many pages it has.
pub fn merge_bytes(
    base: &Document,
    others: &[&Document],
    deterministic: bool,
) -> Result<(Vec<u8>, u32)> {
    let mut edit = base.edit();
    let mut at = base.page_count();
    for doc in others {
        edit.import_pages(doc, 0..doc.page_count(), at)?;
        at += doc.page_count();
    }
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, base.bytes()))?;
    Ok((bytes, at))
}

pub fn merge(
    files: &[PathBuf],
    password: Option<&str>,
    output: &Path,
    deterministic: bool,
    term: Term,
) -> Result<ExitCode> {
    let Some((first, rest)) = files.split_first() else {
        bail!("merge needs at least one input file");
    };
    let sink = out::Sink::new(output, "PDF")?;
    let base = out::open(first, password)?;
    let others: Vec<Document> = rest
        .iter()
        .map(|f| out::open(f, password))
        .collect::<Result<_>>()?;
    let (bytes, at) = merge_bytes(&base, &others.iter().collect::<Vec<_>>(), deterministic)?;
    sink.finish(
        term,
        &bytes,
        &format!("{at} pages"),
        Some(&format!("from {} files", files.len())),
    )?;
    Ok(ExitCode::SUCCESS)
}

// ---- split ----------------------------------------------------------------

pub fn split(
    file: &Path,
    password: Option<&str>,
    spec: Option<&str>,
    dir: &Path,
    deterministic: bool,
    term: Term,
) -> Result<ExitCode> {
    if out::is_stdin(dir) {
        bail!("split writes one file per page; -o takes a directory, not -");
    }
    let doc = out::open(file, password)?;
    let count = doc.page_count();
    let selected = pages::select(spec, count)?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let options = save_options(deterministic, doc.bytes());
    for index in selected {
        let mut edit = doc.edit();
        edit.delete_pages((0..count).filter(|&i| i != index))?;
        let path = dir.join(format!("{}-{}.pdf", out::stem(file), index + 1));
        edit.save(&path, &options)
            .with_context(|| format!("cannot write {}", path.display()))?;
        outln!("{}", term.paint(Style::Ident, &path.display().to_string()));
    }
    Ok(ExitCode::SUCCESS)
}

// ---- slice ----------------------------------------------------------------

/// What `slice` was asked to do to the pages it keeps.
pub struct Slice<'a> {
    pub file: &'a Path,
    pub password: Option<&'a str>,
    pub spec: Option<&'a str>,
    pub rotate: Option<i32>,
    /// The `--crop` box as typed, `x0,y0,x1,y1`.
    pub crop: Option<&'a str>,
    pub output: &'a Path,
    pub deterministic: bool,
}

/// The selected pages of `doc` in document order, rotated or cropped as
/// asked, as the bytes of a saved file, and how many pages were kept.
pub fn slice_bytes(
    doc: &Document,
    spec: Option<&str>,
    rotate: Option<i32>,
    crop: Option<Rect>,
    deterministic: bool,
) -> Result<(Vec<u8>, usize)> {
    let count = doc.page_count();
    let mut keep = pages::select(spec, count)?;
    keep.sort_unstable();
    keep.dedup();
    let mut edit = doc.edit();
    edit.delete_pages((0..count).filter(|i| !keep.contains(i)))?;
    for &index in &keep {
        if let Some(degrees) = rotate {
            edit.set_rotation(index, degrees)?;
        }
        if let Some(rect) = crop {
            edit.set_page_box(index, PageBox::Crop, rect)?;
        }
    }
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, doc.bytes()))?;
    Ok((bytes, keep.len()))
}

/// Keep the selected pages in document order, rotated or cropped as asked.
pub fn slice(req: &Slice<'_>, term: Term) -> Result<ExitCode> {
    let crop = req.crop.map(parse_rect).transpose()?;
    let sink = out::Sink::new(req.output, "PDF")?;
    let doc = out::open(req.file, req.password)?;
    let (bytes, kept) = slice_bytes(&doc, req.spec, req.rotate, crop, req.deterministic)?;
    sink.finish(term, &bytes, &format!("{kept} pages"), None)?;
    Ok(ExitCode::SUCCESS)
}

// ---- delete ---------------------------------------------------------------

/// The pages `spec` deletes from `doc`, sorted and deduplicated, and how
/// many are left. Deleting every page is refused.
pub fn deletion(doc: &Document, spec: &str) -> Result<(Vec<u32>, u32)> {
    let count = doc.page_count();
    let mut gone = pages::select(Some(spec), count)?;
    gone.sort_unstable();
    gone.dedup();
    let left = count.saturating_sub(u32::try_from(gone.len()).unwrap_or(u32::MAX));
    if left == 0 {
        bail!("deleting every page leaves nothing; `pages slice` keeps some");
    }
    Ok((gone, left))
}

/// `doc` without `gone` — [`deletion`]'s pages — as the bytes of a saved
/// file.
pub fn delete_bytes(doc: &Document, gone: &[u32], deterministic: bool) -> Result<Vec<u8>> {
    let mut edit = doc.edit();
    edit.delete_pages(gone.iter().copied())?;
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, doc.bytes()))?;
    Ok(bytes)
}

/// Drop the selected pages; the rest keep their order.
pub fn delete(
    file: &Path,
    password: Option<&str>,
    spec: &str,
    output: &Path,
    deterministic: bool,
    term: Term,
) -> Result<ExitCode> {
    let sink = out::Sink::new(output, "PDF")?;
    let doc = out::open(file, password)?;
    let (gone, left) = deletion(&doc, spec)?;
    let bytes = delete_bytes(&doc, &gone, deterministic)?;
    let deleted = gone.len();
    sink.finish(
        term,
        &bytes,
        &format!(
            "{deleted} page{} deleted",
            if deleted == 1 { "" } else { "s" }
        ),
        Some(&format!("{left} left")),
    )?;
    Ok(ExitCode::SUCCESS)
}

// ---- rotate ---------------------------------------------------------------

/// The `--by` turn: a multiple of 90 that is not zero, either way round.
pub fn parse_turn(by: i32) -> Result<i32> {
    if by == 0 || by % 90 != 0 {
        bail!("--by takes 90, 180, 270 or -90, not {by}");
    }
    Ok(by)
}

/// The selected pages turned `by` degrees clockwise from where each one
/// stands — a page already at 90 turned by 90 is at 180 — as the bytes of a
/// saved file, and how many pages were turned.
pub fn rotate_bytes(
    doc: &Document,
    spec: Option<&str>,
    by: i32,
    deterministic: bool,
) -> Result<(Vec<u8>, usize)> {
    let by = parse_turn(by)?;
    let mut turned = pages::select(spec, doc.page_count())?;
    turned.sort_unstable();
    turned.dedup();
    let mut edit = doc.edit();
    for &index in &turned {
        let current = standing(doc.page(index)?.rotation());
        edit.set_rotation(index, (current + by).rem_euclid(360))?;
    }
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, doc.bytes()))?;
    Ok((bytes, turned.len()))
}

/// A page's `/Rotate` as the signed degrees the turn is added to.
fn standing(rotation: Rotation) -> i32 {
    match rotation {
        Rotation::None => 0,
        Rotation::Quarter => 90,
        Rotation::Half => 180,
        Rotation::ThreeQuarter => 270,
    }
}

/// Turn the selected pages, relative to how each one already stands.
pub fn rotate(
    file: &Path,
    password: Option<&str>,
    spec: Option<&str>,
    by: i32,
    output: &Path,
    deterministic: bool,
    term: Term,
) -> Result<ExitCode> {
    let sink = out::Sink::new(output, "PDF")?;
    let doc = out::open(file, password)?;
    let (bytes, turned) = rotate_bytes(&doc, spec, by, deterministic)?;
    sink.finish(
        term,
        &bytes,
        &format!(
            "{turned} page{} rotated by {by}",
            if turned == 1 { "" } else { "s" }
        ),
        None,
    )?;
    Ok(ExitCode::SUCCESS)
}

// ---- reorder --------------------------------------------------------------

/// Pages in the order named, duplicates included, into a new file.
pub fn reorder(
    file: &Path,
    password: Option<&str>,
    spec: &str,
    output: &Path,
    deterministic: bool,
    term: Term,
) -> Result<ExitCode> {
    let sink = out::Sink::new(output, "PDF")?;
    let doc = out::open(file, password)?;
    let order = pages::select(Some(spec), doc.page_count())?;
    if order.is_empty() {
        bail!("no pages selected");
    }
    let first = doc.page(order.first().copied().unwrap_or(0))?;
    let dest = Document::blank(first.width(), first.height())?;
    let mut edit = dest.edit();
    edit.import_pages(&doc, order.iter().copied(), 0)?;
    // The template's own page sits after the imports; it was only ever a
    // place to hang the tree.
    edit.delete_pages([0u32])?;
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, doc.bytes()))?;
    sink.finish(term, &bytes, &format!("{} pages", order.len()), None)?;
    Ok(ExitCode::SUCCESS)
}

// ---- nup ------------------------------------------------------------------

/// What `nup` was asked for.
#[derive(Clone, Copy)]
pub struct Nup<'a> {
    pub file: &'a Path,
    pub password: Option<&'a str>,
    pub spec: Option<&'a str>,
    /// The `--grid` as typed, columns by rows per sheet: `2x2`.
    pub grid: &'a str,
    /// The `--sheet` as typed: `WIDTHxHEIGHT` in points, `letter` or `a4`.
    pub sheet: &'a str,
    pub output: &'a Path,
    pub deterministic: bool,
}

pub fn nup(req: &Nup<'_>, term: Term) -> Result<ExitCode> {
    let Nup {
        file,
        password,
        spec,
        grid,
        sheet,
        output,
        deterministic,
    } = *req;
    let grid = parse_grid(grid)?;
    let sheet = parse_size(sheet)?;
    let sink = out::Sink::new(output, "PDF")?;
    let doc = out::open(file, password)?;
    let selected = pages::select(spec, doc.page_count())?;
    let dest = Document::blank(sheet.0, sheet.1)?;
    let mut edit = dest.edit();
    edit.n_up(&doc, selected.iter().copied(), grid.0, grid.1, sheet)?;
    edit.delete_pages([0u32])?;
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, doc.bytes()))?;
    let per_sheet = grid.0 * grid.1;
    let sheets = u32::try_from(selected.len())
        .unwrap_or(u32::MAX)
        .div_ceil(per_sheet.max(1));
    sink.finish(
        term,
        &bytes,
        &format!("{sheets} sheets"),
        Some(&format!(
            "{}x{} from {} pages",
            grid.0,
            grid.1,
            selected.len()
        )),
    )?;
    Ok(ExitCode::SUCCESS)
}

// ---- booklet --------------------------------------------------------------

/// The page order for saddle-stitch printing of `count` pages (a multiple of
/// four): each sheet's front is `[last, first]`, its back `[second,
/// second-to-last]`, working inwards.
pub fn booklet_order(count: u32) -> Vec<u32> {
    let mut order = Vec::with_capacity(count as usize);
    for i in 0..count / 4 {
        order.push(count - 1 - 2 * i);
        order.push(2 * i);
        order.push(2 * i + 1);
        order.push(count - 2 - 2 * i);
    }
    order
}

pub fn booklet(
    file: &Path,
    password: Option<&str>,
    output: &Path,
    deterministic: bool,
    term: Term,
) -> Result<ExitCode> {
    let sink = out::Sink::new(output, "PDF")?;
    let doc = out::open(file, password)?;
    let count = doc.page_count();
    if count == 0 {
        bail!("the document has no pages");
    }
    let first = doc.page(0u32)?;
    let (w, h) = (first.width(), first.height());
    // Pad to a multiple of four with blank pages, so every sheet is full.
    let padded_count = count.div_ceil(4) * 4;
    let mut padding = doc.edit();
    for i in count..padded_count {
        padding.add_page(w, h, i)?;
    }
    let mut bytes = Vec::new();
    padding.write_to(&mut bytes, &SaveOptions::default())?;
    let padded = out::open_bytes(bytes, None)?;

    let sheet = (2.0 * w, h);
    let dest = Document::blank(sheet.0, sheet.1)?;
    let mut edit = dest.edit();
    edit.n_up(&padded, booklet_order(padded_count), 2, 1, sheet)?;
    edit.delete_pages([0u32])?;
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, doc.bytes()))?;
    sink.finish(
        term,
        &bytes,
        &format!("{} sheets", padded_count / 4),
        Some(&format!(
            "{} sides for {count} pages{}",
            padded_count / 2,
            if padded_count > count {
                format!(", {} blank", padded_count - count)
            } else {
                String::new()
            }
        )),
    )?;
    Ok(ExitCode::SUCCESS)
}

// ---- create ---------------------------------------------------------------

/// One page per image, each page the image's size at `dpi`.
///
/// JPEG files are embedded as they are (DCT passthrough); PNG files are
/// decoded and stored as flate-compressed samples.
pub fn create(
    images: &[PathBuf],
    dpi: f64,
    output: &Path,
    deterministic: bool,
    term: Term,
) -> Result<ExitCode> {
    if images.is_empty() {
        bail!("create needs at least one image");
    }
    if !(dpi.is_finite() && dpi > 0.0) {
        bail!("the resolution must be a positive number");
    }
    let sink = out::Sink::new(output, "PDF")?;
    let decoded: Vec<Decoded> = images.iter().map(|p| decode(p)).collect::<Result<_>>()?;
    let size = |d: &Decoded| {
        (
            f64::from(d.width) * 72.0 / dpi,
            f64::from(d.height) * 72.0 / dpi,
        )
    };

    // First a document with the right number of blank pages, sized to their
    // images; then, with those pages real, an image on each.
    let (w0, h0) = size(decoded.first().context("no images")?);
    let blank = Document::blank(w0, h0)?;
    let mut shape = blank.edit();
    for (i, d) in decoded.iter().enumerate().skip(1) {
        let (w, h) = size(d);
        shape.add_page(w, h, u32::try_from(i).unwrap_or(u32::MAX))?;
    }
    let mut bytes = Vec::new();
    shape.write_to(&mut bytes, &SaveOptions::default())?;
    let doc = out::open_bytes(bytes, None)?;

    let mut edit = doc.edit();
    let mut page_edits = Vec::with_capacity(decoded.len());
    for (i, d) in decoded.iter().enumerate() {
        let embedded = embed(&mut edit, d)?;
        let (w, h) = size(d);
        let page = doc.page(u32::try_from(i).unwrap_or(u32::MAX))?;
        let mut page_edit = page.edit();
        page_edit
            .push(pdfrum::ImageBuilder::at(embedded.object(), Rect::new(0.0, 0.0, w, h)).build());
        page_edits.push(page_edit);
    }
    let all_input: Vec<u8> = decoded
        .iter()
        .flat_map(|d| d.source.iter().copied())
        .collect();
    let mut bytes = Vec::new();
    edit.write_pages_to(
        &mut bytes,
        &page_edits,
        &save_options(deterministic, &all_input),
    )?;
    sink.finish(
        term,
        &bytes,
        &format!("{} pages", decoded.len()),
        Some(&format!("from {} images", images.len())),
    )?;
    Ok(ExitCode::SUCCESS)
}

/// An image's samples as the file will hold them.
enum Pixels {
    /// A JPEG, embedded as it is.
    Jpeg(Vec<u8>),
    /// Decoded samples, flate-compressed by the save.
    Raw {
        data: Vec<u8>,
        format: pdfrum::PixelFormat,
    },
}

/// A JPEG or PNG file read and, for a PNG, decoded: what `pages create`
/// puts on a page and `stamp image` draws over one.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pixels: Pixels,
    /// The file's bytes, for the deterministic seed.
    source: Vec<u8>,
}

/// `image` as a new image object of `edit`.
pub fn embed(edit: &mut DocEdit<'_>, image: &Decoded) -> Result<EmbeddedImage> {
    Ok(match &image.pixels {
        Pixels::Jpeg(bytes) => edit.embed_jpeg(bytes)?,
        Pixels::Raw { data, format } => {
            edit.embed_image(data, image.width, image.height, *format)?
        }
    })
}

/// The image file at `path`.
pub fn decode(path: &Path) -> Result<Decoded> {
    let source = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    decode_bytes(source, &path.display().to_string())
}

/// The image `source` holds — a JPEG kept as it is, a PNG decoded —
/// `what` naming it in an error.
pub fn decode_bytes(source: Vec<u8>, what: &str) -> Result<Decoded> {
    if source.starts_with(&[0xFF, 0xD8]) {
        let (width, height) = jpeg_size(&source)
            .with_context(|| format!("{what}: cannot find the JPEG frame size"))?;
        return Ok(Decoded {
            width,
            height,
            pixels: Pixels::Jpeg(source.clone()),
            source,
        });
    }
    if source.starts_with(b"\x89PNG") {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(source.as_slice()));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder
            .read_info()
            .with_context(|| format!("{what}: not a readable PNG"))?;
        let mut data = vec![
            0;
            reader
                .output_buffer_size()
                .with_context(|| format!("{what}: too large to decode"))?
        ];
        let info = reader
            .next_frame(&mut data)
            .with_context(|| format!("{what}: cannot decode"))?;
        data.truncate(info.buffer_size());
        let format = match info.color_type {
            png::ColorType::Grayscale => pdfrum::PixelFormat::Gray8,
            png::ColorType::Rgb => pdfrum::PixelFormat::Rgb8,
            png::ColorType::Rgba => pdfrum::PixelFormat::Rgba8,
            png::ColorType::GrayscaleAlpha => {
                // Alpha over gray has no direct PDF form; drop the alpha.
                data = data.as_chunks::<2>().0.iter().map(|&[v, _]| v).collect();
                pdfrum::PixelFormat::Gray8
            }
            png::ColorType::Indexed => {
                bail!("{what}: indexed PNG survived normalization")
            }
        };
        return Ok(Decoded {
            width: info.width,
            height: info.height,
            pixels: Pixels::Raw { data, format },
            source,
        });
    }
    bail!("{what}: not a JPEG or PNG file")
}

/// The width and height from a JPEG's first frame header (SOF0–SOF15,
/// except the DHT/DAC/JPG markers that share the range).
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 9 < bytes.len() {
        if bytes.get(i)? != &0xFF {
            i += 1;
            continue;
        }
        let marker = *bytes.get(i + 1)?;
        let length = usize::from(u16::from_be_bytes([*bytes.get(i + 2)?, *bytes.get(i + 3)?]));
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let height = u32::from(u16::from_be_bytes([*bytes.get(i + 5)?, *bytes.get(i + 6)?]));
            let width = u32::from(u16::from_be_bytes([*bytes.get(i + 7)?, *bytes.get(i + 8)?]));
            return (width > 0 && height > 0).then_some((width, height));
        }
        i += 2 + length.max(2);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{booklet_order, parse_grid, parse_rect, parse_size, parse_turn, seed};

    #[test]
    fn a_booklet_reads_in_order_once_folded() {
        // Eight pages, two sheets: sheet 1 front [8 1], back [2 7];
        // sheet 2 front [6 3], back [4 5]. Folded and nested, that is 1..8.
        assert_eq!(booklet_order(8), [7, 0, 1, 6, 5, 2, 3, 4]);
        assert_eq!(booklet_order(4), [3, 0, 1, 2]);
        assert!(booklet_order(0).is_empty());
    }

    #[test]
    fn the_seed_depends_on_the_bytes_and_nothing_else() {
        assert_eq!(seed(b"abc"), seed(b"abc"));
        assert_ne!(seed(b"abc"), seed(b"abd"));
        assert_ne!(seed(b""), [0; 16]);
    }

    #[test]
    fn sizes_grids_and_boxes_parse_and_refuse_nonsense() {
        assert_eq!(parse_size("letter").unwrap(), (612.0, 792.0));
        assert_eq!(parse_size("100x200").unwrap(), (100.0, 200.0));
        assert!(parse_size("0x5").is_err());
        assert!(parse_size("wide").is_err());
        assert_eq!(parse_grid("2x3").unwrap(), (2, 3));
        assert!(parse_grid("0x3").is_err());
        let r = parse_rect("1, 2, 3, 4").unwrap();
        assert_eq!((r.x0, r.y1), (1.0, 4.0));
        assert!(parse_rect("1,2,3").is_err());
    }

    #[test]
    fn a_turn_is_a_quarter_turn_or_more_either_way_and_never_nothing() {
        assert_eq!(parse_turn(90).unwrap(), 90);
        assert_eq!(parse_turn(-90).unwrap(), -90);
        assert_eq!(parse_turn(270).unwrap(), 270);
        assert!(parse_turn(0).is_err());
        assert!(parse_turn(45).is_err());
    }
}
