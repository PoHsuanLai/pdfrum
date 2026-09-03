//! The `Cascade` seam is **wired**: every hook has a caller, and refusing at
//! one changes what routing does.
//!
//! # Why this file exists
//!
//! M14 landed the seam and its script-free implementation, and every test it
//! shipped passed — including the ones that ran the cascade. None of them
//! could have caught that `commit::run` was called by nothing but its own
//! unit tests, that `route::Context` had no cascade to consult, and that
//! `Cascade::keystroke` had no call site anywhere in the crate. **A cascade
//! that changes nothing is indistinguishable from one that never runs**, and
//! `NoScripts` is exactly the cascade that changes nothing.
//!
//! So these tests assert the opposite of what the rest of the suite asserts.
//! Everywhere else the property is "with no scripts, behaviour is unchanged";
//! here it is "with a script that *does* something, behaviour changes" — and
//! each one fails if the hook it names loses its caller again.

use kurbo::Point;
use pdfrum_doc::ap;
use pdfrum_form::FormSession;
use pdfrum_form::Permissions;
use pdfrum_form::field::FieldState;
use pdfrum_form::route::{self, Context};
use pdfrum_form::{Button, Event, Modifiers};
use pdfrum_form::{Cascade, FieldRef, FieldWrites, Keystroke, KeystrokeOutcome};
use pdfrum_object::{Dict, Name, NoResolve, Object, PdfString};

// ---- the fixture: one text field on one page ----

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

fn catalog() -> Dict {
    catalog_with_fields(None)
}

/// The catalog, optionally with an `/AcroForm /Fields` array.
///
/// `fields` in **reverse** `/Annots` order is what makes the two field index
/// spaces disagree, which is the whole point of the fixture that passes one:
/// `Total` is `FieldId(1)` on the page and form field **0** in the document.
fn catalog_with_fields(fields: Option<Object>) -> Dict {
    let helv = dict([
        (b"Type", name(b"Font")),
        (b"Subtype", name(b"Type1")),
        (b"BaseFont", name(b"Helvetica")),
    ]);
    let mut acro = vec![
        (
            &b"DA"[..],
            Object::Str(PdfString::literal(b"/Helv 0 Tf 0 g")),
        ),
        (
            &b"DR"[..],
            Object::Dict(dict([(
                b"Font",
                Object::Dict(dict([(b"Helv", Object::Dict(helv))])),
            )])),
        ),
    ];
    if let Some(fields) = fields {
        acro.push((&b"Fields"[..], fields));
    }
    dict([(
        b"AcroForm",
        Object::Dict(Dict::from_pairs(
            acro.into_iter()
                .map(|(key, value)| (Name::from(key), value)),
        )),
    )])
}

/// The two field dictionaries the page and the form both name.
fn field_dicts() -> (Dict, Dict) {
    let typed = dict([
        (b"Type", name(b"Annot")),
        (b"Subtype", name(b"Widget")),
        (b"FT", name(b"Tx")),
        (b"T", Object::Str(PdfString::literal(b"Typed"))),
        (b"V", Object::Str(PdfString::literal(b"old"))),
        (b"Rect", rect(20.0, 100.0, 180.0, 130.0)),
        (b"DA", Object::Str(PdfString::literal(b"/Helv 12 Tf 0 g"))),
    ]);
    let total = dict([
        (b"Type", name(b"Annot")),
        (b"Subtype", name(b"Widget")),
        (b"FT", name(b"Tx")),
        (b"T", Object::Str(PdfString::literal(b"Total"))),
        (b"V", Object::Str(PdfString::literal(b""))),
        (b"Rect", rect(20.0, 40.0, 180.0, 70.0)),
        (b"DA", Object::Str(PdfString::literal(b"/Helv 12 Tf 0 g"))),
    ]);
    (typed, total)
}

/// Two text fields: the one events go to, and one a calculation can write.
fn two_text_fields() -> Dict {
    let (typed, total) = field_dicts();
    dict([
        (b"MediaBox", rect(0.0, 0.0, 200.0, 200.0)),
        (
            b"Annots",
            Object::Array(
                [Object::Dict(typed), Object::Dict(total)]
                    .into_iter()
                    .collect(),
            ),
        ),
    ])
}

struct Fixture {
    page: pdfrum_form::PageForm,
    catalog: Dict,
    fonts: std::sync::Arc<ap::FormFonts>,
    resolve: NoResolve,
}

