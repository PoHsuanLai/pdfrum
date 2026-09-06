//! SVG export for pdfrum: a [`RenderDevice`](pdfrum_render::RenderDevice)
//! that accumulates vectors while an ordinary rasterizer does the pixels.
//!
//! [`RenderDevice`](pdfrum_render::RenderDevice) is already vector-typed —
//! `fill_path`, `stroke_path`, `push_clip` and `push_layer` speak
//! `kurbo::BezPath`, `peniko` colours and blend modes, and glyphs above the
//! hinting threshold arrive as outlines — so an SVG consumer needs no change
//! to the engine or to any rasterizer. [`SvgBackend`] wraps a
//! [`RasterBackend`] and records the page
//! target's calls as SVG on the way past.
//!
//! # Converting a page
//!
//! ```
//! use pdfrum_common::Diagnostics;
//! use pdfrum_page::Page;
//! use pdfrum_raster_tinyskia::TinySkiaBackend;
//! use pdfrum_render::RenderOptions;
//! use pdfrum_svg::page_to_svg;
//!
//! # fn main() -> Result<(), pdfrum_render::Error> {
//! let page = Page::empty();
//! let mut diags = Diagnostics::default();
//! let converted = page_to_svg(
//!     &page,
//!     &RenderOptions::default(),
//!     &TinySkiaBackend::new(),
//!     &mut diags,
//! )?;
//!
//! // An empty US Letter page at the default scale.
//! assert!(converted.svg.contains(r#"viewBox="0 0 612 792""#));
//! // Nothing was drawn, so nothing had to be rasterized.
//! assert!(converted.report.is_empty());
//! # Ok(())
//! # }
//! ```
//!
//! # What is vectors and what is pixels
//!
//! Direct fills, strokes, clips, layers and glyph outlines — the large
//! majority of every corpus file — become `<path>`, `<clipPath>` and `<g>`.
//! The regions the engine composites in the pixel domain arrive at the device
//! as `draw_image` and become an embedded PNG, and **every one of them is
//! recorded** in a [`RasterReport`] with a [`RasterCause`]. A silent raster
//! fallback is the failure mode this crate exists to avoid; the report is a
//! deliverable, not a nicety.
//!
//! # Text is outlines, and that overrides one render option
//!
//! [`page_to_svg`] turns
//! [`RenderOptions::subpixel_text_positioning`](pdfrum_render::RenderOptions::subpixel_text_positioning)
//! **on**, whatever the caller passed. It is the one field this crate does not
//! respect, and it is not a preference: with it off — the default, because
//! that is what reproduces the oracle — the engine rasterizes a *glyph bitmap*
//! for every run below fifty device units per em and blits it, which at one
//! pixel per PDF point is nearly all body text. Honouring the default would
//! make an export of a text page some thousands of small embedded PNGs with no
//! vector text in it. The cost is that a glyph lands where the PDF puts it
//! rather than where a golden expects it, which is the trade that option
//! exists to offer and the right side of it for a vector format.
//!
//! `docs/design/svg.md` records the full mapping table and its limits.

#![forbid(unsafe_code)]

mod backend;
mod doc;
mod evidence;
mod png;
mod report;
mod xml;

pub use backend::{SvgBackend, SvgDevice};
pub use report::{RasterCause, RasterRegion, RasterReport};

use pdfrum_common::Diagnostics;
use pdfrum_page::Page;
use pdfrum_render::{Error, RasterBackend, RenderOptions, RenderSession};

/// `opts` with glyph outlines forced on.
///
/// **The one option this crate overrides, and it is not a preference.** With
/// `subpixel_text_positioning` off — the default, because it is what
/// reproduces the oracle — the engine draws every run below fifty device
/// units per em by rasterizing a *glyph bitmap* and blitting it at a snapped
/// origin. At one pixel per PDF point that is nearly all body text, so an
/// export that honoured the default would be some thousands of small embedded
/// PNGs with no vector text in it at all: on `text_foxittext` the difference
/// is 2891 rasterized regions against none.
///
/// Turning it on is what `RenderDevice`'s own documentation means by "glyphs
/// reach the device as outlines above the hinting threshold" — it removes the
/// threshold. The cost is that a glyph lands where the PDF puts it rather
/// than where a golden expects it, which is the trade this option exists to
/// offer and the right side of it for a vector format.
///
/// Every other field of `opts` is the caller's and is passed through.
fn for_vector_text(opts: &RenderOptions) -> RenderOptions {
    RenderOptions {
        subpixel_text_positioning: true,
        ..opts.clone()
    }
}

