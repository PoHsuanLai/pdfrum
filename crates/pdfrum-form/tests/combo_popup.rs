//! The open combo-box dropdown, driven through `route::apply` on the two
//! fixtures whose goldens carry one.
//!
//! # Why this file exists
//!
//! `bug_736695_3.in#form-events` was recorded in `docs/status/M14.md` as a
//! **false pass**: 0.997003, above the 0.99 floor, on an image where the
//! oracle had selected `Spain` and pdfrum had selected nothing and dropped
//! focus. The whole disagreement was a 150×15 box, which SSIM over a
//! 595×342 page cannot resolve — so the metric said pass while the state was
//! wrong in the one way the script was written to check.
//!
//! Every geometric number below is the fixture's own, read from
//! `testing/resources/pixel/bug_736695_4.pdf` and
//! `testing/resources/pixel/bug_1372651.pdf`, and every click coordinate is a
//! line of the matching `.evt`. So these assert the *state* the goldens
//! depict, without a rasterizer in the way.

use pdfrum_doc::ap;
use pdfrum_form::NoScripts;
use pdfrum_form::event::{Button, Event, Modifiers, Point};
use pdfrum_form::field::FieldState;
use pdfrum_form::hit::Permissions;
use pdfrum_form::route::{self, Context};
use pdfrum_form::session::FormSession;
use pdfrum_object::{Dict, Name, NoResolve, Object, PdfString};