impl Fixture {
    fn new() -> Fixture {
        let catalog = catalog();
        let resolve = NoResolve;
        let page = pdfrum_form::read_page(0, &two_text_fields(), &catalog, &resolve);
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

/// A click inside the first text field: move, press, release.
fn click(
    session: &mut FormSession,
    ctx: &Context<'_, NoResolve>,
    cascade: &mut dyn Cascade,
    at: Point,
) {
    for event in [
        Event::MouseMove {
            at,
            modifiers: Modifiers::NONE,
        },
        Event::MouseDown {
            button: Button::Left,
            at,
            modifiers: Modifiers::NONE,
        },
        Event::MouseUp {
            button: Button::Left,
            at,
            modifiers: Modifiers::NONE,
        },
    ] {
        route::apply(session, ctx, cascade, event);
    }
}

fn text_of(session: &FormSession, index: u32) -> Option<String> {
    match session.fields.get(&pdfrum_form::FieldId(index))? {
        FieldState::Text(state) => Some(state.edit.text.clone()),
        _ => None,
    }
}

/// The point inside the first field, and the second.
const IN_TYPED: Point = Point { x: 100.0, y: 115.0 };
const IN_TOTAL: Point = Point { x: 100.0, y: 55.0 };

// ---- keystroke: the hook with no caller at all before this ----

/// A keystroke hook that refuses drops the character.
///
/// `Cascade::keystroke` had **no call site anywhere in the crate** — not even
/// in `commit::run` — so this is the assertion that its wire exists at all.
#[test]
fn a_refusing_keystroke_hook_drops_the_character() {
    struct RefuseEverything;
    impl Cascade for RefuseEverything {
        fn keystroke(&mut self, _f: &FieldRef, _c: Keystroke) -> KeystrokeOutcome {
            KeystrokeOutcome::Reject
        }
    }

    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    let mut session = FormSession::new();
    let mut cascade = RefuseEverything;

    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    assert_eq!(text_of(&session, 0).as_deref(), Some("old"));

    let event = Event::Char {
        ch: 'X',
        modifiers: Modifiers::NONE,
    };
    let response = route::apply(&mut session, &ctx, &mut cascade, event);

    assert!(response.consumed, "a refused character is still consumed");
    assert_eq!(
        text_of(&session, 0).as_deref(),
        Some("old"),
        "the refused character must not reach the field"
    );
}

/// A keystroke hook that **rewrites** the change is answered by what it
/// returned, not by what it was offered.
#[test]
fn a_rewriting_keystroke_hook_decides_what_lands() {
    struct Shout;
    impl Cascade for Shout {
        fn keystroke(&mut self, _f: &FieldRef, change: Keystroke) -> KeystrokeOutcome {
            KeystrokeOutcome::Accept(Keystroke {
                change: change.change.to_uppercase(),
                ..change
            })
        }
    }

    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    let mut session = FormSession::new();
    let mut cascade = Shout;

    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    // Put the caret at the end so the insertion appends.
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::KeyDown {
            key: pdfrum_form::Key::End,
            modifiers: Modifiers::NONE,
        },
    );
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::Char {
            ch: 'x',
            modifiers: Modifiers::NONE,
        },
    );

    assert_eq!(
        text_of(&session, 0).as_deref(),
        Some("oldX"),
        "the hook's rewritten change is what gets applied"
    );
}

// ---- the commit gates: `commit::run` had no caller before this ----

/// A validation hook that refuses reverts the edit **and keeps the field**.
///
/// Two assertions in one, because they are one behaviour: the value goes back
/// and the caret stays, so the user can correct what the script objected to.
/// The oracle drops focus here and that is a defect —
/// `cffl_formfield.cpp:525-531` returning `true` on the revert path, against
/// pdf.js `src/scripting_api/event.js:277`'s `focus: true, // Stay in the
/// field.` See `commit`'s module documentation.
#[test]
fn a_refusing_validate_reverts_the_edit_and_keeps_the_field() {
    struct RefuseValidate;
    impl Cascade for RefuseValidate {
        fn validate(&mut self, _f: &FieldRef, _v: &str) -> bool {
            false
        }
    }

    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    let mut session = FormSession::new();
    let mut cascade = RefuseValidate;

    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::KeyDown {
            key: pdfrum_form::Key::End,
            modifiers: Modifiers::NONE,
        },
    );
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::Char {
            ch: '!',
            modifiers: Modifiers::NONE,
        },
    );
    assert_eq!(text_of(&session, 0).as_deref(), Some("old!"));
    let focused = session.focus;
    assert!(focused.is_some());

    // Leaving the field runs the commit cascade, which refuses.
    route::kill_focus(&mut session, &ctx, &mut cascade);

    assert_eq!(
        text_of(&session, 0).as_deref(),
        Some("old"),
        "a refused commit puts the value back"
    );
    assert_eq!(
        session.focus, focused,
        "[oracle-bug] …and keeps the field, so the user can correct it"
    );
}

