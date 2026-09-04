//! Painting a page's annotation appearances onto the page.
//!
//! An annotation's `/AP` form is part of the page image rather than an
//! overlay a viewer adds. But it does not all arrive by one route, and the
//! split is the thing to know:
//!
//! - **Pass A** is the page render, and it draws the *non*-widgets: the walk
//!   over the annotation list skips every `/Widget`.
//! - **Pass B** is the form-fill draw that follows every bitmap render,
//!   unconditionally. That is where widgets are drawn, one at a time through
//!   their own one-layer render context.
//!
//! Both end in the same placement arithmetic and both draw the normal
//! appearance, so one traversal reproduces them — but **their visibility
//! tests differ**, and merging them would be wrong:
//!
//! | flag | Pass A (non-widgets) | Pass B (widgets) |
//! |---|---|---|
//! | `Invisible` (bit 1) | not tested | **suppresses** |
//! | `Hidden` (bit 2) | suppresses | suppresses |
//! | `Print` (bit 3) | required when printing | not tested |
//! | `NoView` (bit 6) | suppresses on screen | suppresses |
//!
//! `is_visible` therefore keys on the subtype: a widget goes through Pass
//! B's rules and everything else through Pass A's, which is the only place
//! the `Invisible` bit is read.
//!
//! Two more decisions change pixels and neither is obvious from the spec:
//!
//! - **The appearance is placed by fitting, not by translating.** The form's
//!   `/BBox` — mapped through the form's own `/Matrix` and re-bounded — is
//!   fitted into the annotation's `/Rect`, so a form whose `BBox` is a
//!   different size from the rect is *scaled* to it. A degenerate axis takes
//!   scale **1**, not zero, and the skew terms are forced to zero whatever
//!   `/Matrix` said.
//! - **Order is `/Annots` order.** The only sort is a stable one that lifts
//!   pop-ups above everything else, so among non-pop-ups nothing moves. Later
//!   annotations paint over earlier ones with no z-ordering of their own;
//!   `bug_1304714.in` stacks three widgets to pin exactly that.
//!
//! A pop-up is in the list and painted only while it is **open**, and the one
//! thing that opens it is the pointer entering the *parent* annotation's
//! rectangle. So a plain render draws no note cards at all, and a render
//! driven by a script that moves the mouse over an annotated passage draws
//! exactly one.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{ByteSpan, Dict, Resolve, Stream};
use pdfrum_page::{BuildContext, Page, Resources, build_form_object};

use crate::annot::appearance::{ApMode, annot_ap, annot_matrix};
use crate::annot::{AnnotList, Annotation, Subtype};
use crate::ap;
use crate::names;

/// The five stages of this pass, timed into `pdfrum-page`'s accumulator under
/// this crate's `profiling`.
///
/// A module of its own so the pass below reads as the pass rather than as the
/// instrument, and so the feature-off build names no `renderprofile` item at
/// all — the module is `pub` in `pdfrum-page` only with the feature, and a
/// crate cannot `#[cfg]` on another crate's flag.
#[cfg(feature = "profiling")]
mod profile {
    pub use pdfrum_page::renderprofile::{Stage, stage};
}

/// The feature-off twin: the stage names, and a `stage` that is its body.
#[cfg(not(feature = "profiling"))]
mod profile {
    /// The stages this pass names. Only the variants it uses, because with the
    /// feature off nothing reads them and the set exists to keep one spelling
    /// at the call sites.
    #[derive(Debug, Clone, Copy)]
    pub enum Stage {
        AnnotList,
        FormFonts,
        GenerateAppearances,
        OpenAction,
        AnnotLoop,
    }

    /// The body, unclocked.
    #[inline]
    pub fn stage<T>(_stage: Stage, body: impl FnOnce() -> T) -> T {
        body()
    }
}

use profile::{Stage, stage};

/// Appends every visible annotation's appearance to a built page.
///
/// The page's own resources are the fallback for an appearance form that
/// declares none, so a form with no `/Resources` of its own resolves names
/// against the page.
pub fn overlay<R: Resolve>(
    page: &mut Page,
    page_dict: &Dict,
    catalog: &Dict,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    overlay_with(page, page_dict, catalog, r, ctx, limits, diags, None);
}