/// A dictionary from `(name, object)` pairs, spelled once.
fn dict<const N: usize>(pairs: [(&'static [u8], Object); N]) -> Dict {
    Dict::from_pairs(
        pairs
            .into_iter()
            .map(|(key, value)| (Name::from(key), value)),
    )
}

fn name(bytes: &'static [u8]) -> Object {
    Object::Name(Name::from(bytes))
}

fn rect(left: f32, bottom: f32, right: f32, top: f32) -> Object {
    Object::Array(
        [left, bottom, right, top]
            .into_iter()
            .map(Object::Real)
            .collect(),
    )
}

fn options(labels: &[&str]) -> Object {
    Object::Array(
        labels
            .iter()
            .map(|label| Object::Str(PdfString::literal(label.as_bytes())))
            .collect(),
    )
}

/// A catalog whose `/AcroForm /DR /Font /Helv` is the bare Helvetica the two
/// fixtures declare, which is what makes a row 13.392 units tall at the
/// automatic twelve points.
fn catalog() -> Dict {
    let helv = dict([
        (b"Type", name(b"Font")),
        (b"Subtype", name(b"Type1")),
        (b"BaseFont", name(b"Helvetica")),
    ]);
    dict([(
        b"AcroForm",
        Object::Dict(dict([
            (b"DA", Object::Str(PdfString::literal(b"/Helv 0 Tf 0 g"))),
            (
                b"DR",
                Object::Dict(dict([(
                    b"Font",
                    Object::Dict(dict([(b"Helv", Object::Dict(helv))])),
                )])),
            ),
        ])),
    )])
}

/// `bug_736695_4.pdf`'s only widget: an **editable** combo box
/// (`/Ff 393216` = combo | edit) with two options and no stored value, at
/// `/Rect [165.7 315.9 315.7 330.1]` on a 595×342 page.
fn spain_and_sweden() -> Dict {
    let widget = dict([
        (b"Type", name(b"Annot")),
        (b"Subtype", name(b"Widget")),
        (b"FT", name(b"Ch")),
        (b"Ff", Object::Int(393_216)),
        (b"T", Object::Str(PdfString::literal(b"Country Box"))),
        (b"Opt", options(&["Spain", "Sweden"])),
        (b"Rect", rect(165.7, 315.9, 315.7, 330.1)),
        (b"DA", Object::Str(PdfString::literal(b"/Helv 0 Tf 0 g"))),
    ]);
    dict([
        (b"MediaBox", rect(0.0, 0.0, 595.0, 342.0)),
        (
            b"Annots",
            Object::Array([Object::Dict(widget)].into_iter().collect()),
        ),
    ])
}

/// `bug_1372651.pdf`'s dropdown: a **gated** combo (`/Ff 131072`) of three
/// items with `Item3` stored and selected, at `/Rect [70 135 150 155]` on a
/// 200×200 page. Its sibling push button is included because the fixture has
/// one and a click that misses the list must be able to land on it.
fn three_items() -> Dict {
    let combo = dict([
        (b"Type", name(b"Annot")),
        (b"Subtype", name(b"Widget")),
        (b"FT", name(b"Ch")),
        (b"Ff", Object::Int(131_072)),
        (b"T", Object::Str(PdfString::literal(b"Dropdown"))),
        (b"Opt", options(&["Item1", "Item2", "Item3"])),
        (b"V", Object::Str(PdfString::literal(b"Item3"))),
        (b"I", Object::Array([Object::Int(2)].into_iter().collect())),
        (b"Rect", rect(70.0, 135.0, 150.0, 155.0)),
        (
            b"MK",
            Object::Dict(dict([(
                b"BG",
                Object::Array([Object::Real(1.0)].into_iter().collect()),
            )])),
        ),
    ]);
    let button = dict([
        (b"Type", name(b"Annot")),
        (b"Subtype", name(b"Widget")),
        (b"FT", name(b"Btn")),
        (b"Ff", Object::Int(65_536)),
        (b"T", Object::Str(PdfString::literal(b"Button"))),
        (b"Rect", rect(50.0, 90.0, 150.0, 110.0)),
    ]);
    dict([
        (b"MediaBox", rect(0.0, 0.0, 200.0, 200.0)),
        (
            b"Annots",
            Object::Array(
                [Object::Dict(combo), Object::Dict(button)]
                    .into_iter()
                    .collect(),
            ),
        ),
    ])
}

/// Everything a `Context` borrows, kept alive together.
struct Fixture {
    page: pdfrum_form::PageForm,
    catalog: Dict,
    fonts: ap::FormFonts,
    resolve: NoResolve,
}

impl Fixture {
    fn new(page_dict: &Dict) -> Fixture {
        let catalog = catalog();
        let resolve = NoResolve;
        let page = pdfrum_form::page::read(0, page_dict, &catalog, &resolve);
        let mut build = pdfrum_page::BuildContext::new();
        let fonts = ap::FormFonts::load(&catalog, &resolve, &mut build);
        Fixture {
            page,
            catalog,
            fonts,
            resolve,
        }
    }

    fn ctx(&self) -> Context<'_, NoResolve> {
        Context {
            page: &self.page,
            catalog: &self.catalog,
            resolve: &self.resolve,
            fonts: &self.fonts,
            permissions: Permissions::ALL,
        }
    }
}

/// The three lines of `bug_736695_2.evt` / the first three of `_3.evt`: move
/// onto the drop button, press, release.
fn open_the_dropdown(session: &mut FormSession, ctx: &Context<'_, NoResolve>, x: f32, y: f32) {
    let at = Point { x, y };
    route::apply(
        session,
        ctx,
        &mut NoScripts,
        Event::MouseMove {
            at,
            modifiers: Modifiers::NONE,
        },
    );
    route::apply(
        session,
        ctx,
        &mut NoScripts,
        Event::MouseDown {
            button: Button::Left,
            at,
            modifiers: Modifiers::NONE,
        },
    );
    route::apply(
        session,
        ctx,
        &mut NoScripts,
        Event::MouseUp {
            button: Button::Left,
            at,
            modifiers: Modifiers::NONE,
        },
    );
}

/// The state of the one choice field on the page.
///
/// A default rather than a panic, so the helper carries no `expect` of its
/// own: an assertion against a defaulted state fails just as loudly, and with
/// a message about the property under test rather than about the lookup.
fn choice(session: &FormSession) -> pdfrum_form::ChoiceState {
    session
        .fields
        .values()
        .find_map(|state| match state {
            FieldState::Choice(choice) => Some(choice.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// `bug_736695_2.evt`: a click on the drop button opens the list, and the
/// list hangs below the widget because that is where the room is.
#[test]
fn a_click_on_the_drop_button_opens_the_list_below() {
    let page = spain_and_sweden();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 312.0, 324.0);

    assert!(
        choice(&session).popup_open,
        "the drop button opens the list"
    );
    let view = route::popup_view(&session, &ctx).expect("an open list has a view");
    assert_eq!(view.geometry.placement, pdfrum_form::Placement::Below);
    assert_eq!(view.options.len(), 2);
    assert_eq!(view.options[0], "Spain");
    assert_eq!(view.options[1], "Sweden");
    assert_eq!(view.selected, None, "nothing is selected until a row is");
    assert_eq!(view.label_at(0), Some("Spain"));
    assert_eq!(view.label_at(1), Some("Sweden"));
    assert_eq!(view.label_at(2), None, "there is no third row to draw");
    assert!(
        !view.is_banded(0) && !view.is_banded(1),
        "an untouched list bands nothing"
    );
    // Two rows plus a one-unit border on each side, hanging from the
    // widget's own bottom edge — which is what the golden's rows 26..54
    // measure back to. The row height is the *substituted* face's laid-out
    // line, so it is read from the view rather than written down: under the
    // determinism recipe `/Helvetica` resolves to Arimo at 13.392 and the
    // popup is the golden's 28.784, while this test's bare base-14 metrics
    // give 11.244 and 24.488. The **relationship** is the assertion; the
    // absolute number belongs to the conformance board.
    assert!(
        (view.geometry.rect.top - 315.9).abs() < 1e-3,
        "{:?}",
        view.geometry.rect
    );
    let height = view.geometry.rect.top - view.geometry.rect.bottom;
    assert!(
        (height - (2.0 * view.geometry.row_height + 2.0)).abs() < 1e-3,
        "height {height} against two rows of {}",
        view.geometry.row_height
    );
}

/// **The routing gap.** `bug_736695_3.evt` opens the list and then clicks
/// `(312, 310)`, which is page y 310 — *below* the widget's `/Rect` and
/// inside the first row of the list. The oracle selects `Spain`; before this
/// slice the same click was a miss that killed focus, and the row still
/// "passed" at 0.997003 because the difference is a 150×15 box.
#[test]
fn a_click_inside_the_open_list_selects_that_row() {
    let page = spain_and_sweden();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 312.0, 324.0);
    open_the_dropdown(&mut session, &ctx, 312.0, 310.0);

    let choice = choice(&session);
    assert_eq!(
        choice.selected.iter().copied().collect::<Vec<_>>(),
        vec![0],
        "the click at (312, 310) is on `Spain`, not a miss"
    );
    assert_eq!(choice.focused_text(), "Spain");
    assert!(!choice.popup_open, "choosing a row shuts the list");
    assert!(
        session.focus.is_some(),
        "the click was inside the control, so focus survives it — \
         the old miss path killed it"
    );
}

/// `bug_736695_4.evt`: open, hover a row, click far off. The dismissal must
/// leave the stored selection **alone** — hover-select is not selection —
/// which is why the golden for `_4` is byte-identical to the plain render.
#[test]
fn dismissing_without_choosing_changes_nothing() {
    let page = spain_and_sweden();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 312.0, 324.0);
    route::apply(
        &mut session,
        &ctx,
        &mut NoScripts,
        Event::MouseMove {
            at: Point { x: 312.0, y: 310.0 },
            modifiers: Modifiers::NONE,
        },
    );
    assert_eq!(
        choice(&session).hovered,
        Some(0),
        "the list is hover-select, so a move over a row marks it"
    );

    open_the_dropdown(&mut session, &ctx, 6.0, 6.0);

    let choice = choice(&session);
    assert!(!choice.popup_open, "a click off the control shuts the list");
    assert!(
        choice.selected.is_empty(),
        "a hovered row that was never clicked is not a selection"
    );
    assert_eq!(choice.hovered, None);
    assert!(session.focus.is_none(), "a click on nothing drops focus");
}

/// A second press on the drop button shuts the list it opened —
/// `NotifyLButtonDown` is `SetPopup(!is_popup_)`, a toggle.
#[test]
fn the_drop_button_toggles() {
    let page = spain_and_sweden();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 312.0, 324.0);
    assert!(choice(&session).popup_open);
    open_the_dropdown(&mut session, &ctx, 312.0, 324.0);
    assert!(!choice(&session).popup_open, "a second click shuts it");
}

/// A click in the combo's **text** half is not a click on the button, so it
/// leaves the list alone. `RepositionChildWnd` gives the button the right
/// thirteen units of the client rectangle and the edit everything left of it.
#[test]
fn a_click_in_the_text_half_does_not_open_the_list() {
    let page = spain_and_sweden();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 200.0, 324.0);
    assert!(
        !choice(&session).popup_open,
        "only the drop button opens the list"
    );
    assert!(session.focus.is_some(), "but the click still focuses");
}