/// The keystroke-commit gate refuses the same way, which is the other half of
/// `CommitData`'s two guards.
#[test]
fn a_refusing_commit_keystroke_gate_also_keeps_the_field() {
    struct RefuseCommit;
    impl Cascade for RefuseCommit {
        fn keystroke_commit(&mut self, _f: &FieldRef, _v: &str) -> bool {
            false
        }
    }

    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    let mut session = FormSession::new();
    let mut cascade = RefuseCommit;

    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::KeyDown {
            key: pdfrum_form::Key::End,
            modifiers: Modifiers::NONE,
        },
    );
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::Char {
            ch: '?',
            modifiers: Modifiers::NONE,
        },
    );

    route::kill_focus(&mut session, &ctx, &mut cascade);
    assert_eq!(text_of(&session, 0).as_deref(), Some("old"));
    assert!(session.focus.is_some());
}

/// An **accepted** commit lets focus go, which is the ordinary path and the
/// one every other test in the crate depends on.
#[test]
fn an_accepted_commit_lets_focus_go() {
    struct AcceptEverything;
    impl Cascade for AcceptEverything {}

    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    let mut session = FormSession::new();
    let mut cascade = AcceptEverything;

    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::Char {
            ch: 'Z',
            modifiers: Modifiers::NONE,
        },
    );
    route::kill_focus(&mut session, &ctx, &mut cascade);
    assert!(session.focus.is_none(), "an accepted commit drops focus");
}

// ---- calculate: the writes reach the other field ----

/// A calculation writes **other** fields, and its writes land.
///
/// `OnCalculate` sweeps the `/CO` list writing each field in turn
/// (`fpdfsdk/cpdfsdk_interactiveform.cpp:270-310`); one call to
/// `Cascade::calculate` is that whole sweep, and `FieldWrites` is the only
/// channel out.
#[test]
fn a_calculation_writes_the_other_field() {
    struct Calculate;
    impl Cascade for Calculate {
        fn calculate(&mut self, writes: &mut FieldWrites, _t: &FieldRef) {
            writes.set(1, "computed");
        }
    }

    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    let mut session = FormSession::new();
    let mut cascade = Calculate;

    // Touch the second field first so it has state to be written into, then
    // edit and leave the first.
    click(&mut session, &ctx, &mut cascade, IN_TOTAL);
    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::Char {
            ch: '1',
            modifiers: Modifiers::NONE,
        },
    );
    route::kill_focus(&mut session, &ctx, &mut cascade);

    assert_eq!(
        text_of(&session, 1).as_deref(),
        Some("computed"),
        "a calculation's writes must reach the fields it names"
    );
}

/// Nothing runs when the value has not moved — the first gate, and the reason
/// blurring through an untouched field costs nothing.
#[test]
fn an_unchanged_field_runs_no_hook_on_the_way_out() {
    #[derive(Default)]
    struct Counting {
        gates: u32,
    }
    impl Cascade for Counting {
        fn keystroke_commit(&mut self, _f: &FieldRef, _v: &str) -> bool {
            self.gates += 1;
            true
        }
        fn validate(&mut self, _f: &FieldRef, _v: &str) -> bool {
            self.gates += 1;
            true
        }
    }

    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    let mut session = FormSession::new();
    let mut cascade = Counting::default();

    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    route::kill_focus(&mut session, &ctx, &mut cascade);

    assert_eq!(
        cascade.gates, 0,
        "a field whose value never moved must not reach the gates"
    );
}

/// Tabbing away is a commit too: `take_focus` runs the same gates, so a
/// refusal keeps the field rather than moving on to the next one.
#[test]
fn tabbing_away_from_a_refused_field_does_not_move_focus() {
    struct RefuseValidate;
    impl Cascade for RefuseValidate {
        fn validate(&mut self, _f: &FieldRef, _v: &str) -> bool {
            false
        }
    }

    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    let mut session = FormSession::new();
    let mut cascade = RefuseValidate;

    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::Char {
            ch: 'q',
            modifiers: Modifiers::NONE,
        },
    );
    let before = session.focus;

    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::KeyDown {
            key: pdfrum_form::Key::Tab,
            modifiers: Modifiers::NONE,
        },
    );

    assert_eq!(
        session.focus, before,
        "a refused commit keeps the field even when the user asked to leave"
    );
    assert_eq!(text_of(&session, 0).as_deref(), Some("old"));
}

