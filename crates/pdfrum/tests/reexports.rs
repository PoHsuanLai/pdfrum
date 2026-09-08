//! Every type a `pdfrum` signature names is nameable from `pdfrum` alone.
//!
//! This is the gate that installs that rule, and the reason it is a
//! **separate integration test** rather than a `#[cfg(test)]`
//! module inside `lib.rs`: a unit test compiles with the crate, so it can see
//! `pdfrum-render`, `pdfrum-object` and every other member crate through the
//! facade's own `Cargo.toml`. An integration test compiles *against* the
//! published artefact, with only `pdfrum` in scope — which is exactly the
//! position a `cargo add pdfrum` caller is in, and the only position from which
//! "can a caller name this?" has a truthful answer.
//!
//! Nothing here asserts a value. Every function below is a **compile-time**
//! claim: if a type that appears in a public `pdfrum` signature stops being
//! reachable from `pdfrum::*`, this file stops compiling, and the failure
//! names the type. That is the whole point — the surface it guards is one a
//! runtime assertion cannot reach.
//!
//! The list is derived from `docs/api-baseline/pdfrum.txt`, mechanically:
//!
//! ```text
//! grep -vE '^pub use pdfrum::' docs/api-baseline/pdfrum.txt \
//!   | grep -oE 'pdfrum_[a-z0-9_]+::[A-Za-z0-9_:]*[A-Za-z0-9_]' | sort -u
//! ```
//!
//! When that command grows a row, this file grows a line — or the row is a
//! documented escape hatch and is listed in
//! [`the_escape_hatches_are_usable_without_naming_their_crate`]
//! instead.

#![allow(clippy::items_after_statements)]

use std::sync::Arc;

use pdfrum::*;

/// A type that appears in a public signature, named without a module path.
///
/// The body is empty on purpose: naming the type in the turbofish is the
/// entire assertion, and most of these have no cheap constructor. `?Sized`
/// because three of the names below are traits, and a trait a caller cannot
/// name is a bound they cannot write.
fn nameable<T: ?Sized>() {}

/// Every foreign type in a `pdfrum` signature, named from the facade.
///
/// Grouped by the crate it comes from, in the order the derivation command
/// prints them, so a reader can diff this against the snapshot by eye.
#[test]
fn every_type_in_a_public_signature_is_nameable_from_the_facade() {
    // pdfrum-common
    nameable::<Diagnostics>();
    nameable::<DiagKind>();
    nameable::<Diagnostic>();
    nameable::<Limits>();
    nameable::<LimitExceeded>();
    nameable::<Deadline>();
    nameable::<Operation>();
    nameable::<Severity>();
    nameable::<PdfVersion>();
    nameable::<PageIndex>();

    // pdfrum-crypt
    nameable::<Permissions>();

    // pdfrum-doc
    nameable::<AnnotFlags>();
    nameable::<Subtype>();
    nameable::<Focus>();
    nameable::<FocusBox>();
    nameable::<GeneratedAp>();
    nameable::<DocError>();
    nameable::<FieldFlags>();
    nameable::<FieldKind>();
    nameable::<Action>();
    nameable::<ActionKind>();
    nameable::<Dest>();
    nameable::<Link>();

    // pdfrum-edit
    nameable::<SaveError>();

    // pdfrum-form
    nameable::<Button>();
    nameable::<Key>();
    nameable::<Modifiers>();
    nameable::<Event>();
    nameable::<PopupView>();
    nameable::<ScrollView>();
    nameable::<Placement>();
    nameable::<PopupGeometry>();
    nameable::<AnnotId>();
    nameable::<SessionConfig>();
    nameable::<Response>();
    nameable::<AppearanceUpdate>();
    nameable::<UpdateKind>();
    // kurbo and peniko, the five-and-one this crate's signatures name. Not
    // `pdfrum::kurbo::Rect` any more: § replaced `pub use kurbo;` with the
    // types themselves, so these are the assertion that the narrowed set is
    // still the *whole* set a signature can hand back. `FormRect` is gone —
    // `PopupView`/`PopupGeometry` speak `Rect`, and the event methods' points
    // are `Point`.
    nameable::<Affine>();
    nameable::<BezPath>();
    nameable::<Point>();
    nameable::<Rect>();
    nameable::<Size>();
    nameable::<Color>();

    // pdfrum-object
    nameable::<Dict>();
    nameable::<ObjectError>();
    nameable::<Object>();
    nameable::<ObjRef>();
    nameable::<dyn Resolve>();

    // pdfrum-page
    nameable::<BuildContext>();
    nameable::<TextRenderMode>();
    nameable::<PageObject>();
    nameable::<Rotation>();
    nameable::<IndexOutOfRange>();

    // pdfrum-parser
    nameable::<OpenError>();
    nameable::<ReadError>();

    // pdfrum-render
    nameable::<RenderCaches>();
    nameable::<RenderError>();
    nameable::<ColorMode>();
    nameable::<ColorScheme>();
    nameable::<Argb>();
    nameable::<TextAa>();
    nameable::<Pixmap>();

    // pdfrum-text
    nameable::<TextPage>();
    nameable::<CharBox>();
    nameable::<Word>();
    nameable::<FindOptions>();
    nameable::<WebLink>();
    nameable::<TextError>();
    // Two index spaces, and the table that converts between them. Every
    // one appears in a `TextPage` signature: `CharIndex` in `char`, `rects`,
    // `index_at`, `slice` and `WebLink::range`; `TextIndex` in `find`; and
    // `IndexMap` as the public `runs` field a caller needs to cross from one
    // space to the other.
    nameable::<CharIndex>();
    nameable::<TextIndex>();
    nameable::<IndexMap>();
}

