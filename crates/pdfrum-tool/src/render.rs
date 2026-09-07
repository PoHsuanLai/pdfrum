//! `--png` and `--md5`: rendering a page to the file and the stdout line the
//! oracle produces.
//!
//! Three conventions have to match exactly for the harness to diff
//! like-for-like, and none of them is the obvious choice:
//!
//! - **The PNG is RGB, not RGBA**, for a page without transparency. The
//!   oracle discards transparency whenever the page reports none, and the
//!   encoder then drops the fourth channel.
//! - **The MD5 is over the raw bitmap buffer, not the PNG.** It hashes
//!   `stride * height` bytes of 32-bit **BGRA** — and for an opaque page the
//!   fourth byte is padding, which the initial white clear leaves at `0xFF`.
//! - **`--reverse-byte-order` is off by default**, so the buffer really is
//!   BGRA rather than RGBA.
//!
//! The stdout line is `MD5:<path>:<hex>`, where the path is the file just
//! written, and the file name is `<input>.<page>.png`.

// Where each of the three was measured in the oracle: the encoder's channel
// drop is png_codec_libpng.cpp:620-626; the hash reads FPDFBitmap_GetBuffer
// at pdfium_test.cc:1024-1038, an FPDFBitmap_BGRx buffer whose padding byte
// the FillRect(0xFFFFFFFF) clear leaves at 0xFF; the stdout line is
// pdfium_test.cc:311.

use std::path::{Path, PathBuf};

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Name, Resolve};
use pdfrum_page::{BuildContext, OcContext, UsageType, page_visibility};
use pdfrum_parser::PageDict;
use pdfrum_raster_agg::AggBackend;
use pdfrum_raster_tinyskia::TinySkiaBackend;
use pdfrum_raster_vello_cpu::VelloCpuBackend;
use pdfrum_render::{
    Pixmap, RenderCaches, RenderOptions, RenderSession, needs_alpha_background, render_page_with,
};

/// The default rendering scale, in device pixels per PDF point.
///
/// The oracle renders at `scale = 1.0` unless `--scale` says otherwise, and
/// truncates the scaled page size to an integer, so a 612x792 page is a
/// 612x792 bitmap.
pub const DEFAULT_SCALE: f64 = 1.0;

/// The file a page's PNG is written to: `<input>.<page>.png`, beside the
/// input, which is where the harness harvests it from.
#[must_use]
pub fn output_path(input: &Path, index: u32) -> Option<PathBuf> {
    let name = input.file_name()?.to_str()?;
    let file = format!("{name}.{index}.png");
    // The oracle refuses a name of 256 bytes or more rather than truncating.
    if file.len() >= 256 {
        return None;
    }
    Some(input.with_file_name(file))
}

/// What one rendered page produced.
#[derive(Debug, Clone)]
pub struct Rendered {
    /// The PNG bytes, ready to write.
    pub png: Vec<u8>,
    /// The MD5 of the *raw bitmap buffer*, hex-encoded lower case.
    pub digest: String,
}

/// The environment variable that selects a rasterizer.
///
/// This is the *out-of-band* spelling, kept because Tier C drives two runs of
/// one command line and differing only in the environment is what makes them
/// otherwise identical. The in-band spelling is `--use-renderer=`, which the
/// oracle also has, and which [`Backend::resolve`] gives precedence.
pub const BACKEND_ENV: &str = "PDFRUM_BACKEND";

/// Which rasterizer a render uses.
///
/// Three, and the split between them is deliberate. `Agg` is the
/// conformance default: it integrates coverage analytically on the oracle's
/// own subpixel grid — AGG's, the scan converter PDFium itself uses — so a
/// comparison against a golden measures the *engine* rather than a
/// rasterizer's sampling policy. The two wrapped backends stay because Tier
/// C's whole value is that a second implementation disagrees out loud, and
/// because `vello_cpu` is the production rasterizer the facade hands an API
/// user who wants speed rather than byte-comparability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// The analytic AGG-parity rasterizer — the default for `--png`.
    #[default]
    Agg,
    /// `tiny-skia` — the determinism baseline and Tier C's gating partner.
    TinySkia,
    /// `vello_cpu`, at its pinned SIMD level and render mode.
    VelloCpu,
}

