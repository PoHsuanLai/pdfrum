//! What the terminal can do, and the escape sequences that use it.
//!
//! Three capabilities, each decided once from the flags and the
//! environment and then handed around as a value: colour (SGR), hyperlinks
//! (OSC 8), and pictures (kitty's graphics protocol, iTerm2's inline
//! images, or truecolor half-blocks that any 24-bit terminal shows). Every
//! one of them is off when stdout is not a terminal, so a pipe sees plain
//! bytes, and each has a `never` a user can insist on.

use std::fmt::Write;
use std::io::IsTerminal;

use clap::ValueEnum;
use pdfrum::Pixmap;

/// The `auto|always|never` every capability flag takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum When {
    /// On when stdout is a terminal that looks able.
    #[default]
    Auto,
    /// On regardless.
    Always,
    /// Off regardless.
    Never,
}

/// The `--graphics` choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum GraphicsMode {
    /// Pick from the terminal's own announcements.
    #[default]
    Auto,
    /// Kitty's graphics protocol (`kitty`, `WezTerm`, `Ghostty`, `Konsole`).
    Kitty,
    /// `iTerm2`'s inline images (`iTerm2`, `WezTerm`, VS Code's terminal).
    Iterm,
    /// Two pixels per cell with 24-bit colour; any modern terminal.
    Halfblock,
    /// No pictures.
    Off,
}

/// How a picture reaches the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Graphics {
    Kitty,
    Iterm,
    Halfblock,
    Off,
}

/// The roles colour plays in the output, and nothing else is ever painted
/// (`docs/design/cli-style.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// A section heading: `page 3`.
    Heading,
    /// The key column of a record; the header row of a table.
    Key,
    /// Something a person would copy: a path, a page or object number, a name.
    Ident,
    /// Secondary detail after the main fact: a size, a count, an offset.
    Muted,
    /// A good state.
    Ok,
    /// A state to notice.
    Warn,
    /// An error line.
    Error,
    /// The matched text in `search`.
    Match,
    /// A line only the right-hand document has.
    Added,
    /// A line only the left-hand document has.
    Removed,
    /// The pager's status bar.
    Bar,
    /// Text that is a hyperlink: underlined, so it reads as one at rest and
    /// not only on hover.
    Link,
}

impl Style {
    fn sgr(self) -> &'static str {
        match self {
            Self::Heading => "1",
            Self::Key | Self::Muted => "2",
            Self::Ident => "36",
            Self::Ok | Self::Added => "32",
            Self::Warn => "33",
            Self::Error => "1;31",
            Self::Match => "1;33",
            Self::Removed => "31",
            Self::Bar => "7",
            Self::Link => "4;36",
        }
    }
}

/// The capabilities in force for this run.
#[derive(Debug, Clone, Copy)]
pub struct Term {
    pub color: bool,
    pub hyperlinks: bool,
    pub graphics: Graphics,
    /// Whether stdout is a terminal at all — tables and progress bars are
    /// for people, plain columns for pipes.
    pub interactive: bool,
}

impl Term {
    /// Decide from the flags and the environment.
    pub fn detect(color: When, hyperlinks: When, graphics: GraphicsMode) -> Self {
        let interactive = std::io::stdout().is_terminal();
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let term = std::env::var("TERM").unwrap_or_default();
        let dumb = term == "dumb";
        let on = |when: When, able: bool| match when {
            When::Auto => interactive && able,
            When::Always => true,
            When::Never => false,
        };
        let graphics = match graphics {
            GraphicsMode::Off => Graphics::Off,
            GraphicsMode::Kitty => Graphics::Kitty,
            GraphicsMode::Iterm => Graphics::Iterm,
            GraphicsMode::Halfblock => Graphics::Halfblock,
            GraphicsMode::Auto if !interactive || dumb => Graphics::Off,
            GraphicsMode::Auto => announced_graphics(&term),
        };
        Self {
            color: on(color, !no_color && !dumb),
            hyperlinks: on(hyperlinks, !dumb),
            graphics,
            interactive,
        }
    }

    /// `text` in `style`, when colour is on. The only way anything is
    /// painted (`docs/design/cli-style.md` §2).
    pub fn paint(self, style: Style, text: &str) -> String {
        if self.color {
            format!("\x1b[{}m{text}\x1b[0m", style.sgr())
        } else {
            text.to_owned()
        }
    }

    /// `text` as an OSC 8 hyperlink to `url` when hyperlinks are on, and
    /// underlined when colour is, so a terminal that only marks links on
    /// hover still shows there is one.
    pub fn link(self, url: &str, text: &str) -> String {
        if self.hyperlinks && !url.is_empty() {
            let shown = self.paint(Style::Link, text);
            format!("\x1b]8;;{url}\x1b\\{shown}\x1b]8;;\x1b\\")
        } else {
            text.to_owned()
        }
    }
}

/// Columns and rows of the terminal, or a plain 80 by 24 when there is no
/// terminal to ask.
pub fn size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((80, 24))
}

