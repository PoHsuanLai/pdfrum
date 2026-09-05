//! `.evt` scripts, as the oracle's event driver reads them.
//!
//! A script is a comma-separated verb per line. `#` starts a comment. Empty
//! lines, unknown verbs, and lines whose argument count the C++ rejects are
//! skipped rather than failing the parse — `SendPageEvents` never aborts a
//! file. Numbers go through C `atoi` (leading whitespace, optional sign,
//! trailing junk ignored, no digits → 0). Coordinates are **page space** —
//! PDF user space, y-up — for every verb, `mouseup` included.
//!
//! The four mouse verbs **ignore extra tokens** rather than rejecting them.

// Two readings of the oracle behind the paragraph above.
//
// Page space: public/fpdf_formfill.h documents page_x / page_y that way for
// OnMouseMove, OnLButtonDown, OnFocus and DoubleClick, and fpdf_formfill.cpp
// passes them through with no transform. The header's "in device" on
// FORM_OnLButtonUp is an upstream doc bug -- its body is identical to
// OnLButtonDown's.
//
// Extra tokens: the mouse verbs at event.cc:63, 85, 105 and 135 test arity
// with `size < N && size > N+1`, which is never true, so the test is dead.
// That is the C++'s behaviour rather than the comment's, which wanted `||`.

use std::path::{Path, PathBuf};

/// The shift-key bit in an event's `modifiers` mask.
pub const MOD_SHIFT: u32 = 1 << 0;
/// The control-key bit.
pub const MOD_CONTROL: u32 = 1 << 1;
/// The alt-key bit.
pub const MOD_ALT: u32 = 1 << 2;

/// Which mouse button a down/up event names.
///
/// `mousedoubleclick` accepts only [`MouseButton::Left`];
/// a right double-click is skipped the way an unknown button is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// The `left` token.
    Left,
    /// The `right` token.
    Right,
}

/// One scripted form event, as the oracle's driver would dispatch it.
///
/// Public fields are the seam the form-interaction slice consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// `charcode,<code>`.
    CharCode {
        /// `atoi` of the second token, passed to `FORM_OnChar`.
        code: i32,
    },
    /// `keycode,<code>[,modifiers]`.
    ///
    /// The C++ fires `FORM_OnKeyDown` then `FORM_OnKeyUp` with the same
    /// arguments; this variant is that pair, not a single edge.
    KeyCode {
        /// `atoi` of the second token.
        code: i32,
        /// Bitmask from the optional third token; 0 when omitted.
        modifiers: u32,
    },
    /// `mousedown,<left|right>,<x>,<y>[,modifiers]`.
    MouseDown {
        /// `left` or `right`; any other button name is skipped.
        button: MouseButton,
        /// Page-space X, `atoi` of the third token.
        x: i32,
        /// Page-space Y, `atoi` of the fourth token.
        y: i32,
        /// Optional fifth token; 0 when omitted.
        modifiers: u32,
    },
    /// `mouseup,<left|right>,<x>,<y>[,modifiers]`.
    MouseUp {
        /// `left` or `right`; any other button name is skipped.
        button: MouseButton,
        /// Page-space X.
        x: i32,
        /// Page-space Y.
        y: i32,
        /// Optional fifth token; 0 when omitted.
        modifiers: u32,
    },
    /// `mousedoubleclick,left,<x>,<y>[,modifiers]`.
    ///
    /// Only `left` is accepted; a right double-click prints "bad button name"
    /// and is skipped.
    MouseDoubleClick {
        /// Page-space X.
        x: i32,
        /// Page-space Y.
        y: i32,
        /// Optional fifth token; 0 when omitted.
        modifiers: u32,
    },
    /// `mousemove,<x>,<y>`.
    MouseMove {
        /// Page-space X, `atoi` of the first argument.
        x: i32,
        /// Page-space Y, `atoi` of the second argument.
        y: i32,
    },
    /// `mousewheel,<x>,<y>,<dx>,<dy>[,modifiers]`.
    MouseWheel {
        /// Page-space X of the wheel event.
        x: i32,
        /// Page-space Y of the wheel event.
        y: i32,
        /// Horizontal wheel delta.
        delta_x: i32,
        /// Vertical wheel delta.
        delta_y: i32,
        /// Optional sixth token; 0 when omitted.
        modifiers: u32,
    },
    /// `focus,<x>,<y>`.
    Focus {
        /// Page-space X.
        x: i32,
        /// Page-space Y.
        y: i32,
    },
}