/// `bug_1372651.evt`: click `(140, 145)`, which is the drop button of the
/// three-item gated combo. The list opens downward at three rows plus its
/// border, and `Item3` — the file's `/V` — is already the selected row.
#[test]
fn the_three_item_dropdown_opens_with_its_stored_row_selected() {
    let page = three_items();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 140.0, 145.0);

    let view = route::popup_view(&session, &ctx).expect("an open list has a view");
    assert_eq!(view.geometry.placement, pdfrum_form::Placement::Below);
    assert_eq!(view.selected, Some(2), "`/V (Item3)` is row two");
    assert_eq!(
        view.top_visible, 0,
        "three rows all fit, so nothing scrolls"
    );
    assert_eq!(
        view.edit_text, None,
        "a gated combo has no text half to report"
    );
    let height = view.geometry.rect.top - view.geometry.rect.bottom;
    assert!(
        (height - (3.0 * view.geometry.row_height + 2.0)).abs() < 1e-3,
        "height {height} against three rows of {}",
        view.geometry.row_height
    );
    assert!((view.geometry.rect.top - 135.0).abs() < 1e-3);
}

/// The three-item list's rows are where the golden draws them: `Item1` at the
/// top of the plate, `Item3`'s navy band at the bottom.
#[test]
fn the_rows_stack_from_the_top_of_the_plate() {
    let page = three_items();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 140.0, 145.0);
    let view = route::popup_view(&session, &ctx).expect("an open list has a view");

    let row = view.geometry.row_height;
    let first = view.geometry.row_rect(0);
    assert!((first.top - 134.0).abs() < 1e-3, "{first:?}");
    assert!((first.bottom - (134.0 - row)).abs() < 1e-3, "{first:?}");
    // The golden's navy band for `Item3` sits at the bottom of the plate,
    // which is the third row's box: the stack is exact, not merely ordered.
    let third = view.geometry.row_rect(2);
    assert!((third.top - (134.0 - 2.0 * row)).abs() < 1e-3, "{third:?}");
    assert!(
        (third.bottom - view.geometry.plate().bottom).abs() < 1e-3,
        "the last of three rows ends exactly at the plate's floor: {third:?}"
    );
}