/// The facade's own types, for completeness of the block a caller reads.
///
/// Its own function only because the member-crate list above is one line
/// short of the tree's function-length ceiling; the claim is the same.
#[test]
fn every_facade_own_type_is_nameable() {
    nameable::<Document>();
    nameable::<Metadata>();
    nameable::<OpenOptions>();
    nameable::<RenderOptions>();
    nameable::<RenderSession>();
    nameable::<SaveOptions>();
    nameable::<Update>();
    nameable::<DocEdit>();
    nameable::<EmbeddedFont>();
    nameable::<FontEncoding>();
    nameable::<StandardFont>();
    nameable::<EmbeddedImage>();
    nameable::<PixelFormat>();
    nameable::<Rotation>();
    nameable::<Error>();
    nameable::<ImageBuilder>();
    nameable::<PageEdit>();
    nameable::<PathBuilder>();
    nameable::<TextBuilder>();
    nameable::<UnknownField>();
    nameable::<OutlineIter<'_, '_>>();
    nameable::<SubstitutionOptions>();
    nameable::<dyn RenderDevice>();
    #[cfg(feature = "vello-cpu")]
    nameable::<dyn RasterBackend<Device = <VelloCpuBackend as RasterBackend>::Device>>();
    #[cfg(feature = "vello-cpu")]
    nameable::<VelloCpuBackend>();
    #[cfg(feature = "tiny-skia")]
    nameable::<TinySkiaBackend>();
    #[cfg(feature = "agg")]
    nameable::<AggBackend>();
    #[cfg(feature = "vello-gpu")]
    nameable::<VelloGpuBackend<'_>>();
}

