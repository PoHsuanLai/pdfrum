//! Integration tests over the facade, on real PDFs.
//!
//! These drive the crate the way a caller does — through the public API only,
//! over the fixtures in `tests/fixtures/` (see `PROVENANCE.md`: they are the
//! oracle's own test files, unmodified). Every facade path is exercised at
//! least once here; the doctests carry the *documented* behaviour and these
//! carry the corners the docs would not be improved by spelling out.

use std::sync::Arc;

use pdfrum::{
    Backend, Document, FieldKind, FindOptions, OpenOptions, RenderOptions, SaveOptions, Subtype,
    Update,
};

const HELLO: &str = "tests/fixtures/hello_world.pdf";
const FORM: &str = "tests/fixtures/text_form.pdf";
const BOOKMARKS: &str = "tests/fixtures/bookmarks.pdf";
const JPX_TWO_SIZES: &str = "tests/fixtures/jpx_two_sizes.pdf";

/// Whether a page dimension is exactly this many points.
///
/// A page size in these fixtures is a whole number of points written straight
/// into the file, so the comparison really is exact; going through a
/// tolerance says what is meant more clearly than comparing two `f64`s and
/// silencing the lint.
fn is_points(measured: f64, expected: i32) -> bool {
    (measured - f64::from(expected)).abs() < 1e-9
}

/// A clean directory for one test's output files.
///
/// Errors are swallowed rather than asserted: the test that follows writes
/// into the directory and will fail loudly if it is not there, and a helper
/// that is not itself a `#[test]` may not panic under this workspace's lints.
fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfrum-facade-{name}"));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).ok();
    dir
}

// ---------------------------------------------------------------- opening

#[test]
fn a_document_opens_from_a_path_and_from_bytes_identically() {
    let from_path = Document::open(HELLO).expect("open");
    let bytes = std::fs::read(HELLO).expect("read");
    let from_bytes = Document::from_bytes(Arc::from(bytes)).expect("open");
    assert_eq!(from_path.page_count(), from_bytes.page_count());
    assert_eq!(from_path.version(), from_bytes.version());
}

#[test]
fn opening_something_that_is_not_a_pdf_fails_rather_than_panicking() {
    let err =
        Document::from_bytes(Arc::from(b"this is not a PDF".as_slice())).expect_err("not a PDF");
    assert!(
        matches!(err, pdfrum::Error::Open(pdfrum_parser::LoadError::NotPdf)),
        "got {err:?}"
    );
}

#[test]
fn opening_a_missing_file_is_an_io_error_not_a_parse_error() {
    let err = Document::open("tests/fixtures/does-not-exist.pdf").expect_err("missing");
    assert!(matches!(err, pdfrum::Error::Io(_)), "got {err:?}");
}

#[test]
fn an_unencrypted_document_ignores_a_password_rather_than_refusing_it() {
    // A password for a file that wants none is not an error: the handler is
    // never consulted.
    let doc = Document::open_with_password(HELLO, b"irrelevant").expect("open");
    assert_eq!(doc.page_count(), 1);
    assert!(!doc.is_encrypted());
}

#[test]
fn open_options_carry_the_password_and_the_limits() {
    let doc = Document::open_with(
        HELLO,
        &OpenOptions {
            password: None,
            ..OpenOptions::default()
        },
    )
    .expect("open");
    assert_eq!(doc.page_count(), 1);
    // Everything is permitted on a file with no security handler.
    assert_eq!(doc.permissions(false), doc.permissions(true));
}

#[test]
fn a_document_reports_its_version_and_its_bytes() {
    let doc = Document::open(HELLO).expect("open");
    assert_eq!(doc.version(), 17, "%PDF-1.7");
    assert!(doc.bytes().starts_with(b"%PDF-"));
    // This file's cross-reference table is intact, so nothing had to be
    // rebuilt to open it.
    assert!(!doc.xref_was_rebuilt());
}

#[test]
fn a_stream_with_no_length_is_recovered_rather_than_refused() {
    // `hello_world.pdf`'s content stream deliberately carries no `/Length`.
    // The recovery is what makes the page have any text at all, so the proof
    // that it happened is the text — the diagnostics sink on `Document`
    // carries what the *load* repaired, and a stream is read lazily, long
    // after the load returned.
    let doc = Document::open(HELLO).expect("open");
    assert!(
        doc.page(0)
            .expect("page")
            .text()
            .all_text()
            .contains("Hello"),
        "the stream was read despite its missing length"
    );
}

// ------------------------------------------------------------------ pages

#[test]
fn pages_are_reachable_by_index_and_by_iteration_and_agree() {
    let doc = Document::open(BOOKMARKS).expect("open");
    assert_eq!(doc.page_count(), 2);

    let by_iter: Vec<u32> = doc.pages().map(|page| page.index()).collect();
    assert_eq!(by_iter, [0, 1]);

    for index in 0..doc.page_count() {
        let page = doc.page(index).expect("page");
        assert_eq!(page.index(), index);
    }
    assert!(doc.page(2).is_err(), "past the end");
}

