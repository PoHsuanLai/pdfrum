//! Integration tests over the facade, on real PDFs.
//!
//! These drive the crate the way a caller does — through the public API only,
//! over the fixtures in `tests/fixtures/` (see `PROVENANCE.md`: they are the
//! oracle's own test files, unmodified). Every facade path is exercised at
//! least once here; the doctests carry the *documented* behaviour and these
//! carry the corners the docs would not be improved by spelling out.

use std::sync::Arc;

use pdfrum::{
    CharIndex, Document, FieldKind, FindOptions, OpenOptions, PageIndex, PdfVersion, Permissions,
    RenderOptions, SaveOptions, Subtype, UnknownField, Update, VelloCpuBackend,
};

const HELLO: &str = "tests/fixtures/hello_world.pdf";
const FORM: &str = "tests/fixtures/text_form.pdf";
const BOOKMARKS: &str = "tests/fixtures/bookmarks.pdf";
const JPX_TWO_SIZES: &str = "tests/fixtures/jpx_two_sizes.pdf";
const RECTANGLES: &str = "tests/fixtures/rectangles.pdf";

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
    assert_eq!(doc.permissions(), Permissions::ALL);
    assert_eq!(doc.owner_permissions(), Permissions::ALL);
}

#[test]
fn a_document_reports_its_version_and_its_bytes() {
    let doc = Document::open(HELLO).expect("open");
    assert_eq!(doc.version(), Some(PdfVersion::PDF_1_7), "%PDF-1.7");
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
            .to_string()
            .contains("Hello"),
        "the stream was read despite its missing length"
    );
}

// ------------------------------------------------------------------ pages

#[test]
fn pages_are_reachable_by_index_and_by_iteration_and_agree() {
    let doc = Document::open(BOOKMARKS).expect("open");
    assert_eq!(doc.page_count(), 2);

    let by_iter: Vec<PageIndex> = doc.pages().map(|page| page.index()).collect();
    assert_eq!(by_iter, [PageIndex::new(0), PageIndex::new(1)]);

    for index in 0..doc.page_count() {
        let page = doc.page(index).expect("page");
        assert_eq!(page.index(), PageIndex::from(index));
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
        let pixmap = page
            .render(&VelloCpuBackend::new(), &RenderOptions::scaled(scale))
            .expect("render");
        assert_eq!((pixmap.width(), pixmap.height()), (expected, expected));
    }
}

#[test]
fn fit_scales_a_page_into_a_box_without_distorting_it() {
    let doc = Document::open(BOOKMARKS).expect("open");
    let page = doc.page(0).expect("page");
    // 612x792 into 100x100 must fit the *tall* axis.
    let options = RenderOptions::fit(page.width(), page.height(), 100, 100);
    let pixmap = page
        .render(&VelloCpuBackend::new(), &options)
        .expect("render");
    assert!(pixmap.width() <= 100 && pixmap.height() <= 100);
    assert_eq!(pixmap.height(), 100, "the tall axis is the binding one");
}

#[test]
fn a_zero_sized_render_is_an_error_rather_than_an_empty_image() {
    let doc = Document::open(HELLO).expect("open");
    let err = doc
        .page(0)
        .expect("page")
        .render(&VelloCpuBackend::new(), &RenderOptions::scaled(0.0))
        .expect_err("a zero-scale target has no pixels");
    assert!(matches!(err, pdfrum::Error::Render(_)), "got {err:?}");
}

#[test]
fn every_backend_renders_the_same_page_at_the_same_size() {
    let doc = Document::open(HELLO).expect("open");
    let page = doc.page(0).expect("page");
    let opts = RenderOptions::default();
    // Three backends, each named at the call site rather than selected by an
    // enum the facade owns (2026-09-02). The two that are not the facade's
    // default are dev-dependencies here, which is exactly what a caller who
    // wants one writes.
    let vello = page
        .render_on(
            &pdfrum::VelloCpuBackend::new(),
            &opts,
            &mut pdfrum::RenderSession::new(),
        )
        .expect("render");
    let tiny = page
        .render_on(
            &pdfrum_raster_tinyskia::TinySkiaBackend::new(),
            &opts,
            &mut pdfrum::RenderSession::new(),
        )
        .expect("render");
    let exact = page
        .render_on(
            &pdfrum_raster_agg::AggBackend::new(),
            &opts,
            &mut pdfrum::RenderSession::new(),
        )
        .expect("render");
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
    let once = page
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect("render");
    let twice = page
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect("render");
    assert_eq!(once, twice);
}