/// The same pass, with an overlay the caller has already filled in.
///
/// `supplied` is laid over what this function generates, per
/// [`ap::AnnotOverlay::merge_over`]: wherever it has something to say about
/// an annotation the caller's entry wins, and wherever it is untouched the
/// generated one stands. Passing [`None`] is exactly [`overlay`], down to the
/// operators emitted.
///
/// This is how a live edit reaches the page. A form session holds appearances
/// for the fields it has touched — a focused field with a caret, a committed
/// value, or a field whose appearance it has cleared — and hands them here
/// rather than having them regenerated from the document, which would not
/// know about the edit.
///
/// # Keying
///
/// Both overlays are keyed by the **raw** `/Annots` index — the index into
/// the array as the file writes it, which is what `AnnotList::source_indices`
/// recovers after the list has dropped and reordered entries. A caller
/// building `supplied` must use that index and not the position an annotation
/// ended up at in the loaded list.
///
/// # Focus
///
/// `supplied` may also name the annotation that holds the keyboard focus,
/// through [`ap::AnnotOverlay::set_focus`]. That annotation is drawn
/// *without* the widget tint and with `focus_rect`'s dashed outline over
/// whatever focus box it declares — see `focus_rect` for why the two travel
/// together and why most field types declare none.
#[expect(
    clippy::too_many_arguments,
    reason = "the pass reads six independent inputs plus its two sinks; \
              bundling them into a context struct is the god-object shape \
              STYLE §1 forbids"
)]
pub fn overlay_with<R: Resolve>(
    page: &mut Page,
    page_dict: &Dict,
    catalog: &Dict,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
    supplied: Option<&ap::AnnotOverlay>,
) {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a page width beyond f32 has already lost meaning, and the \
                  value only places a synthesized pop-up, which never paints"
    )]
    let page_width = page.crop_box.width() as f32;
    let list = stage(Stage::AnnotList, || {
        AnnotList::load(page_dict, page_width, r)
    });
    let resources = Resources::for_page(page.resources.clone());
    // `CPDF_Annot`'s constructor runs `GenerateAPIfNeeded`, so an annotation
    // that arrives without a usable `/AP /N` is given one *before* anything
    // asks it to draw.
    //
    // The *text-bearing* variant, because `GenerateAPIfNeeded` reaches
    // `GenerateFreeTextAP` on the same constructor as every other generator
    // (`cpdf_generateap.cpp:1603`) — there is no second pass, and no route by
    // which a free-text annotation is described but not drawn. Taking the
    // font-less walk here painted a synthesized free-text appearance as
    // nothing at all while `--annot` reported it in full, which is exactly
    // the shape that let it survive: the tier that compares text matched.
    //
    // The fonts the form's default resources declare, loaded through the same
    // substitution the rest of the page uses. See `ap::FormFonts` for why the
    // stock Helvetica that used to stand in here was the wrong metric source.
    // Only a widget or a free-text annotation lays out text, and only an open
    // pop-up draws a card, so a page with neither never builds the faces: on
    // a fresh session the build is milliseconds, and a page with no form was
    // paying it on every cold render.
    let needs_fonts = !list.popups.is_empty()
        || list
            .annots
            .iter()
            .any(|annot| matches!(annot.subtype, Subtype::Widget | Subtype::FreeText));
    let fonts =
        needs_fonts.then(|| stage(Stage::FormFonts, || ap::FormFonts::load(catalog, r, ctx)));
    let mut generated = stage(Stage::GenerateAppearances, || {
        ap::generate_appearances_with_text(page_dict, catalog, fonts.as_deref(), r, diags)
    });
    if let Some(supplied) = supplied {
        generated.merge_over(supplied);
    }
    // `FORM_DoDocumentOpenAction` runs before the first page is rendered
    // (`pdfium_test.cc:1779`), so a `/Hide` in the catalog's open action has
    // already rewritten the flag words the visibility test below reads.
    let hidden = stage(Stage::OpenAction, || {
        crate::nav::hidden_by_open_action(catalog, r, limits, diags)
    });
    let focus = generated.focus();

    // One span over the whole loop rather than one per annotation: a page with
    // three hundred widgets would otherwise pay three hundred `Instant` pairs
    // for a bucket that is read as a total anyway, and the per-annotation
    // question is `--sample`'s.
    stage(Stage::AnnotLoop, || {
        for (slot, annot) in list.annots.iter().enumerate() {
            let flags = hidden.flags(&annot.dict, r);
            if !is_visible(annot.subtype, flags) {
                continue;
            }
            let index = list.source_indices.get(slot).copied().unwrap_or(slot);
            // A suppressed appearance draws nothing at all — not the file's
            // `/AP`, not a generated one, not the invalid-state outline below.
            // Only the widget highlight survives, because it is painted after the
            // appearance and independently of it.
            if matches!(generated.appearance(index), ap::Appearance::Suppressed) {
                push_chrome(page, annot, index, focus, r, limits, diags);
                continue;
            }
            // A checkbox or radio button whose *state's* appearance stream is
            // missing is outlined instead of drawn, and the branch replaces the
            // appearance rather than following it — see `invalid_outline`.
            if generated.get(index).is_none()
                && let Some(object) = invalid_outline(annot, r)
            {
                page.objects.push(object);
                push_chrome(page, annot, index, focus, r, limits, diags);
                continue;
            }
            // A generated appearance may also move the rectangle it draws into:
            // a text markup annotation with a generated AP is placed at its
            // quadrilaterals' bounding box rather than at its `/Rect`
            // (`CPDF_Annot::RectForDrawing`).
            let (form, placed) = if let Some(made) = generated.get(index) {
                let mut placed = annot.clone();
                placed.rect = generated.rect(index, annot.rect_for_drawing(true));
                (
                    Stream::new(ap::stream_dict(made), ByteSpan::from(made.stream.clone())),
                    placed,
                )
            } else {
                // `kNormal` in both passes, and `bFallbackToNormal` is a no-op
                // when the mode already is normal.
                let Some(form) = annot_ap(&annot.dict, ApMode::Normal, false, r) else {
                    // No appearance to draw — but a widget's highlight is painted
                    // *after* the appearance and independently of it
                    // (`CFFL_InteractiveFormFiller::OnDraw`,
                    // `cffl_interactiveformfiller.cpp:85-94`), so a field with no
                    // `/AP` at all still tints. `password.in` is nothing but two
                    // such fields.
                    push_chrome(page, annot, index, focus, r, limits, diags);
                    continue;
                };
                (form, annot.clone())
            };
            // Placed in *page* space: `render_page` composes its own page matrix
            // on top, which is the `mtUser2Device` the C++ concatenates last. So
            // an identity here is what keeps a rotated or cropped page placing
            // annotations exactly as it places content.
            let matrix = annot_matrix(&placed, &form.dict, 0, kurbo::Affine::IDENTITY, r);
            if !matrix.as_coeffs().iter().all(|c| c.is_finite()) {
                continue;
            }
            // A live edit's appearance is marked so the renderer can draw its text
            // the way the oracle does — with ClearType, which no other text on the
            // page gets. The flag rides the object because by the time anything
            // rasterizes, this form is one entry in the page's object list.
            let live_edit = supplied.is_some_and(|overlay| overlay.is_live_edit(index));
            if let Some(object) = pdfrum_page::build_form_object_with(
                &form, matrix, &resources, r, ctx, limits, diags, live_edit,
            ) {
                page.objects.push(object);
            }
            push_chrome(page, annot, index, focus, r, limits, diags);
        }
        if let Some(fonts) = &fonts {
            push_open_popup(
                page,
                &list,
                generated.hover(),
                fonts,
                &resources,
                r,
                ctx,
                limits,
                diags,
            );
        }
    });
}

