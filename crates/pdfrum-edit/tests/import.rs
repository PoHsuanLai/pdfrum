//! Importing pages between documents, and the four bugs this crate fixes
//! rather than ports (design brief D13–D16, escalation E10).
//!
//! Each fix has its own test here, because "we deliberately diverge" is only
//! a claim until something pins the divergence. Every other quirk in the
//! import path is ported verbatim and tested alongside them.

// Every helper below is only ever called from a `#[test]`, so an `expect` in
// one is a test failure rather than a library panic. `allow-expect-in-tests`
// recognises test *functions*, not the helpers they share, so the allowance is
// stated once here for the file.
#![allow(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum_edit::{
    EditDoc, IdSource, ImportOptions, NUpOptions, PageRange, SaveMode, SaveOptions, import_pages,
    n_page_to_one, save,
};
use pdfrum_object::{Name, ObjRef, Object, Resolve, names};
use pdfrum_parser::{Document, LoadOptions, load};

fn open(bytes: &[u8]) -> Document {
    load(Arc::from(bytes), &LoadOptions::default()).expect("opens")
}

/// Save an edited document and reopen it, which is the only way to see what
/// the edits actually produced.
fn commit(edit: &EditDoc<'_>) -> Document {
    let options = SaveOptions {
        mode: SaveMode::Full,
        id_source: IdSource::Fixed([0x11; 16]),
        ..SaveOptions::default()
    };
    let mut out = Vec::new();
    save(edit, &options, &mut out).expect("saves");
    open(&out)
}

/// A source document of `count` pages, each with a distinguishing key and a
/// shared font so deduplication has something to deduplicate.
fn source(count: usize) -> Vec<u8> {
    let mut out = String::from("%PDF-1.7\n");
    out.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let kids: Vec<String> = (0..count).map(|i| format!("{} 0 R", i + 4)).collect();
    out.push_str(&format!(
        "2 0 obj\n<< /Type /Pages /Count {count} /Kids [{}] \
         /MediaBox [0 0 400 500] /Resources << /Font << /F1 3 0 R >> >> >>\nendobj\n",
        kids.join(" ")
    ));
    out.push_str("3 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n");
    for i in 0..count {
        out.push_str(&format!(
            "{} 0 obj\n<< /Type /Page /Parent 2 0 R /Marker {i} >>\nendobj\n",
            i + 4
        ));
    }
    out.push_str("trailer\n<< /Root 1 0 R /Size 99 >>\n");
    out.into_bytes()
}

/// A destination with one page of its own.
fn destination() -> Vec<u8> {
    b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Existing true >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 4 >>\n"
        .to_vec()
}

/// The `/Marker` of each page, in order — how a test names which source page
/// landed where.
fn markers(doc: &Document) -> Vec<Option<i64>> {
    (0..doc.page_count())
        .filter_map(|i| doc.page(i).ok())
        .map(|p| p.dict.direct_int(&Name::from("Marker")))
        .collect()
}

// ---------------------------------------------------------------------------
// The basics
// ---------------------------------------------------------------------------

#[test]
fn importing_appends_pages_in_the_order_named() {
    let src = open(&source(3));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);

    import_pages(
        &mut edit,
        &src,
        &PageRange::all(3),
        &ImportOptions {
            at: 1,
            ..ImportOptions::default()
        },
    )
    .expect("imports");

    let result = commit(&edit);
    assert_eq!(result.page_count(), 4);
    assert_eq!(markers(&result), vec![None, Some(0), Some(1), Some(2)]);
}

// XFAImportTest (:714): the definitive insertion-index test — the pages land
// as a contiguous run at the index named, shifting what was there.
#[test]
fn pages_land_at_the_index_named_and_shift_the_rest() {
    let src = open(&source(4));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);

    // "1, 4, 2" — note the spaces the grammar strips.
    let range = PageRange::parse("1, 4, 2", 4).expect("parses");
    import_pages(
        &mut edit,
        &src,
        &range,
        &ImportOptions {
            at: 0,
            ..ImportOptions::default()
        },
    )
    .expect("imports");

    let result = commit(&edit);
    assert_eq!(result.page_count(), 4);
    // Source pages 0, 3, 1 at destination 0, 1, 2; the existing page shifts.
    assert_eq!(markers(&result), vec![Some(0), Some(3), Some(1), None]);
}