impl Backend {
    /// The backend a name selects, or `None` when it names none of them.
    ///
    /// `"agg"` is [`Agg`], `"tiny-skia"` / `"tinyskia"` is [`TinySkia`],
    /// `"vello-cpu"` / `"vello_cpu"` / `"vello"` is [`VelloCpu`].
    /// [`Backend::resolve`] falls back to the default rather than erroring, so
    /// an unrecognised name is a silent switch.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "agg" => Some(Self::Agg),
            "tiny-skia" | "tinyskia" => Some(Self::TinySkia),
            "vello-cpu" | "vello_cpu" | "vello" => Some(Self::VelloCpu),
            _ => None,
        }
    }

    /// The backend to render with, given a `--use-renderer=` value.
    ///
    /// The flag wins over the environment, and both fall back to the default
    /// rather than erroring: this is a developer-facing knob, and the oracle
    /// itself accepts `--use-renderer=` values naming rasterizers we do not
    /// have. A typo must not silently change what a conformance run means, so
    /// an unrecognised value lands on the default the run would have used
    /// anyway.
    #[must_use]
    pub fn resolve(flag: Option<&str>) -> Self {
        flag.and_then(Self::from_name)
            .or_else(|| {
                std::env::var(BACKEND_ENV)
                    .ok()
                    .and_then(|v| Self::from_name(&v))
            })
            .unwrap_or_default()
    }
}

/// Everything a form session contributes to one page's image.
///
/// Three facts, and each reaches the annotation pass a different way, which
/// is why they travel together rather than as one map:
///
/// - `updates` are per-annotation **appearances**, keyed positionally;
/// - `focus` says which widget is **not** tinted, and what if anything is
///   stroked in the tint's place;
/// - `hover` says which annotation's synthesized **note card** is open.
///
/// None of the three can be derived from the others, and any one of them on
/// its own is reason to build an overlay.
// Not `Copy`: the popup carries its option labels, which are owned strings —
// see `pdfrum::PopupView` for why the view is owned rather than borrowed from
// the session.
#[derive(Debug, Clone, Default)]
pub struct SessionView<'a> {
    /// The appearance updates this page's event replay produced.
    pub updates: &'a [pdfrum::AppearanceUpdate],
    /// Which annotation holds focus, and its focus rectangle.
    pub focus: Option<pdfrum_doc::ap::Focus>,
    /// Which annotation the pointer is inside, by raw `/Annots` index.
    pub hover: Option<usize>,
    /// The open combo-box dropdown on this page, if one is open.
    ///
    /// The library publishes this rather than drawing it — a dropdown is a
    /// window and `pdfrum-form` does not make windows. This
    /// tool is the host that draws it, in [`crate::chrome`], because the
    /// oracle's form-filler pass does and a golden comparison has to see the
    /// same pixels.
    pub popup: Option<pdfrum::PopupView>,
}

/// One annotation's dictionary, by **raw** `/Annots` index.
///
/// The chrome painter needs the combo box's own dictionary to read the two
/// things the list inherits from it: `/MK`'s colours and `/DA`'s font. Read
/// straight from the array rather than through `AnnotList`, because that list
/// drops pop-ups and reorders, and the index here is the raw one.
fn widget_dict<R: Resolve>(page: &PageDict, index: usize, r: &R) -> Option<pdfrum_object::Dict> {
    page.dict
        .array(pdfrum_object::names::ANNOTS, r)?
        .dict_at(index, r)
}