/// What the environment says about pictures: kitty's protocol where a
/// terminal announces itself as kitty-family, iTerm2's where it is
/// iTerm2 or VS Code, half-blocks everywhere else that speaks 24-bit colour.
fn announced_graphics(term: &str) -> Graphics {
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let program = program.to_ascii_lowercase();
    if std::env::var_os("KITTY_WINDOW_ID").is_some()
        || term.contains("kitty")
        || term.contains("ghostty")
        || program.contains("wezterm")
        || program.contains("ghostty")
        || std::env::var_os("KONSOLE_VERSION").is_some()
    {
        return Graphics::Kitty;
    }
    if program.contains("iterm") || program.contains("vscode") {
        return Graphics::Iterm;
    }
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    if colorterm == "truecolor" || colorterm == "24bit" || term.contains("256color") {
        return Graphics::Halfblock;
    }
    Graphics::Off
}

/// The bytes that show `pixmap` in the terminal at its pixel size (kitty,
/// iTerm2) or `columns` wide (half-blocks, two pixels per row of cells).
/// Half-block rows start `pad` cells in; the pixel protocols draw at the
/// cursor, so a caller places that itself.
pub fn picture(pixmap: &Pixmap, graphics: Graphics, columns: u16, pad: u16) -> Vec<u8> {
    match graphics {
        Graphics::Off => Vec::new(),
        Graphics::Kitty => kitty(pixmap),
        Graphics::Iterm => iterm(pixmap),
        Graphics::Halfblock => halfblock(pixmap, columns, pad),
    }
}

/// The size of one cell in pixels, when the terminal says (`kitty`,
/// `WezTerm`, `Ghostty` and `iTerm2` do), else the common 8 by 16.
#[derive(Debug, Clone, Copy)]
pub struct Screen {
    pub cell_width: f64,
    pub cell_height: f64,
}

pub fn screen() -> Screen {
    let (cell_width, cell_height) = crossterm::terminal::window_size()
        .ok()
        .filter(|w| w.width > 0 && w.height > 0 && w.columns > 0 && w.rows > 0)
        .map_or((8.0, 16.0), |w| {
            (
                f64::from(w.width) / f64::from(w.columns),
                f64::from(w.height) / f64::from(w.rows),
            )
        });
    Screen {
        cell_width,
        cell_height,
    }
}

/// Kitty graphics protocol: the PNG in base64, in 4096-byte chunks, each
/// an APC `_G` command; `f=100` says PNG, `a=T` transmit and display,
/// `m=1` more chunks follow.
fn kitty(pixmap: &Pixmap) -> Vec<u8> {
    let Ok(png) = pixmap.encode_png() else {
        return Vec::new();
    };
    let mut out = kitty_chunks(&png, "a=T,f=100,q=2");
    out.push(b'\n');
    out
}