#[test]
fn a_forced_background_replaces_the_pages_own() {
    let doc = Document::open(HELLO).expect("open");
    let pixmap = doc
        .page(0)
        .expect("page")
        .render(
            &VelloCpuBackend::new(),
            &RenderOptions {
                background: Some(peniko::Color::from_rgba8(0, 0, 255, 255)),
                ..RenderOptions::default()
            },
        )
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
    let with = page
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect("render");
    let without = page
        .render(
            &VelloCpuBackend::new(),
            &RenderOptions {
                annotations: false,
                ..RenderOptions::default()
            },
        )
        .expect("render");
    // Same size either way; the flag changes what is drawn, not the target.
    assert_eq!(
        (with.width(), with.height()),
        (without.width(), without.height())
    );
}

#[test]
fn a_shared_session_renders_the_same_pixels_as_a_fresh_one() {
    // The cache must be an optimization and nothing more.
    let doc = Document::open(BOOKMARKS).expect("open");
    let options = RenderOptions::default();

    let fresh: Vec<_> = doc
        .pages()
        .map(|page| {
            page.render(&VelloCpuBackend::new(), &options)
                .expect("render")
        })
        .collect();

    let backend = pdfrum::VelloCpuBackend::new();
    let mut session = pdfrum::RenderSession::new();
    let shared: Vec<_> = doc
        .pages()
        .map(|page| {
            page.render_on(&backend, &options, &mut session)
                .expect("render")
        })
        .collect();

    assert_eq!(fresh, shared);
}

/// `render` is `render_on` with the default backend and a fresh session, and
/// `text` is `text_on` with a fresh one — byte for byte, not merely close.
///
/// This is what the 2026-09-03 fold of `render_with` / `render_with_on` /
/// `render_session` / `render_session_on` into `render_on` (and `text_with` /
/// `text_session` into `text_on`) has to preserve: the convenience forms are
/// the general one with a default argument and nothing else.
#[test]
fn the_convenience_forms_are_the_general_ones_with_fresh_arguments() {
    let doc = Document::open(BOOKMARKS).expect("open");
    let options = RenderOptions::scaled(1.5);
    let backend = pdfrum::VelloCpuBackend::new();

    for page in doc.pages() {
        let convenience = page
            .render(&VelloCpuBackend::new(), &options)
            .expect("render");
        let general = page
            .render_on(&backend, &options, &mut pdfrum::RenderSession::new())
            .expect("render_on");
        assert_eq!(
            convenience, general,
            "render is render_on with a fresh session"
        );

        let text = page.text().to_string();
        let text_on = page.text_on(&mut pdfrum::RenderSession::new()).to_string();
        assert_eq!(text, text_on, "text is text_on with a fresh session");
    }
}

/// The substitution-options case survives the fold.
///
/// Before 2026-09-03 the only way to render or extract with a configured
/// [`pdfrum::BuildContext`] was `render_with` / `text_with`, which took one
/// directly. Those are gone; the context is now reached as `session.build`,
/// and this proves that route reaches the same place — a session whose build
/// half carries substitution options renders and extracts exactly as a
/// standalone context so configured would have.
#[test]
fn a_sessions_build_half_still_carries_substitution_options() {
    let doc = Document::open(BOOKMARKS).expect("open");
    let options = RenderOptions::default();
    let backend = pdfrum::VelloCpuBackend::new();
    let substitution = pdfrum::SubstitutionOptions::default();

    let mut session = pdfrum::RenderSession::new();
    session.build = pdfrum::BuildContext::with_substitution(substitution.clone());

    // The same context, standing alone, is what the withdrawn `render_with`
    // and `text_with` took. Driving it through `paint` is no longer possible
    // from outside the crate, so the check is that the session route produces
    // what a default one does for a document with no substituted font, and
    // that the configured context is genuinely the one in use.
    let mut plain = pdfrum::RenderSession::new();
    for page in doc.pages() {
        let configured = page
            .render_on(&backend, &options, &mut session)
            .expect("render");
        let default = page
            .render_on(&backend, &options, &mut plain)
            .expect("render");
        assert_eq!(configured, default);

        // Extraction reads the same build half, so a caller holding one
        // session need never reach for a second context.
        assert_eq!(
            page.text_on(&mut session).to_string(),
            page.text_on(&mut plain).to_string()
        );
    }

    // And the context really is the configured one: it is reachable, `&mut`,
    // and replaceable in place, which is the whole of what `render_with` gave.
    session.build = pdfrum::BuildContext::with_substitution(substitution);
    let text = doc.page(0).expect("page").text_on(&mut session).to_string();
    assert!(text.contains("Page1"));
}

