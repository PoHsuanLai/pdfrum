//! Whole-file text extraction: bytes → document → page graph → `TextPage`.
//!
//! Property: never panics. Search, links, and selection run on whatever came
//! out.

#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use pdfrum_object::{Name, Object};
use pdfrum_page::{BuildContext, Resources, build_page_from_dict, parse_content};
use pdfrum_text::{ExtractOptions, FindOptions};

fuzz_target!(|data: &[u8]| {
    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();

    let Ok(doc) = pdfrum_parser::load(Arc::from(data), &pdfrum_parser::LoadOptions::default()) else {
        return;
    };
    // The first few pages only: a document declaring thousands of them would
    // spend the whole run on one input.
    let mut ctx = BuildContext::default();
    for index in 0..doc.page_count().min(4) {
        let Ok(page) = doc.page(index) else { continue };

        let mut content = Vec::new();
        if let Some(contents) = page.dict.get(&Name::from("Contents"), &doc) {
            let mut push = |object: &Object| {
                if let Some(stream) = object.as_stream() {
                    content.extend_from_slice(
                        &pdfrum_filters::decode_chain(stream, 0, &doc, &limits, &mut diags).data,
                    );
                    content.push(b' ');
                }
            };
            match contents.as_direct() {
                Some(direct @ Object::Stream(_)) => push(direct),
                Some(Object::Array(array)) => {
                    for element in array.iter() {
                        if let Ok(resolved) = element.resolve(&doc) {
                            push(resolved.get());
                        }
                    }
                }
                _ => {}
            }
        }
        let ops = parse_content(&content, &limits, &mut diags);
        let resources = Resources::for_page(
            page.inherited(&Name::from("Resources"), &doc)
                .and_then(|object| object.resolve(&doc).ok()?.as_dict().cloned()),
        );
        let built = build_page_from_dict(
            &ops,
            &page.dict,
            |key| page.inherited(key, &doc),
            &resources,
            &doc,
            &mut ctx,
            &limits,
            &mut diags,
        );

        // Both reading directions, because the right-to-left one reverses
        // segments and mirrors characters on a path the other never takes.
        for rtl in [false, true] {
            let text = pdfrum_text::extract(
                &built,
                &doc,
                &ExtractOptions { rtl },
                &limits,
                &mut diags,
            );
            // The query half, whose index arithmetic spans two index spaces
            // that are allowed to disagree.
            let _ = text.web_links();
            let _ = text.rects(..);
            let _ = text.slice(..);
            let _ = text
                .find("e")
                .take(64)
                .count();
        }
    }
});