// ---- the two WP12 defects, closed ----

/// **A format script's output reaches the appearance.**
///
/// The defect this closes: `CommitOutcome::display` was computed and dropped.
/// `pdfrum-form` had nowhere to put it and `UpdateKind` could not carry it, so
/// a field with `AFNumber_Format(2, 0, 0, 0, "", true)` regenerated `(1234)
/// Tj` where the oracle draws `(1,234.00) Tj`.
///
/// One line upstream is the whole mechanism —
/// `pEdit->SetText(sValue.value_or(pField->GetValue()))`
/// (`fpdfsdk/cpdfsdk_appstream.cpp:1752`) — reached from `AfterValueChange`'s
/// `ResetFieldAppearance(pField, OnFormat(pField))`
/// (`fpdfsdk/cpdfsdk_interactiveform.cpp:588`).
#[test]
fn a_format_scripts_display_string_reaches_the_regenerated_appearance() {
    struct Currency;
    impl Cascade for Currency {
        fn format(&mut self, _f: &FieldRef, value: &str) -> Option<String> {
            Some(format!("${value}.00"))
        }
    }

    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    let mut session = FormSession::new();
    let mut cascade = Currency;

    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::KeyDown {
            key: pdfrum_form::Key::End,
            modifiers: Modifiers::NONE,
        },
    );
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::Char {
            ch: '9',
            modifiers: Modifiers::NONE,
        },
    );

    let response = route::kill_focus(&mut session, &ctx, &mut cascade);

    // The value stored is what was typed; the appearance shows what the
    // script said.
    assert_eq!(
        text_of(&session, 0).as_deref(),
        Some("old9"),
        "formatting must not change the stored value"
    );
    assert_eq!(
        session
            .formatted
            .get(&pdfrum_form::FieldId(0))
            .map(String::as_str),
        Some("$old9.00"),
        "the display string must survive the commit that produced it"
    );

    let stream = response
        .updates
        .iter()
        .find_map(|update| update.kind.appearance())
        .expect("the committed field regenerates an appearance");
    let text = String::from_utf8_lossy(&stream.stream);
    assert!(
        text.contains("($old9.00) Tj"),
        "the regenerated stream must draw the formatted string, not the raw \
         value — got {text}"
    );
}

/// …and re-running with **no** formatter puts the raw value back, rather than
/// leaving the last answer drawn.
///
/// `std::nullopt` reaches `sValue.value_or(pField->GetValue())` as the raw
/// value (`cpdfsdk_appstream.cpp:1752`), so a stale display string is not a
/// harmless leftover: it is an answer the document no longer gives.
#[test]
fn a_commit_with_no_formatter_erases_an_earlier_display_string() {
    /// Formats the first commit and nothing after it.
    #[derive(Default)]
    struct Once {
        formatted: bool,
    }
    impl Cascade for Once {
        fn format(&mut self, _f: &FieldRef, value: &str) -> Option<String> {
            if self.formatted {
                return None;
            }
            self.formatted = true;
            Some(format!("[{value}]"))
        }
    }

    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    let mut session = FormSession::new();
    let mut cascade = Once::default();

    let mut edit_and_leave = |session: &mut FormSession, ch: char| {
        click(session, &ctx, &mut cascade, IN_TYPED);
        route::apply(
            session,
            &ctx,
            &mut cascade,
            Event::KeyDown {
                key: pdfrum_form::Key::End,
                modifiers: Modifiers::NONE,
            },
        );
        route::apply(
            session,
            &ctx,
            &mut cascade,
            Event::Char {
                ch,
                modifiers: Modifiers::NONE,
            },
        );
        route::kill_focus(session, &ctx, &mut cascade);
    };

    edit_and_leave(&mut session, 'a');
    assert_eq!(
        session
            .formatted
            .get(&pdfrum_form::FieldId(0))
            .map(String::as_str),
        Some("[olda]")
    );

    edit_and_leave(&mut session, 'b');
    assert_eq!(
        session.formatted.get(&pdfrum_form::FieldId(0)),
        None,
        "a commit whose formatter answered nothing must erase the old answer"
    );
}