/// Collects a form session's updates into the overlay the annotation pass
/// lays over what it generates.
///
/// Keyed by the **raw** `/Annots` index, which is the index space both an
/// `AnnotId` and the overlay use — the two agree on every page without a
/// pop-up, so a mismatch here would be invisible until one appeared.
///
/// A focused field's live appearance and a committed one are laid over
/// identically: which of the two a field produced is its own business, and
/// the difference is already in the stream. An update that reverts to the
/// file's own appearance contributes nothing, which is exactly right —
/// absence from the overlay *is* "use what the file declares".
fn session_overlay(view: &SessionView<'_>) -> Option<pdfrum_doc::AnnotOverlay> {
    let highest = view
        .updates
        .iter()
        .filter(|update| update.kind.appearance().is_some())
        .map(|update| update.annot.index as usize)
        .max();
    // Focus and hover are each reason enough to supply an overlay, even with
    // no appearance in it. "This widget is not tinted" and "this annotation's
    // note is open" are instructions the annotation pass cannot reach any
    // other way, and neither one produces an appearance to carry it: a
    // focused field whose value never changed generates nothing, and a
    // highlight's note card is synthesized by the pass itself.
    if highest.is_none() && view.focus.is_none() && view.hover.is_none() {
        return None;
    }
    // Sized to the highest index anything names, since `set` is positional
    // even though `set_focus` and `set_hover` are not.
    let highest = highest
        .into_iter()
        .chain(view.focus.map(|focus| focus.annot))
        .chain(view.hover)
        .max()
        .unwrap_or(0);

    let mut overlay = pdfrum_doc::AnnotOverlay::with_capacity(highest + 1);
    for update in view.updates {
        if let Some(appearance) = update.kind.appearance() {
            overlay.set(update.annot.index as usize, appearance.clone());
        }
        // **The one place the oracle draws text with ClearType.**
        // `CPWL_EditImpl::DrawTextString` (`cpwl_edit_impl.cpp:40-57`) builds
        // a *local* `CPDF_RenderOptions` whose constructor sets
        // `bClearType = true`, and the two public entry points that would
        // clear it from `FPDF_LCD_TEXT` (`cpdfsdk_renderpage.cpp:37`,
        // `fpdf_formfill.cpp:272`) are not on this path. So on a page whose
        // every other run is grayscale, a live edit's glyphs are subpixel.
        //
        // Marked per annotation and folded in per subtree, never hoisted to
        // the page's own options: the whole point of the oracle's *local*
        // options is that the rest of the page keeps its own antialiasing.
        if update.kind.is_live_edit() {
            overlay.set_live_edit(update.annot.index as usize);
        }
    }
    if let Some(focus) = view.focus {
        overlay.set_focus(focus);
    }
    if let Some(hover) = view.hover {
        overlay.set_hover(hover);
    }
    Some(overlay)
}

/// Render one page and encode it the way the oracle does, with whatever
/// appearance updates a form session produced for it — the page pass plus the
/// form-filler pass.
///
/// Returns `None` when the page has no renderable size, which the oracle
/// also skips — a page still counts as processed either way.
///
/// # What the oracle does here, and what this can do yet
///
/// The oracle renders in two steps: the first paints the page and every
/// annotation's `/AP`, and the second paints the **form-filler's** view of
/// the widgets over the same bitmap — the focused field's live editor state,
/// its caret and its selection band — on top. That second pass is the only thing an event can
/// change about an image, which is why an unfocused fixture renders
/// identically with and without `--send-events`.
///
/// Both steps are now here. `annot_render::overlay_with` is the first, and
/// the second is what `updates` carries: each
/// [`UpdateKind::Regenerated`](pdfrum::UpdateKind) or
/// [`LiveEdit`](pdfrum::UpdateKind) holds the `GeneratedAp` that replaces
/// what the annotation would otherwise draw, collected by
/// [`session_overlay`] and laid over the pass's own output.
///
/// So an event can change an image from here on. A fixture whose events leave
/// nothing focused and commit no value still renders identically with and
/// without `--send-events`, because the session produces no appearance to lay
/// over — which is the property that keeps the unfocused rows stable.
///
/// The **third** thing a session contributes is hover, and it is not part of
/// either pass above: a text highlight's note card is synthesized by the
/// annotation pass itself, and is drawn only while the pointer is inside its
/// parent. Nothing a file can say opens one, so [`SessionView::hover`] is the
/// whole of that signal — which is why the six `annotation_highlight_*`
/// fixtures are scripts of bare `mousemove` lines and nothing else.
#[must_use]
pub fn render<R: Resolve>(
    page: &PageDict,
    catalog: &pdfrum_object::Dict,
    r: &R,
    scale: f64,
    backend: Backend,
    ctx: &mut BuildContext,
    session: &SessionView<'_>,
) -> Option<Rendered> {
    let (pixmap, has_transparency) = rasterize(page, catalog, r, scale, backend, ctx, session)?;
    Some(encode(&pixmap, has_transparency))
}

