//! `--send-events`: driving a [`pdfrum::FormSession`] from a parsed `.evt`
//! script, the way the oracle drives its form handle from one.
//!
//! Two enums meet here and they are deliberately different layers.
//! [`crate::events::Event`] is the
//! **grammar**: one variant per `.evt` verb, `i32` straight out of `atoi`, a
//! `u32` modifier mask, and a `KeyCode` variant that is the down/up *pair* the
//! verb emits. [`pdfrum::FormSession`]'s methods are the **semantics**: page
//! space coordinates as a [`kurbo::Point`], a typed [`Key`], typed
//! [`Modifiers`], and no key-up at all. This module is the bridge, and it is the only place in the
//! tool that knows both.
//!
//! Five of the harness reviewer's seven bridging facts
//! (`docs/reviews/claude-review-evt-harness.md`) are decisions taken here
//! rather than notes about them; the remaining two are the caller's
//! (`run.rs`) and the golden store's.
//!
//! 1. **Coordinates are page space, y-up, and no transform happens.** The
//!    `.evt` integers are PDF user space as given. The widening to `f64` is
//!    the *whole* conversion, and it is exact.
//! 2. **`keycode` is the down/up pair → one `key_down`.** The verb fires a
//!    key-down and then a key-up with identical arguments, and the up edge
//!    does nothing at all upstream. It is dropped rather than modelled: the
//!    facade has no method to send it to, by design.
//! 3. **`charcode` carries an `i32` code point, narrowed fallibly.**
//!    `form_textfield_focused_rtl.evt` sends 1488-1514 (Hebrew), so this path
//!    is corpus-exercised, not hypothetical. A value that is not a Unicode
//!    scalar — negative, past `char::MAX`, or a surrogate — is **skipped with
//!    a line on stderr**, never lossily replaced: substituting U+FFFD would
//!    type a visible glyph the oracle never typed.
//! 4. **Modifiers pass through.** The three `MOD_*` bits the grammar can set
//!    are exactly `Modifiers::{SHIFT, CONTROL, ALT}`; nothing else in the mask
//!    is reachable from a script.
//! 5. **Unbalanced mouse events are never repaired.** A `mousedown` with no
//!    `mouseup` is the point of `scrollable_widgets1.evt`. This module holds
//!    no pairing state whatsoever, so there is nothing here that *could*
//!    synthesize the missing edge.
//!
//! The right button is not dropped either. It has no method on the facade —
//! a `button` argument and a `down: bool` beside it was an enum spelled as
//! arguments — so a right-button line becomes an [`Event::MouseDown`] or
//! [`Event::MouseUp`] handed to [`pdfrum::FormSession::apply`], which is the
//! shape the value already had. The correct behaviour for those lines is to
//! consume nothing, and that is still what happens.

// The oracle readings behind facts 1 and 2 above, kept out of the published
// docs but not lost. Page space: public/fpdf_formfill.h documents page_x and
// page_y that way, and FORM_OnLButtonUp's "in device" comment is an upstream
// doc bug whose body is identical to OnLButtonDown's. The key pair:
// event.cc:46-58 fires FORM_OnKeyDown and then FORM_OnKeyUp with identical
// arguments, and FORM_OnKeyUp is documented as permanently unimplemented and
// always answers false.

use std::io::Write;

use kurbo::Point;
use pdfrum::{Button, Event, FormSession, Key, Modifiers, Response};

use crate::events::{self, MOD_ALT, MOD_CONTROL, MOD_SHIFT};

/// Replays a whole parsed script against one page, in order.
///
/// The *entire* stream, once per page, before anything is written for that
/// page. The session is the caller's and outlives the call, because the
/// oracle holds one form handle for the whole document and every page's
/// replay sees whatever the previous page's left behind.
// The oracle's SendPageEvents, event.cc:163-195.
///
/// Returns every appearance update the run produced, in the order the session
/// reported them, so the caller can render the post-event state.
pub fn replay_page(
    session: &mut FormSession<'_>,
    page: u32,
    script: &[events::Event],
    err: &mut dyn Write,
) -> Vec<pdfrum::AppearanceUpdate> {
    // The page being replayed *is* the page in view: `pdfium_test` holds one
    // page at a time and passes it to every `FORM_On*` call. Without this a
    // Tab with nothing focused would enter the focus ring on page 0 whichever
    // page the script is being replayed against.
    session.set_viewed_page(page);
    let mut updates = Vec::new();
    for event in script {
        let Some(call) = to_call(event) else {
            // Fact 3: skipped, and said out loud. Stderr rather than stdout
            // because every other per-event notice the oracle writes goes
            // there, and stdout is the byte-compared stream.
            if let events::Event::CharCode { code } = *event {
                let _ = writeln!(
                    err,
                    "charcode {code} is not a Unicode scalar value; skipped."
                );
            }
            continue;
        };
        updates.extend(perform(session, page, call).updates);
    }
    updates
}