#[test]
fn a_pages_boxes_and_rotation_are_read_through_inheritance() {
    // `hello_world.pdf` states its `/MediaBox` on the *pages node*, not on
    // the page, so reading 200x200 here is the inheritance walk working.
    let doc = Document::open(HELLO).expect("open");
    let page = doc.page(0).expect("page");
    assert!(is_points(page.media_box().width(), 200));
    assert!(is_points(page.media_box().height(), 200));
    // No `/CropBox`, so it defaults to the media box.
    assert_eq!(page.crop_box(), page.media_box());
    assert_eq!(page.rotation(), pdfrum::Rotation::None);
    assert_eq!(page.rotation().degrees(), 0);
    assert!(is_points(page.width(), 200) && is_points(page.height(), 200));
}

#[test]
fn a_page_exposes_its_interpreted_objects() {
    let doc = Document::open(HELLO).expect("open");
    let objects = doc.page(0).expect("page").objects();
    // Two `Tj` strings become two text objects.
    assert_eq!(objects.objects.len(), 2);
    assert!(
        objects
            .objects
            .iter()
            .all(|o| matches!(o, pdfrum_page::PageObject::Text(_)))
    );
}

// --------------------------------------------------------------- rendering

#[test]
fn the_render_scale_decides_the_output_size() {
    let doc = Document::open(HELLO).expect("open");
    let page = doc.page(0).expect("page");
    for (scale, expected) in [(1.0, 200), (0.5, 100), (3.0, 600)] {
        let pixmap = page.render(&RenderOptions::scaled(scale)).expect("render");
        assert_eq!((pixmap.width(), pixmap.height()), (expected, expected));
    }
}

#[test]
fn fit_scales_a_page_into_a_box_without_distorting_it() {
    let doc = Document::open(BOOKMARKS).expect("open");
    let page = doc.page(0).expect("page");
    // 612x792 into 100x100 must fit the *tall* axis.
    let options = RenderOptions::fit(page.width(), page.height(), 100, 100);
    let pixmap = page.render(&options).expect("render");
    assert!(pixmap.width() <= 100 && pixmap.height() <= 100);
    assert_eq!(pixmap.height(), 100, "the tall axis is the binding one");
}

#[test]
fn a_zero_sized_render_is_an_error_rather_than_an_empty_image() {
    let doc = Document::open(HELLO).expect("open");
    let err = doc
        .page(0)
        .expect("page")
        .render(&RenderOptions::scaled(0.0))
        .expect_err("a zero-scale target has no pixels");
    assert!(matches!(err, pdfrum::Error::Render(_)), "got {err:?}");
}

#[test]
fn every_backend_renders_the_same_page_at_the_same_size() {
    let doc = Document::open(HELLO).expect("open");
    let page = doc.page(0).expect("page");
    let render = |backend| {
        page.render(&RenderOptions {
            backend,
            ..RenderOptions::default()
        })
        .expect("render")
    };
    let vello = render(Backend::VelloCpu);
    let tiny = render(Backend::TinySkia);
    let exact = render(Backend::Agg);
    // The size is an *engine* decision, so it cannot depend on the backend.
    for pixmap in [&tiny, &exact] {
        assert_eq!(
            (vello.width(), vello.height()),
            (pixmap.width(), pixmap.height())
        );
    }
    // And each must actually draw the text — an all-white page would pass a
    // size check while rendering nothing.
    for pixmap in [&vello, &tiny, &exact] {
        assert!(
            pixmap.data().chunks(4).any(|px| px[0] < 128),
            "the page has black glyphs on it"
        );
    }
}

#[test]
fn rendering_is_deterministic() {
    let doc = Document::open(HELLO).expect("open");
    let page = doc.page(0).expect("page");
    let once = page.render(&RenderOptions::default()).expect("render");
    let twice = page.render(&RenderOptions::default()).expect("render");
    assert_eq!(once, twice);
}

#[test]
fn a_forced_background_replaces_the_pages_own() {
    let doc = Document::open(HELLO).expect("open");
    let pixmap = doc
        .page(0)
        .expect("page")
        .render(&RenderOptions {
            background: Some(peniko::Color::from_rgba8(0, 0, 255, 255)),
            ..RenderOptions::default()
        })
        .expect("render");
    // A corner the glyphs do not reach is the background we asked for.
    let corner = pixmap.pixel(0, 0).expect("in bounds");
    assert_eq!(corner[2], 255, "blue channel");
    assert_eq!(corner[0], 0, "red channel");
}