/// One page converted: the document and what could not be said in vectors.
///
/// A record of two facts rather than a handle with methods — the conversion
/// is finished by the time this exists, and both fields are the caller's to
/// take.
#[derive(Debug, Clone, PartialEq)]
pub struct SvgPage {
    /// The SVG document, root element and all.
    ///
    /// A `String` rather than a file: a caller composes it — into an HTML
    /// page, a multi-page sheet, a byte stream — and this crate has no
    /// opinion about where it goes.
    pub svg: String,
    /// Every region the walk delivered as pixels, with its cause.
    pub report: RasterReport,
}

/// Convert one page to SVG, rasterizing with `backend` what SVG cannot say.
///
/// The document's `viewBox` is the device box the same page would render into
/// under `opts`, so the SVG and a raster render of the same page are in the
/// same coordinates and can be compared pixel for pixel.
///
/// # Errors
///
/// The same as [`pdfrum_render::render_page`]: `Error::TargetEmpty` when the
/// page's box under `opts.transform` is not at least one pixel on both axes,
/// and `Error::TargetTooLarge` when either axis is too big. Damage inside the
/// page is reported through `diags` and never becomes an error.
///
/// ```
/// use pdfrum_common::Diagnostics;
/// use pdfrum_page::Page;
/// use pdfrum_raster_tinyskia::TinySkiaBackend;
/// use pdfrum_render::RenderOptions;
/// use pdfrum_svg::page_to_svg;
///
/// # fn main() -> Result<(), pdfrum_render::Error> {
/// let converted = page_to_svg(
///     &Page::empty(),
///     &RenderOptions::default(),
///     &TinySkiaBackend::new(),
///     &mut Diagnostics::default(),
/// )?;
/// assert!(converted.svg.starts_with("<svg "));
/// # Ok(())
/// # }
/// ```
pub fn page_to_svg<B: RasterBackend>(
    page: &Page,
    opts: &RenderOptions,
    backend: &B,
    diags: &mut Diagnostics,
) -> Result<SvgPage, Error> {
    page_to_svg_with(page, opts, backend, RenderSession::default(), diags)
}

/// Convert one page to SVG, reusing caller-owned caches and honouring
/// optional content.
///
/// The general entry point: [`page_to_svg`] is this with a default
/// [`RenderSession`], and is the right call when neither of the session's
/// parts applies.
///
/// # Errors
///
/// As [`page_to_svg`].
///
/// ```
/// use pdfrum_common::Diagnostics;
/// use pdfrum_page::Page;
/// use pdfrum_raster_tinyskia::TinySkiaBackend;
/// use pdfrum_render::{RenderOptions, RenderSession};
/// use pdfrum_svg::page_to_svg_with;
///
/// # fn main() -> Result<(), pdfrum_render::Error> {
/// let converted = page_to_svg_with(
///     &Page::empty(),
///     &RenderOptions::default(),
///     &TinySkiaBackend::new(),
///     RenderSession::default(),
///     &mut Diagnostics::default(),
/// )?;
/// assert!(converted.report.is_empty());
/// # Ok(())
/// # }
/// ```
pub fn page_to_svg_with<B: RasterBackend>(
    page: &Page,
    opts: &RenderOptions,
    backend: &B,
    session: RenderSession<'_>,
    diags: &mut Diagnostics,
) -> Result<SvgPage, Error> {
    let recorder = SvgBackend::new(backend);
    // The pixmap is discarded: it is the *engine's* working surface, which is
    // what makes the compositing arithmetic — `remove_backdrop`,
    // `knockout_over`, the mask products — come out right. The output is the
    // document the recorder built alongside it.
    pdfrum_render::render_page_with(page, &for_vector_text(opts), &recorder, session, diags)?;
    let (svg, report) = recorder.into_svg();
    Ok(SvgPage { svg, report })
}

#[cfg(test)]
mod tests {
    use pdfrum_raster_tinyskia::TinySkiaBackend;

    use super::*;

    #[test]
    fn an_empty_page_is_an_all_vector_document_of_the_right_size() {
        let converted = page_to_svg(
            &Page::empty(),
            &RenderOptions::default(),
            &TinySkiaBackend::new(),
            &mut Diagnostics::default(),
        )
        .expect("an empty US Letter page renders");
        assert!(converted.svg.contains("viewBox=\"0 0 612 792\""));
        assert!(converted.report.is_empty());
    }

    #[test]
    fn a_borrowed_backend_is_enough() {
        // `SvgBackend<&B>` is what `page_to_svg` builds, so a caller keeps
        // their rasterizer rather than handing it over.
        let tiny = TinySkiaBackend::new();
        for _ in 0..2 {
            let converted = page_to_svg(
                &Page::empty(),
                &RenderOptions::default(),
                &tiny,
                &mut Diagnostics::default(),
            )
            .expect("renders");
            assert!(converted.svg.starts_with("<svg "));
        }
    }
}