/// A commit that **never ran** says nothing about what the field shows.
///
/// The distinction `CommitOutcome::formats` exists for: blurring through an
/// untouched field runs no hook at all (`AfterValueChange` is provoked by a
/// *change*), so it must not be read as "no formatter, draw the raw value".
#[test]
fn blurring_through_an_unchanged_field_keeps_its_display_string() {
    struct Currency;
    impl Cascade for Currency {
        fn format(&mut self, _f: &FieldRef, value: &str) -> Option<String> {
            Some(format!("${value}"))
        }
    }

    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    let mut session = FormSession::new();
    let mut cascade = Currency;

    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::Char {
            ch: 'q',
            modifiers: Modifiers::NONE,
        },
    );
    route::kill_focus(&mut session, &ctx, &mut cascade);
    let after_edit = session.formatted.get(&pdfrum_form::FieldId(0)).cloned();
    assert!(after_edit.is_some(), "the edit produced a display string");

    // In and straight out again, changing nothing.
    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    route::kill_focus(&mut session, &ctx, &mut cascade);

    assert_eq!(
        session.formatted.get(&pdfrum_form::FieldId(0)).cloned(),
        after_edit,
        "a commit that ran no hooks must leave the display string alone"
    );
}

/// A **focused** field draws the raw value, not the formatted one.
///
/// `CFFL_FormField`'s editor is seeded from `GetValue()`, so a user who
/// clicks into a currency field edits `1234` and not `1,234.00`.
#[test]
fn a_focused_field_edits_the_raw_value_and_not_the_display_string() {
    struct Currency;
    impl Cascade for Currency {
        fn format(&mut self, _f: &FieldRef, value: &str) -> Option<String> {
            Some(format!("${value}.00"))
        }
    }

    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    let mut session = FormSession::new();
    let mut cascade = Currency;

    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::Char {
            ch: '5',
            modifiers: Modifiers::NONE,
        },
    );
    route::kill_focus(&mut session, &ctx, &mut cascade);
    assert!(session.formatted.contains_key(&pdfrum_form::FieldId(0)));

    // Click back in. The live edit shows what is stored.
    let response = click_reporting(&mut session, &ctx, &mut cascade, IN_TYPED);
    let live = response
        .updates
        .iter()
        .rev()
        .find(|update| update.kind.is_live_edit())
        .and_then(|update| update.kind.appearance())
        .expect("a focused field draws its live editor state");
    let text = String::from_utf8_lossy(&live.stream);
    assert!(
        !text.contains('$'),
        "the caret edits the stored value, never the formatted one — got {text}"
    );
}

/// [`click`], keeping the last response.
fn click_reporting(
    session: &mut FormSession,
    ctx: &Context<'_, NoResolve>,
    cascade: &mut dyn Cascade,
    at: Point,
) -> pdfrum_form::Response {
    let mut last = pdfrum_form::Response::ignored();
    for event in [
        Event::MouseMove {
            at,
            modifiers: Modifiers::NONE,
        },
        Event::MouseDown {
            button: Button::Left,
            at,
            modifiers: Modifiers::NONE,
        },
        Event::MouseUp {
            button: Button::Left,
            at,
            modifiers: Modifiers::NONE,
        },
    ] {
        last = route::apply(session, ctx, cascade, event);
    }
    last
}

