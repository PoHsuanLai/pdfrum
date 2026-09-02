//! `--send-events`: driving a [`pdfrum::FormSession`] from a parsed `.evt`
//! script, the way `pdfium_test` drives `FPDF_FORMHANDLE` from one.
//!
//! Two enums meet here and they are deliberately different layers
//! (`docs/design/pdfrum-form.md` §4.2). [`crate::events::Event`] is the
//! **grammar**: one variant per `.evt` verb, `i32` straight out of `atoi`, a
//! `u32` modifier mask, and a `KeyCode` variant that is the down/up *pair* the
//! verb emits. [`pdfrum::FormSession`]'s methods are the **semantics**: page
//! space coordinates, a typed [`VirtualKey`], typed [`EventModifiers`], and
//! no key-up at all. This module is the bridge, and it is the only place in the
//! tool that knows both.
//!
//! Five of the harness reviewer's seven bridging facts
//! (`docs/reviews/claude-review-evt-harness.md`) are decisions taken here
//! rather than notes about them; the remaining two are the caller's
//! (`run.rs`) and the golden store's.
//!
//! 1. **Coordinates are page space, y-up, and no transform happens.** The
//!    `.evt` integers are what `FORM_On*` receives, and
//!    `public/fpdf_formfill.h` documents those as PDF user space. The widening
//!    to `f32` is the *whole* conversion. (`FORM_OnLButtonUp`'s "in device"
//!    comment is an upstream doc bug — its body is `OnLButtonDown`'s.)
//! 2. **`keycode` is the down/up pair → one `on_key_down`.** `event.cc:46-58`
//!    fires `FORM_OnKeyDown` then `FORM_OnKeyUp` with identical arguments, and
//!    `FORM_OnKeyUp` is documented as permanently unimplemented, always
//!    answering false. The up edge is dropped rather than modelled: the facade
//!    has no method to send it to, by design.
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
//! The right button is not dropped either: `on_button` exists on the facade
//! precisely because scripts contain right-button lines, and the correct
//! behaviour for them is to consume nothing.

use std::io::Write;

use pdfrum::{EventModifiers, EventResponse, FormSession, MouseButton, VirtualKey};

use crate::events::{self, MOD_ALT, MOD_CONTROL, MOD_SHIFT};

/// Replays a whole parsed script against one page, in order.
///
/// This is `SendPageEvents` (`event.cc:163-195`): the *entire* stream, once
/// per page, before anything is written for that page. The session is the
/// caller's and outlives the call, because `pdfium_test` holds one
/// `FPDF_FORMHANDLE` for the document and every page's replay sees whatever
/// the previous page's left behind.
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
    session.set_page_in_view(page);
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
    /// `on_mouse_move`.
    MouseMove {
        x: f32,
        y: f32,
        modifiers: EventModifiers,
    },
    /// `on_mouse_down` — the left button only.
    MouseDown {
        x: f32,
        y: f32,
        modifiers: EventModifiers,
    },
    /// `on_mouse_up` — the left button only.
    MouseUp {
        x: f32,
        y: f32,
        modifiers: EventModifiers,
    },
    /// `on_button`, which is where a non-primary button goes.
    Button {
        button: MouseButton,
        down: bool,
        x: f32,
        y: f32,
        modifiers: EventModifiers,
    },
    /// `on_double_click`.
    DoubleClick {
        x: f32,
        y: f32,
        modifiers: EventModifiers,
    },
    /// `on_mouse_wheel`.
    MouseWheel {
        x: f32,
        y: f32,
        delta_x: i32,
        delta_y: i32,
        modifiers: EventModifiers,
    },
    /// `on_focus_at`.
    FocusAt {
        x: f32,
        y: f32,
        modifiers: EventModifiers,
    },
    /// `on_key_down`. There is no `KeyUp` variant, and that is fact 2.
    KeyDown {
        key: VirtualKey,
        modifiers: EventModifiers,
    },
    /// `on_char`.
    Char { ch: char, modifiers: EventModifiers },
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
            x: at(x),
            y: at(y),
            modifiers: EventModifiers::NONE,
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
            x: at(x),
            y: at(y),
            modifiers: to_modifiers(modifiers),
        }),
        events::Event::MouseWheel {
            x,
            y,
            delta_x,
            delta_y,
            modifiers,
        } => Some(Call::MouseWheel {
            x: at(x),
            y: at(y),
            delta_x,
            delta_y,
            modifiers: to_modifiers(modifiers),
        }),
        // `focus,<x>,<y>` has no modifier field either.
        events::Event::Focus { x, y } => Some(Call::FocusAt {
            x: at(x),
            y: at(y),
            modifiers: EventModifiers::NONE,
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
            modifiers: EventModifiers::NONE,
        }),
    }
}