/// Why an `.evt` script could not be parsed.
///
/// `SendPageEvents` never fails a file: unknown verbs and bad argument
/// counts are skipped (a line on stderr) and the rest of the script still
/// runs. [`parse_evt`] matches that and currently always returns `Ok`. This
/// type exists so the public signature can name a failure if a later caller
/// needs a hard-error channel.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvtError {
    /// A hard failure. Not produced by [`parse_evt`] today.
    #[error("evt: {0}")]
    Message(
        /// What went wrong, already phrased for the reader.
        String,
    ),
}

/// Parses a whole `.evt` script the way the oracle's driver walks one.
///
/// # Errors
///
/// Never, with the current grammar: every malformed line is skipped. The
/// `Result` is the public contract.
pub fn parse_evt(script: &str) -> Result<Vec<Event>, EvtError> {
    Ok(script.split('\n').filter_map(parse_line).collect())
}

/// The sibling `.evt` path `--send-events` looks for.
///
/// The first `".pdf"` **substring** is replaced with `".evt"` — a substring,
/// not a suffix, so a directory named `x.pdf/` is rewritten too. No match
/// means `--send-events` is a no-op. The file is not required to exist; the
/// caller checks that.
// pdfium_test.cc:2152-2156.
#[must_use]
pub fn sibling_evt_path(pdf_path: &str) -> Option<PathBuf> {
    let pos = pdf_path.find(".pdf")?;
    let mut event_filename = pdf_path.to_owned();
    event_filename.replace_range(pos..pos.saturating_add(4), ".evt");
    Some(PathBuf::from(event_filename))
}

/// Whether `path` is a readable regular file, tested before a script is
/// loaded.
// The oracle's access(..., R_OK) at pdfium_test.cc:2157.
#[must_use]
pub fn evt_is_readable(path: &Path) -> bool {
    path.is_file()
}

/// One line → at most one event. `None` is a skip: an unrecognized verb and
/// a bad argument count are both skipped rather than failing the parse.
fn parse_line(line: &str) -> Option<Event> {
    // StringSplit(line, '#'): everything after the first hash is a comment.
    // An empty pre-hash piece (blank line, or a line that is only a comment)
    // is skipped.
    let command = string_split(line, '#');
    let body = command.first().map_or("", String::as_str);
    if body.is_empty() {
        return None;
    }
    let tokens = string_split(body, ',');
    let verb = tokens.first().map_or("", String::as_str);
    match verb {
        "charcode" => char_code(&tokens),
        "keycode" => key_code(&tokens),
        "mousedown" => mouse_down(&tokens),
        "mouseup" => mouse_up(&tokens),
        "mousedoubleclick" => mouse_double_click(&tokens),
        "mousemove" => mouse_move(&tokens),
        "mousewheel" => mouse_wheel(&tokens),
        "focus" => focus(&tokens),
        // event.cc:190-192: "Unrecognized event: %s" and continue.
        _ => None,
    }
}

/// The `CharCode` verb: exactly two tokens.
fn char_code(tokens: &[String]) -> Option<Event> {
    if tokens.len() != 2 {
        return None;
    }
    Some(Event::CharCode {
        code: atoi(token(tokens, 1)),
    })
}

/// The `KeyCode` verb: two or three tokens.
fn key_code(tokens: &[String]) -> Option<Event> {
    if tokens.len() < 2 || tokens.len() > 3 {
        return None;
    }
    Some(Event::KeyCode {
        code: atoi(token(tokens, 1)),
        modifiers: optional_modifiers(tokens, 2),
    })
}

/// The `MouseDown` verb.
///
/// The arity test is dead (`< 4 && > 5`). Extra tokens are ignored. Missing
/// `left`/`right`/`x`/`y` cannot be indexed in C++ (it would crash); we skip
/// rather than panic.
fn mouse_down(tokens: &[String]) -> Option<Event> {
    mouse_button_event(tokens, |button, x, y, modifiers| Event::MouseDown {
        button,
        x,
        y,
        modifiers,
    })
}

/// The `MouseUp` verb. Same dead arity test as mousedown.
fn mouse_up(tokens: &[String]) -> Option<Event> {
    mouse_button_event(tokens, |button, x, y, modifiers| Event::MouseUp {
        button,
        x,
        y,
        modifiers,
    })
}