/// What one grammar event asks the facade to do.
///
/// A named intention rather than a direct call, for one reason: the facade's
/// dispatch is inert today (every event reports itself unhandled, deliberately
/// and documented as such), so a test that drove a real session could not tell
/// a correct bridge from a bridge that sent nothing. This enum is what the
/// bridge *decided*, and it is a pure function of the grammar event — so the
/// ordering, the key-pair collapse, the skipped code point and the untouched
/// mouse imbalance are all assertable without a document.
///
/// [`perform`] is the only thing that turns one into a facade call, and it is
/// a total match, so a variant cannot be added without a call being wired.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Call {
    /// `mouse_move`.
    MouseMove { at: Point, modifiers: Modifiers },
    /// `mouse_down` — the left button only.
    MouseDown { at: Point, modifiers: Modifiers },
    /// `mouse_up` — the left button only.
    MouseUp { at: Point, modifiers: Modifiers },
    /// A non-primary button, applied as an [`Event`] because the facade has
    /// no method for one.
    Button {
        button: Button,
        down: bool,
        at: Point,
        modifiers: Modifiers,
    },
    /// `double_click`.
    DoubleClick { at: Point, modifiers: Modifiers },
    /// `mouse_wheel`.
    MouseWheel {
        at: Point,
        delta: (i32, i32),
        modifiers: Modifiers,
    },
    /// `focus_at`.
    FocusAt { at: Point, modifiers: Modifiers },
    /// `key_down`. There is no `KeyUp` variant, and that is fact 2.
    KeyDown { key: Key, modifiers: Modifiers },
    /// `character`.
    Char { ch: char, modifiers: Modifiers },
}

/// One grammar event onto the call it means, or `None` when it means none.
///
/// `None` is not an error: it is the `charcode` whose integer names no
/// Unicode scalar (fact 3). Every other verb has exactly one call, including
/// the right-button lines and including a `mousedown` whose `mouseup` never
/// comes — this function holds no state at all, which is fact 5 made
/// structural rather than promised.
#[must_use]
pub fn to_call(event: &events::Event) -> Option<Call> {
    match *event {
        // The grammar has no modifier field on `mousemove`, so this is a
        // hardcoded zero rather than a lost value.
        events::Event::MouseMove { x, y } => Some(Call::MouseMove {
            at: at(x, y),
            modifiers: Modifiers::NONE,
        }),
        events::Event::MouseDown {
            button,
            x,
            y,
            modifiers,
        } => Some(button_call(button, true, x, y, modifiers)),
        events::Event::MouseUp {
            button,
            x,
            y,
            modifiers,
        } => Some(button_call(button, false, x, y, modifiers)),
        events::Event::MouseDoubleClick { x, y, modifiers } => Some(Call::DoubleClick {
            at: at(x, y),
            modifiers: to_modifiers(modifiers),
        }),
        events::Event::MouseWheel {
            x,
            y,
            delta_x,
            delta_y,
            modifiers,
        } => Some(Call::MouseWheel {
            at: at(x, y),
            delta: (delta_x, delta_y),
            modifiers: to_modifiers(modifiers),
        }),
        // `focus,<x>,<y>` has no modifier field either.
        events::Event::Focus { x, y } => Some(Call::FocusAt {
            at: at(x, y),
            modifiers: Modifiers::NONE,
        }),
        // Fact 2: the verb is the down/up pair, and the up edge is a no-op
        // the facade has no method for. One `KeyDown` is the whole meaning of
        // the line, so the collapse happens here and leaves no trace to undo.
        events::Event::KeyCode { code, modifiers } => Some(Call::KeyDown {
            key: to_key(code),
            modifiers: to_modifiers(modifiers),
        }),
        // The grammar hardcodes no modifiers on `charcode`.
        events::Event::CharCode { code } => Some(Call::Char {
            ch: to_char(code)?,
            modifiers: Modifiers::NONE,
        }),
    }
}