#[test]
fn an_index_past_the_end_appends() {
    let src = open(&source(1));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);

    import_pages(
        &mut edit,
        &src,
        &PageRange::all(1),
        &ImportOptions {
            at: 999,
            ..ImportOptions::default()
        },
    )
    .expect("imports");
    assert_eq!(markers(&commit(&edit)), vec![None, Some(0)]);
}

// GoodIndices (:454): a page named twice is imported twice.
#[test]
fn a_page_named_twice_lands_twice() {
    let src = open(&source(2));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);

    import_pages(
        &mut edit,
        &src,
        &PageRange::of([0, 0, 0]),
        &ImportOptions::default(),
    )
    .expect("imports");

    let result = commit(&edit);
    assert_eq!(result.page_count(), 4);
    assert_eq!(markers(&result), vec![Some(0), Some(0), Some(0), None]);
}

// BadIndices (:428): a page the source does not have fails the import.
#[test]
fn an_out_of_range_index_fails() {
    let src = open(&source(1));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);

    assert!(matches!(
        import_pages(
            &mut edit,
            &src,
            &PageRange::of([42]),
            &ImportOptions::default()
        ),
        Err(pdfrum_edit::Error::PageIndexOutOfRange(42))
    ));
}

// ---------------------------------------------------------------------------
// D14 — import is transactional
// ---------------------------------------------------------------------------

// The C++ leaves a stray blank page behind a failed import, and leaves the
// earlier pages of a failed batch in place. Ours stages every mutation in the
// overlay and commits only on success.
#[test]
fn a_failed_import_leaves_the_destination_untouched() {
    let src = open(&source(2));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);

    // The second index is out of range, so the whole import must fail — and
    // the first page must not have been created.
    assert!(
        import_pages(
            &mut edit,
            &src,
            &PageRange::of([0, 99]),
            &ImportOptions::default()
        )
        .is_err()
    );

    let result = commit(&edit);
    assert_eq!(result.page_count(), 1, "no stray page was created");
    assert_eq!(markers(&result), vec![None]);
}

// ---------------------------------------------------------------------------
// D13 — `/Type /Pages` resolves to the destination's real node
// ---------------------------------------------------------------------------

// The C++ hardcodes object 4, which is right only because a freshly created
// document happens to number its pages node 4. A destination numbered any
// other way gets a reference to whatever object 4 happens to be.
#[test]
fn a_source_pages_node_resolves_to_the_destinations_own() {
    // A destination whose pages node is object 7, not 4.
    let dest_bytes = b"%PDF-1.7\n\
7 0 obj\n<< /Type /Pages /Count 1 /Kids [8 0 R] >>\nendobj\n\
8 0 obj\n<< /Type /Page /Parent 7 0 R /MediaBox [0 0 612 792] >>\nendobj\n\
9 0 obj\n<< /Type /Catalog /Pages 7 0 R >>\nendobj\n\
trailer\n<< /Root 9 0 R /Size 10 >>\n"
        .to_vec();
    let dest = open(&dest_bytes);
    let src = open(&source(1));
    let mut edit = EditDoc::new(&dest);

    import_pages(
        &mut edit,
        &src,
        &PageRange::all(1),
        &ImportOptions::default(),
    )
    .expect("imports");

    let result = commit(&edit);
    assert_eq!(result.page_count(), 2);

    // Every imported page's `/Parent` names the destination's own node, and
    // that node really is a `/Pages` — not whatever object 4 held.
    for i in 0..result.page_count() {
        let page = result.page(i).expect("a page");
        let parent = page
            .dict
            .reference(names::PARENT)
            .expect("a parent reference");
        let node = result.fetch(parent).expect("resolves");
        assert_eq!(
            node.as_dict().and_then(|d| d.name(names::TYPE)),
            Some(names::PAGES),
            "page {i}'s parent is not a pages node"
        );
    }
}

// ---------------------------------------------------------------------------
// D15 — importing never mutates the source
// ---------------------------------------------------------------------------