/// Draws the note card belonging to the annotation the pointer is inside.
///
/// A synthesized pop-up is appended to the list *after* every annotation the
/// file declares, and the display walk is that list in order, so the card
/// paints **last** — over the page's own text and over its parent, which is
/// what makes a note legible where it overlaps the passage it annotates.
///
/// At most one card is ever open, because the pointer is in one place. The
/// hover index is a raw `/Annots` index naming the *parent*: the card itself
/// has no index to be named by, since it is not in the file.
#[expect(
    clippy::too_many_arguments,
    reason = "the same six inputs plus two sinks the pass itself carries; \
              see `overlay_with`"
)]
fn push_open_popup<R: Resolve>(
    page: &mut Page,
    list: &AnnotList,
    hover: Option<usize>,
    fonts: &ap::FormFonts,
    resources: &Resources,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    let Some(hover) = hover else {
        return;
    };
    // The hover names a raw `/Annots` index; the pop-up list is keyed by
    // position in the *loaded* list, which has dropped the file's own pop-ups.
    let Some(slot) = list.source_indices.iter().position(|&index| index == hover) else {
        return;
    };
    let Some((_, popup)) = list.popups.iter().find(|(parent, _)| *parent == slot) else {
        return;
    };
    // No `/DA` names a face, so the card takes the fallback the form fonts
    // always carry — the loaded Helvetica, whose ascent and descent are the
    // ones the wrap must be measured with.
    let Some(font) = fonts.face(b"") else {
        return;
    };
    let width = |code: u32| ap::TextFont::char_width(font, code);
    let metrics = ap::TextFont::metrics_of(font, &width);
    let encode = |code: u32| {
        ap::TextFont {
            font,
            metrics: ap::TextFont::metrics_of(font, &width),
        }
        .encode(code)
    };
    let Some(made) = ap::popup::popup(&popup.dict, &metrics, &encode, r) else {
        return;
    };
    diags.record(
        pdfrum_common::Severity::Recovered,
        pdfrum_common::DiagKind::AppearanceGenerated,
        None,
    );
    let generated = ap::GeneratedAp {
        stream: made.stream,
        bbox: popup.rect,
        matrix: kurbo::Affine::IDENTITY,
        resources: ap::resources_dict(
            ap::ext_gstate_dict(&popup.dict, false, r),
            made.font_resources,
        ),
        rect_override: None,
        as_override: None,
    };
    let form = Stream::new(
        ap::stream_dict(&generated),
        ByteSpan::from(generated.stream.clone()),
    );
    let matrix = annot_matrix(popup, &form.dict, 0, kurbo::Affine::IDENTITY, r);
    if !matrix.as_coeffs().iter().all(|c| c.is_finite()) {
        return;
    }
    if let Some(object) = build_form_object(&form, matrix, resources, r, ctx, limits, diags) {
        page.objects.push(object);
    }
}

/// Appends whichever of the two pieces of widget chrome this annotation
/// earns, after its appearance is down.
///
/// The two are **exclusive**, and that exclusivity is the whole of this
/// function. Whether a widget has a live form-field control behind it decides
/// which it gets:
///
/// - With one, the control's own appearance is drawn and the pass then ends —
///   whether the widget is not the focused one, or the focus box came back
///   empty, or the focus rectangle was stroked. **A widget being edited is
///   never tinted.**
/// - Without one, the file's appearance is drawn and the tint goes over it.
///
/// A live control exists only for a widget an event has reached, and a
/// session focuses one field at a time, so the focused annotation is the one
/// that takes the first branch. Every other annotation on the page takes the
/// second and is unaffected by focus existing at all.
fn push_chrome<R: Resolve>(
    page: &mut Page,
    annot: &Annotation,
    index: usize,
    focus: Option<ap::Focus>,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    if let Some(focus) = focus.filter(|focus| focus.annot == index) {
        if let Some(object) = focus_rect(annot, focus.box_) {
            page.objects.push(object);
        }
        return;
    }
    if let Some(object) = highlight(annot, r, limits, diags) {
        page.objects.push(object);
    }
}

/// The dashed black rectangle stroked around a focused widget's focus box.
///
/// The path is the box's four corners walked explicitly — top-left,
/// bottom-left, bottom-right, top-right, back to top-left — stroked in opaque
/// black with a one-on-one-off dash: width **1.0**, phase **0**, butt caps,
/// miter joins, all of them the stroke defaults. Nothing is filled, which is
/// [`pdfrum_page::FillRule::None`] here, the same spelling
/// [`invalid_outline`] uses for the same reason.
// The oracle's spelling of that: CFX_DrawUtils::DrawFocusRect
// (core/fxge/cfx_drawutils.cpp:16-39) strokes with a CFX_GraphStateData
// carrying nothing but set_dash_array({1.0f}); the rest is that struct's own
// defaults (cfx_graphstatedata.h:52-55). Its fill argb is 0, so the
// EvenOddOptions() beside it names a rule for a fill that never happens.
///
/// # Which widgets have a focus box at all
///
/// Most have none, and the empty answer is not an edge case — it is the
/// common one. The live control decides, and the controls disagree:
///
/// | control | focus rectangle | box |
/// |---|---|---|
/// | text field | empty | none |
/// | combo box | empty | none |
/// | list box, multi-select | the caret item's rectangle, clipped to the client area | [`ap::FocusBox::Rect`] |
/// | list box, single-select, and the check box and radio button | the widget rectangle inflated by 1 | [`ap::FocusBox::Inflated`] |
/// | push button | the widget rectangle *deflated* by the border width | [`ap::FocusBox::Rect`] |
///
/// So a focused text field draws no outline whatever, which is what the four
/// `form_textfield_focused_*` goldens carry: a caret and glyphs over plain
/// white, with neither a tint nor a dashed box. The one corpus file that does
/// stroke one is `scrollable_widgets1`, a multi-select list box, and its
/// dashes trace a **14-row band inside** the widget — the caret item — rather
/// than the widget's own edges.
///
/// A caller that has the list control's scroll and caret state names the
/// rectangle it computed; a caller that does not says [`ap::FocusBox::None`]
/// and still gets the tint suppressed, which is the half of the behaviour
/// that does not need the control.
///
/// # The page-space clip that is not applied here
///
/// The rectangle is also dropped outright when the page's `/MediaBox` does
/// not *contain* it — a containment test rather than an intersection, so a
/// box hanging one unit off the page edge is discarded whole rather than
/// clipped. That test belongs to whoever computes the rectangle, because it
/// needs the page box; this function strokes what it is given.
#[must_use]
fn focus_rect(annot: &Annotation, box_: ap::FocusBox) -> Option<pdfrum_page::PageObject> {
    let rect = match box_ {
        ap::FocusBox::None => return None,
        ap::FocusBox::Rect(rect) => rect,
        // `CFX_FloatRect::Inflate(1, 1)` then `Normalize()`, which is
        // `CPWL_Wnd::GetFocusRect` over a window rectangle that is the
        // annotation's own — `CFFL_FormField`'s window is created at
        // `GetPDFAnnotRect` and mapped back by `PWLtoFFL`.
        ap::FocusBox::Inflated => normalized(annot.rect).inflate(1.0, 1.0),
    };
    let rect = normalized(rect);
    // `CFX_FloatRect::IsEmpty` is `right <= left || top <= bottom`, so a
    // degenerate box strokes nothing and `OnDraw` returns at `:76-78`.
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let mut stroke = pdfrum_page::ColorValue::default();
    stroke.set_space(std::sync::Arc::new(pdfrum_page::ColorSpace::DeviceRgb));
    let _ = stroke.set_components(&[0.0, 0.0, 0.0]);
    let state = pdfrum_page::GraphicsState {
        stroke,
        stroke_params: pdfrum_page::StrokeParams {
            // `CFX_GraphStateData`'s own defaults, none of which
            // `DrawFocusRect` overrides.
            width: 1.0,
            dash: [1.0].into_iter().collect(),
            dash_phase: 0.0,
            ..pdfrum_page::StrokeParams::default()
        },
        ..pdfrum_page::GraphicsState::default()
    };
    Some(pdfrum_page::PageObject::Path(Box::new(
        pdfrum_page::Content {
            object: pdfrum_page::PathObject {
                path: kurbo::Shape::to_path(&rect, 0.1),
                matrix: kurbo::Affine::IDENTITY,
                // Fill argb 0 beside the black stroke: nothing is filled.
                fill_rule: pdfrum_page::FillRule::None,
                stroke: true,
            },
            state,
            marks: pdfrum_page::ContentMarks::default(),
            content_stream: None,
            // Annotation chrome is drawn into the page graph but is not page
            // content: it belongs to no `/Contents` element and must never
            // make an ordinary render count as a mutation.
            dirty: false,
            active: true,
        },
    )))
}

