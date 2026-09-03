//! What a script asked the host to do, as data the host reads.
//!
//! **A value, not a trait.** The script does not need the host's answer to
//! continue, so the line goes on a list and the host reads the list on its own
//! schedule.
//!
//! It is also the conformance comparison surface: a golden run compares the
//! oracle's stdout, line-wise and byte-exact, against these lines rendered.
//! [`TranscriptLine::render`] is that contract rather than a debugging
//! convenience.

use std::fmt::Write as _;

/// One thing a script asked the host to do.
///
/// Ten shapes, because the goldens carry ten. Each variant's `render` output
/// is quoted from the callback that produces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptLine {
    /// `app.alert` — 1933 of the 2004 golden lines.
    ///
    /// The message is printed with a conditional decoration: see
    /// [`TranscriptLine::render`].
    Alert {
        /// `cTitle`, defaulting to the literal `"Alert"`.
        title: String,
        /// `cMsg`. An array argument is joined first — see `app.alert`.
        message: String,
        /// `nIcon`, defaulting to 0.
        icon: i32,
        /// `nType`, defaulting to 0.
        button: i32,
    },
    /// `app.beep(n)` — `ExampleAppBeep`.
    Beep(i32),
    /// `app.response(...)` — `ExampleAppResponse`. The host answers with the
    /// empty string; nothing is prompted.
    Response {
        /// `cQuestion`.
        question: String,
        /// `cTitle`.
        title: String,
        /// `cDefault`.
        default_value: String,
        /// `cLabel`.
        label: String,
        /// `bPassword`.
        password: bool,
    },
    /// `app.mailMsg` / `Doc.mailDoc` / `Doc.mailForm` — `ExampleDocMail`.
    /// **Nothing is sent**: no network, no MAPI, no process spawn.
    MailMsg {
        /// `bUI`.
        ui: bool,
        /// `To`.
        to: String,
        /// `cc`.
        cc: String,
        /// `bcc`.
        bcc: String,
        /// `cSubject`.
        subject: String,
        /// `cMsg`.
        body: String,
    },
    /// `Doc.print` — `ExampleDocPrint`. **Nothing is printed.**
    Print {
        /// `bUI`.
        ui: bool,
        /// `nStart`.
        start: i32,
        /// `nEnd`.
        end: i32,
        /// `bSilent`.
        silent: bool,
        /// `bShrinkToFit`.
        shrink_to_fit: bool,
        /// `bPrintAsImage`.
        print_as_image: bool,
        /// `bReverse`.
        reverse: bool,
        /// `bAnnotations`.
        annotations: bool,
    },
    /// `Doc.submitForm` — `ExampleDocSubmitForm`.
    ///
    /// **The most dangerous entry point in the object model, and the one
    /// where "hand back the data, do not act on it" matters most.** No request
    /// is made; the URL and the bytes come back here for a host to decide
    /// about.
    SubmitForm {
        /// Where the script wanted to post.
        url: String,
        /// What it wanted to post.
        data: Vec<u8>,
    },
    /// `Doc.gotoNamedDest` and page navigation — `ExampleDocGotoPage`.
    GotoPage(i32),
    /// A named action fired from a non-JavaScript callback —
    /// `ExampleNamedAction`. `named_action.in` is its one fixture.
    NamedAction(String),
    /// `console.println`. **The oracle discards it**, so this variant renders
    /// to nothing and is carried only so a host can see what a script
    /// logged.
    ConsolePrintln(String),
    /// An `AF*` function's own alert, which is not routed through `app.alert`.
    ///
    /// The **function's own name** is the title, always with `icon = 3` and
    /// `button = 0` — so these render in the decorated form and appear
    /// *unprefixed by `Alert:`*, interleaved with the plain lines. Getting
    /// that interleaving right is a scoring requirement.
    FunctionAlert {
        /// The function's own name, e.g. `AFNumber_Keystroke`.
        caller: String,
        /// The message.
        message: String,
    },
}

/// The literal `cTitle` an `app.alert` with no title takes.
pub const DEFAULT_ALERT_TITLE: &str = "Alert";

