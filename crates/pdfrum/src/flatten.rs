//! Flattening: an annotation's appearance baked into the page content, so
//! the page looks the same with the annotation gone.

use crate::DocEdit;
use kurbo::{Affine, Rect};
use pdfrum_common::{Diagnostics, PageIndex};
use pdfrum_object::{Array, ByteSpan, Dict, Name, ObjRef, Object, Resolve, Stream};
use std::collections::HashSet;

/// Which annotations flattening draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlattenMode {
    /// What a viewer shows: every annotation not flagged invisible.
    Display,
    /// What a printer shows: only annotations flagged for print.
    Print,
}

/// What flattening a page did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flattened {
    /// The page's annotations were drawn into its content and removed.
    Done,
    /// The page had no annotations to draw.
    NothingToDo,
}

impl DocEdit<'_> {
    /// The annotations flatten draws for `mode`, in `/Annots` order: pop-ups
    /// and hidden ones never; for display every non-invisible one, for print
    /// only those flagged for print.
    fn flatten_candidates(&self, page: &Dict, mode: FlattenMode) -> Vec<(usize, Dict)> {
        let Some(annots) = page.array(&Name::from("Annots"), &self.inner) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for index in 0..annots.len() {
            let Some(dict) = annots.dict_at(index, &self.inner) else {
                continue;
            };
            let is_popup = dict
                .get(&Name::from("Subtype"), &self.inner)
                .is_some_and(|o| o.get().as_name().is_some_and(|n| n.as_bytes() == b"Popup"));
            if is_popup {
                continue;
            }
            let flags = dict.int(&Name::from("F"), &self.inner).unwrap_or(0);
            if flags & 2 != 0 {
                continue;
            }
            let keep = match mode {
                FlattenMode::Display => flags & 1 == 0,
                FlattenMode::Print => flags & 4 != 0,
            };
            if keep {
                out.push((index, dict));
            }
        }
        out
    }

    /// The normal appearance flatten draws — `/AP /N` when it is a stream;
    /// otherwise the `/AS` state's entry of the `/N` dictionary, or with no
    /// `/AS` its first entry when that is a stream — as a reference and the
    /// stream: an inline stream is copied into a new indirect object first.
    fn flatten_appearance(&mut self, annot: &Dict) -> Option<(ObjRef, Stream)> {
        let ap = annot.dict(&Name::from("AP"), &self.inner)?;
        let states: Dict = match ap.raw(&Name::from("N"))? {
            Object::Ref(reference) => {
                let object = self.inner.fetch(*reference).ok()?;
                if let Some(stream) = object.as_stream() {
                    return Some((*reference, stream.clone()));
                }
                object.as_dict()?.clone()
            }
            Object::Stream(stream) => {
                let stream = stream.clone();
                let reference = self.inner.add(Object::Stream(stream.clone()));
                return Some((reference, *stream));
            }
            Object::Dict(states) => states.clone(),
            _ => return None,
        };
        let state = annot
            .get(&Name::from("AS"), &self.inner)
            .and_then(|o| o.get().as_name().cloned());
        let entry = match state {
            Some(state) => states.raw(&state).cloned()?,
            None => states.iter().next().map(|(_, o)| o.clone())?,
        };
        match entry {
            Object::Ref(reference) => {
                let object = self.inner.fetch(reference).ok()?;
                Some((reference, object.as_stream()?.clone()))
            }
            Object::Stream(stream) => {
                let reference = self.inner.add(Object::Stream(stream.clone()));
                Some((reference, *stream))
            }
            _ => None,
        }
    }

    /// Drops every font `/Encoding` in `resources`' `/Font` whose
    /// `/BaseEncoding` is not one of the three standard names (an absent
    /// `/BaseEncoding` is fine): an indirect font dictionary is replaced in
    /// the document, an inline one rewritten in the returned copy of
    /// `resources`.
    fn sanitize_resources(&mut self, mut resources: Dict) -> Dict {
        let font_key = Name::from("Font");
        let Some(fonts) = resources.dict(&font_key, &self.inner) else {
            return resources;
        };
        let mut fonts_out = fonts.clone();
        for (key, value) in fonts.iter() {
            let (reference, font): (Option<ObjRef>, Dict) = match value {
                Object::Ref(reference) => {
                    let Ok(object) = self.inner.fetch(*reference) else {
                        continue;
                    };
                    let Some(dict) = object.as_dict().cloned() else {
                        continue;
                    };
                    (Some(*reference), dict)
                }
                Object::Dict(dict) => (None, dict.clone()),
                _ => continue,
            };
            let Some(encoding) = font.dict(&Name::from("Encoding"), &self.inner) else {
                continue;
            };
            let base = encoding
                .get(&Name::from("BaseEncoding"), &self.inner)
                .and_then(|o| o.get().as_name().cloned());
            let valid = base.is_none_or(|name| {
                matches!(
                    name.as_bytes(),
                    b"WinAnsiEncoding" | b"MacRomanEncoding" | b"MacExpertEncoding"
                )
            });
            if valid {
                continue;
            }
            let mut fixed = font.clone();
            fixed.remove(&Name::from("Encoding"));
            match reference {
                Some(reference) => self.inner.replace(reference, Object::Dict(fixed)),
                None => fonts_out.insert(key.clone(), Object::Dict(fixed)),
            }
        }
        match resources.raw(&font_key).cloned() {
            Some(Object::Ref(reference)) => self.inner.replace(reference, Object::Dict(fonts_out)),
            Some(Object::Dict(_)) => resources.insert(font_key, Object::Dict(fonts_out)),
            _ => {}
        }
        resources
    }

    /// Wraps the page's content in `q … Q` and appends the flattened form's
    /// `Do`, the reference's way: no `/Contents` gets one new stream; an
    /// array gets a `q` stream inserted first and `Q` then the `Do` streams
    /// appended; a single stream is rewritten unfiltered as
    /// `q\n<its decoded bytes>\nQ` and becomes the first element of a new
    /// array followed by the `Do` stream.
    fn set_page_contents(&mut self, page: &mut Dict, key: &str) {
        let contents = Name::from("Contents");
        let do_text = format!("q 1 0 0 1 0 0 cm /{key} Do Q");
        let mut array: Option<(Array, Option<ObjRef>)> = None;
        let mut single: Option<(Stream, ObjRef)> = None;
        match page.raw(&contents).cloned() {
            Some(Object::Ref(reference)) => {
                if let Ok(object) = self.inner.fetch(reference) {
                    if let Some(a) = object.as_array() {
                        array = Some((a.clone(), Some(reference)));
                    } else if let Some(s) = object.as_stream() {
                        single = Some((s.clone(), reference));
                    }
                }
            }
            Some(Object::Array(a)) => array = Some((a, None)),
            Some(Object::Stream(s)) => {
                let reference = self.inner.add(Object::Stream(s.clone()));
                single = Some((*s, reference));
            }
            _ => {}
        }
        if let Some((mut a, reference)) = array {
            let q = self.raw_stream(b"q".to_vec());
            a.insert(0, Object::Ref(q));
            let restore = self.raw_stream(b"Q".to_vec());
            a.push(Object::Ref(restore));
            let draw = self.raw_stream(do_text.into_bytes());
            a.push(Object::Ref(draw));
            match reference {
                Some(reference) => self.inner.replace(reference, Object::Array(a)),
                None => page.insert(contents, Object::Array(a)),
            }
        } else if let Some((stream, reference)) = single {
            let mut diags = Diagnostics::default();
            let decoded =
                pdfrum_filters::decode_chain(&stream, 0, &self.inner, &self.doc.limits, &mut diags)
                    .data;
            let mut bytes = b"q\n".to_vec();
            bytes.extend_from_slice(&decoded);
            bytes.extend_from_slice(b"\nQ");
            let mut dict = stream.dict.clone();
            dict.remove(&Name::from("Filter"));
            dict.remove(&Name::from("DecodeParms"));
            dict.remove(&Name::from("Length"));
            self.inner.replace(
                reference,
                Object::Stream(Box::new(Stream::new(dict, ByteSpan::from(bytes)))),
            );
            let draw = self.raw_stream(do_text.into_bytes());
            let array_ref = self.inner.add(Object::Array(Array::of([
                Object::Ref(reference),
                Object::Ref(draw),
            ])));
            page.insert(contents, Object::Ref(array_ref));
        } else {
            let draw = self.raw_stream(do_text.into_bytes());
            page.insert(contents, Object::Ref(draw));
        }
    }

    /// A new unfiltered stream holding `bytes`.
    fn raw_stream(&mut self, bytes: Vec<u8>) -> ObjRef {
        self.inner.add(Object::Stream(Box::new(Stream::new(
            Dict::new(),
            ByteSpan::from(bytes),
        ))))
    }

    /// The widget annotations of `page` that no other page also lists, by
    /// reference: the fields flattening retires.
    fn flattened_widgets(&self, page: &Dict, page_ref: Option<ObjRef>) -> HashSet<ObjRef> {
        let annots_key = Name::from("Annots");
        let mut widgets = HashSet::new();
        if let Some(annots) = page.array(&annots_key, &self.inner) {
            for index in 0..annots.len() {
                let (Some(reference), Some(dict)) = (
                    annots.reference_at(index),
                    annots.dict_at(index, &self.inner),
                ) else {
                    continue;
                };
                let is_widget = dict
                    .get(&Name::from("Subtype"), &self.inner)
                    .is_some_and(|o| o.get().as_name().is_some_and(|n| n.as_bytes() == b"Widget"));
                if is_widget {
                    widgets.insert(reference);
                }
            }
        }
        for index in 0..self.doc.page_count() {
            if widgets.is_empty() {
                break;
            }
            let Ok(other) = self.doc.inner.page(index) else {
                continue;
            };
            if other.reference.is_some() && other.reference == page_ref {
                continue;
            }
            // Through the edits: a page flattened earlier in this session no
            // longer lists anything, and must not keep its neighbour's widgets
            // alive.
            let other_dict = other
                .reference
                .and_then(|reference| self.inner.fetch(reference).ok())
                .and_then(|object| object.as_dict().cloned())
                .unwrap_or(other.dict);
            if let Some(annots) = other_dict.array(&annots_key, &self.inner) {
                for slot in 0..annots.len() {
                    if let Some(reference) = annots.reference_at(slot) {
                        widgets.remove(&reference);
                    }
                }
            }
        }
        widgets
    }

    /// Removes the retired widgets from `fields`, recursing through `/Kids`
    /// (a field whose kids all go goes too); whether `fields` is empty
    /// afterwards.
    fn prune_fields(&mut self, fields: &mut Array, widgets: &HashSet<ObjRef>, level: u32) -> bool {
        if level > 32 {
            return fields.is_empty();
        }
        let kids_key = Name::from("Kids");
        for index in (0..fields.len()).rev() {
            let Some(reference) = fields.reference_at(index) else {
                continue;
            };
            let mut prune = widgets.contains(&reference);
            if !prune
                && let Ok(object) = self.inner.fetch(reference)
                && let Some(field) = object.as_dict()
            {
                match field.raw(&kids_key).cloned() {
                    Some(Object::Ref(kids_ref)) => {
                        if let Ok(kids_object) = self.inner.fetch(kids_ref)
                            && let Some(kids) = kids_object.as_array()
                        {
                            let mut kids = kids.clone();
                            prune = self.prune_fields(&mut kids, widgets, level + 1);
                            self.inner.replace(kids_ref, Object::Array(kids));
                        }
                    }
                    Some(Object::Array(mut kids)) => {
                        prune = self.prune_fields(&mut kids, widgets, level + 1);
                        let mut field = field.clone();
                        field.insert(kids_key.clone(), Object::Array(kids));
                        self.inner.replace(reference, Object::Dict(field));
                    }
                    _ => {}
                }
            }
            if prune {
                fields.remove(index);
            }
        }
        fields.is_empty()
    }

    /// Prunes the retired widgets from `/AcroForm /Fields` and removes the
    /// form itself when its fields are empty and it has no `/XFA`; whatever
    /// was rewritten is replaced where indirect and rewritten in its parent
    /// where inline.
    fn remove_flattened_fields(&mut self, widgets: &HashSet<ObjRef>) {
        if widgets.is_empty() {
            return;
        }
        let Some(root) = self.doc.inner.trailer().reference(&Name::from("Root")) else {
            return;
        };
        let Some(mut catalog) = self
            .inner
            .fetch(root)
            .ok()
            .as_deref()
            .and_then(Object::as_dict)
            .cloned()
        else {
            return;
        };
        let acro_key = Name::from("AcroForm");
        let fields_key = Name::from("Fields");
        let (acro_ref, mut acro) = match catalog.raw(&acro_key).cloned() {
            Some(Object::Ref(reference)) => {
                let Some(dict) = self
                    .inner
                    .fetch(reference)
                    .ok()
                    .as_deref()
                    .and_then(Object::as_dict)
                    .cloned()
                else {
                    return;
                };
                (Some(reference), dict)
            }
            Some(Object::Dict(dict)) => (None, dict),
            _ => return,
        };
        let (fields_ref, mut fields) = match acro.raw(&fields_key).cloned() {
            Some(Object::Ref(reference)) => {
                let Some(array) = self
                    .inner
                    .fetch(reference)
                    .ok()
                    .as_deref()
                    .and_then(Object::as_array)
                    .cloned()
                else {
                    return;
                };
                (Some(reference), array)
            }
            Some(Object::Array(array)) => (None, array),
            _ => return,
        };
        let empty = self.prune_fields(&mut fields, widgets, 0);
        match fields_ref {
            Some(reference) => self.inner.replace(reference, Object::Array(fields)),
            None => acro.insert(fields_key, Object::Array(fields)),
        }
        if empty && !acro.contains_key(&Name::from("XFA")) {
            catalog.remove(&acro_key);
            self.inner.replace(root, Object::Dict(catalog));
            return;
        }
        if let Some(reference) = acro_ref {
            self.inner.replace(reference, Object::Dict(acro));
        } else {
            catalog.insert(acro_key, Object::Dict(acro));
            self.inner.replace(root, Object::Dict(catalog));
        }
    }

    /// Draws one annotation's appearance into the flattened form: the
    /// appearance stream becomes form `XObject` `F<index>` of the form's
    /// resources and `content` gains its `Do` through the flattening matrix.
    fn flatten_one(
        &mut self,
        index: usize,
        annot: &Dict,
        generated: Option<&pdfrum_doc::ap::GeneratedAp>,
        xobject_dict: &mut Dict,
        content: &mut String,
    ) {
        use std::fmt::Write as _;
        let mut annot_rect = annot.rect(&Name::from("Rect"), &self.inner).abs();
        // What a viewer draws: the appearance generated for a widget that has
        // none, or whose form asks for regeneration, is the one flattened —
        // the reference generates it at load time and flattens that.
        let appearance = match generated {
            Some(made) if !made.stream.is_empty() => {
                if let Some(rect) = made.rect_override {
                    annot_rect = rect.abs();
                }
                let stream = Stream::new(
                    pdfrum_doc::ap::stream_dict(made),
                    ByteSpan::from(made.stream.clone()),
                );
                let reference = self.inner.add(Object::Stream(Box::new(stream.clone())));
                Some((reference, stream))
            }
            _ => self.flatten_appearance(annot),
        };
        let Some((ap_ref, ap_stream)) = appearance else {
            return;
        };
        let mut ap_dict = ap_stream.dict.clone();
        let box_key = if ap_dict.contains_key(&Name::from("Rect")) {
            Name::from("Rect")
        } else {
            Name::from("BBox")
        };
        let mut stream_rect = ap_dict.rect(&box_key, &self.inner).abs();
        if stream_rect.width() <= 0.0 || stream_rect.height() <= 0.0 {
            // An appearance with no box of its own is drawn at the origin
            // over the annotation's own extent, which is what a viewer
            // shows; the reference skips it and the page goes blank.
            if annot_rect.width() <= 0.0 || annot_rect.height() <= 0.0 {
                return;
            }
            stream_rect = Rect::new(0.0, 0.0, annot_rect.width(), annot_rect.height());
            ap_dict.insert(Name::from("BBox"), rect_object(stream_rect));
        }
        ap_dict.insert(Name::from("Type"), Object::Name(Name::from("XObject")));
        ap_dict.insert(Name::from("Subtype"), Object::Name(Name::from("Form")));
        if let Some(res) = ap_dict.dict(&Name::from("Resources"), &self.inner) {
            let res = self.sanitize_resources(res);
            ap_dict.insert(Name::from("Resources"), Object::Dict(res));
        }
        let matrix = flatten_matrix(
            annot_rect,
            stream_rect,
            ap_stream.dict.matrix(&Name::from("Matrix"), &self.inner),
        );
        self.inner.replace(
            ap_ref,
            Object::Stream(Box::new(Stream::new(ap_dict, ap_stream.data.clone()))),
        );
        let name = format!("F{index}");
        xobject_dict.insert(Name::from(name.as_str()), Object::Ref(ap_ref));
        let [sx, ky, kx, sy, tx, ty] = matrix.as_coeffs();
        let _ = writeln!(content, "q {sx} {ky} {kx} {sy} {tx} {ty} cm /{name} Do Q");
    }

    /// Bakes the page's annotation appearances into its content and removes
    /// the annotations, the reference implementation's way: one form `XObject`
    /// `FFT<n>` (the first such name free in the page's `/XObject`
    /// resources), its box the crop box, drawing each appearance stream
    /// through the matrix that lands its box on the annotation's rectangle;
    /// the page's own content wrapped in `q … Q` with the form's `Do` after
    /// it; `/MediaBox` and `/CropBox` normalized and written; the retired
    /// widgets pruned from the interactive form, which goes when it empties.
    /// A widget another page also lists keeps the form.
    ///
    /// ```
    /// use pdfrum::{Document, FlattenMode, Flattened, SaveOptions};
    ///
    /// let doc = Document::open("tests/fixtures/annotiter.pdf")?;
    /// let mut edit = doc.edit();
    /// assert_eq!(edit.flatten(0, FlattenMode::Display)?, Flattened::Done);
    /// let mut flat = Vec::new();
    /// edit.write_to(&mut flat, &SaveOptions::default())?;
    ///
    /// let plain = Document::open("tests/fixtures/hello_world.pdf")?;
    /// assert_eq!(plain.edit().flatten(0, FlattenMode::Display)?, Flattened::NothingToDo);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// When `page` is out of range or the document has no catalog.
    pub fn flatten(
        &mut self,
        page: impl Into<PageIndex>,
        mode: FlattenMode,
    ) -> crate::Result<Flattened> {
        let page_dict = self.doc.inner.page(page.into())?;
        let page_ref = page_dict.reference;
        let mut dict = page_dict.dict.clone();
        if dict.array(&Name::from("Annots"), &self.inner).is_none() {
            return Ok(Flattened::NothingToDo);
        }
        let candidates = self.flatten_candidates(&dict, mode);
        let catalog = self.doc.catalog();
        let mut diags = Diagnostics::default();
        let mut build = pdfrum_page::BuildContext::new();
        let fonts = pdfrum_doc::ap::FormFonts::load(&catalog, &self.inner, &mut build);
        let generated = pdfrum_doc::ap::generate_appearances_with_text(
            &dict,
            &catalog,
            Some(&fonts),
            &self.inner,
            &mut diags,
        );
        self.doc.note(&diags);

        let crop_key = Name::from("CropBox");
        let mut media = dict.rect(&Name::from("MediaBox"), &self.inner).abs();
        if dict.contains_key(&crop_key) {
            media = dict.rect(&crop_key, &self.inner).abs();
        }
        if media.width() <= 0.0 || media.height() <= 0.0 {
            media = Rect::new(0.0, 0.0, 612.0, 792.0);
        }
        let mut crop = if dict.contains_key(&crop_key) {
            dict.rect(&crop_key, &self.inner).abs()
        } else {
            Rect::ZERO
        };
        if crop.width() <= 0.0 || crop.height() <= 0.0 {
            crop = media;
        }
        dict.insert(Name::from("MediaBox"), rect_object(media));
        dict.insert(crop_key, rect_object(crop));

        let mut resources = dict
            .dict(&Name::from("Resources"), &self.inner)
            .unwrap_or_default();
        let mut xobjects = resources
            .dict(&Name::from("XObject"), &self.inner)
            .unwrap_or_default();
        let key = if candidates.is_empty() {
            None
        } else {
            (0..u32::MAX)
                .map(|i| format!("FFT{i}"))
                .find(|k| !xobjects.contains_key(&Name::from(k.as_str())))
        };
        if let Some(key) = key {
            self.set_page_contents(&mut dict, &key);
            let mut xobject_dict = Dict::new();
            let mut content = String::new();
            for (slot, (index, annot)) in candidates.iter().enumerate() {
                let made_appearance = generated.get(*index);
                self.flatten_one(
                    slot,
                    annot,
                    made_appearance,
                    &mut xobject_dict,
                    &mut content,
                );
            }
            let mut form_resources = Dict::new();
            form_resources.insert(Name::from("XObject"), Object::Dict(xobject_dict));
            let mut form_dict = Dict::new();
            form_dict.insert(Name::from("Type"), Object::Name(Name::from("XObject")));
            form_dict.insert(Name::from("Subtype"), Object::Name(Name::from("Form")));
            form_dict.insert(Name::from("FormType"), Object::Int(1));
            form_dict.insert(Name::from("BBox"), rect_object(crop));
            form_dict.insert(Name::from("Resources"), Object::Dict(form_resources));
            let form_ref = self.inner.add(Object::Stream(Box::new(Stream::new(
                form_dict,
                ByteSpan::from(content.into_bytes()),
            ))));
            xobjects.insert(Name::from(key.as_str()), Object::Ref(form_ref));
            resources.insert(Name::from("XObject"), Object::Dict(xobjects));
            dict.insert(Name::from("Resources"), Object::Dict(resources));
        }

        let widgets = self.flattened_widgets(&dict, page_ref);
        dict.remove(&Name::from("Annots"));
        if let Some(reference) = page_ref {
            self.inner.replace(reference, Object::Dict(dict));
        }
        self.remove_flattened_fields(&widgets);
        Ok(Flattened::Done)
    }
}

/// A rectangle as the four-number array the file writes.
#[expect(
    clippy::cast_possible_truncation,
    reason = "page geometry fits f32, as the file wrote it"
)]
fn rect_object(rect: Rect) -> Object {
    Object::Array(Array::of([
        Object::Real(rect.x0 as f32),
        Object::Real(rect.y0 as f32),
        Object::Real(rect.x1 as f32),
        Object::Real(rect.y1 as f32),
    ]))
}

/// The matrix that lands the appearance's bounding box, taken through its own
/// `/Matrix`, on the annotation's rectangle — a scale and a translation only,
/// `b` and `c` zeroed as the reference does.
fn flatten_matrix(annot_rect: Rect, stream_rect: Rect, matrix: Affine) -> Affine {
    let transformed = matrix.transform_rect_bbox(stream_rect).abs();
    if transformed.width() <= 0.0 || transformed.height() <= 0.0 {
        return Affine::IDENTITY;
    }
    let a = annot_rect.width() / transformed.width();
    let d = annot_rect.height() / transformed.height();
    Affine::new([
        a,
        0.0,
        0.0,
        d,
        annot_rect.x0 - transformed.x0 * a,
        annot_rect.y0 - transformed.y0 * d,
    ])
}