/// Performs one decided call against the session.
///
/// Every variant but `Button` has a method; `Button` is an [`Event`] applied
/// against the page in view, which [`replay_page`] has already set to `page`.
fn perform(session: &mut FormSession<'_>, page: u32, call: Call) -> Response {
    match call {
        Call::MouseMove { at, modifiers } => session.mouse_move(page, at, modifiers),
        Call::MouseDown { at, modifiers } => session.mouse_down(page, at, modifiers),
        Call::MouseUp { at, modifiers } => session.mouse_up(page, at, modifiers),
        Call::Button {
            button,
            down,
            at,
            modifiers,
        } => session.apply(if down {
            Event::MouseDown {
                button,
                at,
                modifiers,
            }
        } else {
            Event::MouseUp {
                button,
                at,
                modifiers,
            }
        }),
        Call::DoubleClick { at, modifiers } => session.double_click(page, at, modifiers),
        Call::MouseWheel {
            at,
            delta,
            modifiers,
        } => session.mouse_wheel(page, at, delta, modifiers),
        Call::FocusAt { at, modifiers } => session.focus_at(page, at, modifiers),
        Call::KeyDown { key, modifiers } => session.key_down(key, modifiers),
        Call::Char { ch, modifiers } => session.character(ch, modifiers),
    }
}

/// A `mousedown`/`mouseup` for either button.
///
/// The left button goes through the dedicated methods and the right through a
/// whole [`Event`], which is the facade's own split: the left pair is what
/// every ported assertion drives, and the value form exists so a script's
/// right-button line has somewhere to go rather than being dropped.
fn button_call(button: events::MouseButton, down: bool, x: i32, y: i32, modifiers: u32) -> Call {
    let (at, modifiers) = (at(x, y), to_modifiers(modifiers));
    match (button, down) {
        (events::MouseButton::Left, true) => Call::MouseDown { at, modifiers },
        (events::MouseButton::Left, false) => Call::MouseUp { at, modifiers },
        (events::MouseButton::Right, _) => Call::Button {
            button: Button::Right,
            down,
            at,
            modifiers,
        },
    }
}

/// Fact 1: a page-space integer pair widened, and nothing else.
///
/// `f64::from` is total and exact over the whole of `i32`, which is what the
/// C++ does too — `FORM_On*` take `double page_x, page_y` and the harness
/// hands them the `atoi` result. There is no lossy cast here and no `expect`
/// attribute needed to permit one: the `as f32` this replaced was exact for
/// every coordinate the corpus holds, but only because the corpus's largest
/// is four digits. `pdfrum-form` narrows to its own `f32` in `route::apply`,
/// which is where the oracle narrows as well.
fn at(x: i32, y: i32) -> Point {
    Point::new(f64::from(x), f64::from(y))
}

/// Fact 4: the grammar's three bits onto the semantic ones.
///
/// The `.evt` mask is `GetModifiers`' three `FWL_EVENTFLAG_*` values, which
/// are the only ones a script can name; the facade's set is larger because the
/// ported assertions send more. Bits outside the three are unreachable from a
/// script, so this is a total mapping rather than a lossy one.
fn to_modifiers(mask: u32) -> Modifiers {
    let mut modifiers = Modifiers::NONE;
    if mask & MOD_SHIFT != 0 {
        modifiers = modifiers.union(Modifiers::SHIFT);
    }
    if mask & MOD_CONTROL != 0 {
        modifiers = modifiers.union(Modifiers::CONTROL);
    }
    if mask & MOD_ALT != 0 {
        modifiers = modifiers.union(Modifiers::ALT);
    }
    modifiers
}

/// A key code onto the library's key.
///
/// The grammar speaks raw integers and the library speaks a key, so this is
/// the boundary between them — `Key::from_virtual` — with one gate in front
/// of it. A code the form layer does not branch on must still arrive, because
/// the ported assertions send F1 and digits precisely to check they are *not*
/// consumed; `from_virtual` carries those through as `Other`. A code outside
/// `u16` cannot name a virtual key on any platform, so it becomes `Unknown` —
/// which is a key the layer explicitly handles rather than a sentinel.
fn to_key(code: i32) -> Key {
    u16::try_from(code).map_or(Key::Unknown, Key::from_virtual)
}