/// [`render`] stopping at the pixels: the page pass, the form-filler pass and
/// the raster, with neither the hash nor the encoder over them.
///
/// Split out because a page rasterizes for output formats that write no image
/// — and for none at all. Encoding a PNG and hashing a buffer nothing will
/// read is not work the oracle does on those paths, so charging it to them
/// would make an unwritten page cost more here than it does there.
///
/// The second half of the pair is what the encoding depends on:
/// `FPDFPage_HasTransparency` is not the page's `/Group`, and the caller
/// cannot recompute it once the page graph has been dropped.
#[must_use]
pub fn rasterize<R: Resolve>(
    page: &PageDict,
    catalog: &pdfrum_object::Dict,
    r: &R,
    scale: f64,
    backend: Backend,
    ctx: &mut BuildContext,
    session: &SessionView<'_>,
) -> Option<(Pixmap, bool)> {
    let limits = Limits::default();
    let mut build_diags = Diagnostics::default();
    // The decode target has to be set before the build, because the build is
    // what decodes the images. `scale` is a uniform scale here —
    // `pdfium_test` has no other kind — so the device box is the display size
    // times it, truncated exactly as the bitmap allocation truncates.
    let (page_w, page_h) = pdfrum_page::display_size_from_dict(
        &page.dict,
        |key| page.inherited(key, r),
        r,
        &mut build_diags,
    );
    let previous = std::mem::replace(
        &mut ctx.decode_target,
        pdfrum_page::RequestedSize::for_device(page_w * scale, page_h * scale),
    );
    let mut built = crate::content::build(page, r, ctx, &limits, &mut build_diags);
    // `pdfium_test --png` renders with `FPDF_ANNOT`, so an annotation's
    // appearance form is part of the page image. It is appended here rather
    // than in `content::build` because only *this* pass wants it: `--txt`
    // reads the content stream's text and `--annot` describes the
    // annotations rather than drawing them, and both would double-count an
    // appearance the page graph had already absorbed.
    let supplied = session_overlay(session);
    pdfrum_doc::annot_render::overlay_with(
        &mut built,
        &page.dict,
        catalog,
        r,
        ctx,
        &limits,
        &mut build_diags,
        supplied.as_ref(),
    );
    // The one piece of chrome the annotation pass cannot place, because it
    // falls **outside** the widget's `/Rect` and an appearance form is fitted
    // into that rectangle rather than translated to it. Appended after the
    // pass, which is also the order the oracle composites in: `FPDF_FFLDraw`
    // runs after `FPDF_RenderPageBitmap`, so the list covers whatever it
    // overlaps.
    if let Some(popup) = &session.popup
        && let Some(widget) = widget_dict(page, popup.annot.index as usize, r)
    {
        crate::chrome::push_popup(
            &mut built,
            popup,
            &widget,
            catalog,
            r,
            ctx,
            &limits,
            &mut build_diags,
        );
    }
    ctx.decode_target = previous;
    let page = built;
    let opts = RenderOptions {
        transform: kurbo::Affine::scale(scale),
        ..RenderOptions::default()
    };
    let mut diags = Diagnostics::default();
    // Optional content is decided here, between building the page and drawing
    // it: the pre-pass reads the catalog's `/OCProperties` through the
    // resolver and hands the walk plain data. `pdfium_test` renders with the
    // default `View` usage and the document's default configuration, which is
    // what `OcContext::new` with no usage override reads.
    let mut oc = OcContext::new(
        catalog.dict(&Name::from("OCProperties"), r),
        UsageType::View,
    );
    let visible = page_visibility(&page, &mut oc, r, &mut build_diags);
    // The analytic AGG-parity backend is the default. Every backend here is
    // reproducible — tiny-skia is deterministic by construction and
    // `vello_cpu` pins its SIMD level so its output does not move with the
    // host — so the default is chosen for *parity* rather than for
    // determinism: it integrates coverage the way the oracle does, which is
    // what makes a golden comparison measure the engine.
    let mut caches = RenderCaches::new();
    let session = RenderSession {
        caches: Some(&mut caches),
        visible: Some(&visible),
        deadline: None,
    };
    let pixmap = match backend {
        Backend::Agg => {
            render_page_with(&page, &opts, &AggBackend::new(), session, &mut diags).ok()?
        }
        Backend::TinySkia => {
            render_page_with(&page, &opts, &TinySkiaBackend::new(), session, &mut diags).ok()?
        }
        Backend::VelloCpu => {
            render_page_with(&page, &opts, &VelloCpuBackend::new(), session, &mut diags).ok()?
        }
    };
    // `FPDFPage_HasTransparency` is not the page's `/Group`: it is set only
    // by a blend mode above Multiply. The engine exposes the same predicate
    // so the output encoding and the background clear cannot disagree.
    Some((pixmap, needs_alpha_background(&page)))
}