impl TranscriptLine {
    /// The line the oracle would print, without its newline.
    ///
    /// `None` for a shape that prints nothing — [`TranscriptLine::ConsolePrintln`]
    /// alone, because the oracle's `console` methods are empty.
    ///
    /// # The alert decoration is conditional, and it is exact
    ///
    /// Three forms, chosen by what differs from the default:
    ///
    /// | condition | line |
    /// |---|---|
    /// | default title, icon and type | `Alert: <msg>` |
    /// | non-default title only | `<title>: <msg>` |
    /// | non-default icon **or** type | `<title>[icon=N,type=N]: <msg>` |
    ///
    /// An `AF*` alert always takes the third form with `icon=3,type=0`, so it
    /// reads `AFNumber_Keystroke[icon=3,type=0]: The input value is invalid.`
    /// with no `Alert:` prefix.
    #[must_use]
    pub fn render(&self) -> Option<String> {
        let mut out = String::new();
        match self {
            TranscriptLine::Alert {
                title,
                message,
                icon,
                button,
            } => {
                if *icon != 0 || *button != 0 {
                    let _ = write!(out, "{title}[icon={icon},type={button}]: {message}");
                } else {
                    let _ = write!(out, "{title}: {message}");
                }
            }
            TranscriptLine::FunctionAlert { caller, message } => {
                // `AlertIfPossible` is always icon 3, type 0.
                let _ = write!(out, "{caller}[icon=3,type=0]: {message}");
            }
            TranscriptLine::Beep(kind) => {
                let _ = write!(out, "BEEP!!! {kind}");
            }
            TranscriptLine::Response {
                question,
                title,
                default_value,
                label,
                password,
            } => {
                // `"%ls: %ls, …"` — **title first, then the question**, and
                // `length` is the *buffer* the host offered rather than
                // anything the script said. `pdfium_test` always passes 2048.
                let _ = write!(
                    out,
                    "{title}: {question}, defaultValue={default_value}, label={label}, \
                     isPassword={}, length=2048",
                    u8::from(*password)
                );
            }
            TranscriptLine::MailMsg {
                ui,
                to,
                cc,
                bcc,
                subject,
                body,
            } => {
                let _ = write!(
                    out,
                    "Mail Msg: {}, to={to}, cc={cc}, bcc={bcc}, subject={subject}, body={body}",
                    u8::from(*ui)
                );
            }
            TranscriptLine::Print {
                ui,
                start,
                end,
                silent,
                shrink_to_fit,
                print_as_image,
                reverse,
                annotations,
            } => {
                let _ = write!(
                    out,
                    "Doc Print: {}, {start}, {end}, {}, {}, {}, {}, {}",
                    u8::from(*ui),
                    u8::from(*silent),
                    u8::from(*shrink_to_fit),
                    u8::from(*print_as_image),
                    u8::from(*reverse),
                    u8::from(*annotations)
                );
            }
            TranscriptLine::SubmitForm { url, data } => {
                // Two lines: the header, then **every byte on one line**,
                // each as `" %02x"` — a *leading* space, no wrapping and no
                // indent, however long the form is. `submitform_expected.txt`
                // carries a 174-byte dump as a single line.
                let _ = writeln!(
                    out,
                    "Doc Submit Form: url={url} + {} data bytes:",
                    data.len()
                );
                for byte in data {
                    let _ = write!(out, " {byte:02x}");
                }
            }
            TranscriptLine::GotoPage(page) => {
                let _ = write!(out, "Goto Page: {page}");
            }
            TranscriptLine::NamedAction(name) => {
                let _ = write!(out, "Execute named action: {name}");
            }
            // Upstream's `console` is four empty functions.
            TranscriptLine::ConsolePrintln(_) => return None,
        }
        Some(out)
    }

    /// A default-decoration alert: the shape 1922 of the 2004 golden lines
    /// take.
    #[must_use]
    pub fn alert(message: impl Into<String>) -> TranscriptLine {
        TranscriptLine::Alert {
            title: DEFAULT_ALERT_TITLE.to_string(),
            message: message.into(),
            icon: 0,
            button: 0,
        }
    }
}