/// Every `Error` variant's payload can be bound and inspected by a
/// facade-only caller.
///
/// The hole this closes: all seven variants carried a foreign error type that
/// no `cargo add pdfrum` caller could name, so the payload could be
/// `Display`ed and never matched. Binding it in a pattern with an explicit
/// annotation is the strongest form the claim takes — a `_` binding would
/// compile even if nothing were re-exported.
#[test]
fn every_error_variant_payload_is_nameable() {
    fn inspect(error: Error) {
        match error {
            Error::Open(payload) => {
                let _: OpenError = payload;
            }
            Error::Read(payload) => {
                let _: ReadError = payload;
            }
            Error::Render(payload) => {
                let _: RenderError = payload;
            }
            Error::Doc(payload) => {
                let _: DocError = payload;
            }
            Error::Save(payload) => {
                let _: SaveError = payload;
            }
            Error::Text(payload) => {
                let _: TextError = payload;
            }
            Error::Limit(payload) => {
                let _: LimitExceeded = payload;
            }
            Error::Io(payload) => {
                let _: std::io::Error = payload;
            }
            // `Error` is `#[non_exhaustive]`, so a caller must write this arm
            // and so must this test. A new variant does not break the build
            // here — it is the *payload* nameability this file guards, and a
            // new variant's payload gets its own arm above when it lands.
            _ => {}
        }
    }

    inspect(Error::Io(std::io::Error::other("not opened")));
}

/// `Document::fetch`'s three foreign types, all nameable.
///
/// It is an escape hatch — `Resolve` is `pdfrum-object`'s trait and its
/// rustdoc says so — but an escape hatch whose *return* type a caller cannot
/// spell is not an escape hatch, it is a dead end. The trait is re-exported so
/// the bound is writable and the `Arc<Object>` is bindable.
#[test]
fn the_object_store_escape_hatch_is_writable() {
    fn fetch_one(doc: &impl Resolve, at: ObjRef) -> std::result::Result<Arc<Object>, ObjectError> {
        doc.fetch(at)
    }

    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("fixture opens");
    // Object 1 is the catalog in every fixture here; what it *is* does not
    // matter, only that the call site above compiles from `pdfrum::*` alone.
    let _ = fetch_one(&doc, ObjRef::new(1, 0));
}

/// The escape hatches are the deliberate second category, and they are
/// reachable *by inference* even though their spelled-out type is not.
///
/// A method documented as an escape hatch in the first sentence of its
/// rustdoc may name a sibling-crate type, precisely so the collision with the facade's own [`Document`] and
/// [`Page`] stays visible at the call site. A caller who wants to *write* one
/// of these types adds the member crate — that is what the hatch is for and
/// what its first sentence tells them to do. What must still hold from
/// `pdfrum` alone is that the returned value is *usable*: `let`-bound, passed
/// on, and its inherent methods called. This test is the record that the four
/// are a category rather than an oversight the list above missed.
///
/// `Annotation::dict` is the one exception in the group: `Dict` is
/// re-exported anyway, because `GeneratedAp::resources` is a `Dict` and that
/// is an ordinary payload rather than a hatch.
#[test]
fn the_escape_hatches_are_usable_without_naming_their_crate() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("fixture opens");

    let parser = doc.parser();
    assert!(parser.page_count() > 0);

    let page = doc.page(0).expect("page 0");
    let graph = page.objects();
    let _ = graph.objects.len();

    let mut edit = page.edit();
    let _ = edit.graph().objects.len();
    let _ = edit.graph_mut().objects.len();

    for annot in page.annotations() {
        // Spelled out, because `Dict` *is* re-exported — see the doc comment.
        let dict: &Dict = annot.dict();
        let _ = dict.len();
    }
}

/// `ColorMode::Forced` is constructible by a facade-only caller.
///
/// The first known instance of the class of bug this file exists to catch, and
/// the one that is not a naming problem at all: `ColorScheme` was re-exported,
/// all four of its fields are `Argb`, `Argb` was not, and `ColorScheme` has no
/// `Default` and no constructor. There was no expression a `cargo add pdfrum`
/// caller could write that produced a `ColorMode::Forced` — a variant visible
/// in the rustdoc and unreachable from the API. One `pub use` line fixes it,
/// and this test is what keeps it fixed.
#[test]
fn a_forced_colour_scheme_is_constructible_from_the_facade() {
    let white = Argb {
        a: 255,
        r: 255,
        g: 255,
        b: 255,
    };
    let black = Argb {
        a: 255,
        r: 0,
        g: 0,
        b: 0,
    };

    let options = RenderOptions {
        color_mode: ColorMode::Forced(ColorScheme {
            path_fill: black,
            path_stroke: black,
            text_fill: black,
            text_stroke: white,
        }),
        ..RenderOptions::default()
    };

    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("fixture opens");
    let pixmap = doc
        .page(0)
        .expect("page 0")
        .render(&VelloCpuBackend::new(), &options)
        .expect("forced-colour render");
    assert!(pixmap.width() > 0 && pixmap.height() > 0);
}