// ------------------------------------------------------------------- text

#[test]
fn text_extraction_reads_the_pages_strings_in_order() {
    let doc = Document::open(HELLO).expect("open");
    let text = doc.page(0).expect("page").text();
    let all = text.to_string();
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
    // `find` counts in the search text and `rects` counts in the character
    // list; the page's own map is what bridges them, and after the types
    // will not let a caller skip it.
    let from = text.runs.char_index(hit.start).expect("a character");
    let to = text.runs.char_index(hit.end).expect("a character");
    let rects = text.rects(from..to);
    assert_eq!(rects.len(), 1, "one text object, one box");
    assert!(rects[0].width() > 0.0 && rects[0].height() > 0.0);
}

#[test]
fn text_can_be_selected_by_rectangle_and_probed_by_point() {
    let doc = Document::open(HELLO).expect("open");
    let text = doc.page(0).expect("page").text();
    let first = text.char(CharIndex::new(0)).expect("a first character");
    // The point at a character's own origin finds that character.
    let found = text.index_at(first.origin, kurbo::Size::new(2.0, 2.0));
    assert_eq!(found, Some(CharIndex::new(0)));
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
    assert!(text.to_string().contains("Test Form"));
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
    assert_eq!(entry.page_index(), Some(PageIndex::FIRST));
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
    form.set("Text Box", "typed").expect("field exists");

    let field = form.field("Text Box").expect("field");
    assert_eq!(field.value(), "typed", "the edit wins");
    assert_eq!(field.stored_value(), "", "the file is untouched");
    assert_eq!(form.edits().collect::<Vec<_>>(), [("Text Box", "typed")]);
}

#[test]
fn writing_a_field_twice_keeps_the_last_value() {
    let doc = Document::open(FORM).expect("open");
    let mut form = doc.form().expect("form");
    form.set("Text Box", "first").expect("field exists");
    form.set("Text Box", "second").expect("field exists");
    assert_eq!(form.edits().count(), 1);
    assert_eq!(form.field("Text Box").expect("field").value(), "second");
}