// PDFium's page constructor writes `/Type /Page` into a source dictionary
// that lacked one, so its import path is not read-only with respect to what
// it copies from. Ours cannot be: the source is shared immutable bytes.
#[test]
fn importing_never_mutates_the_source() {
    // A source page with no `/Type` at all.
    let src_bytes = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Parent 2 0 R /MediaBox [0 0 100 100] /Marker 7 >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 4 >>\n"
        .to_vec();
    let src = open(&src_bytes);
    let before = src.page(0).expect("a page").dict.clone();
    assert!(
        !before.contains_key(names::TYPE),
        "the fixture has no /Type"
    );

    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);
    import_pages(
        &mut edit,
        &src,
        &PageRange::all(1),
        &ImportOptions::default(),
    )
    .expect("imports");

    // The source is unchanged...
    assert_eq!(src.page(0).expect("a page").dict, before);
    // ...and the *copy* got the `/Type` it needed.
    let result = commit(&edit);
    let imported = result.page(0).expect("the imported page");
    assert_eq!(imported.dict.name(names::TYPE), Some(names::PAGE));
    assert_eq!(imported.dict.direct_int(&Name::from("Marker")), Some(7));
}

// ---------------------------------------------------------------------------
// Inheritance flattening
// ---------------------------------------------------------------------------

// An imported page is detached from its `/Parent` chain, so what it was
// inheriting has to be written onto the copy.
#[test]
fn inherited_attributes_are_flattened_onto_the_copy() {
    let src = open(&source(1));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);
    import_pages(
        &mut edit,
        &src,
        &PageRange::all(1),
        &ImportOptions::default(),
    )
    .expect("imports");

    let result = commit(&edit);
    let page = result.page(0).expect("the imported page");
    // The source page stated neither, and inherited both from its `/Pages`.
    let media = page.dict.array(names::MEDIA_BOX, &result).expect("a box");
    // The box came through as the source's numbers, so an exact comparison
    // is what the assertion means.
    #[expect(clippy::float_cmp, reason = "the box is copied, not computed")]
    {
        assert_eq!(media.as_rect().width(), 400.0);
    }
    assert!(
        page.dict.raw(names::RESOURCES).is_some(),
        "the inherited resources came with it"
    );
}

// A page inheriting nothing gets US Letter and an empty resource dictionary,
// which is what makes an unopenable source page still import.
#[test]
fn a_page_inheriting_nothing_gets_letter_and_empty_resources() {
    let src_bytes = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 4 >>\n"
        .to_vec();
    let src = open(&src_bytes);
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);
    import_pages(
        &mut edit,
        &src,
        &PageRange::all(1),
        &ImportOptions::default(),
    )
    .expect("imports");

    let result = commit(&edit);
    let page = result.page(0).expect("the imported page");
    let media = page.dict.array(names::MEDIA_BOX, &result).expect("a box");
    assert_eq!(
        (media.as_rect().width(), media.as_rect().height()),
        (612.0, 792.0),
        "US Letter is the last fallback"
    );
    assert!(page.dict.raw(names::RESOURCES).is_some());
}

// ---------------------------------------------------------------------------
// Deduplication
// ---------------------------------------------------------------------------

// The object map is not cleared between pages, so a font two imported pages
// share is copied once and both point at it.
#[test]
fn two_imported_pages_sharing_a_font_get_one_copy() {
    let src = open(&source(2));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);
    import_pages(
        &mut edit,
        &src,
        &PageRange::all(2),
        &ImportOptions::default(),
    )
    .expect("imports");

    let result = commit(&edit);
    let font_of = |i: u32| -> Option<ObjRef> {
        result
            .page(i)
            .ok()?
            .dict
            .dict(names::RESOURCES, &result)?
            .dict(&Name::from("Font"), &result)?
            .reference(&Name::from("F1"))
    };
    let first = font_of(0).expect("page 0's font");
    assert_eq!(first, font_of(1).expect("page 1's font"), "one copy");
}

// ---------------------------------------------------------------------------
// Damage tolerance
// ---------------------------------------------------------------------------

// A `/Parent` chain that cycles makes the attribute simply not inheritable,
// rather than hanging.
#[test]
fn a_self_referential_page_parent_imports() {
    let src_bytes = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] /Parent 2 0 R >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 3 0 R /Marker 1 >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 4 >>\n"
        .to_vec();
    let src = open(&src_bytes);
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);
    import_pages(
        &mut edit,
        &src,
        &PageRange::all(1),
        &ImportOptions::default(),
    )
    .expect("imports");
    assert_eq!(commit(&edit).page_count(), 2);
}