/// Renders a whole transcript the way the oracle writes stdout: one line
/// each, newline-terminated, lines that print nothing omitted.
#[must_use]
pub fn render(lines: &[TranscriptLine]) -> String {
    let mut out = String::new();
    for line in lines {
        if let Some(rendered) = line.render() {
            out.push_str(&rendered);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default form, which is 1922 of the 2004 golden lines.
    #[test]
    fn a_default_alert_is_prefixed_with_the_literal_alert() {
        assert_eq!(
            TranscriptLine::alert("Hello").render().as_deref(),
            Some("Alert: Hello")
        );
    }

    /// A non-default title replaces the prefix rather than adding to it.
    #[test]
    fn a_titled_alert_prints_its_own_title() {
        let line = TranscriptLine::Alert {
            title: "Warning".to_string(),
            message: "careful".to_string(),
            icon: 0,
            button: 0,
        };
        assert_eq!(line.render().as_deref(), Some("Warning: careful"));
    }

    /// A non-default icon **or** type adds the bracket, and both are printed
    /// whichever one differs.
    #[test]
    fn a_decorated_alert_prints_both_numbers() {
        let with_icon = TranscriptLine::Alert {
            title: "Alert".to_string(),
            message: "m".to_string(),
            icon: 3,
            button: 0,
        };
        assert_eq!(
            with_icon.render().as_deref(),
            Some("Alert[icon=3,type=0]: m")
        );

        let with_type = TranscriptLine::Alert {
            title: "Alert".to_string(),
            message: "m".to_string(),
            icon: 0,
            button: 2,
        };
        assert_eq!(
            with_type.render().as_deref(),
            Some("Alert[icon=0,type=2]: m")
        );
    }

    /// An `AF*` alert takes the third form with the function's own name and
    /// no `Alert:` prefix — `public_methods_expected.txt`'s interleaving.
    #[test]
    fn a_function_alert_is_titled_by_its_caller() {
        let line = TranscriptLine::FunctionAlert {
            caller: "AFNumber_Keystroke".to_string(),
            message: "The input value is invalid.".to_string(),
        };
        assert_eq!(
            line.render().as_deref(),
            Some("AFNumber_Keystroke[icon=3,type=0]: The input value is invalid.")
        );
    }

    /// `console.println` prints nothing, because upstream's four console
    /// methods are empty — a golden expecting output would pin behaviour
    /// PDFium does not have.
    #[test]
    fn console_output_is_discarded_as_upstream_discards_it() {
        assert_eq!(
            TranscriptLine::ConsolePrintln("x".to_string()).render(),
            None
        );
        // And it is omitted from the rendered transcript rather than leaving
        // a blank line, which a line-wise diff would see.
        let rendered = render(&[
            TranscriptLine::alert("one"),
            TranscriptLine::ConsolePrintln("hidden".to_string()),
            TranscriptLine::alert("two"),
        ]);
        assert_eq!(rendered, "Alert: one\nAlert: two\n");
    }

    #[test]
    fn the_other_seven_shapes_render_their_golden_form() {
        assert_eq!(
            TranscriptLine::Beep(2).render().as_deref(),
            Some("BEEP!!! 2")
        );
        assert_eq!(
            TranscriptLine::GotoPage(4).render().as_deref(),
            Some("Goto Page: 4")
        );
        assert_eq!(
            TranscriptLine::NamedAction("Print".to_string())
                .render()
                .as_deref(),
            Some("Execute named action: Print")
        );
        assert_eq!(
            TranscriptLine::Response {
                question: "q".to_string(),
                title: "t".to_string(),
                default_value: String::new(),
                label: String::new(),
                password: false,
            }
            .render()
            .as_deref(),
            Some("t: q, defaultValue=, label=, isPassword=0, length=2048")
        );
        assert_eq!(
            TranscriptLine::MailMsg {
                ui: true,
                to: String::new(),
                cc: String::new(),
                bcc: String::new(),
                subject: String::new(),
                body: String::new(),
            }
            .render()
            .as_deref(),
            Some("Mail Msg: 1, to=, cc=, bcc=, subject=, body=")
        );
        assert_eq!(
            TranscriptLine::Print {
                ui: false,
                start: 0,
                end: 0,
                silent: false,
                shrink_to_fit: false,
                print_as_image: false,
                reverse: false,
                annotations: false,
            }
            .render()
            .as_deref(),
            Some("Doc Print: 0, 0, 0, 0, 0, 0, 0, 0")
        );
    }

    /// **The dump does not wrap.** Every byte goes on one line as `" %02x"`,
    /// however long the form is — `submitform_expected.txt` carries 174 bytes
    /// as a single line, and a reimplementation that wrapped at sixteen would
    /// fail a byte-exact diff on both of the two fixtures that reach it.
    #[test]
    fn a_submitted_form_dumps_every_byte_on_one_line() {
        let line = TranscriptLine::SubmitForm {
            url: "https://example.com".to_string(),
            // `submitform.in`'s shorter dump, verbatim: "name=Tralfaz&age=12".
            data: b"name=Tralfaz&age=12".to_vec(),
        };
        let rendered = line.render().unwrap_or_default();
        let mut lines = rendered.lines();
        assert_eq!(
            lines.next(),
            Some("Doc Submit Form: url=https://example.com + 19 data bytes:")
        );
        assert_eq!(
            lines.next(),
            Some(" 6e 61 6d 65 3d 54 72 61 6c 66 61 7a 26 61 67 65 3d 31 32")
        );
        assert_eq!(lines.next(), None);
    }
}