#[test]
fn a_render_can_leave_the_annotations_out() {
    let doc = Document::open(FORM).expect("open");
    let page = doc.page(0).expect("page");
    let with = page.render(&RenderOptions::default()).expect("render");
    let without = page
        .render(&RenderOptions {
            annotations: false,
            ..RenderOptions::default()
        })
        .expect("render");
    // Same size either way; the flag changes what is drawn, not the target.
    assert_eq!(
        (with.width(), with.height()),
        (without.width(), without.height())
    );
}

#[test]
fn a_shared_build_context_renders_the_same_pixels_as_a_fresh_one() {
    // The cache must be an optimization and nothing more.
    let doc = Document::open(BOOKMARKS).expect("open");
    let options = RenderOptions::default();

    let fresh: Vec<_> = doc
        .pages()
        .map(|page| page.render(&options).expect("render"))
        .collect();

    let mut ctx = pdfrum::BuildContext::new();
    let shared: Vec<_> = doc
        .pages()
        .map(|page| page.render_with(&options, &mut ctx).expect("render"))
        .collect();

    assert_eq!(fresh, shared);
}

// ------------------------------------------------------------------- text

#[test]
fn text_extraction_reads_the_pages_strings_in_order() {
    let doc = Document::open(HELLO).expect("open");
    let text = doc.page(0).expect("page").text();
    let all = text.all_text();
    assert!(all.contains("Hello, world!"));
    assert!(all.contains("Goodbye, world!"));
    assert_eq!(text.char_count(), 30, "13 + 15 characters plus separators");
}

#[test]
fn search_is_case_insensitive_by_default_and_case_sensitive_on_request() {
    let doc = Document::open(HELLO).expect("open");
    let text = doc.page(0).expect("page").text();

    let insensitive: Vec<_> = text.find("WORLD", FindOptions::default()).collect();
    assert_eq!(insensitive.len(), 2, "both greetings");

    let sensitive: Vec<_> = text
        .find(
            "WORLD",
            FindOptions {
                match_case: true,
                ..FindOptions::default()
            },
        )
        .collect();
    assert!(sensitive.is_empty(), "the file spells it lower-case");
}

#[test]
fn a_search_hit_maps_back_to_boxes_on_the_page() {
    let doc = Document::open(HELLO).expect("open");
    let text = doc.page(0).expect("page").text();
    let hit = text
        .find("Hello", FindOptions::default())
        .next()
        .expect("a hit");
    let rects = text.rects(hit.start, Some(hit.len()));
    assert_eq!(rects.len(), 1, "one text object, one box");
    assert!(rects[0].width() > 0.0 && rects[0].height() > 0.0);
}

#[test]
fn text_can_be_selected_by_rectangle_and_probed_by_point() {
    let doc = Document::open(HELLO).expect("open");
    let text = doc.page(0).expect("page").text();
    let first = text.char_at(0).expect("a first character");
    // The point at a character's own origin finds that character.
    let found = text.index_at(first.origin, kurbo::Size::new(2.0, 2.0));
    assert_eq!(found, Some(0));
    // A rectangle covering the whole page selects everything drawn on it.
    let all = text.text_in_rect(kurbo::Rect::new(0.0, 0.0, 200.0, 200.0));
    assert!(all.contains("Hello"));
}

#[test]
fn a_page_with_no_text_extracts_nothing_rather_than_failing() {
    let doc = Document::open(FORM).expect("open");
    // This page *does* have text, so assert the shape rather than emptiness:
    // extraction is infallible, and an empty page is a valid answer.
    let text = doc.page(0).expect("page").text();
    assert!(text.all_text().contains("Test Form"));
}

// ------------------------------------------------------- document features

#[test]
fn metadata_is_absent_rather_than_empty_when_the_file_has_no_info_dict() {
    let doc = Document::open(HELLO).expect("open");
    let meta = doc.metadata();
    assert_eq!(meta, pdfrum::Metadata::default());
    assert!(meta.title.is_none());
    assert!(doc.xmp_metadata().is_none());
}

#[test]
fn the_outline_is_a_preorder_walk_carrying_depth() {
    let doc = Document::open(BOOKMARKS).expect("open");
    let outline = doc.outline();
    assert!(!outline.is_empty());
    assert_eq!(outline.len(), 6);

    let shape: Vec<(usize, String)> = outline.iter().map(|b| (b.depth(), b.title())).collect();
    assert_eq!(shape[0], (0, "A Good Beginning".to_owned()));
    assert_eq!(shape[1], (0, "Open Middle".to_owned()));
    assert_eq!(shape[2], (1, "Open Middle Descendant".to_owned()));

    // Depth never jumps by more than one going down, which is what makes the
    // flat list rebuildable into a tree.
    for pair in shape.windows(2) {
        assert!(pair[1].0 <= pair[0].0 + 1, "{pair:?}");
    }
}