/// A rectangle with its corners sorted.
fn normalized(rect: kurbo::Rect) -> kurbo::Rect {
    kurbo::Rect::new(
        rect.x0.min(rect.x1),
        rect.y0.min(rect.y1),
        rect.x0.max(rect.x1),
        rect.y0.max(rect.y1),
    )
}

/// The hairline grey box drawn over a checkbox or radio button whose state
/// has no appearance stream.
///
/// # Two validity tests, not one
///
/// This is the second of two `/AP` tests that read almost the same and answer
/// differently, and keeping them apart is the whole of this function:
///
/// - **Shallow** — is there an `/AP` dictionary at all? Gates
///   *regeneration*: a widget with any `/AP` dictionary is never given a new
///   appearance, however unusable that dictionary is.
///   [`ap::widget::needs_appearance`] is this one.
/// - **Deep** — gates *this outline*. For a checkbox or radio button it
///   requires `/AP /N /<AS>` to resolve to a **stream**.
///
/// A radio button whose `/AP /N` lists only its on-state while `/AS` reads
/// `Off` passes the first and fails the second: it keeps having no appearance
/// *and* gets outlined. Porting either test alone is a measured loss, which is
/// why they landed together.
///
/// Three details are behavior rather than incident:
///
/// - **Only checkboxes and radio buttons.** Every other field type — and every
///   non-widget — falls to the ordinary appearance path. A push button with an
///   unusable `/AP` draws nothing at all.
/// - **The state is `/AS` alone.** The `/V`-and-`/Parent` fallback that
///   [`annot_ap`] performs is not consulted here, so a widget with no `/AS`
///   looks up the empty state name and fails this test even where `annot_ap`
///   would have found `Off`.
/// - **The rectangle is `/Rect`, normalized, with no border inset**, stroked
///   at line width zero — a hairline, which this engine draws as the thinnest
///   line the device has.
fn invalid_outline<R: Resolve>(annot: &Annotation, r: &R) -> Option<pdfrum_page::PageObject> {
    /// `0xAA` on every channel: the one grey this outline is stroked with.
    const OUTLINE_GREY: f32 = 0xAA_u8 as f32 / 255.0;

    if annot.subtype != Subtype::Widget {
        return None;
    }
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    let flags = crate::form::FieldFlags::from_bits(
        crate::form::attr::field_attr(&annot.dict, names::FF, r, &limits, &mut diags)
            .and_then(|value| value.as_int())
            .unwrap_or(0),
    );
    let field_type = crate::form::attr::field_attr(&annot.dict, names::FT, r, &limits, &mut diags)
        .map(|value| value.to_byte_string())
        .unwrap_or_default();
    if !matches!(
        crate::form::FieldKind::classify(&field_type, flags),
        Some(crate::form::FieldKind::Check | crate::form::FieldKind::Radio)
    ) {
        return None;
    }
    if state_appearance_resolves(&annot.dict, r) {
        return None;
    }

    let rect = normalized(annot.rect);
    let mut stroke = pdfrum_page::ColorValue::default();
    stroke.set_space(std::sync::Arc::new(pdfrum_page::ColorSpace::DeviceRgb));
    let _ = stroke.set_components(&[OUTLINE_GREY, OUTLINE_GREY, OUTLINE_GREY]);
    let state = pdfrum_page::GraphicsState {
        stroke,
        stroke_params: pdfrum_page::StrokeParams {
            // `gsd.set_line_width(0.0f)` — a hairline, not a zero-area stroke.
            width: 0.0,
            ..pdfrum_page::StrokeParams::default()
        },
        ..pdfrum_page::GraphicsState::default()
    };
    Some(pdfrum_page::PageObject::Path(Box::new(
        pdfrum_page::Content {
            object: pdfrum_page::PathObject {
                path: kurbo::Shape::to_path(&rect, 0.1),
                matrix: kurbo::Affine::IDENTITY,
                // `DrawPath` is handed fill argb **0** — fully transparent —
                // beside the grey stroke, so `EvenOddOptions()` names a rule
                // for a fill that never happens. `FillRule::None` is how this
                // engine spells that, and spelling it `EvenOdd` paints the
                // box solid instead of outlining it.
                fill_rule: pdfrum_page::FillRule::None,
                stroke: true,
            },
            state,
            marks: pdfrum_page::ContentMarks::default(),
            content_stream: None,
            // Annotation chrome is drawn into the page graph but is not page
            // content: it belongs to no `/Contents` element and must never
            // make an ordinary render count as a mutation.
            dirty: false,
            active: true,
        },
    )))
}

/// Whether `/AP /N /<AS>` resolves to a stream, with `/AS` read alone.
fn state_appearance_resolves<R: Resolve>(dict: &Dict, r: &R) -> bool {
    let Some(sub) = dict
        .dict(names::AP, r)
        .and_then(|ap| ap.get(names::N, r).map(|value| value.get().clone()))
    else {
        return false;
    };
    // A `/N` that is a stream outright is valid whatever `/AS` says; the
    // switch on field type only reaches the state lookup for a dictionary.
    let Some(states) = sub.as_dict() else {
        return matches!(sub, pdfrum_object::Object::Stream(_));
    };
    let state = dict.byte_string(names::AS, r).unwrap_or_default();
    states.stream(&pdfrum_object::Name::new(state), r).is_some()
}