/// A row chosen from the list of a **gated** combo becomes the field's
/// focused text; the list shuts on the release.
#[test]
fn choosing_the_second_row_selects_it() {
    let page = three_items();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 140.0, 145.0);
    // The middle of the second row, computed from the geometry the session
    // itself published rather than from a row height written down here.
    let second = route::popup_view(&session, &ctx)
        .expect("an open list has a view")
        .geometry
        .row_rect(1);
    open_the_dropdown(
        &mut session,
        &ctx,
        100.0,
        f32::midpoint(second.top, second.bottom),
    );

    let choice = choice(&session);
    assert_eq!(choice.selected.iter().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(choice.focused_text(), "Item2");
    assert!(!choice.popup_open);
}

/// The **intent** entry points: a host that drew the list from `popup_view`
/// reports a choice without synthesizing a click, and gets the same state a
/// click would have produced.
#[test]
fn the_host_can_report_a_choice_without_a_click() {
    let page = three_items();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 140.0, 145.0);
    let annot = route::popup_view(&session, &ctx)
        .expect("an open list has a view")
        .annot;

    let response = route::choose(&mut session, &ctx, &mut NoScripts, annot, 0);
    assert!(response.consumed);
    let choice = choice(&session);
    assert_eq!(choice.selected.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(choice.focused_text(), "Item1");
    assert!(!choice.popup_open, "choosing shuts the list");
    assert!(route::popup_view(&session, &ctx).is_none());
}

/// An out-of-range index from a host is ignored rather than corrupting the
/// field, and the list stays open.
#[test]
fn a_choice_past_the_end_is_ignored() {
    let page = three_items();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 140.0, 145.0);
    let annot = route::popup_view(&session, &ctx)
        .expect("an open list has a view")
        .annot;

    let response = route::choose(&mut session, &ctx, &mut NoScripts, annot, 9);
    assert!(!response.consumed);
    assert_eq!(
        choice(&session)
            .selected
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![2],
        "the stored row is untouched"
    );
    assert!(choice(&session).popup_open);
}