#[test]
fn an_outline_entry_resolves_the_page_its_destination_names() {
    let doc = Document::open(BOOKMARKS).expect("open");
    let entry = doc
        .outline()
        .iter()
        .find(|b| b.title() == "Open Middle Descendant")
        .expect("the entry with an explicit destination");
    assert_eq!(entry.page_index(), Some(0));
}

#[test]
fn an_outline_entry_exposes_its_action_and_its_open_state() {
    let doc = Document::open(BOOKMARKS).expect("open");
    let entry = doc
        .outline()
        .iter()
        .find(|b| b.title() == "Open Middle")
        .expect("the entry with a URI action");
    assert!(entry.is_open(), "/Count is positive");
    let action = entry.action().expect("a /A action");
    assert_eq!(action.kind(), pdfrum::ActionKind::Uri);
}

#[test]
fn a_document_without_an_outline_reports_an_empty_one() {
    let doc = Document::open(HELLO).expect("open");
    assert!(doc.outline().is_empty());
    assert_eq!(doc.outline().len(), 0);
}

#[test]
fn a_document_without_page_labels_numbers_its_pages_plainly() {
    let doc = Document::open(BOOKMARKS).expect("open");
    assert_eq!(doc.page_label(0), None);
    assert_eq!(doc.page_label(99), None, "out of range");
}

#[test]
fn a_document_without_attachments_reports_none() {
    let doc = Document::open(HELLO).expect("open");
    assert!(doc.attachments().is_empty());
}

// ------------------------------------------------------------- annotations

#[test]
fn a_pages_annotations_are_read_with_their_geometry_and_flags() {
    let doc = Document::open(FORM).expect("open");
    let annots = doc.page(0).expect("page").annotations();
    assert_eq!(annots.len(), 1);

    let widget = &annots[0];
    assert_eq!(widget.subtype(), Subtype::Widget);
    assert_eq!(widget.rect(), kurbo::Rect::new(100.0, 100.0, 200.0, 130.0));
    assert!(!widget.is_hidden());
    assert!(widget.quad_points().is_empty(), "a widget has no quads");
    // The escape hatch reaches the keys the facade does not surface.
    assert!(widget.dict().contains_key(&pdfrum_object::Name::from("FT")));
}

#[test]
fn a_page_without_annotations_reports_none() {
    let doc = Document::open(HELLO).expect("open");
    assert!(doc.page(0).expect("page").annotations().is_empty());
    assert!(doc.page(0).expect("page").links().is_empty());
}

// -------------------------------------------------------------------- form

#[test]
fn a_form_enumerates_its_fields_with_their_kinds_and_names() {
    let doc = Document::open(FORM).expect("open");
    let form = doc.form().expect("an AcroForm");
    assert_eq!(form.field_count(), 1);
    assert!(!form.need_appearances());

    let field = &form.fields()[0];
    assert_eq!(field.name(), "Text Box");
    assert_eq!(field.kind(), FieldKind::Text);
    assert_eq!(field.index(), 0);
    assert_eq!(field.value(), "");
    assert!(!field.is_read_only());
    assert!(!field.is_required());
    assert_eq!(field.widget_count(), 1, "field and widget are merged");
    assert!(field.options().is_empty(), "not a choice field");
}

#[test]
fn a_document_without_a_form_reports_none() {
    let doc = Document::open(HELLO).expect("open");
    assert!(doc.form().is_none());
}

#[test]
fn a_written_value_reads_back_before_it_is_saved() {
    let doc = Document::open(FORM).expect("open");
    let mut form = doc.form().expect("form");
    form.set("Text Box", "typed");

    let field = form.field("Text Box").expect("field");
    assert_eq!(field.value(), "typed", "the edit wins");
    assert_eq!(field.stored_value(), "", "the file is untouched");
    assert_eq!(form.edits().collect::<Vec<_>>(), [("Text Box", "typed")]);
}

#[test]
fn writing_a_field_twice_keeps_the_last_value() {
    let doc = Document::open(FORM).expect("open");
    let mut form = doc.form().expect("form");
    form.set("Text Box", "first");
    form.set("Text Box", "second");
    assert_eq!(form.edits().count(), 1);
    assert_eq!(form.field("Text Box").expect("field").value(), "second");
}