/// **A calculation names its target in the document's field space, not the
/// page's.**
///
/// The defect this closes: `FieldRef::index` conflated a page-local widget id
/// with a `/Fields` position. `page::read` allocates a `FieldId` per page as
/// that page's `/Annots` are walked, `Form::calculation_order` answers
/// positions in the document's flat terminal-field list, and
/// `route::commit_field` spent a calculation's writes as `FieldId(index)` —
/// which is the same number only for a single-page form whose widgets happen
/// to appear in `/Fields` order.
///
/// The fixture makes them disagree by one transposition: `/Annots` is
/// `[Typed, Total]` and `/Fields` is `[Total, Typed]`. So `Total` is
/// `FieldId(1)` on the page and form field **0** in the document, and a
/// calculation writing form field 0 must reach `Total` — where the old
/// spelling would have written `Typed`, silently overwriting the field the
/// user had just edited.
#[test]
fn a_calculation_writes_the_field_the_document_names_not_the_page() {
    struct WriteFormFieldZero;
    impl Cascade for WriteFormFieldZero {
        fn calculate(&mut self, writes: &mut FieldWrites, _t: &FieldRef) {
            writes.set(0, "computed");
        }
    }

    let (typed, total) = field_dicts();
    // `/Fields` in the opposite order from `/Annots`.
    let catalog = catalog_with_fields(Some(Object::Array(
        [Object::Dict(total), Object::Dict(typed)]
            .into_iter()
            .collect(),
    )));
    let resolve = NoResolve;
    let page = pdfrum_form::read_page(0, &two_text_fields(), &catalog, &resolve);
    let mut build = pdfrum_page::BuildContext::new();
    let fonts = ap::FormFonts::load(&catalog, &resolve, &mut build);
    let ctx = Context {
        page: &page,
        catalog: &catalog,
        resolve: &resolve,
        fonts: &fonts,
        permissions: Permissions::ALL,
    };

    // The two spaces really do disagree in this fixture, which is what makes
    // the assertion below able to fail.
    let widgets = &ctx.page.widgets;
    assert_eq!(widgets[0].name, "Typed");
    assert_eq!(widgets[0].field, pdfrum_form::FieldId(0));
    assert_eq!(widgets[0].field_index, Some(1), "Typed is form field 1");
    assert_eq!(widgets[1].name, "Total");
    assert_eq!(widgets[1].field, pdfrum_form::FieldId(1));
    assert_eq!(widgets[1].field_index, Some(0), "Total is form field 0");

    let mut session = FormSession::new();
    let mut cascade = WriteFormFieldZero;
    click(&mut session, &ctx, &mut cascade, IN_TOTAL);
    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::Char {
            ch: '1',
            modifiers: Modifiers::NONE,
        },
    );
    route::kill_focus(&mut session, &ctx, &mut cascade);

    assert_eq!(
        text_of(&session, 1).as_deref(),
        Some("computed"),
        "form field 0 is `Total`, which this page holds as FieldId(1)"
    );
    assert_ne!(
        text_of(&session, 0).as_deref(),
        Some("computed"),
        "the page-local id must not be spent as a document-wide one"
    );
}

/// …and the hook that runs is the one the *document* installed for that
/// field, which is the other half of the same conflation.
///
/// `ScriptCascade::script_for` looks a field's `/AA` entries up by
/// `FieldRef::index`, and the facade installs them under the same number, so
/// the two spaces disagreeing would run one field's format script against
/// another's value.
#[test]
fn a_field_ref_carries_the_documents_field_position() {
    #[derive(Default)]
    struct Recording {
        seen: Vec<(String, Option<u32>)>,
    }
    impl Cascade for Recording {
        fn validate(&mut self, field: &FieldRef, _v: &str) -> bool {
            self.seen.push((field.name.clone(), field.index));
            true
        }
    }

    let (typed, total) = field_dicts();
    let catalog = catalog_with_fields(Some(Object::Array(
        [Object::Dict(total), Object::Dict(typed)]
            .into_iter()
            .collect(),
    )));
    let resolve = NoResolve;
    let page = pdfrum_form::read_page(0, &two_text_fields(), &catalog, &resolve);
    let mut build = pdfrum_page::BuildContext::new();
    let fonts = ap::FormFonts::load(&catalog, &resolve, &mut build);
    let ctx = Context {
        page: &page,
        catalog: &catalog,
        resolve: &resolve,
        fonts: &fonts,
        permissions: Permissions::ALL,
    };

    let mut session = FormSession::new();
    let mut cascade = Recording::default();
    click(&mut session, &ctx, &mut cascade, IN_TYPED);
    route::apply(
        &mut session,
        &ctx,
        &mut cascade,
        Event::Char {
            ch: 'Z',
            modifiers: Modifiers::NONE,
        },
    );
    route::kill_focus(&mut session, &ctx, &mut cascade);

    assert_eq!(
        cascade.seen,
        vec![("Typed".to_string(), Some(1))],
        "the gate must be told the document's field position, not the page's"
    );
}

/// A widget the form's field list does not reach has **no** document-wide
/// position, and says so rather than inventing one.
///
/// The oracle's answer too: `GetFieldByDict` returns null for it and
/// `CountFields` never counted it, so a script cannot name it either.
#[test]
fn a_widget_outside_the_forms_field_list_has_no_document_position() {
    let fixture = Fixture::new();
    // `Fixture`'s catalog has no `/Fields` at all.
    for widget in &fixture.ctx().page.widgets {
        assert_eq!(
            widget.field_index, None,
            "a form that lists no fields gives none of them a position"
        );
    }
}