/// Encode a rendered pixmap as the oracle's PNG and MD5.
#[must_use]
pub fn encode(pixmap: &Pixmap, has_transparency: bool) -> Rendered {
    // The hash is over the 32-bit BGRA buffer — BGRx for an opaque page,
    // whose padding byte the white clear leaves at 0xFF.
    let buffer = pixmap.to_straight_bgra(!has_transparency);
    let digest = md5_hex(&buffer);
    let png = if has_transparency {
        encode_png(
            pixmap.width(),
            pixmap.height(),
            &pixmap.to_straight_rgba(),
            png::ColorType::Rgba,
        )
    } else {
        encode_png(
            pixmap.width(),
            pixmap.height(),
            &pixmap.to_straight_rgb(),
            png::ColorType::Rgb,
        )
    };
    Rendered { png, digest }
}

/// Wraps a raw eight-bit bitmap as a PNG; an encoder failure yields no bytes.
fn encode_png(width: u32, height: u32, data: &[u8], color: png::ColorType) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let Ok(mut writer) = encoder.write_header() else {
            return Vec::new();
        };
        if writer.write_image_data(data).is_err() {
            return Vec::new();
        }
    }
    out
}

/// The lowercase hex MD5 the `MD5:` line carries, over the raw bitmap buffer.
fn md5_hex(bytes: &[u8]) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(32), |mut acc, b| {
            use std::fmt::Write;
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

/// The stdout line `--md5` prints for one written file.
#[must_use]
pub fn md5_line(path: &Path, digest: &str) -> String {
    format!("MD5:{}:{digest}\n", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page document whose content stream fills the left half of a
    /// 20x10 page black. The two halves are what make a *render* visible: an
    /// unrendered page and a rendered one differ only in that the second has
    /// the fill in it.
    const HALF_BLACK: &[u8] = b"%PDF-1.7\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 20 10]/Contents 4 0 R>>endobj\n\
4 0 obj<</Length 26>>stream\n\
0 g 0 0 10 10 re f\n\
endstream endobj\n\
trailer<</Root 1 0 R/Size 5>>\n";

    /// `rasterize` produces the page's pixels, and stops there.
    ///
    /// The two assertions are the two halves of the split: the fill really
    /// drew (so this is a render and not a page load), and what comes back is
    /// pixels rather than a hash and a PNG (so the encode is the caller's).
    #[test]
    fn rasterize_draws_the_page_and_hands_back_the_pixels() {
        let doc = pdfrum_parser::load(
            std::sync::Arc::from(HALF_BLACK),
            &pdfrum_parser::LoadOptions::default(),
        )
        .expect("loads");
        let page = doc.page(0u32).expect("one page");
        let catalog = doc.catalog().unwrap_or_default();
        let mut ctx = BuildContext::new();
        let (pixmap, transparency) = rasterize(
            &page,
            &catalog,
            &doc,
            DEFAULT_SCALE,
            Backend::Agg,
            &mut ctx,
            &SessionView::default(),
        )
        .expect("renders");
        assert_eq!((pixmap.width(), pixmap.height()), (20, 10));
        assert!(!transparency, "no blend mode above Multiply on this page");
        // PDF space is y-up, so the fill covers the *bottom* left; the device
        // rows run the other way and the whole left half is black either way.
        let left = pixmap.pixel(2, 5).expect("inside");
        let right = pixmap.pixel(17, 5).expect("inside");
        assert_eq!(left[3], 0xFF, "opaque");
        assert!(
            left[0] < 0x20 && left[1] < 0x20 && left[2] < 0x20,
            "the fill drew: {left:?}"
        );
        assert!(
            right[0] > 0xE0 && right[1] > 0xE0 && right[2] > 0xE0,
            "the other half is the white clear: {right:?}"
        );
    }

    /// `render` is `rasterize` plus `encode`, with the transparency flag
    /// carried between them.
    ///
    /// The flag is the one thing the split can drop silently: it decides the
    /// PNG's colour type and whether the hash reads `BGRx` or BGRA, and both
    /// halves stay perfectly well-formed if it is lost. The opaque page here
    /// must encode as RGB, which is exactly what a hard-coded `true` would
    /// turn into RGBA.
    #[test]
    fn render_encodes_what_rasterize_drew_with_the_flag_it_reported() {
        let doc = pdfrum_parser::load(
            std::sync::Arc::from(HALF_BLACK),
            &pdfrum_parser::LoadOptions::default(),
        )
        .expect("loads");
        let page = doc.page(0u32).expect("one page");
        let catalog = doc.catalog().unwrap_or_default();

        let mut ctx = BuildContext::new();
        let (pixmap, transparency) = rasterize(
            &page,
            &catalog,
            &doc,
            DEFAULT_SCALE,
            Backend::Agg,
            &mut ctx,
            &SessionView::default(),
        )
        .expect("renders");

        let mut ctx = BuildContext::new();
        let rendered = render(
            &page,
            &catalog,
            &doc,
            DEFAULT_SCALE,
            Backend::Agg,
            &mut ctx,
            &SessionView::default(),
        )
        .expect("renders");

        let expected = encode(&pixmap, transparency);
        assert_eq!(rendered.digest, expected.digest);
        assert_eq!(rendered.png, expected.png);
        // IHDR colour type 2 is RGB; a lost flag makes it 6.
        assert_eq!(rendered.png.get(25).copied(), Some(2));
    }

    #[test]
    fn output_paths_follow_the_oracle_naming() {
        let path = output_path(Path::new("/tmp/input.pdf"), 3).expect("named");
        assert_eq!(path, PathBuf::from("/tmp/input.pdf.3.png"));
    }

    #[test]
    fn an_over_long_name_is_refused_rather_than_truncated() {
        let long = "x".repeat(300);
        assert!(output_path(Path::new(&long), 0).is_none());
    }

    #[test]
    fn the_md5_is_over_the_raw_bgra_buffer_not_the_png() {
        // A 2x1 opaque white pixmap hashes as eight 0xFF bytes: four per
        // pixel, the fourth being BGRx's padding, which the white clear
        // leaves at 0xFF.
        let pixmap = Pixmap::filled(2, 1, peniko::Color::WHITE);
        let out = encode(&pixmap, false);
        assert_eq!(out.digest, md5_hex(&[0xFF; 8]));
        // And it is *not* the hash of the PNG file.
        assert_ne!(out.digest, md5_hex(&out.png));
    }

    #[test]
    fn an_opaque_page_encodes_as_rgb() {
        let pixmap = Pixmap::filled(4, 3, peniko::Color::from_rgba8(1, 2, 3, 255));
        let out = encode(&pixmap, false);
        // IHDR colour type 2 is RGB; 6 would be RGBA.
        assert_eq!(
            out.png.get(25).copied(),
            Some(2),
            "opaque pages drop the alpha channel"
        );
    }

    #[test]
    fn a_transparent_page_encodes_as_rgba() {
        let pixmap = Pixmap::new(4, 3);
        let out = encode(&pixmap, true);
        assert_eq!(out.png.get(25).copied(), Some(6));
    }

    #[test]
    fn the_md5_line_is_the_oracle_format() {
        assert_eq!(
            md5_line(Path::new("input.pdf.0.png"), "deadbeef"),
            "MD5:input.pdf.0.png:deadbeef\n"
        );
    }

    #[test]
    fn md5_hex_is_lower_case_and_32_wide() {
        let digest = md5_hex(b"");
        assert_eq!(digest, "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(digest.len(), 32);
    }
}