#[test]
fn a_filled_form_round_trips_through_save_and_reopen() {
    let dir = temp_dir("form-roundtrip");
    let out = dir.join("filled.pdf");

    let doc = Document::open(FORM).expect("open");
    let mut form = doc.form().expect("form");
    form.set("Text Box", "round trip");
    doc.save_form(&out, &form, &SaveOptions::default())
        .expect("save");

    let reopened = Document::open(&out).expect("reopen");
    let field = reopened
        .form()
        .expect("form")
        .field("Text Box")
        .expect("field");
    assert_eq!(field.value(), "round trip");
    // And it is the *file* that holds it now, not an edit buffer.
    assert_eq!(field.stored_value(), "round trip");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_filled_widget_gets_the_chrome_the_engine_draws_and_no_text_body() {
    // The documented boundary (SPEC.md §10, ruling E1): appearance
    // generation builds a widget's *chrome* — background, border, and the
    // check and radio glyphs — and deliberately does not lay out a text
    // field's body, which upstream does in a second layout engine this
    // project does not port.
    //
    // This fixture's widget has no `/MK` and no border, so its chrome is
    // empty and the save writes no appearance stream for it. The value is
    // still stored, which is what a form fill is for: a reader that lays out
    // its own field text shows it, and one that does not shows the field
    // empty. The test pins that split rather than wishing it away.
    let dir = temp_dir("form-appearance");
    let out = dir.join("filled.pdf");

    let doc = Document::open(FORM).expect("open");
    assert!(!doc.page(0).expect("page").annotations()[0].has_appearance());

    let mut form = doc.form().expect("form");
    form.set("Text Box", "drawn");
    doc.save_form(&out, &form, &SaveOptions::default())
        .expect("save");

    let reopened = Document::open(&out).expect("reopen");
    // The value is in the file.
    assert_eq!(
        reopened
            .form()
            .expect("form")
            .field("Text Box")
            .expect("field")
            .stored_value(),
        "drawn"
    );
    // And the page still renders, with or without an appearance to draw.
    assert!(
        reopened
            .page(0)
            .expect("page")
            .render(&RenderOptions::default())
            .is_ok()
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_write_to_a_field_that_does_not_exist_is_ignored_rather_than_fatal() {
    let dir = temp_dir("form-unknown-field");
    let out = dir.join("out.pdf");

    let doc = Document::open(FORM).expect("open");
    let mut form = doc.form().expect("form");
    form.set("No Such Field", "value");
    doc.save_form(&out, &form, &SaveOptions::default())
        .expect("save succeeds anyway");

    assert_eq!(Document::open(&out).expect("reopen").page_count(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

// ------------------------------------------------------------------- save

#[test]
fn a_saved_document_reopens_with_the_same_pages() {
    let dir = temp_dir("save");
    let out = dir.join("saved.pdf");

    let doc = Document::open(BOOKMARKS).expect("open");
    doc.save(&out).expect("save");

    let reopened = Document::open(&out).expect("reopen");
    assert_eq!(reopened.page_count(), doc.page_count());
    // The outline survives the round trip.
    assert_eq!(reopened.outline().len(), doc.outline().len());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_saved_document_renders_the_same_pixels_as_its_original() {
    let dir = temp_dir("save-render");
    let out = dir.join("saved.pdf");

    let doc = Document::open(HELLO).expect("open");
    let before = doc
        .page(0)
        .expect("page")
        .render(&RenderOptions::default())
        .expect("render");
    doc.save(&out).expect("save");

    let reopened = Document::open(&out).expect("reopen");
    let after = reopened
        .page(0)
        .expect("page")
        .render(&RenderOptions::default())
        .expect("render");
    assert_eq!(
        before, after,
        "a save must not change what a page looks like"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_incremental_save_keeps_the_original_bytes_as_its_prefix() {
    let dir = temp_dir("save-incremental");
    let out = dir.join("appended.pdf");

    let original = std::fs::read(BOOKMARKS).expect("read");
    let doc = Document::open(BOOKMARKS).expect("open");
    doc.save_incremental(&out).expect("save");

    let saved = std::fs::read(&out).expect("read back");
    assert!(
        saved.starts_with(&original),
        "an incremental update appends rather than rewrites"
    );
    assert!(saved.len() > original.len(), "something was appended");
    assert_eq!(Document::open(&out).expect("reopen").page_count(), 2);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_full_save_is_smaller_than_the_incremental_one_it_replaces() {
    let dir = temp_dir("save-modes");
    let full = dir.join("full.pdf");
    let incremental = dir.join("incremental.pdf");

    let doc = Document::open(BOOKMARKS).expect("open");
    doc.save_with(
        &full,
        &SaveOptions {
            update: Update::Rewrite,
            ..SaveOptions::default()
        },
    )
    .expect("save");
    doc.save_incremental(&incremental).expect("save");

    let full_len = std::fs::metadata(&full).expect("stat").len();
    let incremental_len = std::fs::metadata(&incremental).expect("stat").len();
    assert!(
        full_len < incremental_len,
        "a rewrite drops what nothing points at: {full_len} vs {incremental_len}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_document_writes_to_any_sink() {
    let doc = Document::open(HELLO).expect("open");
    let mut bytes = Vec::new();
    doc.write_to(&mut bytes, &SaveOptions::default())
        .expect("write");
    assert!(bytes.starts_with(b"%PDF-"));
    assert!(
        bytes.ends_with(b"%%EOF\r\n") || bytes.ends_with(b"%%EOF\n") || bytes.ends_with(b"%%EOF")
    );
    // What was written to memory opens as a document.
    assert_eq!(
        Document::from_bytes(Arc::from(bytes))
            .expect("reopen")
            .page_count(),
        1
    );
}

#[test]
fn the_declared_version_can_be_overridden_on_save() {
    let dir = temp_dir("save-version");
    let out = dir.join("v14.pdf");

    let doc = Document::open(HELLO).expect("open");
    doc.save_with(
        &out,
        &SaveOptions {
            version: Some(14),
            ..SaveOptions::default()
        },
    )
    .expect("save");

    assert_eq!(Document::open(&out).expect("reopen").version(), 14);
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------- importing

#[test]
fn pages_import_from_another_document() {
    let dir = temp_dir("import");
    let out = dir.join("merged.pdf");

    let doc = Document::open(HELLO).expect("open");
    let source = Document::open(BOOKMARKS).expect("open");
    doc.import_pages(&out, &source, &[0, 1], doc.page_count())
        .expect("import");

    let merged = Document::open(&out).expect("reopen");
    assert_eq!(merged.page_count(), 3);
    // The appended pages keep their own geometry.
    let width = |index: u32| merged.page(index).expect("page").width();
    assert!(is_points(width(0), 200));
    assert!(is_points(width(1), 612));
    assert!(is_points(width(2), 612));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn importing_at_the_front_shifts_the_existing_pages_up() {
    let dir = temp_dir("import-front");
    let out = dir.join("merged.pdf");

    let doc = Document::open(HELLO).expect("open");
    let source = Document::open(BOOKMARKS).expect("open");
    doc.import_pages(&out, &source, &[0], 0).expect("import");

    let merged = Document::open(&out).expect("reopen");
    assert_eq!(merged.page_count(), 2);
    let width = |index: u32| merged.page(index).expect("page").width();
    assert!(is_points(width(0), 612), "the import");
    assert!(is_points(width(1), 200), "the original");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn importing_a_page_the_source_does_not_have_fails_without_writing() {
    let dir = temp_dir("import-bad");
    let out = dir.join("never.pdf");

    let doc = Document::open(HELLO).expect("open");
    let source = Document::open(BOOKMARKS).expect("open");
    let err = doc
        .import_pages(&out, &source, &[0, 99], 0)
        .expect_err("page 99 does not exist");
    assert!(matches!(err, pdfrum::Error::Save(_)), "got {err:?}");
    assert!(!out.exists(), "nothing is written when the import fails");

    std::fs::remove_dir_all(&dir).ok();
}

// -------------------------------------------------------------- parallelism

#[test]
fn pages_render_in_parallel_to_the_same_pixels_as_in_series() {
    use rayon::prelude::*;

    let doc = Document::open(BOOKMARKS).expect("open");
    let options = RenderOptions::scaled(1.5);
    let pages: Vec<_> = doc.pages().collect();

    let serial: Vec<_> = pages
        .iter()
        .map(|page| page.render(&options).expect("render"))
        .collect();

    let parallel: Vec<_> = pages
        .par_iter()
        .map(|page| page.render(&options).expect("render"))
        .collect();

    let per_worker_cache: Vec<_> = pages
        .par_iter()
        .map_init(pdfrum::BuildContext::new, |ctx, page| {
            page.render_with(&options, ctx).expect("render")
        })
        .collect();

    let per_worker_session: Vec<_> = pages
        .par_iter()
        .map_init(pdfrum::RenderSession::new, |session, page| {
            page.render_session(&options, session).expect("render")
        })
        .collect();

    assert_eq!(serial, parallel);
    assert_eq!(serial, per_worker_cache);
    assert_eq!(serial, per_worker_session);
}

// ------------------------------------------------------------ render session

#[test]
fn a_shared_session_renders_the_same_pixels_as_a_fresh_one_per_page() {
    let doc = Document::open(BOOKMARKS).expect("open");
    let options = RenderOptions::scaled(2.0);

    let fresh: Vec<_> = doc
        .pages()
        .map(|page| page.render(&options).expect("render"))
        .collect();

    // One session across both pages: the second page draws with a glyph cache
    // the first page warmed. These fixtures share a font, so this is the case
    // the reuse exists for, and it must not change what is drawn.
    let mut session = pdfrum::RenderSession::new();
    let shared: Vec<_> = doc
        .pages()
        .map(|page| page.render_session(&options, &mut session).expect("render"))
        .collect();

    assert_eq!(fresh, shared);
}

#[test]
fn one_image_drawn_at_two_sizes_is_right_at_both() {
    // The correctness half of the decode target (SPEC.md §7). The page draws
    // one 1269x1643 JPEG 2000 `XObject` twice: nearly full-page, and as a
    // 40x50 thumbnail. Rendering the page at 1/16 scale asks for a reduced
    // decode; rendering it at full scale asks for a much larger one. If
    // resolution were not part of cache identity, the second render would be
    // handed the first's thumbnail and the whole page would be a blur — the
    // kind of failure that leaves the *size* right and only the pixels wrong,
    // so nothing but a pixel comparison catches it.
    let doc = Document::open(JPX_TWO_SIZES).expect("open");
    let small = RenderOptions::scaled(1.0 / 16.0);
    let large = RenderOptions::default();

    // Each rendered alone, with nothing cached before it.
    let alone_small = doc
        .page(0)
        .expect("page")
        .render(&small)
        .expect("small render");
    let alone_large = doc
        .page(0)
        .expect("page")
        .render(&large)
        .expect("large render");

    // Then both through one session, small first, so the large render meets a
    // cache holding a decode far too coarse for it.
    let mut session = pdfrum::RenderSession::new();
    let shared_small = doc
        .page(0)
        .expect("page")
        .render_session(&small, &mut session)
        .expect("small render");
    let shared_large = doc
        .page(0)
        .expect("page")
        .render_session(&large, &mut session)
        .expect("large render");

    assert_eq!(
        alone_small, shared_small,
        "the small render is unaffected by sharing a session"
    );
    assert_eq!(
        alone_large, shared_large,
        "the large render must not inherit the small one's decode"
    );

    // The order the other way round is deliberately *not* symmetric, and the
    // asymmetry is the oracle's. `CPDF_PageImageCache::Entry::IsCacheValid`
    // (`cpdf_pageimagecache.cpp:347-358`) accepts a cached bitmap whenever it
    // is at least as large as the new request, so a small draw that follows a
    // large one reuses the large decode rather than decoding again. The
    // guarantee is therefore one-directional: a hit is never *coarser* than a
    // fresh decode would have been, and may be finer. What must hold is that
    // the large render is unaffected either way.
    let mut reversed = pdfrum::RenderSession::new();
    let large_first = doc
        .page(0)
        .expect("page")
        .render_session(&large, &mut reversed)
        .expect("large render");
    let small_after = doc
        .page(0)
        .expect("page")
        .render_session(&small, &mut reversed)
        .expect("small render");
    assert_eq!(alone_large, large_first);
    assert_eq!(
        (small_after.width(), small_after.height()),
        (alone_small.width(), alone_small.height()),
        "reusing a finer decode changes pixels, never the output size"
    );
    assert!(
        at_least_as_detailed(detail(&small_after), detail(&alone_small)),
        "a hit on a finer decode is at worst as good as decoding afresh"
    );

    // And the sizes really are the two the test claims, so the assertions
    // above are about two genuinely different decode targets rather than about
    // one target asked for twice.
    assert_eq!((alone_large.width(), alone_large.height()), (600, 800));
    assert_eq!((alone_small.width(), alone_small.height()), (37, 50));
}

/// Total absolute difference between horizontally adjacent red samples, and
/// how many pairs it is over — a stand-in for "how much of the source survived
/// into these pixels".
///
/// Returned as the pair rather than the quotient so callers compare it exactly,
/// by cross-multiplying: the assertions here are `>=`, and a float division
/// would put rounding between two integers whose order is the whole question.
///
/// Comparing two renders of the *same* size, a coarser decode is smoother, so
/// more is better. It says nothing across sizes, where the number is dominated
/// by how many source samples one output pixel spans.
fn detail(pixmap: &pdfrum::Pixmap) -> (u64, u64) {
    let data = pixmap.data();
    let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
    let mut total = 0u64;
    let mut count = 0u64;
    for y in 0..h {
        for x in 1..w {
            let a = data.get((y * w + x) * 4).copied().unwrap_or(0);
            let b = data.get((y * w + x - 1) * 4).copied().unwrap_or(0);
            total += u64::from(a.abs_diff(b));
            count += 1;
        }
    }
    (total, count)
}

/// Whether `a`'s mean detail is at least `b`'s, compared without dividing.
fn at_least_as_detailed(a: (u64, u64), b: (u64, u64)) -> bool {
    // a.0 / a.1 >= b.0 / b.1, cross-multiplied. An empty pixmap has no detail
    // and loses to anything that has some.
    a.0.saturating_mul(b.1) >= b.0.saturating_mul(a.1)
}

#[test]
fn a_session_serves_extraction_and_rendering_from_one_set_of_caches() {
    let doc = Document::open(BOOKMARKS).expect("open");
    let mut session = pdfrum::RenderSession::new();

    for page in doc.pages() {
        let pixmap = page
            .render_session(&RenderOptions::default(), &mut session)
            .expect("render");
        let text = page.text_session(&mut session).all_text();
        assert!(pixmap.width() > 0);
        assert!(text.contains("Page"));
    }

    // The build half is shared with `render_with`/`text_with`, so a caller
    // holding a session can still reach the narrower API.
    let text = doc
        .page(0)
        .expect("page")
        .text_with(&mut session.build)
        .all_text();
    assert!(text.contains("Page1"));
}

// --------------------------------------------------------- diagnostics view

#[test]
fn a_repair_found_after_the_load_reaches_the_document_wide_view() {
    let doc = Document::open(HELLO).expect("open");

    // This fixture's content stream omits its `/Length`, so the lexer has to
    // resync on `endstream`. That happens when the stream is first *read* —
    // long after the load returned — which is exactly the damage the
    // load-time snapshot structurally cannot see.
    assert!(
        doc.diagnostics().is_empty(),
        "the load itself touched no content stream"
    );
    assert!(doc.all_diagnostics().is_empty(), "and nothing has yet");

    let _ = doc
        .page(0)
        .expect("page")
        .render(&RenderOptions::default())
        .expect("render");

    assert!(
        doc.all_diagnostics()
            .contains(&pdfrum::DiagKind::KeywordResync),
        "the missing /Length is reported once the stream is read: {:?}",
        doc.all_diagnostics().entries()
    );
    // The load-time snapshot is unchanged by the work, which is the whole
    // point of keeping it a separate accessor: an existing caller's answer
    // does not start moving under it.
    assert!(doc.diagnostics().is_empty());
}

#[test]
fn a_repair_this_crates_own_reads_make_reaches_the_view_too() {
    // Not all late damage is the parser's. Generating a widget's missing
    // appearance is something *this* crate's render path asks for, through a
    // sink that used to be dropped on return.
    let doc = Document::open(FORM).expect("open");
    let _ = doc
        .page(0)
        .expect("page")
        .render(&RenderOptions::default())
        .expect("render");

    assert!(
        doc.all_diagnostics()
            .contains(&pdfrum::DiagKind::AppearanceGenerated),
        "{:?}",
        doc.all_diagnostics().entries()
    );
}

#[test]
fn the_document_wide_view_is_a_snapshot_rather_than_a_drain() {
    let doc = Document::open(HELLO).expect("open");
    let _ = doc.page(0).expect("page").text();

    let once = doc.all_diagnostics();
    let twice = doc.all_diagnostics();
    assert_eq!(once.len(), twice.len());
    assert_eq!(once.recorded(), twice.recorded());
}

#[test]
fn the_document_wide_view_grows_as_the_document_is_used() {
    let doc = Document::open(HELLO).expect("open");
    let before = doc.all_diagnostics().recorded();
    for page in doc.pages() {
        let _ = page.text();
        let _ = page.render(&RenderOptions::default());
    }
    let after = doc.all_diagnostics().recorded();
    assert!(
        after >= before,
        "a running total only grows: {before} then {after}"
    );
}

#[test]
fn text_extracts_in_parallel_too() {
    use rayon::prelude::*;

    let doc = Document::open(BOOKMARKS).expect("open");
    let pages: Vec<_> = doc.pages().collect();
    let texts: Vec<String> = pages
        .par_iter()
        .map(|page| page.text().all_text())
        .collect();
    assert_eq!(texts.len(), 2);
    assert!(texts[0].contains("Page1"));
    assert!(texts[1].contains("Page2"));
}

#[test]
fn a_document_can_be_shared_across_threads_by_reference() {
    let doc = Document::open(BOOKMARKS).expect("open");
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..doc.page_count())
            .map(|index| {
                let doc = &doc;
                scope.spawn(move || {
                    doc.page(index)
                        .expect("page")
                        .render(&RenderOptions::default())
                        .expect("render")
                        .width()
                })
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.join().expect("thread"), 612);
        }
    });
}

// ------------------------------------------------------------------ errors

#[test]
fn every_error_prints_a_message_naming_its_domain() {
    let not_pdf = Document::from_bytes(Arc::from(b"nope".as_slice())).expect_err("err");
    assert!(not_pdf.to_string().contains("cannot open document"));

    let doc = Document::open(HELLO).expect("open");
    let no_page = doc.page(7).expect_err("err");
    assert!(no_page.to_string().contains("cannot read document"));

    let no_pixels = doc
        .page(0)
        .expect("page")
        .render(&RenderOptions::scaled(0.0))
        .expect_err("err");
    assert!(no_pixels.to_string().contains("cannot render page"));
}

#[test]
fn an_error_keeps_the_member_crates_own_error_as_its_source() {
    use std::error::Error as _;

    let err = Document::from_bytes(Arc::from(b"nope".as_slice())).expect_err("err");
    let source = err.source().expect("the wrapped error survives");
    assert_eq!(source.to_string(), "not a PDF file");
}