/// Fact 3: an `i32` code point onto a `char`, fallibly.
///
/// Two gates, and both reject rather than repair: `u32::try_from` refuses a
/// negative, and `char::from_u32` refuses a surrogate (U+D800–U+DFFF) and
/// anything past U+10FFFF.
fn to_char(code: i32) -> Option<char> {
    char::from_u32(u32::try_from(code).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_grammar_modifiers_map_across_and_combine() {
        assert_eq!(to_modifiers(0), Modifiers::NONE);
        assert_eq!(to_modifiers(MOD_SHIFT), Modifiers::SHIFT);
        assert_eq!(to_modifiers(MOD_CONTROL), Modifiers::CONTROL);
        assert_eq!(to_modifiers(MOD_ALT), Modifiers::ALT);
        assert_eq!(
            to_modifiers(MOD_SHIFT | MOD_CONTROL | MOD_ALT),
            Modifiers::SHIFT
                .union(Modifiers::CONTROL)
                .union(Modifiers::ALT)
        );
    }

    #[test]
    fn a_bit_no_script_can_set_maps_to_nothing() {
        // The grammar's mask only ever holds the low three bits; anything
        // else is unreachable, and must not be forwarded as a guess.
        assert_eq!(to_modifiers(1 << 9), Modifiers::NONE);
    }

    #[test]
    fn a_code_point_narrows_to_a_char_when_it_is_a_scalar_value() {
        assert_eq!(to_char(97), Some('a'));
        assert_eq!(to_char(13), Some('\r'));
        // `form_textfield_focused_rtl.evt`'s range, the reason this path is
        // fallible rather than a cast.
        assert_eq!(to_char(1488), Some('\u{5D0}'));
        assert_eq!(to_char(1514), Some('\u{5EA}'));
        assert_eq!(to_char(0x0010_FFFF), Some('\u{10FFFF}'));
    }

    #[test]
    fn an_invalid_code_point_is_skipped_rather_than_replaced() {
        // Negative: `atoi` of "-4" is a real possibility the grammar admits.
        assert_eq!(to_char(-4), None);
        // A lone surrogate is a valid u32 and not a valid char.
        assert_eq!(to_char(0xD800), None);
        assert_eq!(to_char(0xDFFF), None);
        // Past the last code point.
        assert_eq!(to_char(0x11_0000), None);
        assert_eq!(to_char(i32::MAX), None);
        // And nothing is substituted: U+FFFD would type a glyph.
        assert_ne!(to_char(-1), Some('\u{FFFD}'));
    }

    #[test]
    fn a_key_code_narrows_and_an_impossible_one_becomes_unknown() {
        assert_eq!(to_key(9), Key::Tab);
        assert_eq!(to_key(0x0D), Key::Return);
        assert_eq!(to_key(0x41), Key::A);
        assert_eq!(to_key(0), Key::Unknown);
        // Outside u16, so it names no virtual key anywhere.
        assert_eq!(to_key(-1), Key::Unknown);
        assert_eq!(to_key(70_000), Key::Unknown);
    }

    fn calls(script: &str) -> Vec<Call> {
        let events = crate::events::parse_evt(script).expect("the grammar is total");
        events.iter().filter_map(to_call).collect()
    }

    #[test]
    fn a_keycode_line_becomes_one_key_down_and_no_key_up() {
        // Fact 2. `event.cc:46-58` fires OnKeyDown *and* OnKeyUp with the
        // same arguments; the up edge is a documented permanent no-op, and
        // the facade has no method for it. One call, not two.
        assert_eq!(
            calls("keycode,9"),
            [Call::KeyDown {
                key: Key::Tab,
                modifiers: Modifiers::NONE
            }]
        );
        // Fact 4 rides along on the same verb, which is the only one whose
        // script form can carry a modifier onto the keyboard.
        assert_eq!(
            calls("keycode,90,shiftcontrol"),
            [Call::KeyDown {
                key: Key::Z,
                modifiers: Modifiers::SHIFT.union(Modifiers::CONTROL),
            }]
        );
        // Three lines are three key-downs, not three pairs.
        assert_eq!(calls("keycode,9\nkeycode,9\nkeycode,9").len(), 3);
    }

    #[test]
    fn a_charcode_that_is_not_a_scalar_value_produces_no_call_at_all() {
        // Fact 3. The surrounding lines still map, so the skip is of the one
        // line and not of the rest of the script.
        assert_eq!(
            calls("charcode,97\ncharcode,55296\ncharcode,98"),
            [
                Call::Char {
                    ch: 'a',
                    modifiers: Modifiers::NONE
                },
                Call::Char {
                    ch: 'b',
                    modifiers: Modifiers::NONE
                },
            ]
        );
        // A negative code point: `atoi` admits one, `char` does not.
        assert!(calls("charcode,-1").is_empty());
    }

    #[test]
    fn an_unbalanced_mousedown_is_preserved_exactly_as_written() {
        // Fact 5: `scrollable_widgets1.evt` depends on a down with no up.
        // Nothing here holds pairing state, so there is no `mouseup` to
        // find and none is invented.
        let script = calls("mousemove,150,415\nmousedown,left,150,415");
        assert_eq!(
            script,
            [
                Call::MouseMove {
                    at: Point::new(150.0, 415.0),
                    modifiers: Modifiers::NONE
                },
                Call::MouseDown {
                    at: Point::new(150.0, 415.0),
                    modifiers: Modifiers::NONE
                },
            ]
        );
        assert!(
            !script
                .iter()
                .any(|call| matches!(call, Call::MouseUp { .. })),
            "no mouseup may be synthesized"
        );
        // And the mirror image: an up with no down is equally untouched.
        assert_eq!(
            calls("mouseup,left,10,20"),
            [Call::MouseUp {
                at: Point::new(10.0, 20.0),
                modifiers: Modifiers::NONE
            }]
        );
    }

    #[test]
    fn a_right_button_line_becomes_an_event_rather_than_being_dropped() {
        assert_eq!(
            calls("mousedown,right,1,2\nmouseup,right,1,2"),
            [
                Call::Button {
                    button: Button::Right,
                    down: true,
                    at: Point::new(1.0, 2.0),
                    modifiers: Modifiers::NONE
                },
                Call::Button {
                    button: Button::Right,
                    down: false,
                    at: Point::new(1.0, 2.0),
                    modifiers: Modifiers::NONE
                },
            ]
        );
    }

    #[test]
    fn a_script_maps_verb_for_verb_in_order() {
        // Every verb the grammar has, once, in an order no sort would
        // produce. The bridge reorders nothing and merges nothing.
        let script = "mousemove,1,2\n\
                      mousedown,left,3,4\n\
                      charcode,97\n\
                      mouseup,left,5,6\n\
                      keycode,37\n\
                      mousedoubleclick,left,7,8\n\
                      mousewheel,9,10,0,-1\n\
                      focus,11,12";
        let got = calls(script);
        assert_eq!(got.len(), 8, "one call per verb line: {got:?}");
        assert!(matches!(got[0], Call::MouseMove { .. }));
        assert!(matches!(got[1], Call::MouseDown { .. }));
        assert!(matches!(got[2], Call::Char { ch: 'a', .. }));
        assert!(matches!(got[3], Call::MouseUp { .. }));
        assert!(matches!(got[4], Call::KeyDown { key: Key::Left, .. }));
        assert!(matches!(got[5], Call::DoubleClick { .. }));
        assert!(matches!(got[6], Call::MouseWheel { delta: (0, -1), .. }));
        assert!(matches!(got[7], Call::FocusAt { .. }));
    }

    /// The read-only C++ PDFium checkout, resolved as every script and test in
    /// this repository resolves it: `$PDFRUM_ORACLE_CHECKOUT`, else the
    /// sibling `../pdfium-c++` directory the README names.
    fn oracle_checkout() -> std::path::PathBuf {
        std::env::var_os("PDFRUM_ORACLE_CHECKOUT").map_or_else(
            || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pdfium-c++"),
            std::path::PathBuf::from,
        )
    }

    #[test]
    fn every_corpus_script_maps_to_a_call_per_line_but_for_bad_code_points() {
        // The whole in-scope corpus through the bridge, asserting the only
        // thing that may shrink a script is fact 3's skip.
        let root = oracle_checkout().join("testing");
        if !root.is_dir() {
            return;
        }
        let mut files = Vec::new();
        collect_evt(&root, &mut files);
        files.sort();
        assert!(!files.is_empty());
        let mut total = 0usize;
        for path in &files {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let events = crate::events::parse_evt(&text).expect("total grammar");
            let unmappable = events
                .iter()
                .filter(|event| match **event {
                    crate::events::Event::CharCode { code } => to_char(code).is_none(),
                    _ => false,
                })
                .count();
            let mapped: Vec<Call> = events.iter().filter_map(to_call).collect();
            assert_eq!(
                mapped.len() + unmappable,
                events.len(),
                "{} lost a line for a reason other than an invalid code point",
                path.display()
            );
            total += mapped.len();
        }
        assert!(
            total > 200,
            "the corpus should map to many calls, got {total}"
        );
    }

    fn collect_evt(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_evt(&path, out);
            } else if path.extension() == Some(std::ffi::OsStr::new("evt")) {
                out.push(path);
            }
        }
    }

    #[test]
    fn a_coordinate_is_widened_and_not_transformed() {
        // Fact 1: no y-flip, no page-height subtraction, no scale — and now
        // an equality rather than an epsilon, because `f64::from` is exact
        // over the whole of `i32` where the `as f32` it replaced was only
        // exact below 2^24.
        assert_eq!(at(0, 0), Point::new(0.0, 0.0));
        assert_eq!(at(145, 415), Point::new(145.0, 415.0));
        assert_eq!(at(-3, i32::MAX), Point::new(-3.0, 2_147_483_647.0));
    }
}