// InitDestDoc's repairs: a destination whose `/Kids` is not an array has both
// `/Kids` and `/Count` rewritten, because the two are written together.
#[test]
fn a_destination_with_a_broken_kids_is_repaired() {
    let dest_bytes = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 5 /Kids 99 >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 3 >>\n"
        .to_vec();
    let Ok(dest) = load(Arc::from(&dest_bytes[..]), &LoadOptions::default()) else {
        // A destination with no usable page tree may refuse to open, which
        // is the reader's call and not this crate's.
        return;
    };
    let src = open(&source(1));
    let mut edit = EditDoc::new(&dest);
    import_pages(
        &mut edit,
        &src,
        &PageRange::all(1),
        &ImportOptions::default(),
    )
    .expect("imports");

    let result = commit(&edit);
    // The lying `/Count 5` did not survive.
    assert_eq!(result.page_count(), 1);
}

// A destination with a wrong-but-present catalog `/Type` is left alone: a
// producer that wrote something else may have meant it.
#[test]
fn a_wrong_catalog_type_is_not_repaired() {
    let dest_bytes = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Whatever /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 4 >>\n"
        .to_vec();
    let dest = open(&dest_bytes);
    let src = open(&source(1));
    let mut edit = EditDoc::new(&dest);
    import_pages(
        &mut edit,
        &src,
        &PageRange::all(1),
        &ImportOptions::default(),
    )
    .expect("imports");

    let result = commit(&edit);
    let catalog = result.catalog().expect("a catalog");
    assert_eq!(
        catalog.name(names::TYPE),
        Some(&Name::from("Whatever")),
        "a wrong type is left alone"
    );
    assert_eq!(result.page_count(), 2);
}

// ---------------------------------------------------------------------------
// Viewer preferences
// ---------------------------------------------------------------------------

#[test]
fn viewer_preferences_are_copied_when_asked_for() {
    let src_bytes = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R /ViewerPreferences \
<< /HideToolbar true /Junk 9 0 R >> >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 4 >>\n"
        .to_vec();
    let src = open(&src_bytes);
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);

    import_pages(
        &mut edit,
        &src,
        &PageRange::all(1),
        &ImportOptions {
            at: 0,
            viewer_preferences: true,
        },
    )
    .expect("imports");

    let result = commit(&edit);
    let prefs = result
        .catalog()
        .expect("a catalog")
        .dict(names::VIEWER_PREFERENCES, &result)
        .expect("preferences were copied");
    assert_eq!(prefs.bool(&Name::from("HideToolbar")), Some(true));
    assert!(
        !prefs.contains_key(&Name::from("Junk")),
        "a reference is filtered out — that is the circular-graph defence"
    );
}

#[test]
fn viewer_preferences_are_left_alone_when_not_asked_for() {
    let src = open(&source(1));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);
    import_pages(
        &mut edit,
        &src,
        &PageRange::all(1),
        &ImportOptions::default(),
    )
    .expect("imports");

    let result = commit(&edit);
    assert!(
        result
            .catalog()
            .expect("a catalog")
            .raw(names::VIEWER_PREFERENCES)
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// N-up
// ---------------------------------------------------------------------------

// ImportNPages (:108): the sheet counts, over a real document.
#[test]
fn n_up_produces_the_expected_sheet_counts() {
    for (x, y, pages, sheets) in [(2u32, 1u32, 5usize, 3u32), (5, 1, 5, 1), (3, 1, 5, 2)] {
        let src = open(&source(pages));
        let dest_bytes = destination();
        let dest = open(&dest_bytes);
        let mut edit = EditDoc::new(&dest);

        n_page_to_one(
            &mut edit,
            &src,
            &PageRange::all(u32::try_from(pages).expect("fits")),
            &NUpOptions {
                sheet: (612.0, 792.0),
                grid: (x, y),
            },
        )
        .expect("imposes");

        let result = commit(&edit);
        // The destination's own page is still there, plus the sheets.
        assert_eq!(
            result.page_count(),
            sheets + 1,
            "{x}x{y} over {pages} pages"
        );
    }
}

// BadNupParams (:129): a zero in the grid or the sheet is refused.
#[test]
fn n_up_refuses_a_zero_dimension() {
    let src = open(&source(2));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);

    for options in [
        NUpOptions {
            sheet: (612.0, 792.0),
            grid: (0, 1),
        },
        NUpOptions {
            sheet: (612.0, 792.0),
            grid: (1, 0),
        },
        NUpOptions {
            sheet: (0.0, 792.0),
            grid: (1, 1),
        },
        NUpOptions {
            sheet: (612.0, 0.0),
            grid: (1, 1),
        },
    ] {
        let mut edit = EditDoc::new(&dest);
        assert!(
            matches!(
                n_page_to_one(&mut edit, &src, &PageRange::all(2), &options),
                Err(pdfrum_edit::Error::BadNupParams)
            ),
            "{options:?} must be refused"
        );
    }
}

