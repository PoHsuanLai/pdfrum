//! A page's thumbnail image (`/Thumb`, ISO 32000-1 §12.3.2).

use crate::{Page, Pixmap};
use pdfrum_object::{Name, Stream};

impl Page<'_> {
    /// The `/Thumb` stream, looked for only on a page dictionary that
    /// declares its `/Type`.
    fn thumbnail_stream(&self) -> Option<Stream> {
        if !self.dict.dict.contains_key(&Name::from("Type")) {
            return None;
        }
        self.dict.dict.stream(&Name::from("Thumb"), &self.doc.inner)
    }

    /// The thumbnail stream's bytes as stored, filters and all; `None` when
    /// the page has no thumbnail.
    #[must_use]
    pub fn thumbnail_raw(&self) -> Option<Vec<u8>> {
        self.thumbnail_stream()
            .map(|stream| stream.data.as_bytes().to_vec())
    }

    /// The thumbnail stream decoded through its filters — the image's samples,
    /// not yet a picture; `None` when the page has no thumbnail.
    #[must_use]
    pub fn thumbnail_data(&self) -> Option<Vec<u8>> {
        let mut diags = pdfrum_common::Diagnostics::default();
        let data = self.thumbnail_stream().map(|stream| {
            pdfrum_filters::decode_chain(&stream, 0, &self.doc.inner, &self.doc.limits, &mut diags)
                .data
        });
        self.doc.note(&diags);
        data
    }

    /// The thumbnail as a picture, decoded against the page's resources;
    /// `None` when the page has no thumbnail or the stream does not decode,
    /// an empty stream included.
    ///
    /// ```
    /// use pdfrum::Document;
    ///
    /// let doc = Document::open("tests/fixtures/simple_thumbnail.pdf")?;
    /// let thumbnail = doc.page(0)?.thumbnail().expect("the page carries one");
    /// assert_eq!((thumbnail.width(), thumbnail.height()), (50, 50));
    ///
    /// let plain = Document::open("tests/fixtures/hello_world.pdf")?;
    /// assert!(plain.page(0)?.thumbnail().is_none());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn thumbnail(&self) -> Option<Pixmap> {
        let mut diags = pdfrum_common::Diagnostics::default();
        let pixmap = self.thumbnail_stream().and_then(|stream| {
            let r = &self.doc.inner;
            let page_resources = self
                .dict
                .inherited(&Name::from("Resources"), r)
                .and_then(|object| object.resolve(r).ok()?.get().as_dict().cloned());
            let mut functions = pdfrum_page::FunctionCache::default();
            pdfrum_page::decode_image(
                &stream,
                None,
                page_resources.as_ref(),
                pdfrum_page::RequestedSize::Full,
                r,
                &mut functions,
                &self.doc.limits,
                &mut diags,
            )
            .ok()
            .map(|image| pdfrum_render::image_to_pixmap(&image))
        });
        self.doc.note(&diags);
        pixmap
    }
}