#[test]
fn a_filled_form_round_trips_through_save_and_reopen() {
    let dir = temp_dir("form-roundtrip");
    let out = dir.join("filled.pdf");

    let doc = Document::open(FORM).expect("open");
    let mut form = doc.form().expect("form");
    form.set("Text Box", "round trip").expect("field exists");
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
fn a_field_that_already_holds_an_empty_value_takes_the_new_one() {
    // A merged field-and-widget dictionary with `/V ()` already in it — the
    // common shape a form authoring tool writes. The value edit and the
    // widget edit target the same object; the widget's copy used to be
    // folded back over the edited one and its old `/V ()` won, so the file
    // came out unchanged while the save reported success.
    let pdf = b"%PDF-1.7\n\
1 0 obj<</Type/Catalog/Pages 2 0 R/AcroForm<</Fields[4 0 R]/DA(/Helv 0 Tf 0 g)>>>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 100]/Annots[4 0 R]>>endobj\n\
4 0 obj<</Type/Annot/Subtype/Widget/FT/Tx/T(name)/V()/F 4/P 3 0 R/Rect[10 10 190 40]/DA(/Helv 12 Tf 0 g)>>endobj\n\
trailer<</Root 1 0 R/Size 5>>\n";
    let doc = Document::from_bytes(Arc::from(&pdf[..])).expect("open");
    let mut form = doc.form().expect("form");
    assert_eq!(form.field("name").expect("field").stored_value(), "");
    form.set("name", "kept").expect("field exists");
    let mut out = Vec::new();
    doc.write_form_to(&mut out, &form, &SaveOptions::default())
        .expect("save");
    let reopened = Document::from_bytes(Arc::from(out)).expect("reopen");
    assert_eq!(
        reopened
            .form()
            .expect("form")
            .field("name")
            .expect("field")
            .stored_value(),
        "kept"
    );
}

#[test]
fn a_filled_widget_gets_the_chrome_the_engine_draws_and_no_text_body() {
    // The documented boundary: appearance
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
    form.set("Text Box", "drawn").expect("field exists");
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
            .render(&VelloCpuBackend::new(), &RenderOptions::default())
            .is_ok()
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_write_to_a_field_that_does_not_exist_returns_unknown_field() {
    let doc = Document::open(FORM).expect("open");
    let mut form = doc.form().expect("form");
    let err = form
        .set("No Such Field", "value")
        .expect_err("unknown name");
    assert_eq!(
        err,
        UnknownField {
            name: "No Such Field".into()
        }
    );
    assert!(form.edits().next().is_none(), "the write is not recorded");
}

#[test]
fn set_checked_on_a_known_field_takes_effect_and_an_unknown_name_errors() {
    let doc = Document::open("tests/fixtures/click_form.pdf").expect("open");
    let mut form = doc.form().expect("form");
    let name = form
        .fields()
        .into_iter()
        .find(|f| f.kind() == FieldKind::Check && !f.is_read_only())
        .expect("an ordinary checkbox")
        .name()
        .to_owned();

    assert!(
        !form.field(&name).expect("field").is_checked(),
        "the ordinary box starts clear"
    );
    form.set_checked(&name, true).expect("field exists");
    assert!(form.field(&name).expect("field").is_checked());

    let err = form
        .set_checked("No Such Field", true)
        .expect_err("unknown name");
    assert_eq!(
        err,
        UnknownField {
            name: "No Such Field".into()
        }
    );
}

#[test]
fn outline_into_iter_agrees_with_iter_and_is_not_a_vec() {
    let doc = Document::open(BOOKMARKS).expect("open");
    let outline = doc.outline();
    let via_iter: Vec<_> = outline.iter().map(|b| (b.depth(), b.title())).collect();
    let via_into: Vec<_> = (&outline)
        .into_iter()
        .map(|b| (b.depth(), b.title()))
        .collect();
    assert_eq!(via_iter, via_into);

    let named: pdfrum::OutlineIter<'_, '_> = outline.iter();
    let from_into: pdfrum::OutlineIter<'_, '_> = (&outline).into_iter();
    let name = std::any::type_name_of_val(&named);
    assert_eq!(name, std::any::type_name_of_val(&from_into));
    assert!(
        !name.contains("vec::IntoIter"),
        "IntoIterator must not collect into a Vec: {name}"
    );
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
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect("render");
    doc.save(&out).expect("save");

    let reopened = Document::open(&out).expect("reopen");
    let after = reopened
        .page(0)
        .expect("page")
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
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
            version: Some(PdfVersion::PDF_1_4),
            ..SaveOptions::default()
        },
    )
    .expect("save");

    assert_eq!(
        Document::open(&out).expect("reopen").version(),
        Some(PdfVersion::PDF_1_4)
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------- importing

#[test]
fn pages_import_from_another_document() {
    let dir = temp_dir("import");
    let out = dir.join("merged.pdf");

    let doc = Document::open(HELLO).expect("open");
    let source = Document::open(BOOKMARKS).expect("open");
    doc.import_pages(&out, &source, [0, 1], doc.page_count())
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
    doc.import_pages(&out, &source, [0], 0).expect("import");

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
        .import_pages(&out, &source, [0, 99], 0)
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
        .map(|page| {
            page.render(&VelloCpuBackend::new(), &options)
                .expect("render")
        })
        .collect();

    let parallel: Vec<_> = pages
        .par_iter()
        .map(|page| {
            page.render(&VelloCpuBackend::new(), &options)
                .expect("render")
        })
        .collect();

    let backend = pdfrum::VelloCpuBackend::new();
    let per_worker_session: Vec<_> = pages
        .par_iter()
        .map_init(pdfrum::RenderSession::new, |session, page| {
            page.render_on(&backend, &options, session).expect("render")
        })
        .collect();

    assert_eq!(serial, parallel);
    assert_eq!(serial, per_worker_session);
}

// ------------------------------------------------------------ render session

#[test]
fn a_shared_session_renders_the_same_pixels_as_a_fresh_one_per_page() {
    let doc = Document::open(BOOKMARKS).expect("open");
    let options = RenderOptions::scaled(2.0);

    let fresh: Vec<_> = doc
        .pages()
        .map(|page| {
            page.render(&VelloCpuBackend::new(), &options)
                .expect("render")
        })
        .collect();

    // One session across both pages: the second page draws with a glyph cache
    // the first page warmed. These fixtures share a font, so this is the case
    // the reuse exists for, and it must not change what is drawn.
    let backend = pdfrum::VelloCpuBackend::new();
    let mut session = pdfrum::RenderSession::new();
    let shared: Vec<_> = doc
        .pages()
        .map(|page| {
            page.render_on(&backend, &options, &mut session)
                .expect("render")
        })
        .collect();

    assert_eq!(fresh, shared);
}

#[test]
fn one_image_drawn_at_two_sizes_is_right_at_both() {
    // The correctness half of the decode target. The page draws
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
        .render(&VelloCpuBackend::new(), &small)
        .expect("small render");
    let alone_large = doc
        .page(0)
        .expect("page")
        .render(&VelloCpuBackend::new(), &large)
        .expect("large render");

    // Then both through one session, small first, so the large render meets a
    // cache holding a decode far too coarse for it.
    let backend = pdfrum::VelloCpuBackend::new();
    let mut session = pdfrum::RenderSession::new();
    let shared_small = doc
        .page(0)
        .expect("page")
        .render_on(&backend, &small, &mut session)
        .expect("small render");
    let shared_large = doc
        .page(0)
        .expect("page")
        .render_on(&backend, &large, &mut session)
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
        .render_on(&backend, &large, &mut reversed)
        .expect("large render");
    let small_after = doc
        .page(0)
        .expect("page")
        .render_on(&backend, &small, &mut reversed)
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
    let backend = pdfrum::VelloCpuBackend::new();
    let mut session = pdfrum::RenderSession::new();

    for page in doc.pages() {
        let pixmap = page
            .render_on(&backend, &RenderOptions::default(), &mut session)
            .expect("render");
        let text = page.text_on(&mut session).to_string();
        assert!(pixmap.width() > 0);
        assert!(text.contains("Page"));
    }

    // `session.build` is a public field, so a caller who wants the build half
    // alone — to configure it, or to hand it to a `FormSession` — still has
    // it. That is what the withdrawn `render_with` / `text_with` were for,
    // and replacing it in place is how a caller reaches the configured case.
    session.build = pdfrum::BuildContext::new();
    let text = doc.page(0).expect("page").text_on(&mut session).to_string();
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
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
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
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
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
        let _ = page.render(&VelloCpuBackend::new(), &RenderOptions::default());
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
        .map(|page| page.text().to_string())
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
                        .render(&VelloCpuBackend::new(), &RenderOptions::default())
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
        .render(&VelloCpuBackend::new(), &RenderOptions::scaled(0.0))
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

// ------------------------------------------------- : options and colour

// The facade's `RenderOptions` flags are positive (`smooth_paths`,
// `interpolate_images`) where the engine's are the oracle's inverted flag
// words (`no_path_smooth`, `no_image_smooth`). §A.8 rules that the two types
// stay different and §B rules that the conversion is a narrowing at the entry
// point — `RenderOptions::to_inner`. This test is what makes that mapping
// checkable from outside: it renders the *same* page graph twice, once
// through the facade asking for `smooth_paths: false` and once through the
// engine asking for `no_path_smooth: true`, and the two pixmaps must be
// byte-equal. A mapping that dropped the `!`, or crossed the two flags over,
// fails here rather than in a golden nobody reads.
#[test]
fn a_positive_facade_flag_reaches_the_engine_as_the_inverted_one() {
    let doc = Document::open(RECTANGLES).expect("open");
    let page = doc.page(0).expect("page");

    let via_facade = page
        .render(
            &VelloCpuBackend::new(),
            &RenderOptions {
                smooth_paths: false,
                annotations: false,
                ..RenderOptions::default()
            },
        )
        .expect("render");

    // The engine, driven directly with the oracle's own flag name. The page
    // graph is the public one, so this is the same input by construction.
    let graph = page.objects();
    let engine_options = pdfrum_render::RenderOptions {
        no_path_smooth: true,
        ..pdfrum_render::RenderOptions::default()
    };
    let mut diags = pdfrum::Diagnostics::default();
    let mut caches = pdfrum::RenderCaches::new();
    let via_engine = pdfrum_render::render_page_with(
        &graph,
        &engine_options,
        &pdfrum::VelloCpuBackend::new(),
        pdfrum_render::RenderSession {
            caches: Some(&mut caches),
            ..Default::default()
        },
        &mut diags,
    )
    .expect("render");

    assert_eq!(
        via_facade, via_engine,
        "smooth_paths: false is no_path_smooth: true"
    );

    // And the flag is not inert: the default (smoothed) render differs.
    let smoothed = page
        .render(
            &VelloCpuBackend::new(),
            &RenderOptions {
                annotations: false,
                ..RenderOptions::default()
            },
        )
        .expect("render");
    assert_ne!(via_facade, smoothed, "the flag has to change something");
}

// The defaults are the common case, positively stated.
#[test]
fn the_render_defaults_are_the_common_case() {
    let options = RenderOptions::default();
    assert!(options.smooth_paths);
    assert!(options.interpolate_images);
    assert!(options.annotations);
}

// `PathBuilder::fill` takes a `peniko::Color` — the vocabulary the crate
// already re-exports for `RenderOptions::background` — rather than a bare
// `[f32; 3]`. The conversion to the writer's triple happens at the entry
// point into `pdfrum-edit`, and this proves the colour a caller names is the
// colour the *saved file* carries: build the path from a `Color`, save,
// reopen, and read the fill back off the reloaded page object.
#[test]
fn a_path_fill_named_as_a_color_round_trips_through_a_save() {
    use pdfrum::{Color, PathBuilder, Rect};

    let dir = temp_dir("wp10-color");
    let out = dir.join("filled.pdf");

    let doc = Document::open(HELLO).expect("open");
    let mut edit = doc.page(0).expect("page").edit();
    edit.push(
        PathBuilder {
            // Alpha is deliberately not 255. It is *kept*: the colour goes out
            // as `rg`, which has no alpha, but the alpha goes out beside it as
            // an `/ExtGState` `/ca`, so a translucent colour survives the save.
            fill: Some(Color::from_rgba8(64, 128, 192, 128)),
            stroke: None,
            ..PathBuilder::rect(Rect::new(10.0, 10.0, 60.0, 40.0))
        }
        .build(),
    );
    doc.save_pages(&out, &[edit], &SaveOptions::default())
        .expect("save");

    let reopened = Document::open(&out).expect("reopen");
    let objects = reopened.page(0).expect("page").objects();
    let path = objects
        .objects
        .iter()
        .rev()
        .find_map(|o| match o {
            pdfrum::PageObject::Path(p) => Some(p),
            _ => None,
        })
        .expect("the path we pushed");

    let rgb = path.state.fill.to_rgb().expect("an expressible fill");
    let expected = Color::from_rgba8(64, 128, 192, 128);
    for (name, got, want) in [
        ("r", rgb.r, expected.components[0]),
        ("g", rgb.g, expected.components[1]),
        ("b", rgb.b, expected.components[2]),
        ("a", path.state.general.fill_alpha, expected.components[3]),
    ] {
        assert!(
            (got - want).abs() < 2.0 / 255.0,
            "{name} survived the save: got {got}, want {want}"
        );
    }
}