/// An `UpdateKind` payload can be reached without a second dependency.
///
/// `Regenerated` and `LiveEdit` box a `GeneratedAp`; `ActionRequested` carries
/// a boxed `Action` and a `Modifiers`. All three are the payload of an
/// ordinary `Response` a caller matches on every event, so rules them
/// re-exports rather than escape hatches.
#[test]
fn an_update_payload_is_reachable() {
    fn describe(update: &UpdateKind) -> &'static str {
        match update {
            UpdateKind::Regenerated(ap) | UpdateKind::LiveEdit(ap) => {
                let _: &GeneratedAp = ap;
                let _: &[u8] = &ap.stream;
                "appearance"
            }
            UpdateKind::ActionRequested { action, modifiers } => {
                let _: &Action = action;
                let _: Modifiers = *modifiers;
                "action"
            }
            UpdateKind::RevertedToFileAppearance => "reverted",
            UpdateKind::FocusChanged { from, to } => {
                let _: (&Option<AnnotId>, &Option<AnnotId>) = (from, to);
                "focus"
            }
        }
    }

    assert_eq!(describe(&UpdateKind::RevertedToFileAppearance), "reverted");
}

/// A renderer can act on `focus_for_page` without a second dependency.
///
/// rules this one explicitly not an escape hatch: it is the question a
/// renderer asks every frame , so both halves of its answer
/// `Focus` and the `FocusBox` inside it — belong in the facade's block.
#[test]
fn a_focus_answer_is_matchable() {
    fn stroke(focus: Option<Focus>) -> Option<usize> {
        let focus: Focus = focus?;
        match focus.box_ {
            FocusBox::None => None,
            FocusBox::Inflated | FocusBox::Rect(_) => Some(focus.annot),
        }
    }

    assert_eq!(stroke(Some(Focus::at(3))), None);
}

/// Every type [`FormSession::with_cascade`] names is reachable **with the
/// `javascript` feature off**, which is the whole point of it not being gated.
///
/// A host writing its own commit gate — a validator, an audit log, a policy
/// that refuses a keystroke — needs `Cascade` and the four payload types, and
/// needs them from a default `cargo add pdfrum`. re-exports them
/// unconditionally for exactly that reason; only `ScriptCascade` is behind the
/// feature.
#[test]
fn the_cascade_seam_is_nameable_without_the_script_feature() {
    nameable::<dyn Cascade>();
    nameable::<NoScripts>();
    nameable::<FieldRef>();
    nameable::<FieldWrites>();
    nameable::<Keystroke>();
    nameable::<KeystrokeOutcome>();

    /// A caller's own cascade, written with nothing but `pdfrum` in scope.
    struct Mine;
    impl Cascade for Mine {
        fn keystroke(&mut self, field: &FieldRef, change: Keystroke) -> KeystrokeOutcome {
            let _: &str = &field.name;
            let _: Option<u32> = field.index;
            KeystrokeOutcome::Accept(change)
        }
        fn calculate(&mut self, writes: &mut FieldWrites, _trigger: &FieldRef) {
            writes.set(0, "computed");
        }
    }

    let doc = Document::open("tests/fixtures/text_form.pdf").expect("fixture opens");
    let session = FormSession::with_cascade(&doc, Mine);
    assert!(session.focused_annot().is_none());
}

