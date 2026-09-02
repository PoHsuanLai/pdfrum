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
use pdfrum_form::cascade::{Cascade, FieldRef, FieldWrites, Keystroke, KeystrokeOutcome};
use pdfrum_form::event::{Button, Event, Modifiers};
use pdfrum_form::field::FieldState;
use pdfrum_form::hit::Permissions;
use pdfrum_form::route::{self, Context};
use pdfrum_form::session::FormSession;
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

/// Two text fields: the one events go to, and one a calculation can write.
fn two_text_fields() -> Dict {
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
        let page = pdfrum_form::page::read(0, &two_text_fields(), &catalog, &resolve);
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
    match session.fields.get(&pdfrum_form::session::FieldId(index))? {
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
            key: pdfrum_form::event::Key::End,
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
            key: pdfrum_form::event::Key::End,
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
            key: pdfrum_form::event::Key::End,
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
            key: pdfrum_form::event::Key::Tab,
            modifiers: Modifiers::NONE,
        },
    );

    assert_eq!(
        session.focus, before,
        "a refused commit keeps the field even when the user asked to leave"
    );
    assert_eq!(text_of(&session, 0).as_deref(), Some("old"));
}