/// The form-field highlight painted over every fillable widget.
///
/// This is a **host** decision, not a document one, and it is why so many
/// otherwise-correct form pages differ by a flat tint over every field: once
/// the appearance is down, the widget's `/Rect` is filled with the host's
/// highlight colour at the host's highlight alpha.
///
/// Three things about it are easy to get wrong:
///
/// - **The colour word is BGR.** `0xFFE4DD` is blue `0xFF`, green `0xE4`, red
///   `0xDD` — a pale blue, not the pink the hex reads as. Over white at
///   100/255 that is `(241, 244, 255)`, which is exactly what the goldens
///   carry.
/// - **It is a hard-edged integer rect.** Every edge is **truncated** to a
///   whole device pixel, so the tint covers `[floor(left), floor(right))`
///   with no antialiasing on any side. A `/Rect` of `[100 100 200 130]` on a
///   200-tall page tints device rows 70..99 and columns 100..199 — 30 x 100
///   pixels exactly.
/// - **It is gated on the *field's* flags, not the annotation's.** The
///   read-only test reads bit **0** of the inherited `/Ff`, where the
///   annotation's own `ReadOnly` is bit 6 of `/F`. A push button never tints,
///   and neither does a widget with no `/FT` to classify. A **signature**
///   field is excluded one level higher still: it never reaches the form
///   filler at all.
// The host's two calls are FPDF_SetFormFieldHighlightColor(form,
// FPDF_FORMFIELD_UNKNOWN, 0xFFE4DD) and FPDF_SetFormFieldHighlightAlpha(form,
// 100) (pdfium_test.cc:1776-1777); CPDFSDK_Widget::DrawShadow
// (cpdfsdk_widget.cpp:982-1006) is what paints them. The truncation is
// ToFxRect (fx_coordinates.cpp:324-327); the read-only bit is
// form_flags::kReadOnly (constants/form_flags.h:13); the /FT-less widget is
// refused by IsNeedHighLight(kUnknown) and the push button by
// IsFillingAllowed.
fn highlight<R: Resolve>(
    annot: &Annotation,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<pdfrum_page::PageObject> {
    if annot.subtype != Subtype::Widget {
        return None;
    }
    let field_type = crate::form::attr::field_attr(&annot.dict, names::FT, r, limits, diags)
        .map(|value| value.to_byte_string())
        .unwrap_or_default();
    let flags = crate::form::FieldFlags::from_bits(
        crate::form::attr::field_attr(&annot.dict, names::FF, r, limits, diags)
            .and_then(|value| value.as_int())
            .unwrap_or(0),
    );
    // `IsNeedHighLight(kUnknown)` is false, so a widget whose `/FT` names no
    // field type is not tinted at all.
    let kind = crate::form::FieldKind::classify(&field_type, flags)?;
    // A push button is refused by `IsFillingAllowed`; a signature widget
    // never reaches the form filler at all, because `CPDFSDK_Widget::OnDraw`
    // short-circuits it to `DrawAppearance` and returns
    // (`cpdfsdk_widget.cpp:719-724`). Six corpus signature files say so, four
    // of them byte-exact.
    if flags.is_read_only()
        || matches!(
            kind,
            crate::form::FieldKind::Button | crate::form::FieldKind::Signature
        )
    {
        return None;
    }
    // A signature widget never reaches the form filler at all:
    // `CPDFSDK_Widget::OnDraw` draws its appearance and *returns*
    // (`cpdfsdk_widget.cpp:719-724`), so the highlight that every other
    // fillable field gets is never painted over it. Six corpus signature
    // files say so, four of them byte-exact.
    // `CFX_FloatRect::Normalize` before `ToFxRect`: `GetRect` hands back a
    // normalized rectangle, and a `/Rect` written corner-first would
    // otherwise truncate to an empty one.
    let rect = normalized(annot.rect);
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    Some(pdfrum_page::PageObject::Path(Box::new(
        pdfrum_page::Content {
            object: pdfrum_page::PathObject {
                path: kurbo::Shape::to_path(&rect, 0.1),
                matrix: kurbo::Affine::IDENTITY,
                fill_rule: pdfrum_page::FillRule::Winding,
                stroke: false,
            },
            state: highlight_state(),
            marks: pdfrum_page::ContentMarks::default(),
            content_stream: None,
            // Annotation chrome is drawn into the page graph but is not page
            // content: it belongs to no `/Contents` element and must never
            // make an ordinary render count as a mutation.
            dirty: false,
            active: true,
        },
    )))
}

/// The graphics state the highlight rectangle fills under.
///
/// The colour is `0xFFE4DD` read as BGR, and the alpha is the 100/255 the
/// host asks for. It reaches the fill as `/ca` rather than as an alpha in the
/// colour word because that is where this engine keeps a constant alpha, and
/// the two are the same source-over multiply — `FillRect`'s `CompositeRect`
/// and an ordinary alpha fill differ in *antialiasing*, which the hard-edged
/// rectangle already settles, not in arithmetic.
fn highlight_state() -> pdfrum_page::GraphicsState {
    /// `0xFFE4DD` as an `FX_COLORREF`: blue high, then green, then red.
    const HIGHLIGHT_BGR: u32 = 0x00FF_E4DD;
    /// The host's highlight alpha, 100 of 255.
    const HIGHLIGHT_ALPHA: f32 = 100.0 / 255.0;

    #[allow(clippy::cast_precision_loss)]
    let channel = |shift: u32| ((HIGHLIGHT_BGR >> shift) & 0xff) as f32 / 255.0;
    let mut fill = pdfrum_page::ColorValue::default();
    fill.set_space(std::sync::Arc::new(pdfrum_page::ColorSpace::DeviceRgb));
    // Red is the low byte and blue the high one: `FX_COLORREF` is BGR, so
    // `0xFFE4DD` is a pale blue rather than the pink it reads as.
    let _ = fill.set_components(&[channel(0), channel(8), channel(16)]);
    pdfrum_page::GraphicsState {
        fill,
        general: pdfrum_page::GeneralState {
            fill_alpha: HIGHLIGHT_ALPHA,
            ..pdfrum_page::GeneralState::default()
        },
        ..pdfrum_page::GraphicsState::default()
    }
}

/// Whether an annotation is painted at all on a screen render.
///
/// The two passes disagree about `Invisible`, so the subtype picks which test
/// applies. Neither pass reads `Print` here, because a screen render is not
/// printing: Pass A's `Print` requirement is gated on the printing flag, and
/// Pass B has no print check at all.
// Where the two passes come from: Pass A is CPDFSDK_RenderPage building a
// CPDF_AnnotList and calling DisplayAnnots(..., bShowWidget=false), whose
// DisplayPass skips every /Widget (cpdf_annotlist.cpp:250-253). Pass B is
// FPDF_FFLDraw, which pdfium_test calls after every bitmap render with no
// flag guard (pdfium_test.cc:1045-1050); its own test is
// CPDFSDK_BAAnnot::IsVisible, which is the one that reads kInvisible.
// pdfium_test --png seeds its flags with FPDF_ANNOT unconditionally
// (pdfium_test.cc:220), which is why annotations are in the page image at
// all.
fn is_visible(subtype: Subtype, flags: crate::annot::AnnotFlags) -> bool {
    if subtype == Subtype::Popup {
        // A pop-up never draws on this walk. The file's own pop-ups are
        // dropped from the list before it, and a synthesized one is drawn
        // afterwards by `push_open_popup` — which owns the open-state test
        // this function has no way to make.
        return false;
    }
    if flags.is_hidden() || flags.no_view() {
        return false;
    }
    // `CPDFSDK_BAAnnot::IsVisible` adds `kInvisible`, and only widgets reach
    // it — Pass A never tests that bit.
    if subtype == Subtype::Widget && flags.contains(crate::annot::AnnotFlags::INVISIBLE) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{focus_rect, highlight, highlight_state, invalid_outline, is_visible, push_chrome};
    use crate::annot::{AnnotFlags, Annotation, Subtype};
    use crate::ap;
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Dict, Name, NoResolve, Object};
    use pdfrum_page::PageObject;

    fn annot(subtype: Subtype, flags: i64) -> Annotation {
        let mut annot = Annotation::read(&Dict::new(), &NoResolve);
        annot.flags = AnnotFlags::from_bits(flags);
        annot.subtype = subtype;
        annot
    }

    /// A widget over `[100 100 200 130]` with the given field type and flags.
    fn widget(field_type: &str, ff: i64) -> Annotation {
        let dict = Dict::from_pairs([
            (Name::from("Subtype"), Object::Name(Name::from("Widget"))),
            (Name::from("FT"), Object::Name(Name::from(field_type))),
            (Name::from("Ff"), Object::Int(ff)),
            (
                Name::from("Rect"),
                Object::Array(pdfrum_object::Array::of([
                    Object::Int(100),
                    Object::Int(100),
                    Object::Int(200),
                    Object::Int(130),
                ])),
            ),
        ]);
        Annotation::read(&dict, &NoResolve)
    }

    fn tinted(annot: &Annotation) -> bool {
        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        highlight(annot, &NoResolve, &limits, &mut diags).is_some()
    }

    /// `0xFFE4DD` is an `FX_COLORREF`, so it is **BGR**: a pale blue. Over
    /// white at alpha 100/255 that is `(241, 244, 255)`, which is what every
    /// form golden in the corpus carries over its fields.
    #[test]
    fn the_highlight_colour_is_bgr_and_composites_to_the_goldens_tint() {
        let state = highlight_state();
        let rgb = state.fill.to_rgb().expect("a resolved colour");
        assert_eq!(rgb.to_bytes(), [0xDD, 0xE4, 0xFF]);
        #[allow(clippy::cast_possible_truncation)]
        let alpha = (state.general.fill_alpha * 255.0) as i32;
        assert_eq!(alpha, 100);
        // The truncating `AlphaMerge` upstream composites it with.
        let over_white = |c: i32| ((255 * (255 - alpha)) + c * alpha) / 255;
        assert_eq!(
            [over_white(0xDD), over_white(0xE4), over_white(0xFF)],
            [241, 244, 255]
        );
    }

    #[test]
    fn every_fillable_field_type_is_tinted() {
        for ft in ["Tx", "Ch"] {
            assert!(tinted(&widget(ft, 0)), "{ft}");
        }
        // A check box and a radio button are both `/Btn` without bit 17.
        assert!(tinted(&widget("Btn", 0)));
        assert!(tinted(&widget("Btn", 1 << 15)), "radio");
    }

    #[test]
    fn the_three_kinds_of_field_that_are_never_tinted() {
        // A push button: `IsFillingAllowed` refuses it.
        assert!(!tinted(&widget("Btn", 1 << 16)));
        // A signature: `CPDFSDK_Widget::OnDraw` never reaches the form filler.
        assert!(!tinted(&widget("Sig", 0)));
        // A read-only field, on the *form* flag — bit 0 of `/Ff`, not the
        // annotation's own `ReadOnly` at bit 6 of `/F`.
        assert!(!tinted(&widget("Tx", 1)));
        let mut not_read_only = widget("Tx", 0);
        not_read_only.flags = AnnotFlags::from_bits(64);
        assert!(
            tinted(&not_read_only),
            "the annotation's ReadOnly bit is a different flag word"
        );
    }

    #[test]
    fn a_widget_with_no_field_type_is_not_tinted() {
        // `IsNeedHighLight(kUnknown)` is false, and a widget with no `/FT`
        // classifies to nothing.
        let bare = Dict::from_pairs([(Name::from("Subtype"), Object::Name(Name::from("Widget")))]);
        assert!(!tinted(&Annotation::read(&bare, &NoResolve)));
        // And nothing that is not a widget is ever tinted.
        assert!(!tinted(&annot(Subtype::Square, 0)));
    }

    /// A widget with the given field type, flags, `/AS` and `/AP /N` states.
    fn stateful(field_type: &str, ff: i64, as_: &str, states: &[&str]) -> Annotation {
        let normal = Dict::from_pairs(states.iter().map(|state| {
            (
                Name::from(*state),
                Object::Stream(Box::new(pdfrum_object::Stream::new(
                    Dict::new(),
                    pdfrum_object::ByteSpan::from(b"x".to_vec()),
                ))),
            )
        }));
        let mut annot = widget(field_type, ff);
        let mut dict = annot.dict.clone();
        dict.push(
            Name::from("AP"),
            Object::Dict(Dict::from_pairs([(Name::from("N"), Object::Dict(normal))])),
        );
        dict.push(Name::from("AS"), Object::Name(Name::from(as_)));
        annot.dict = dict;
        annot
    }

    fn outlined(annot: &Annotation) -> bool {
        invalid_outline(annot, &NoResolve).is_some()
    }

    #[test]
    fn a_state_with_no_stream_outlines_a_checkbox_and_a_radio() {
        // `/AS /Off` against an `/AP /N` that lists only `Yes` — the shape
        // every widget in `checkbox_radiobutton` has.
        assert!(outlined(&stateful("Btn", 0, "Off", &["Yes"])), "checkbox");
        assert!(
            outlined(&stateful("Btn", 1 << 15, "Off", &["value1"])),
            "radio"
        );
        // And a state that does resolve is drawn rather than outlined.
        assert!(!outlined(&stateful("Btn", 0, "Yes", &["Yes", "Off"])));
    }

    #[test]
    fn only_a_checkbox_or_a_radio_is_ever_outlined() {
        // The switch in `IsWidgetAppearanceValid` gives every other field type
        // the `pSub->IsStream()` arm, and `DrawAppearance`'s branch names only
        // these two anyway.
        for (ft, ff) in [("Tx", 0), ("Ch", 0), ("Btn", 1 << 16), ("Sig", 0)] {
            assert!(!outlined(&stateful(ft, ff, "Off", &["Yes"])), "{ft}");
        }
        assert!(!outlined(&annot(Subtype::Square, 0)));
    }

    #[test]
    fn the_state_is_read_from_as_alone() {
        // `GetAppState` reads `/AS` and stops — no `/V` fallback, no
        // `/Parent`. A widget with no `/AS` looks up the empty state name and
        // finds nothing, where `annot_ap` would have fallen back to `Off`.
        let with_state = stateful("Btn", 0, "Off", &["Off"]);
        assert!(!outlined(&with_state), "an `Off` stream resolves");
        let mut no_state = with_state.clone();
        no_state.dict = Dict::from_pairs(
            with_state
                .dict
                .keys()
                .filter(|key| key.as_bytes() != b"AS")
                .filter_map(|key| {
                    with_state
                        .dict
                        .get(key, &NoResolve)
                        .map(|value| (key.clone(), value.get().clone()))
                })
                .collect::<Vec<_>>(),
        );
        assert!(outlined(&no_state), "with no `/AS` nothing resolves");
    }

    #[test]
    fn the_outline_is_a_hairline_grey_stroke_and_fills_nothing() {
        let object = invalid_outline(&stateful("Btn", 0, "Off", &["Yes"]), &NoResolve)
            .expect("an invalid checkbox");
        let pdfrum_page::PageObject::Path(path) = object else {
            panic!("a path")
        };
        assert!(path.object.stroke);
        // `DrawPath` is handed fill argb 0, so nothing is filled — spelling
        // the rule `EvenOdd` here would paint the box solid.
        assert_eq!(path.object.fill_rule, pdfrum_page::FillRule::None);
        assert!(path.state.stroke_params.width.abs() < f32::EPSILON);
        assert_eq!(
            path.state
                .stroke
                .to_rgb()
                .expect("a resolved colour")
                .to_bytes(),
            [0xAA, 0xAA, 0xAA]
        );
    }

    /// `is_visible` over an annotation's own flags, which is what every test
    /// below means by it — the open-action override is exercised in
    /// `nav::open_action`.
    fn visible(subtype: Subtype, flags: i64) -> bool {
        is_visible(subtype, AnnotFlags::from_bits(flags))
    }

    #[test]
    fn hidden_and_noview_suppress_in_both_passes_and_print_does_not() {
        for subtype in [Subtype::Widget, Subtype::Square] {
            assert!(visible(subtype, 0), "{subtype:?}");
            assert!(visible(subtype, 4), "Print alone still shows");
            assert!(!visible(subtype, 2), "Hidden");
            assert!(!visible(subtype, 32), "NoView");
            assert!(!visible(subtype, 4 | 32), "NoView beats Print");
        }
    }

    #[test]
    fn invisible_suppresses_a_widget_and_only_a_widget() {
        // Pass B tests `kInvisible`; Pass A does not. A square carrying the
        // bit still draws, and a widget carrying it does not.
        assert!(!visible(Subtype::Widget, 1));
        assert!(visible(Subtype::Square, 1));
    }

    #[test]
    fn a_popup_is_never_painted() {
        assert!(!visible(Subtype::Popup, 0));
    }

    /// The chrome one annotation earns, as the loop appends it.
    fn chrome(annot: &Annotation, index: usize, focus: Option<ap::Focus>) -> Vec<PageObject> {
        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        let mut page = pdfrum_page::Page::empty();
        push_chrome(
            &mut page, annot, index, focus, &NoResolve, &limits, &mut diags,
        );
        page.objects
    }

    /// The one path object a slice of chrome holds.
    fn only_path(objects: &[PageObject]) -> &pdfrum_page::Content<pdfrum_page::PathObject> {
        match objects {
            [PageObject::Path(path)] => path,
            _ => panic!("exactly one path"),
        }
    }

    /// `CFFL_InteractiveFormFiller::OnDraw`'s live-control branch returns
    /// before `DrawShadow` through all three of its exits, so the widget
    /// being edited carries none of the tint every other fillable field does.
    /// Measured on `form_textfield_focused_ltr`: the plain golden tints all
    /// 3000 pixels of the widget and every `--send-events` golden of the same
    /// file tints none of them.
    #[test]
    fn the_focused_annotation_loses_its_tint() {
        let widget = widget("Tx", 0);
        assert_eq!(chrome(&widget, 3, None).len(), 1, "unfocused: a tint");
        assert!(
            chrome(&widget, 3, Some(ap::Focus::at(3))).is_empty(),
            "focused with no focus box: nothing at all"
        );
    }

    /// A focused text field draws no outline: its focus rectangle comes back
    /// empty and the pass ends there. All four `form_textfield_focused_*`
    /// goldens carry a caret and glyphs over plain white with no dashes
    /// anywhere.
    #[test]
    fn a_focus_box_of_none_strokes_nothing_but_still_suppresses_the_tint() {
        let widget = widget("Tx", 0);
        let focused = ap::Focus {
            annot: 0,
            box_: ap::FocusBox::None,
        };
        assert!(chrome(&widget, 0, Some(focused)).is_empty());
        assert_eq!(focus_rect(&widget, ap::FocusBox::None), None);
    }

    /// The dashed rectangle itself: opaque black, width 1, a one-unit dash
    /// array, and nothing filled.
    #[test]
    fn a_focused_widget_strokes_a_dashed_black_hairline_over_its_focus_box() {
        let widget = widget("Tx", 0);
        let box_ = kurbo::Rect::new(101.0, 402.0, 186.0, 416.0);
        let objects = chrome(
            &widget,
            0,
            Some(ap::Focus {
                annot: 0,
                box_: ap::FocusBox::Rect(box_),
            }),
        );
        let path = only_path(&objects);
        assert!(path.object.stroke);
        // Fill argb 0 beside the stroke: `EvenOddOptions()` names a rule for
        // a fill that never happens.
        assert_eq!(path.object.fill_rule, pdfrum_page::FillRule::None);
        assert_eq!(
            path.state.stroke.to_rgb().expect("a colour").to_bytes(),
            [0, 0, 0]
        );
        // `CFX_GraphStateData`'s defaults, none of which `DrawFocusRect`
        // overrides but the dash array.
        assert!((path.state.stroke_params.width - 1.0).abs() < f32::EPSILON);
        assert_eq!(path.state.stroke_params.dash.as_slice(), [1.0]);
        assert!(path.state.stroke_params.dash_phase.abs() < f32::EPSILON);
        // And it traces exactly the box it was handed, in page space.
        assert_eq!(path.object.matrix, kurbo::Affine::IDENTITY);
        assert_eq!(kurbo::Shape::bounding_box(&path.object.path), box_);
        assert!(!path.dirty, "annotation chrome is not page content");
    }

    /// `scrollable_widgets1`'s acceptance geometry, read off the oracle's own
    /// `--send-events` goldens.
    ///
    /// The file is a **multi-select list box** (`/FT /Ch`, `/Ff 2097152`)
    /// over `/Rect [100 400 200 430]` on a 300x600 page, so the widget covers
    /// device rows 170..199 and columns 100..199. Both events goldens stroke
    /// a dashed box over **rows 185..198 and columns 101..186** (the second,
    /// with a different scroll, over rows 171..184) — a 14-row band *inside*
    /// the widget, which is `CPWL_ListBox::GetFocusRect`'s caret item
    /// clipped to the client area, not the widget's own edges. Neither
    /// golden carries a single tinted pixel.
    #[test]
    fn the_list_box_focus_box_is_the_caret_item_not_the_widget_rect() {
        let mut listbox = widget("Ch", 1 << 21);
        listbox.rect = kurbo::Rect::new(100.0, 400.0, 200.0, 430.0);
        // Device row 185 on a 600-tall page is y = 415..414; the golden's
        // band is rows 185..198, so y = 401 up to y = 415.
        let caret_item = kurbo::Rect::new(101.0, 401.0, 186.0, 415.0);
        let objects = chrome(
            &listbox,
            0,
            Some(ap::Focus {
                annot: 0,
                box_: ap::FocusBox::Rect(caret_item),
            }),
        );
        let bounds = kurbo::Shape::bounding_box(&only_path(&objects).object.path);
        assert_eq!(bounds, caret_item);
        // 14 device rows tall and 85 columns wide, inside a widget that is 30
        // by 100.
        assert!((bounds.height() - 14.0).abs() < f64::EPSILON);
        assert!(bounds.width() < listbox.rect.width());
    }

    /// `CPWL_Wnd::GetFocusRect` inflates the window rectangle by one unit on
    /// every side, which for a check box or a radio button is the
    /// annotation's own rectangle grown by one.
    #[test]
    fn an_inflated_focus_box_grows_the_annotation_rect_by_one_unit() {
        let mut check = widget("Btn", 0);
        check.rect = kurbo::Rect::new(100.0, 100.0, 200.0, 130.0);
        let objects = chrome(
            &check,
            0,
            Some(ap::Focus {
                annot: 0,
                box_: ap::FocusBox::Inflated,
            }),
        );
        assert_eq!(
            kurbo::Shape::bounding_box(&only_path(&objects).object.path),
            kurbo::Rect::new(99.0, 99.0, 201.0, 131.0)
        );
        // A `/Rect` written corner-first normalizes first, so inflating it
        // grows rather than collapses it.
        let mut backwards = check.clone();
        backwards.rect = kurbo::Rect::new(200.0, 130.0, 100.0, 100.0);
        let objects = chrome(
            &backwards,
            0,
            Some(ap::Focus {
                annot: 0,
                box_: ap::FocusBox::Inflated,
            }),
        );
        assert_eq!(
            kurbo::Shape::bounding_box(&only_path(&objects).object.path),
            kurbo::Rect::new(99.0, 99.0, 201.0, 131.0)
        );
    }

    /// A degenerate box — zero wide or zero tall — strokes nothing, but the
    /// tint is still gone, because that early return is inside the
    /// live-control branch.
    #[test]
    fn an_empty_focus_box_strokes_nothing_and_does_not_bring_the_tint_back() {
        let widget = widget("Tx", 0);
        for degenerate in [
            kurbo::Rect::new(100.0, 100.0, 100.0, 130.0),
            kurbo::Rect::new(100.0, 100.0, 200.0, 100.0),
        ] {
            assert_eq!(focus_rect(&widget, ap::FocusBox::Rect(degenerate)), None);
            assert!(
                chrome(
                    &widget,
                    0,
                    Some(ap::Focus {
                        annot: 0,
                        box_: ap::FocusBox::Rect(degenerate)
                    })
                )
                .is_empty()
            );
        }
    }

    /// Focus is one index, and every other annotation on the page draws
    /// exactly what it drew before focus existed — the same objects, compared
    /// by value.
    #[test]
    fn every_index_but_the_focused_one_is_unchanged() {
        let widgets = [
            widget("Tx", 0),
            widget("Ch", 0),
            widget("Btn", 0),
            // The three that were never tinted anyway.
            widget("Btn", 1 << 16),
            widget("Sig", 0),
            widget("Tx", 1),
        ];
        let focused = ap::Focus {
            annot: 1,
            box_: ap::FocusBox::Inflated,
        };
        for (index, annot) in widgets.iter().enumerate() {
            let before = chrome(annot, index, None);
            let after = chrome(annot, index, Some(focused));
            if index == focused.annot {
                assert_ne!(before, after, "the focused index does change");
                continue;
            }
            assert_eq!(before, after, "index {index} moved");
        }
    }

    /// The `None` path is the byte-identical one: with no focus at all every
    /// annotation gets exactly the tint decision `highlight` alone makes,
    /// which is what the pass did before focus was expressible.
    #[test]
    fn no_focus_is_the_pass_as_it_was() {
        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        for annot in [
            widget("Tx", 0),
            widget("Ch", 0),
            widget("Btn", 0),
            widget("Btn", 1 << 15),
            widget("Btn", 1 << 16),
            widget("Sig", 0),
            widget("Tx", 1),
            annot(Subtype::Square, 0),
        ] {
            let expected: Vec<PageObject> = highlight(&annot, &NoResolve, &limits, &mut diags)
                .into_iter()
                .collect();
            for index in 0..3 {
                assert_eq!(chrome(&annot, index, None), expected, "index {index}");
            }
        }
    }

    /// An overlay's focus survives a merge and is not bounded by its length —
    /// a session sized for the one appearance it produced can still name the
    /// annotation that holds the focus.
    #[test]
    fn focus_merges_over_and_is_not_bounded_by_the_overlay() {
        let mut base = ap::AnnotOverlay::with_capacity(4);
        assert_eq!(base.focus(), None);
        let mut supplied = ap::AnnotOverlay::with_capacity(1);
        supplied.set_focus(ap::Focus::at(9));
        base.merge_over(&supplied);
        assert_eq!(base.focus(), Some(ap::Focus::at(9)));
        // An overlay with nothing to say about focus leaves the base's alone.
        base.merge_over(&ap::AnnotOverlay::with_capacity(4));
        assert_eq!(base.focus(), Some(ap::Focus::at(9)));
    }
}