/// Performs one decided call against the session.
fn perform(session: &mut FormSession<'_>, page: u32, call: Call) -> EventResponse {
    match call {
        Call::MouseMove { x, y, modifiers } => session.on_mouse_move(page, x, y, modifiers),
        Call::MouseDown { x, y, modifiers } => session.on_mouse_down(page, x, y, modifiers),
        Call::MouseUp { x, y, modifiers } => session.on_mouse_up(page, x, y, modifiers),
        Call::Button {
            button,
            down,
            x,
            y,
            modifiers,
        } => session.on_button(page, button, down, x, y, modifiers),
        Call::DoubleClick { x, y, modifiers } => session.on_double_click(page, x, y, modifiers),
        Call::MouseWheel {
            x,
            y,
            delta_x,
            delta_y,
            modifiers,
        } => session.on_mouse_wheel(page, x, y, delta_x, delta_y, modifiers),
        Call::FocusAt { x, y, modifiers } => session.on_focus_at(page, x, y, modifiers),
        Call::KeyDown { key, modifiers } => session.on_key_down(key, modifiers),
        Call::Char { ch, modifiers } => session.on_char(ch, modifiers),
    }
}

/// A `mousedown`/`mouseup` for either button.
///
/// The left button goes through the dedicated methods and the right through
/// `on_button`, which is the facade's own split: the left pair is what every
/// ported assertion drives, and `on_button` exists so a script's right-button
/// line has somewhere to go rather than being dropped.
fn button_call(button: events::MouseButton, down: bool, x: i32, y: i32, modifiers: u32) -> Call {
    let (x, y, modifiers) = (at(x), at(y), to_modifiers(modifiers));
    match (button, down) {
        (events::MouseButton::Left, true) => Call::MouseDown { x, y, modifiers },
        (events::MouseButton::Left, false) => Call::MouseUp { x, y, modifiers },
        (events::MouseButton::Right, _) => Call::Button {
            button: MouseButton::Right,
            down,
            x,
            y,
            modifiers,
        },
    }
}

/// Fact 1: a page-space integer widened, and nothing else.
///
/// `i32` to `f32` is lossless for every coordinate a `.evt` can hold — the
/// corpus's largest is four digits, and `f32` is exact to 2^24 — and a value
/// past that rounds rather than trapping, which is what a `double` in the C++
/// would also do.
///
/// **This stays `f32` until the facade's own signatures move.** `pdfrum-form`
/// now takes a [`kurbo::Point`] and narrows it in `route::apply`, but
/// `FormSession`'s `on_*` methods still take the flattened `x: f32, y: f32`
/// pair, so widening here would only add an `as f32` at the dispatch below.
/// The `f64::from` §A.6 prices belongs with those signatures, in the facade's
/// own package.
#[expect(
    clippy::cast_precision_loss,
    reason = "page coordinates; exact below 2^24 and the C++ widens to double here too"
)]
fn at(value: i32) -> f32 {
    value as f32
}