#[test]
fn every_n_up_sheet_has_the_size_asked_for() {
    let src = open(&source(4));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);

    n_page_to_one(
        &mut edit,
        &src,
        &PageRange::all(4),
        &NUpOptions {
            sheet: (792.0, 612.0),
            grid: (2, 1),
        },
    )
    .expect("imposes");

    let result = commit(&edit);
    // Skip the destination's own page, which keeps its own size.
    for i in 0..result.page_count() {
        let page = result.page(i).expect("a page");
        if page.dict.bool(&Name::from("Existing")) == Some(true) {
            continue;
        }
        let media = page.dict.array(names::MEDIA_BOX, &result).expect("a box");
        assert_eq!(
            (media.as_rect().width(), media.as_rect().height()),
            (792.0, 612.0),
            "sheet {i}"
        );
    }
}

// D16: a source page reused on a *later* sheet must be registered in that
// sheet's own `/Resources /XObject`. The C++ reuses the name from a map it
// never clears while clearing the per-sheet registry, so the sub-page renders
// blank on every sheet after the first.
#[test]
fn a_page_reused_on_a_later_sheet_is_registered_on_that_sheet() {
    let src = open(&source(1));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);

    // The same source page four times, one per sheet.
    n_page_to_one(
        &mut edit,
        &src,
        &PageRange::of([0, 0, 0, 0]),
        &NUpOptions {
            sheet: (612.0, 792.0),
            grid: (1, 1),
        },
    )
    .expect("imposes");

    let result = commit(&edit);
    let mut sheets = 0usize;
    for i in 0..result.page_count() {
        let page = result.page(i).expect("a page");
        if page.dict.bool(&Name::from("Existing")) == Some(true) {
            continue;
        }
        sheets += 1;

        let xobjects = page
            .dict
            .dict(names::RESOURCES, &result)
            .and_then(|r| r.dict(&Name::from("XObject"), &result))
            .unwrap_or_else(|| panic!("sheet {i} has no /XObject dictionary"));
        assert!(
            !xobjects.is_empty(),
            "sheet {i}'s /XObject dictionary is empty — the form is unreachable"
        );

        // Every name the content stream invokes must be in that dictionary.
        let contents = page
            .dict
            .stream(names::CONTENTS, &result)
            .expect("a content stream");
        let text = String::from_utf8_lossy(&pdfrum_parser::decoded_stream(
            &contents,
            &result,
            &pdfrum_common::Limits::default(),
            &mut pdfrum_common::Diagnostics::default(),
        ))
        .into_owned();
        for token in text.split_whitespace().collect::<Vec<_>>().windows(2) {
            let (Some(name), Some(op)) = (token.first(), token.get(1)) else {
                continue;
            };
            if *op == "Do"
                && let Some(bare) = name.strip_prefix('/')
            {
                assert!(
                    xobjects.contains_key(&Name::from(bare)),
                    "sheet {i} invokes /{bare} but does not name it"
                );
            }
        }
    }
    assert_eq!(sheets, 4, "one sheet per invocation");
}

// The same page used twice makes one form and two invocations, not two forms.
#[test]
fn a_page_used_twice_makes_one_form() {
    let src = open(&source(1));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);

    n_page_to_one(
        &mut edit,
        &src,
        &PageRange::of([0, 0]),
        &NUpOptions {
            sheet: (612.0, 792.0),
            grid: (2, 1),
        },
    )
    .expect("imposes");

    let result = commit(&edit);
    let sheet = (0..result.page_count())
        .filter_map(|i| result.page(i).ok())
        .find(|p| p.dict.bool(&Name::from("Existing")) != Some(true))
        .expect("a sheet");

    let xobjects = sheet
        .dict
        .dict(names::RESOURCES, &result)
        .and_then(|r| r.dict(&Name::from("XObject"), &result))
        .expect("an /XObject dictionary");
    let targets: Vec<ObjRef> = xobjects.iter().filter_map(|(_, v)| v.as_ref_id()).collect();
    assert_eq!(targets.len(), 2, "two slots");
    assert_eq!(
        targets.first(),
        targets.get(1),
        "both slots name the same form"
    );
}