/// The PNG as chunked `_G` commands whose first carries `control`.
fn kitty_chunks(png: &[u8], control: &str) -> Vec<u8> {
    let encoded = base64(png);
    let mut out = Vec::with_capacity(encoded.len() + 64);
    let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(4096).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let last = i + 1 == chunks.len();
        let head = if i == 0 {
            format!("{control},m={}", u8::from(!last))
        } else {
            format!("m={}", u8::from(!last))
        };
        out.extend_from_slice(b"\x1b_G");
        out.extend_from_slice(head.as_bytes());
        out.push(b';');
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

/// Kitty, in two steps so a page is sent once and shown many times:
/// [`kitty_transmit`] stores `png` under `id` without drawing it,
/// [`kitty_place`] draws the stored image at the cursor, and
/// [`kitty_delete`] frees it. Placing is a few bytes; transmitting is the
/// whole picture, so a pager transmits while it waits for a key.
pub fn kitty_transmit(id: u32, png: &[u8]) -> Vec<u8> {
    kitty_chunks(png, &format!("a=t,f=100,q=2,i={id}"))
}

/// Draw image `id` at the cursor, scaled into exactly `cols` by `rows`
/// cells (`c=`, `r=`), and leave the cursor where it is (`C=1`) — so a
/// picture can never run past the row it was given and scroll the screen.
pub fn kitty_place(id: u32, cols: u16, rows: u16) -> Vec<u8> {
    format!("\x1b_Ga=p,q=2,i={id},c={cols},r={rows},C=1\x1b\\").into_bytes()
}

/// iTerm2 inline image of `png`, sized in cells rather than pixels, so it
/// fills the box it was laid out for and no more.
pub fn iterm_cells(png: &[u8], cols: u16, rows: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(png.len() * 4 / 3 + 64);
    out.extend_from_slice(
        format!(
            "\x1b]1337;File=inline=1;size={};width={cols};height={rows};preserveAspectRatio=1:",
            png.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(base64(png).as_bytes());
    out.extend_from_slice(b"\x07");
    out
}

pub fn kitty_delete(id: u32) -> Vec<u8> {
    format!("\x1b_Ga=d,d=I,q=2,i={id}\x1b\\").into_bytes()
}

/// iTerm2 inline image: OSC 1337 `File=inline=1:` then the PNG in base64.
fn iterm(pixmap: &Pixmap) -> Vec<u8> {
    let Ok(png) = pixmap.encode_png() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(png.len() * 4 / 3 + 64);
    out.extend_from_slice(
        format!(
            "\x1b]1337;File=inline=1;size={};width={}px;height={}px;preserveAspectRatio=1:",
            png.len(),
            pixmap.width(),
            pixmap.height()
        )
        .as_bytes(),
    );
    out.extend_from_slice(base64(&png).as_bytes());
    out.extend_from_slice(b"\x07\n");
    out
}

/// Half-blocks: each cell shows two pixels, the upper as the foreground of
/// `▀` and the lower as its background, in 24-bit colour. The picture is
/// scaled to `columns` cells wide by nearest-neighbour sampling.
fn halfblock(pixmap: &Pixmap, columns: u16, pad: u16) -> Vec<u8> {
    let (w, h) = (pixmap.width(), pixmap.height());
    if w == 0 || h == 0 || columns == 0 {
        return Vec::new();
    }
    let rgba = pixmap.to_straight_rgba();
    let cols = u32::from(columns).min(w);
    let step = f64::from(w) / f64::from(cols);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a positive cell count no larger than the picture's height"
    )]
    let rows = ((f64::from(h) / step) / 2.0).ceil().max(1.0) as u32;
    let pixel = |x: u32, y: u32| -> [u8; 3] {
        let i = ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize;
        // Composite over white, the way a page is meant to be seen.
        let a = u32::from(rgba.get(i + 3).copied().unwrap_or(255));
        let over = |c: u8| u8::try_from((u32::from(c) * a + 255 * (255 - a)) / 255).unwrap_or(255);
        [
            over(rgba.get(i).copied().unwrap_or(255)),
            over(rgba.get(i + 1).copied().unwrap_or(255)),
            over(rgba.get(i + 2).copied().unwrap_or(255)),
        ]
    };
    let mut out = String::new();
    let margin = " ".repeat(usize::from(pad));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "cell coordinates scaled back to pixels, all inside the picture"
    )]
    for row in 0..rows {
        out.push_str(&margin);
        let y_top = (f64::from(row) * 2.0 * step) as u32;
        let y_bottom = ((f64::from(row) * 2.0 + 1.0) * step) as u32;
        for col in 0..cols {
            let x = (f64::from(col) * step) as u32;
            let top = pixel(x, y_top);
            let bottom = if y_bottom < h {
                pixel(x, y_bottom)
            } else {
                [255, 255, 255]
            };
            let _ = write!(
                out,
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m\u{2580}",
                top[0], top[1], top[2], bottom[0], bottom[1], bottom[2]
            );
        }
        out.push_str("\x1b[0m\n");
    }
    out.into_bytes()
}

/// Standard base64 with padding; forty lines are cheaper than a crate.
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk.first().copied().unwrap_or(0);
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        let sextet = |shift: u32| {
            TABLE
                .get(((n >> shift) & 0x3f) as usize)
                .copied()
                .unwrap_or(b'A')
        };
        out.push(char::from(sextet(18)));
        out.push(char::from(sextet(12)));
        out.push(if chunk.len() > 1 {
            char::from(sextet(6))
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(sextet(0))
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        Graphics, base64, halfblock, iterm_cells, kitty, kitty_delete, kitty_place, kitty_transmit,
    };
    use pdfrum::Pixmap;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_kitty_picture_is_apc_chunks_and_a_halfblock_one_is_rows_of_cells() {
        let pixmap = Pixmap::filled(4, 4, pdfrum::Color::from_rgb8(255, 0, 0));
        let k = kitty(&pixmap);
        assert!(
            k.starts_with(b"\x1b_Ga=T,f=100,q=2,m=0;"),
            "one chunk for a tiny PNG"
        );
        assert!(k.ends_with(b"\x1b\\\n"));
        let stored = kitty_transmit(7, b"png-bytes");
        assert!(
            stored.starts_with(b"\x1b_Ga=t,f=100,q=2,i=7,m=0;"),
            "{stored:?}"
        );
        assert_eq!(
            kitty_place(7, 57, 40),
            b"\x1b_Ga=p,q=2,i=7,c=57,r=40,C=1\x1b\\"
        );
        let cells = iterm_cells(b"png-bytes", 57, 40);
        assert!(
            cells.starts_with(b"\x1b]1337;File=inline=1;size=9;width=57;height=40;"),
            "{cells:?}"
        );
        assert_eq!(kitty_delete(7), b"\x1b_Ga=d,d=I,q=2,i=7\x1b\\");
        let h = String::from_utf8(halfblock(&pixmap, 2, 0)).unwrap();
        assert_eq!(
            h.matches('\u{2580}').count(),
            2,
            "2 columns, one row of two-pixel cells"
        );
        assert!(h.contains("38;2;255;0;0m"));
        assert!(super::picture(&pixmap, Graphics::Off, 10, 0).is_empty());
        let padded = String::from_utf8(halfblock(&pixmap, 2, 3)).unwrap();
        assert!(
            padded.starts_with("   \x1b["),
            "three cells of margin: {padded:?}"
        );
    }
}