/// `close_popup` is `SetPopup(false)` and nothing more, and is safe to call
/// on an annotation whose list is already shut.
#[test]
fn the_host_can_dismiss_the_list() {
    let page = three_items();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 140.0, 145.0);
    let annot = route::popup_view(&session, &ctx)
        .expect("an open list has a view")
        .annot;

    assert!(route::close_popup(&mut session, &ctx, annot).consumed);
    assert!(!choice(&session).popup_open);
    assert_eq!(
        choice(&session)
            .selected
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![2],
        "dismissal leaves the selection alone"
    );
    assert!(
        !route::close_popup(&mut session, &ctx, annot).consumed,
        "a second dismissal has nothing to do"
    );
}

/// Focus leaving the field shuts the list —
/// `CPWL_ComboBox::KillFocus` runs `SetPopup(false)` before the base class.
#[test]
fn losing_focus_shuts_the_list() {
    let page = three_items();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 140.0, 145.0);
    assert!(choice(&session).popup_open);

    route::kill_focus(&mut session, &ctx, &mut NoScripts);
    assert!(!choice(&session).popup_open);
    assert!(route::popup_view(&session, &ctx).is_none());
}

/// `Return` toggles the list on an **editable** combo, and a second one shuts
/// it — `CPWL_ComboBox::OnChar`'s `kReturn` case, which takes no editability
/// branch.
#[test]
fn return_toggles_the_list_on_an_editable_combo() {
    let page = spain_and_sweden();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    // Focus the field without touching the button.
    open_the_dropdown(&mut session, &ctx, 200.0, 324.0);
    assert!(!choice(&session).popup_open);

    let ret = Event::Char {
        ch: '\r',
        modifiers: Modifiers::NONE,
    };
    assert!(route::apply(&mut session, &ctx, &mut NoScripts, ret).consumed);
    assert!(choice(&session).popup_open, "Return opens it");
    assert!(route::apply(&mut session, &ctx, &mut NoScripts, ret).consumed);
    assert!(!choice(&session).popup_open, "a second Return shuts it");
}

/// `Space` opens a **gated** combo's list and — unlike `Return` — never shuts
/// it: `OnChar`'s `kSpace` case runs `SetPopup(true)` only when the list is
/// already closed, and returns `true` either way.
#[test]
fn space_opens_a_gated_combo_and_never_shuts_it() {
    let page = three_items();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    // A click in the box's text half focuses it and leaves the list shut.
    open_the_dropdown(&mut session, &ctx, 100.0, 145.0);
    assert!(!choice(&session).popup_open);

    let space = Event::Char {
        ch: ' ',
        modifiers: Modifiers::NONE,
    };
    assert!(route::apply(&mut session, &ctx, &mut NoScripts, space).consumed);
    assert!(choice(&session).popup_open, "Space opens it");
    assert!(route::apply(&mut session, &ctx, &mut NoScripts, space).consumed);
    assert!(
        choice(&session).popup_open,
        "a second Space leaves it open — this is the asymmetry with Return"
    );
}

/// `scroll_view` answers for a closed combo too, because a scroll bar is host
/// chrome whether or not a dropdown is involved.
#[test]
fn a_scroll_view_reports_rows_whether_or_not_the_list_is_open() {
    let page = three_items();
    let fixture = Fixture::new(&page);
    let ctx = fixture.ctx();
    let mut session = FormSession::default();

    open_the_dropdown(&mut session, &ctx, 100.0, 145.0);
    let annot = fixture.page.widgets[0].id;

    let closed = route::scroll_view(&session, &ctx, annot).expect("a choice widget has one");
    assert_eq!(closed.total, 3);
    assert_eq!(closed.top_visible, 0);

    open_the_dropdown(&mut session, &ctx, 140.0, 145.0);
    let open = route::scroll_view(&session, &ctx, annot).expect("still a choice widget");
    assert_eq!(open.total, 3);
    assert!(
        !open.is_scrollable(),
        "three rows in a three-row popup do not scroll"
    );
}