/// Shared `left`/`right` + x + y + optional modifiers shape.
fn mouse_button_event<F>(tokens: &[String], wrap: F) -> Option<Event>
where
    F: FnOnce(MouseButton, i32, i32, u32) -> Event,
{
    let button = parse_button(tokens.get(1)?)?;
    let x = atoi(tokens.get(2)?);
    let y = atoi(tokens.get(3)?);
    let modifiers = optional_modifiers(tokens, 4);
    Some(wrap(button, x, y, modifiers))
}

/// The `MouseDoubleClick` verb: `left` only.
fn mouse_double_click(tokens: &[String]) -> Option<Event> {
    if tokens.get(1).map(String::as_str) != Some("left") {
        return None;
    }
    let x = atoi(tokens.get(2)?);
    let y = atoi(tokens.get(3)?);
    Some(Event::MouseDoubleClick {
        x,
        y,
        modifiers: optional_modifiers(tokens, 4),
    })
}

/// The `MouseMove` verb: exactly three tokens.
fn mouse_move(tokens: &[String]) -> Option<Event> {
    if tokens.len() != 3 {
        return None;
    }
    Some(Event::MouseMove {
        x: atoi(token(tokens, 1)),
        y: atoi(token(tokens, 2)),
    })
}

/// The `MouseWheel` verb. Dead arity test (`< 5 && > 6`).
fn mouse_wheel(tokens: &[String]) -> Option<Event> {
    Some(Event::MouseWheel {
        x: atoi(tokens.get(1)?),
        y: atoi(tokens.get(2)?),
        delta_x: atoi(tokens.get(3)?),
        delta_y: atoi(tokens.get(4)?),
        modifiers: optional_modifiers(tokens, 5),
    })
}

/// The `Focus` verb: exactly three tokens.
fn focus(tokens: &[String]) -> Option<Event> {
    if tokens.len() != 3 {
        return None;
    }
    Some(Event::Focus {
        x: atoi(token(tokens, 1)),
        y: atoi(token(tokens, 2)),
    })
}

/// The button a `left`/`right` token names; any other spelling is unknown.
fn parse_button(name: &str) -> Option<MouseButton> {
    match name {
        "left" => Some(MouseButton::Left),
        "right" => Some(MouseButton::Right),
        _ => None,
    }
}

/// The modifier mask: substring search, not token equality.
fn parse_modifiers(text: &str) -> u32 {
    let mut modifiers = 0;
    if text.contains("shift") {
        modifiers |= MOD_SHIFT;
    }
    if text.contains("control") {
        modifiers |= MOD_CONTROL;
    }
    if text.contains("alt") {
        modifiers |= MOD_ALT;
    }
    modifiers
}

/// The modifier mask at `index`, or an empty mask when the line omits it.
fn optional_modifiers(tokens: &[String], index: usize) -> u32 {
    tokens.get(index).map_or(0, |text| parse_modifiers(text))
}

/// The token at `index`, or the empty string, which `atoi` reads as zero.
fn token(tokens: &[String], index: usize) -> &str {
    tokens.get(index).map_or("", String::as_str)
}

/// Split on `delimiter`, keeping empty pieces, always at least one element
/// (the tail after the last hit).
fn string_split(text: &str, delimiter: char) -> Vec<String> {
    text.split(delimiter).map(str::to_owned).collect()
}

/// C `atoi`: skip leading `isspace`, optional `+`/`-`, then digits. No
/// conversion yields 0. Overflow saturates to `i32`'s range, which is what
/// glibc `strtol` does for `atoi` on this platform.
fn atoi(text: &str) -> i32 {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() && is_c_space(bytes[i]) {
        i += 1;
    }
    let mut negative = false;
    if let Some(&b) = bytes.get(i) {
        match b {
            b'+' => i += 1,
            b'-' => {
                negative = true;
                i += 1;
            }
            _ => {}
        }
    }
    let mut acc: i64 = 0;
    let mut seen_digit = false;
    while let Some(&b) = bytes.get(i) {
        if !b.is_ascii_digit() {
            break;
        }
        seen_digit = true;
        acc = acc.saturating_mul(10).saturating_add(i64::from(b - b'0'));
        i += 1;
    }
    if !seen_digit {
        return 0;
    }
    let signed = if negative { -acc } else { acc };
    i32::try_from(signed.clamp(i64::from(i32::MIN), i64::from(i32::MAX))).unwrap_or(0)
}