/// With the feature **on**, everything a caller needs to build, drive and read
/// a `ScriptCascade` is nameable from `pdfrum` too.
///
/// `ScriptCascade::new` returns `Result<_, BuildError>` and `stops()` answers
/// `&[ScriptFailure]`, so both of those types appear in signatures a caller
/// writes and both are re-exported — `BuildError` as `ScriptBuildError`,
/// because the bare name beside `BuildContext` in one `pdfrum::*` namespace
/// would read as that type's error and it is not.
#[test]
#[cfg(feature = "javascript")]
fn the_script_types_are_nameable_from_the_facade() {
    nameable::<ScriptCascade>();
    nameable::<ScriptConfig>();
    nameable::<TranscriptLine>();
    nameable::<ScriptBuildError>();
    nameable::<ScriptFailure>();
    nameable::<ScriptStop>();
    nameable::<FieldActions>();

    // The two signatures those types exist for, written out as a caller would.
    let doc = Document::open("tests/fixtures/text_form.pdf").expect("fixture opens");
    let built: std::result::Result<FormSession<'_>, ScriptBuildError> =
        FormSession::with_scripts(&doc, &ScriptConfig::wall_clock());
    let session = built.expect("boa builds a realm on any input");
    let stops: &[ScriptFailure] = session.scripts().expect("a scripted session").stops();
    assert!(stops.is_empty());
}

/// Every form-session entry point is *writable* from `pdfrum::*` alone.
///
/// The `nameable::<T>()` lines above prove a type can be named. This proves
/// the methods can be **called** — that `apply`'s only argument is
/// constructible, that every wrapper's point is a `pdfrum::Point` a caller can
/// build, and that no signature reaches for a name the facade does not carry.
/// `apply`'s argument type is re-exported, so a `cargo add pdfrum` caller
/// can write the call.
///
/// The assertions are the compiler's. The one runtime claim is the last, and
/// it is the behaviour a right button must have: consume nothing.
#[test]
fn every_form_session_entry_point_is_writable_from_the_facade() {
    let doc = Document::open("tests/fixtures/text_form.pdf").expect("fixture opens");
    let mut session = FormSession::new(&doc);

    // The field is `/Rect [100 100 200 130]`, so this is inside it.
    let at = Point::new(120.0, 115.0);
    let none = Modifiers::NONE;

    // `apply`, once per `Event` variant, so a variant that grows a field the
    // facade cannot name stops this compiling.
    let _: Response = session.apply(Event::MouseMove {
        at,
        modifiers: none,
    });
    let _: Response = session.apply(Event::MouseDown {
        button: Button::Left,
        at,
        modifiers: none,
    });
    let _: Response = session.apply(Event::MouseUp {
        button: Button::Left,
        at,
        modifiers: none,
    });
    let _: Response = session.apply(Event::DoubleClick {
        at,
        modifiers: none,
    });
    let _: Response = session.apply(Event::MouseWheel {
        at,
        delta: (0, -1),
        modifiers: none,
    });
    let _: Response = session.apply(Event::Focus {
        at,
        modifiers: none,
    });
    let _: Response = session.apply(Event::KeyDown {
        key: Key::Tab,
        modifiers: none,
    });
    let _: Response = session.apply(Event::Char {
        ch: 'a',
        modifiers: none,
    });

    // And every thin wrapper over it, page-taking or not.
    let _: Response = session.mouse_move(0, at, none);
    let _: Response = session.mouse_down(0, at, none);
    let _: Response = session.mouse_up(0, at, none);
    let _: Response = session.double_click(0, at, none);
    let _: Response = session.mouse_wheel(0, at, (0, -1), none);
    let _: Response = session.focus_at(0, at, none);
    let _: Response = session.key_down(Key::Return, none);
    let _: Response = session.character('a', none);
    let _: Response = session.blur();

    session.set_viewed_page(0);
    let _: PageIndex = session.viewed_page();

    // The right button has no method and does not need one: it is the value
    // form, and the correct behaviour for it is to consume nothing.
    let right = session.apply(Event::MouseDown {
        button: Button::Right,
        at,
        modifiers: none,
    });
    assert!(!right.consumed);
}