#[test]
fn an_n_up_form_is_a_form_xobject_carrying_only_resources() {
    let src = open(&source(1));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);

    n_page_to_one(
        &mut edit,
        &src,
        &PageRange::all(1),
        &NUpOptions {
            sheet: (612.0, 792.0),
            grid: (1, 1),
        },
    )
    .expect("imposes");

    let result = commit(&edit);
    let sheet = (0..result.page_count())
        .filter_map(|i| result.page(i).ok())
        .find(|p| p.dict.bool(&Name::from("Existing")) != Some(true))
        .expect("a sheet");
    let form_ref = sheet
        .dict
        .dict(names::RESOURCES, &result)
        .and_then(|r| r.dict(&Name::from("XObject"), &result))
        .and_then(|x| x.iter().next().and_then(|(_, v)| v.as_ref_id()))
        .expect("a form reference");
    let form = result.fetch(form_ref).expect("resolves");
    let form = form.as_stream().expect("a stream");

    assert_eq!(
        form.dict.name(names::SUBTYPE),
        Some(&Name::from("Form")),
        "the object must be a form XObject"
    );
    assert_eq!(form.dict.direct_int(&Name::from("FormType")), Some(1));
    assert!(form.dict.raw(&Name::from("BBox")).is_some());
    assert!(form.dict.raw(&Name::from("Matrix")).is_some());
    assert!(form.dict.raw(names::RESOURCES).is_some());
    // Everything else about the source page is dropped.
    assert!(!form.dict.contains_key(names::ANNOTS));
    assert!(!form.dict.contains_key(names::CROP_BOX));
}

// ---------------------------------------------------------------------------
// The destination stays a valid document
// ---------------------------------------------------------------------------

#[test]
fn an_imported_document_is_one_another_reader_opens() {
    let src = open(&source(3));
    let dest_bytes = destination();
    let dest = open(&dest_bytes);
    let mut edit = EditDoc::new(&dest);
    import_pages(
        &mut edit,
        &src,
        &PageRange::all(3),
        &ImportOptions::default(),
    )
    .expect("imports");

    let options = SaveOptions {
        mode: SaveMode::Full,
        id_source: IdSource::Fixed([0x11; 16]),
        ..SaveOptions::default()
    };
    let mut out = Vec::new();
    save(&edit, &options, &mut out).expect("saves");

    // It opens, and every reference in it resolves.
    let result = open(&out);
    assert_eq!(result.page_count(), 4);
    for number in 1..=result.xref().last_object_number() {
        let Ok(object) = result.fetch(ObjRef::new(number, 0)) else {
            continue;
        };
        for target in references(&object) {
            assert!(
                result.fetch(target).is_ok(),
                "object {number} names {} unresolvably",
                target.num
            );
        }
    }
}

fn references(object: &Object) -> Vec<ObjRef> {
    let mut out = Vec::new();
    walk(object, &mut out, 0);
    out
}

fn walk(object: &Object, out: &mut Vec<ObjRef>, depth: u32) {
    if depth > 64 {
        return;
    }
    match object {
        Object::Ref(r) => out.push(*r),
        Object::Array(a) => a.iter().for_each(|v| walk(v, out, depth + 1)),
        Object::Dict(d) => d.iter().for_each(|(_, v)| walk(v, out, depth + 1)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| walk(v, out, depth + 1)),
        _ => {}
    }
}

// A destination that cannot be repaired is the one hard failure.
#[test]
fn a_destination_with_no_catalog_is_the_one_hard_failure() {
    // No `/Root` at all, and nothing the rebuild can find.
    let Ok(dest) = load(
        Arc::from(&b"%PDF-1.7\n1 0 obj\n<< /Type /Page >>\nendobj\ntrailer\n<< >>\n"[..]),
        &LoadOptions::default(),
    ) else {
        // Refusing to open is the reader's answer and is also correct.
        return;
    };
    let src = open(&source(1));
    let mut edit = EditDoc::new(&dest);
    assert!(matches!(
        import_pages(
            &mut edit,
            &src,
            &PageRange::all(1),
            &ImportOptions::default()
        ),
        Err(pdfrum_edit::Error::NoDestinationCatalog)
    ));
}
