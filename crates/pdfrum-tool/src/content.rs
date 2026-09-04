//! Assembling a page's content stream and interpreting it.
//!
//! `/Contents` is either one stream or an array of them, and the array's
//! members are **joined with a single space each** — including one after the
//! last, which is why a stream ending mid-token still terminates. Getting the
//! separator wrong merges the last operator of one stream with the first of
//! the next.
//!
//! This lives in the tool rather than in `pdfrum-page` because it is the step
//! that needs a *document*: the page crate's contract starts at operators and
//! a resource dictionary, deliberately, so a content stream can be
//! interpreted without a file behind it.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Name, Object, Resolve};
use pdfrum_page::{BuildContext, Page, Resources, build_page_from_dict, parse_content};
use pdfrum_parser::PageDict;

/// A page's whole content stream, decoded and joined.
///
/// Empty when `/Contents` is absent or is neither a stream nor an array,
/// which yields a page with no objects rather than an error — the same thing
/// the oracle's parser does when its content stage fails.
#[must_use]
pub fn assemble<R: Resolve>(
    page: &Dict,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Vec<u8> {
    let key = Name::from("Contents");
    let Some(contents) = page.get(&key, r) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut push = |object: &Object| {
        if let Some(stream) = object.as_stream() {
            let decoded = pdfrum_filters::decode_chain(stream, 0, r, limits, diags);
            out.extend_from_slice(&decoded.data);
            // One space after **every** stream, the last included.
            out.push(b' ');
        }
    };
    let Some(direct) = contents.as_direct() else {
        return out;
    };
    match direct {
        Object::Stream(_) => push(direct),
        Object::Array(array) => {
            for element in array.iter() {
                // A reference that will not resolve contributes nothing, the
                // same as a non-stream element.
                if let Ok(resolved) = element.resolve(r) {
                    push(resolved.get());
                }
            }
        }
        _ => {}
    }
    out
}

/// Interprets one page: assemble its content, resolve its resources, fold the
/// operators into a page-object graph.
#[must_use]
pub fn build<R: Resolve>(
    page: &PageDict,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Page {
    let bytes = assemble(&page.dict, r, limits, diags);
    let ops = parse_content(&bytes, limits, diags);
    let resources = Resources::for_page(
        page.inherited(&Name::from("Resources"), r)
            .and_then(|object| object.resolve(r).ok()?.as_dict().cloned()),
    );
    build_page_from_dict(
        &ops,
        &page.dict,
        |key| page.inherited(key, r),
        &resources,
        r,
        ctx,
        limits,
        diags,
    )
}

/// A page's content, joined, plus where each `/Contents` element ends.
///
/// [`assemble`] with the boundaries kept. Only the editor wants them: the
/// interpreter reads the joined buffer as one run, and an element's index
/// matters solely to a save that will write one of them again.
#[must_use]
pub fn assemble_segments<R: Resolve>(
    page: &Dict,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> (Vec<u8>, Vec<usize>) {
    let key = Name::from("Contents");
    let Some(contents) = page.get(&key, r) else {
        return (Vec::new(), Vec::new());
    };
    let mut out = Vec::new();
    let mut ends = Vec::new();
    let mut push = |object: &Object, out: &mut Vec<u8>, ends: &mut Vec<usize>| {
        if let Some(stream) = object.as_stream() {
            let decoded = pdfrum_filters::decode_chain(stream, 0, r, limits, diags);
            out.extend_from_slice(&decoded.data);
            out.push(b' ');
        }
        ends.push(out.len());
    };
    let Some(direct) = contents.as_direct() else {
        return (out, ends);
    };
    match direct {
        Object::Stream(_) => push(direct, &mut out, &mut ends),
        Object::Array(array) => {
            for element in array.iter() {
                if let Ok(resolved) = element.resolve(r) {
                    push(resolved.get(), &mut out, &mut ends);
                } else {
                    // A dangling element still occupies an index, so the ones
                    // after it keep their numbers.
                    ends.push(out.len());
                }
            }
        }
        _ => {}
    }
    (out, ends)
}

/// Interprets one page, recording which `/Contents` element each object came
/// from.
///
/// [`build`] plus the two facts only an editor needs: each object's element
/// index, and the transform each element leaves behind.
#[must_use]
pub fn build_page_for_edit<R: Resolve>(
    doc: &R,
    page: &PageDict,
    resources: &Dict,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Page {
    let (bytes, ends) = assemble_segments(&page.dict, doc, limits, diags);
    let ops = parse_content(&bytes, limits, diags);
    let bounds = pdfrum_page::StreamBounds::from_joined(&bytes, ops.len(), &ends, limits);
    pdfrum_page::build_page_streams(
        &ops,
        &bounds,
        &page.dict,
        |key| page.inherited(key, doc),
        &Resources::for_page(Some(resources.clone())),
        doc,
        ctx,
        limits,
        diags,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfrum_object::{Array, NoResolve, PdfString, Stream};

    fn stream(bytes: &[u8]) -> Object {
        Object::Stream(Box::new(Stream::new(Dict::new(), bytes.to_vec().into())))
    }

    fn assembled(page: &Dict) -> Vec<u8> {
        let mut diags = Diagnostics::default();
        assemble(page, &NoResolve, &Limits::default(), &mut diags)
    }

    #[test]
    fn a_missing_contents_yields_nothing() {
        assert!(assembled(&Dict::new()).is_empty());
    }

    #[test]
    fn one_stream_is_followed_by_a_space() {
        // The trailing space is not cosmetic: it terminates a stream that
        // ends mid-token.
        let page = Dict::from_pairs([(Name::from("Contents"), stream(b"0 0 m"))]);
        assert_eq!(assembled(&page), b"0 0 m ");
    }

    #[test]
    fn an_array_joins_its_streams_with_single_spaces() {
        let page = Dict::from_pairs([(
            Name::from("Contents"),
            Object::Array(Array::of([stream(b"q"), stream(b"Q")])),
        )]);
        assert_eq!(assembled(&page), b"q Q ");
    }

    #[test]
    fn a_contents_that_is_not_a_stream_or_array_yields_nothing() {
        for value in [
            Object::Int(7),
            Object::Str(PdfString::literal(b"q")),
            Object::Null,
        ] {
            let page = Dict::from_pairs([(Name::from("Contents"), value)]);
            assert!(assembled(&page).is_empty());
        }
    }

    #[test]
    fn a_non_stream_element_of_the_array_is_skipped() {
        let page = Dict::from_pairs([(
            Name::from("Contents"),
            Object::Array(Array::of([stream(b"q"), Object::Int(1), stream(b"Q")])),
        )]);
        assert_eq!(assembled(&page), b"q Q ");
    }
}