/// C `isspace` over bytes, the leading run `atoi` skips.
fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn parse(script: &str) -> Vec<Event> {
        parse_evt(script).expect("parse_evt matches event.cc and does not fail a script")
    }

    #[test]
    fn evt_error_displays() {
        let err = EvtError::Message("unused".to_owned());
        assert_eq!(err.to_string(), "evt: unused");
    }

    #[test]
    fn charcode_takes_exactly_one_integer() {
        assert_eq!(parse("charcode,97"), [Event::CharCode { code: 97 }]);
        assert_eq!(parse("charcode,13"), [Event::CharCode { code: 13 }]);
        assert!(parse("charcode").is_empty());
        assert!(parse("charcode,97,1").is_empty());
    }

    #[test]
    fn keycode_takes_an_optional_modifier_string() {
        assert_eq!(
            parse("keycode,9"),
            [Event::KeyCode {
                code: 9,
                modifiers: 0
            }]
        );
        assert_eq!(
            parse("keycode,9,shift"),
            [Event::KeyCode {
                code: 9,
                modifiers: MOD_SHIFT
            }]
        );
        assert!(parse("keycode").is_empty());
        assert!(parse("keycode,9,shift,extra").is_empty());
    }

    #[test]
    fn mousedown_and_mouseup_accept_left_and_right() {
        assert_eq!(
            parse("mousedown,left,145,220"),
            [Event::MouseDown {
                button: MouseButton::Left,
                x: 145,
                y: 220,
                modifiers: 0,
            }]
        );
        assert_eq!(
            parse("mouseup,right,125,225"),
            [Event::MouseUp {
                button: MouseButton::Right,
                x: 125,
                y: 225,
                modifiers: 0,
            }]
        );
        assert_eq!(
            parse("mousedown,left,370,145,shift"),
            [Event::MouseDown {
                button: MouseButton::Left,
                x: 370,
                y: 145,
                modifiers: MOD_SHIFT,
            }]
        );
    }

    #[test]
    fn a_bad_mouse_button_is_skipped() {
        assert!(parse("mousedown,middle,1,2").is_empty());
        assert!(parse("mouseup,wheel,1,2").is_empty());
        assert!(parse("mousedoubleclick,right,1,2").is_empty());
    }

    #[test]
    fn mousedoubleclick_is_left_only() {
        assert_eq!(
            parse("mousedoubleclick,left,125,225"),
            [Event::MouseDoubleClick {
                x: 125,
                y: 225,
                modifiers: 0,
            }]
        );
    }

    #[test]
    fn mousemove_and_focus_take_two_coordinates() {
        assert_eq!(
            parse("mousemove,125,225"),
            [Event::MouseMove { x: 125, y: 225 }]
        );
        assert_eq!(parse("focus,125,225"), [Event::Focus { x: 125, y: 225 }]);
        assert!(parse("mousemove,125").is_empty());
        assert!(parse("focus,125,225,0").is_empty());
    }

    #[test]
    fn mousewheel_takes_point_and_delta() {
        assert_eq!(
            parse("mousewheel,150,415,0,-1"),
            [Event::MouseWheel {
                x: 150,
                y: 415,
                delta_x: 0,
                delta_y: -1,
                modifiers: 0,
            }]
        );
        assert_eq!(
            parse("mousewheel,1,2,3,4,alt"),
            [Event::MouseWheel {
                x: 1,
                y: 2,
                delta_x: 3,
                delta_y: 4,
                modifiers: MOD_ALT,
            }]
        );
    }

    #[test]
    fn mouse_verbs_ignore_extra_tokens_because_the_arity_test_is_dead() {
        // event.cc:63 is `size < 4 && size > 5`, never true. A sixth token
        // is ignored rather than rejecting the line.
        assert_eq!(
            parse("mousedown,left,1,2,shift,junk"),
            [Event::MouseDown {
                button: MouseButton::Left,
                x: 1,
                y: 2,
                modifiers: MOD_SHIFT,
            }]
        );
        assert_eq!(
            parse("mousewheel,1,2,3,4,alt,junk"),
            [Event::MouseWheel {
                x: 1,
                y: 2,
                delta_x: 3,
                delta_y: 4,
                modifiers: MOD_ALT,
            }]
        );
    }

    #[test]
    fn a_short_mouse_line_is_skipped_rather_than_panicking() {
        // C++ would index past `tokens.size()`; we skip.
        assert!(parse("mousedown,left").is_empty());
        assert!(parse("mousewheel,1,2").is_empty());
        assert!(parse("mousedoubleclick,left").is_empty());
    }

    #[test]
    fn unknown_verbs_and_blank_and_comment_lines_are_skipped() {
        let script = "\n\
                      # just a comment\n\
                      unknown,1,2\n\
                      mousemove,1,2\n\
                      ";
        assert_eq!(parse(script), [Event::MouseMove { x: 1, y: 2 }]);
    }

    #[test]
    fn a_hash_starts_a_comment_even_mid_line() {
        assert_eq!(
            parse("charcode,97 # letter a"),
            [Event::CharCode { code: 97 }]
        );
    }

    #[test]
    fn atoi_matches_c_on_junk_whitespace_and_signs() {
        assert_eq!(parse("charcode,  97"), [Event::CharCode { code: 97 }]);
        assert_eq!(parse("charcode,+13"), [Event::CharCode { code: 13 }]);
        assert_eq!(parse("charcode,-4"), [Event::CharCode { code: -4 }]);
        assert_eq!(parse("charcode,3x"), [Event::CharCode { code: 3 }]);
        assert_eq!(parse("charcode,abc"), [Event::CharCode { code: 0 }]);
        assert_eq!(parse("charcode,"), [Event::CharCode { code: 0 }]);
    }

    #[test]
    fn modifiers_are_substrings_and_combine() {
        assert_eq!(
            parse("keycode,9,shiftcontrolalt"),
            [Event::KeyCode {
                code: 9,
                modifiers: MOD_SHIFT | MOD_CONTROL | MOD_ALT,
            }]
        );
        // Case-sensitive, like `string::find`.
        assert_eq!(
            parse("keycode,9,Shift"),
            [Event::KeyCode {
                code: 9,
                modifiers: 0
            }]
        );
    }

    #[test]
    fn sibling_evt_path_replaces_the_first_pdf_substring() {
        assert_eq!(
            sibling_evt_path("input.pdf").as_deref(),
            Some(Path::new("input.evt"))
        );
        assert_eq!(
            sibling_evt_path("/tmp/foo.pdf").as_deref(),
            Some(Path::new("/tmp/foo.evt"))
        );
        // First occurrence, the way `std::string::find` works.
        assert_eq!(
            sibling_evt_path("/tmp/a.pdf/b.pdf").as_deref(),
            Some(Path::new("/tmp/a.evt/b.pdf"))
        );
        assert_eq!(sibling_evt_path("no-extension"), None);
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
    fn every_corpus_evt_parses_without_error() {
        let root = oracle_checkout().join("testing");
        if !root.is_dir() {
            return;
        }
        let mut files = Vec::new();
        collect_evt(&root, &mut files);
        files.sort();
        assert_eq!(
            files.len(),
            59,
            "oracle checkout should still hold 59 .evt files, found {}",
            files.len()
        );
        // `parse_evt` is infallible by construction, so `Ok` alone proves
        // nothing. Every corpus script holds at least one real verb, so the
        // load-bearing assertion is that each file yields events: a grammar
        // regression that skipped every line would still return `Ok(vec![])`.
        let mut total = 0usize;
        for path in &files {
            let text = std::fs::read_to_string(path).unwrap_or_else(|err| {
                panic!("reading {}: {err}", path.display());
            });
            let events = parse_evt(&text).unwrap_or_else(|err| {
                panic!("{}: {err}", path.display());
            });
            assert!(
                !events.is_empty(),
                "{} parsed to no events at all",
                path.display()
            );
            // Every non-comment, non-blank line of the corpus is a verb the
            // grammar knows, so nothing may be dropped.
            let verbs = text
                .split('\n')
                .filter(|line| !line.split('#').next().unwrap_or("").is_empty())
                .count();
            assert_eq!(
                events.len(),
                verbs,
                "{}: {verbs} verb lines but {} events",
                path.display(),
                events.len()
            );
            total += events.len();
        }
        assert!(
            total > 200,
            "corpus should parse to many events, got {total}"
        );
    }

    fn collect_evt(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_evt(&path, out);
            } else if path.extension() == Some(OsStr::new("evt")) {
                out.push(path);
            }
        }
    }
}