/// Fact 4: the grammar's three bits onto the semantic ones.
///
/// The `.evt` mask is `GetModifiers`' three `FWL_EVENTFLAG_*` values, which
/// are the only ones a script can name; the facade's set is larger because the
/// ported assertions send more. Bits outside the three are unreachable from a
/// script, so this is a total mapping rather than a lossy one.
fn to_modifiers(mask: u32) -> EventModifiers {
    let mut modifiers = EventModifiers::NONE;
    if mask & MOD_SHIFT != 0 {
        modifiers = modifiers.union(EventModifiers::SHIFT);
    }
    if mask & MOD_CONTROL != 0 {
        modifiers = modifiers.union(EventModifiers::CONTROL);
    }
    if mask & MOD_ALT != 0 {
        modifiers = modifiers.union(EventModifiers::ALT);
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
fn to_key(code: i32) -> VirtualKey {
    u16::try_from(code).map_or(VirtualKey::Unknown, VirtualKey::from_virtual)
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
        assert_eq!(to_modifiers(0), EventModifiers::NONE);
        assert_eq!(to_modifiers(MOD_SHIFT), EventModifiers::SHIFT);
        assert_eq!(to_modifiers(MOD_CONTROL), EventModifiers::CONTROL);
        assert_eq!(to_modifiers(MOD_ALT), EventModifiers::ALT);
        assert_eq!(
            to_modifiers(MOD_SHIFT | MOD_CONTROL | MOD_ALT),
            EventModifiers::SHIFT
                .union(EventModifiers::CONTROL)
                .union(EventModifiers::ALT)
        );
    }

    #[test]
    fn a_bit_no_script_can_set_maps_to_nothing() {
        // The grammar's mask only ever holds the low three bits; anything
        // else is unreachable, and must not be forwarded as a guess.
        assert_eq!(to_modifiers(1 << 9), EventModifiers::NONE);
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
        assert_eq!(to_key(9), VirtualKey::Tab);
        assert_eq!(to_key(0x0D), VirtualKey::Return);
        assert_eq!(to_key(0x41), VirtualKey::A);
        assert_eq!(to_key(0), VirtualKey::Unknown);
        // Outside u16, so it names no virtual key anywhere.
        assert_eq!(to_key(-1), VirtualKey::Unknown);
        assert_eq!(to_key(70_000), VirtualKey::Unknown);
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
                key: VirtualKey::Tab,
                modifiers: EventModifiers::NONE
            }]
        );
        // Fact 4 rides along on the same verb, which is the only one whose
        // script form can carry a modifier onto the keyboard.
        assert_eq!(
            calls("keycode,90,shiftcontrol"),
            [Call::KeyDown {
                key: VirtualKey::Z,
                modifiers: EventModifiers::SHIFT.union(EventModifiers::CONTROL),
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
                    modifiers: EventModifiers::NONE
                },
                Call::Char {
                    ch: 'b',
                    modifiers: EventModifiers::NONE
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
                    x: 150.0,
                    y: 415.0,
                    modifiers: EventModifiers::NONE
                },
                Call::MouseDown {
                    x: 150.0,
                    y: 415.0,
                    modifiers: EventModifiers::NONE
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
                x: 10.0,
                y: 20.0,
                modifiers: EventModifiers::NONE
            }]
        );
    }

    #[test]
    fn a_right_button_line_reaches_on_button_rather_than_being_dropped() {
        assert_eq!(
            calls("mousedown,right,1,2\nmouseup,right,1,2"),
            [
                Call::Button {
                    button: MouseButton::Right,
                    down: true,
                    x: 1.0,
                    y: 2.0,
                    modifiers: EventModifiers::NONE
                },
                Call::Button {
                    button: MouseButton::Right,
                    down: false,
                    x: 1.0,
                    y: 2.0,
                    modifiers: EventModifiers::NONE
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
        assert!(matches!(
            got[4],
            Call::KeyDown {
                key: VirtualKey::Left,
                ..
            }
        ));
        assert!(matches!(got[5], Call::DoubleClick { .. }));
        assert!(matches!(
            got[6],
            Call::MouseWheel {
                delta_x: 0,
                delta_y: -1,
                ..
            }
        ));
        assert!(matches!(got[7], Call::FocusAt { .. }));
    }

    #[test]
    fn every_corpus_script_maps_to_a_call_per_line_but_for_bad_code_points() {
        // The whole in-scope corpus through the bridge, asserting the only
        // thing that may shrink a script is fact 3's skip.
        let root = std::path::Path::new("/mnt/data2/pdfium/pdfium-c++/testing");
        if !root.is_dir() {
            return;
        }
        let mut files = Vec::new();
        collect_evt(root, &mut files);
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
        // Fact 1: no y-flip, no page-height subtraction, no scale.
        assert!((at(0) - 0.0).abs() < f32::EPSILON);
        assert!((at(145) - 145.0).abs() < f32::EPSILON);
        assert!((at(-3) - -3.0).abs() < f32::EPSILON);
    }
}
