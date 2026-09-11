//! The page handle: size, render, text, words, links, search, markdown,
//! images.

use wasm_bindgen::Clamped;
use wasm_bindgen::prelude::wasm_bindgen;

use crate::error::{Failure, Result};
use crate::options::RenderOptions;
use crate::value::{Hit, ImageInfo, Link, RenderResult, Word};

/// One page of an open document.
///
/// Holds a reference to its document, so the document may be freed while the
/// page is still in use. Free it with `free()` when done — see
/// [`Document`][crate::document::Document] for why that is not the garbage
/// collector's job.
#[wasm_bindgen]
#[derive(Debug)]
pub struct Page(pdfrum::OwnedPage);

/// The rasterizer every render in this binding uses.
///
/// `vello-cpu`: pure Rust, no threads and no GPU handshake, which is what
/// `wasm32-unknown-unknown` supports. The binding takes no backend argument
/// for the same reason the C library does not — choosing a rasterizer is a
/// decision about which crate to compile, and this module is compiled with
/// one.
fn backend() -> pdfrum::VelloCpuBackend {
    pdfrum::VelloCpuBackend::new()
}

impl Page {
    /// Wraps a facade page as a handle for JavaScript.
    pub(crate) fn new(page: pdfrum::OwnedPage) -> Page {
        Page(page)
    }

    /// The pixel size this page renders to at `scale`, checked.
    ///
    /// Rejects a scale that is not a positive finite number, and a scaled
    /// page that does not fit a pixmap, before anything is allocated.
    ///
    /// # Errors
    ///
    /// `Failure::Argument` for either rejection.
    fn render_size(&self, scale: f64) -> Result<(u32, u32)> {
        if !(scale.is_finite() && scale > 0.0) {
            return Err(Failure::Argument("scale must be finite and positive"));
        }
        let width = (self.0.width() * scale).ceil();
        let height = (self.0.height() * scale).ceil();
        if !(width.is_finite() && height.is_finite())
            || width < 1.0
            || height < 1.0
            || width > f64::from(u32::MAX)
            || height > f64::from(u32::MAX)
        {
            return Err(Failure::Argument(
                "the scaled page does not fit in a pixmap",
            ));
        }
        // The bounds above put both values in `u32`, and `as` on a checked
        // finite positive `f64` truncates toward zero, which after `ceil` is
        // the integer itself.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok((width as u32, height as u32))
    }
}

#[wasm_bindgen]
impl Page {
    /// The page's width in PDF points, as its boxes and rotation give it.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn width(&self) -> f64 {
        self.0.width()
    }

    /// The page's height in PDF points.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn height(&self) -> f64 {
        self.0.height()
    }

    /// Renders the page at `scale`, where 1 is 72 dpi.
    ///
    /// The result's `data` is RGBA8 with straight (not premultiplied) alpha,
    /// row-major and top-down — what `ImageData` takes:
    ///
    /// ```js
    /// const r = page.render(2);
    /// ctx.putImageData(new ImageData(r.data, r.width, r.height), 0, 0);
    /// ```
    ///
    /// # Errors
    ///
    /// Throws with `.code === 9` when the render would exceed the document's
    /// `maxRenderPixels` or its cancellation flag has been raised, with
    /// `.code === 5` when the page cannot be rasterized, and with
    /// `.code === 100` for a scale that is not a positive finite number.
    pub fn render(&self, scale: f64, options: Option<RenderOptions>) -> Result<RenderResult> {
        let (width, height) = self.render_size(scale)?;
        let mut facade = pdfrum::RenderOptions::scaled(scale);
        facade.annotations = options.unwrap_or_default().with_annotations;
        let pixmap = self.0.render_with(backend(), &facade)?;
        Ok(RenderResult {
            width,
            height,
            // The facade's pixmap is premultiplied; `ImageData` is straight,
            // so the conversion happens here rather than being left to a
            // caller who would have to know it was needed.
            data: Clamped(pixmap.to_straight_rgba()),
        })
    }

    /// The page's text, in reading order.
    ///
    /// The indices in [`Word`] and [`Hit`] are character indices into this
    /// string.
    pub fn text(&self) -> String {
        self.0.text().to_string()
    }

    /// The page's words, in reading order, each with its box and font.
    #[must_use]
    pub fn words(&self) -> Vec<Word> {
        self.0
            .words()
            .into_iter()
            .map(|word| Word {
                text: word.text.clone(),
                x0: word.rect.x0,
                y0: word.rect.y0,
                x1: word.rect.x1,
                y1: word.rect.y1,
                size: word.size,
                start: word.range.start.get(),
                end: word.range.end.get(),
                font: word.font.clone(),
            })
            .collect()
    }

    /// The page's links.
    #[must_use]
    pub fn links(&self) -> Vec<Link> {
        self.0
            .page_links()
            .into_iter()
            .map(|link| {
                let (uri, page_index) = match link.target {
                    pdfrum::LinkTarget::Uri(uri) => (Some(uri), None),
                    pdfrum::LinkTarget::Page(index) => (None, Some(index.get())),
                    _ => (None, None),
                };
                Link {
                    x0: link.rect.x0,
                    y0: link.rect.y0,
                    x1: link.rect.x1,
                    y1: link.rect.y1,
                    uri,
                    page_index,
                }
            })
            .collect()
    }

    /// Every occurrence of `needle` in the page's text.
    ///
    /// `ignoreCase` defaults to false — a case-sensitive search. An empty
    /// needle yields an empty array rather than a hit at every position.
    pub fn search(&self, needle: &str, ignore_case: Option<bool>) -> Vec<Hit> {
        if needle.is_empty() {
            return Vec::new();
        }
        let options = pdfrum::FindOptions {
            match_case: !ignore_case.unwrap_or(false),
            ..pdfrum::FindOptions::default()
        };
        self.0
            .text()
            .find(needle, options)
            .map(|range| Hit {
                start: range.start.get(),
                end: range.end.get(),
            })
            .collect()
    }

    /// The page as Markdown.
    ///
    /// The structure tree where the document has one, typography where it does
    /// not.
    #[must_use]
    pub fn markdown(&self) -> String {
        self.0.markdown()
    }

    /// The images drawn on the page, decoded.
    ///
    /// Each carries its own pixels in the same RGBA8 straight-alpha shape
    /// [`Page::render`] produces, so the same `ImageData` line displays one.
    #[must_use]
    pub fn images(&self) -> Vec<ImageInfo> {
        self.0
            .images()
            .into_iter()
            .map(|image| ImageInfo {
                width: image.width,
                height: image.height,
                is_mask: image.is_mask,
                data: Clamped(image.pixmap().to_straight_rgba()),
            })
            .collect()
    }
}
