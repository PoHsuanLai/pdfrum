//! Opening the document and shaping output: the two things every command
//! shares.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use pdfrum::{Document, OpenOptions, Rect};
use serde::Serialize;

use crate::term::{Style, Term};

/// Open `file`, with `password` when one was given.
///
/// An encrypted file with no password is the one error a user hits most, so
/// it names the flag that fixes it.
pub fn open(file: &Path, password: Option<&str>) -> Result<Document> {
    let doc = open_quietly(file, password)?;
    if doc.diagnostics().is_empty() {
        return Ok(doc);
    }
    let n = doc.diagnostics().len();
    let rebuilt = if doc.xref_was_rebuilt() {
        "; the cross-reference table was rebuilt"
    } else {
        ""
    };
    eprintln!(
        "pdfrum: {} needed recovery ({n} notice{}{rebuilt}); `pdfrum doctor` lists them",
        file.display(),
        if n == 1 { "" } else { "s" },
    );
    Ok(doc)
}

/// [`open`] without the notice — for `doctor`, whose whole output is the
/// notices.
pub fn open_quietly(file: &Path, password: Option<&str>) -> Result<Document> {
    if is_stdin(file) {
        return open_input(password);
    }
    let first = match password {
        Some(password) => Document::open_with_password(file, password.as_bytes()),
        None => Document::open(file),
    };
    let doc = match first {
        // No password was given, the file wants one, and a person is at the
        // keyboard: ask once, silently, the way `ssh` does.
        Err(pdfrum::Error::WrongPassword)
            if password.is_none()
                && std::io::stdin().is_terminal()
                && std::io::stderr().is_terminal() =>
        {
            let typed = rpassword::prompt_password(format!("password for {}: ", file.display()))
                .context("cannot read a password")?;
            Document::open_with_password(file, typed.as_bytes())
        }
        other => other,
    };
    doc.with_context(|| format!("cannot open {}", file.display()))
}

/// Whether `file` is `-`, the name every reading command gives stdin.
pub fn is_stdin(file: &Path) -> bool {
    file == Path::new("-")
}

/// The document on stdin, read whole: a PDF's cross-reference table is at
/// its end, so there is nothing to stream.
fn open_input(password: Option<&str>) -> Result<Document> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .context("cannot read stdin")?;
    open_bytes(bytes, password).context("cannot open -")
}

/// A document from bytes already in hand — stdin, or a file this run wrote
/// and reads back.
pub fn open_bytes(bytes: Vec<u8>, password: Option<&str>) -> Result<Document> {
    let options = OpenOptions {
        password: password.map(|p| p.as_bytes().to_vec()),
        ..OpenOptions::default()
    };
    Ok(Document::from_bytes_with(bytes.into(), &options)?)
}

/// The input's name without directory or extension, for the files a command
/// derives from it (`{stem}-{n}.png`); `stdin` when the input was `-`.
pub fn stem(file: &Path) -> String {
    if is_stdin(file) {
        return "stdin".to_owned();
    }
    file.file_stem()
        .map_or_else(|| "output".to_owned(), |s| s.to_string_lossy().into_owned())
}

/// A notice: `pdfrum: <file>: <what happened>` on stderr, one line.
pub fn notice(file: &Path, what: &str) {
    eprintln!("pdfrum: {}: {what}", file.display());
}

/// An error: `pdfrum: <what failed>: <why>` on stderr, in `Error`.
pub fn error(term: Term, err: &anyhow::Error) {
    eprintln!(
        "{}",
        term.paint(Style::Error, &format!("pdfrum: {}", error_line(err)))
    );
}

/// The error's causes, outermost first, joined by `: ` — with a cause
/// left out when the message above it already quotes it, which the
/// library's errors do, so nothing is said twice.
pub fn error_line(err: &anyhow::Error) -> String {
    let mut line = String::new();
    for cause in err.chain() {
        let text = cause.to_string();
        if line.contains(&text) {
            continue;
        }
        if !line.is_empty() {
            line.push_str(": ");
        }
        line.push_str(&text);
    }
    line
}

/// `each` over several files the way `grep` goes: a file that fails is
/// reported on stderr and the run continues with the next. Returns what
/// the files that worked gave, and whether any failed — which is exit 1 at
/// the end, after everything that could be answered was.
pub fn per_file<T>(
    files: &[PathBuf],
    term: Term,
    mut each: impl FnMut(&Path) -> Result<T>,
) -> (Vec<T>, bool) {
    let mut results = Vec::with_capacity(files.len());
    let mut failed = false;
    for file in files {
        match each(file) {
            Ok(result) => results.push(result),
            Err(err) => {
                error(term, &err);
                failed = true;
            }
        }
    }
    (results, failed)
}

/// The JSON of a command given several files: the one document when one
/// file was named, the array of per-file documents otherwise.
pub fn documents<T: Serialize>(files: &[PathBuf], reports: &[T]) -> Result<()> {
    if files.len() == 1 {
        return match reports.first() {
            Some(report) => json(report),
            None => Ok(()),
        };
    }
    json(&reports)
}

/// Exit 1 when a file among several failed, else `otherwise`.
pub fn exit(failed: bool, otherwise: ExitCode) -> ExitCode {
    if failed { ExitCode::from(1) } else { otherwise }
}

/// Where a writing command's result goes: the file named, or stdout for
/// `-`. A command makes one first, so `-` on a terminal is refused before
/// any work is done, produces its bytes through the facade's `write_*_to`
/// twin of the save it wants, and hands them to [`Sink::finish`].
pub struct Sink<'a> {
    path: &'a Path,
}

impl<'a> Sink<'a> {
    /// `kind` names what the bytes are, for the refusal: `PDF`, `PNG`.
    pub fn new(path: &'a Path, kind: &str) -> Result<Self> {
        if is_stdin(path) && std::io::stdout().is_terminal() {
            bail!("refusing to write a {kind} to a terminal; give -o a path or pipe it");
        }
        Ok(Self { path })
    }

    /// Whether the bytes go to stdout.
    pub fn is_stdout(&self) -> bool {
        is_stdin(self.path)
    }

    /// Write the bytes, then say what was done: the summary line on stdout
    /// for a file, or — when stdout is the file — the same words as a
    /// notice on stderr, `pdfrum: -: 3 pages`.
    pub fn finish(&self, term: Term, bytes: &[u8], what: &str, detail: Option<&str>) -> Result<()> {
        if self.is_stdout() {
            write_bytes(bytes);
            match detail {
                Some(d) if !d.is_empty() => notice(self.path, &format!("{what}, {d}")),
                _ => notice(self.path, what),
            }
        } else {
            std::fs::write(self.path, bytes)
                .with_context(|| format!("cannot write {}", self.path.display()))?;
            summary(term, self.path, what, detail);
        }
        Ok(())
    }
}

/// Print `value` as one pretty JSON document.
pub fn json(value: &impl Serialize) -> Result<()> {
    let text = serde_json::to_string_pretty(value).context("cannot encode JSON")?;
    write_all(format_args!("{text}\n"));
    Ok(())
}

/// Write to stdout, and treat a closed pipe as the reader being done rather
/// than as a failure: `pdfrum extract text big.pdf | head` must exit 0 and
/// quietly, the way every other Unix tool does.
pub fn write_all(args: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    if let Err(err) = stdout.write_fmt(args) {
        if err.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        eprintln!("pdfrum: cannot write to stdout: {err}");
        std::process::exit(1);
    }
}

/// [`write_all`] for bytes that are not text: a decoded stream, a
/// completion script.
pub fn write_bytes(bytes: &[u8]) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    if let Err(err) = stdout.write_all(bytes).and_then(|()| stdout.flush()) {
        if err.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        eprintln!("pdfrum: cannot write to stdout: {err}");
        std::process::exit(1);
    }
}

/// `println!` for a command's output; see [`write_all`].
macro_rules! outln {
    () => {
        $crate::out::write_all(format_args!("\n"))
    };
    ($($arg:tt)+) => {
        $crate::out::write_all(format_args!("{}\n", format_args!($($arg)+)))
    };
}
/// `print!` for a command's output; see [`write_all`].
macro_rules! out {
    ($($arg:tt)*) => {
        $crate::out::write_all(format_args!($($arg)*))
    };
}
pub(crate) use {out, outln};

// ---- the four forms of docs/design/cli-style.md §3 -------------------------

/// A record: keys padded to the longest, in `Key`; values plain. A `None`
/// value prints as `none`.
pub fn record(term: Term, rows: &[(&str, Option<String>)]) {
    let width = rows
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    for (key, value) in rows {
        let value = value.as_deref().unwrap_or("none");
        let mut lines = value.lines();
        let first = lines.next().unwrap_or("");
        let padded = format!("{key:<width$}");
        outln!("{}{first}", term.paint(Style::Key, &padded));
        for continued in lines {
            outln!("{}{continued}", " ".repeat(width));
        }
    }
}

/// Which way a table column lines up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// A table: a header row in `Key`, columns two spaces apart, the last one
/// unpadded, a column that is empty in every row dropped.
pub struct Table {
    columns: Vec<(&'static str, Align)>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(columns: &[(&'static str, Align)]) -> Self {
        Self {
            columns: columns.to_vec(),
            rows: Vec::new(),
        }
    }

    /// Add one row; missing trailing cells are empty.
    pub fn row(&mut self, cells: Vec<String>) {
        self.rows.push(cells);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Print with each row indented by `indent` spaces (a section's rows
    /// sit under their heading by two).
    pub fn print(&self, term: Term, indent: usize) {
        fn cell(row: &[String], i: usize) -> &str {
            row.get(i).map_or("", String::as_str)
        }
        let keep: Vec<usize> = (0..self.columns.len())
            .filter(|&i| self.rows.iter().any(|r| !cell(r, i).is_empty()))
            .collect();
        let widths: Vec<usize> = keep
            .iter()
            .map(|&i| {
                self.rows
                    .iter()
                    .map(|r| visible_width(cell(r, i)))
                    .chain(std::iter::once(self.columns[i].0.chars().count()))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let pad = " ".repeat(indent);
        let header: Vec<String> = keep
            .iter()
            .zip(&widths)
            .map(|(&i, &w)| fit(self.columns[i].0, w, self.columns[i].1))
            .collect();
        outln!(
            "{pad}{}",
            term.paint(Style::Key, header.join("  ").trim_end())
        );
        for row in &self.rows {
            let cells: Vec<String> = keep
                .iter()
                .zip(&widths)
                .map(|(&i, &w)| fit(cell(row, i), w, self.columns[i].1))
                .collect();
            outln!("{pad}{}", cells.join("  ").trim_end());
        }
    }
}

/// `text` padded to `width` visible characters on the side its alignment
/// leaves free; escape sequences do not count.
fn fit(text: &str, width: usize, align: Align) -> String {
    let fill = width.saturating_sub(visible_width(text));
    match align {
        Align::Left => format!("{text}{}", " ".repeat(fill)),
        Align::Right => format!("{}{text}", " ".repeat(fill)),
    }
}

/// Characters a terminal shows: everything outside `ESC [ … m` and
/// `ESC ] … ESC \` sequences.
pub fn visible_width(text: &str) -> usize {
    let mut width = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            width += 1;
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            Some(']') => {
                let mut last = ' ';
                for c in chars.by_ref() {
                    if c == '\x07' || (last == '\x1b' && c == '\\') {
                        break;
                    }
                    last = c;
                }
            }
            _ => {}
        }
    }
    width
}

/// A section heading: `page 3`.
pub fn heading(term: Term, text: &str) {
    outln!("{}", term.paint(Style::Heading, text));
}

/// The one line a writing command prints: the path, what was done, the
/// detail muted.
pub fn summary(term: Term, path: &Path, what: &str, detail: Option<&str>) {
    let path = term.paint(Style::Ident, &path.display().to_string());
    match detail {
        Some(d) if !d.is_empty() => outln!("{path}: {what}, {}", term.paint(Style::Muted, d)),
        _ => outln!("{path}: {what}"),
    }
}

/// Nothing found: `no <things>`.
pub fn none(what: &str) {
    outln!("no {what}");
}

/// A size for a person: `3 B`, `1.2 KB`, `24.3 KB`, `1.8 MB`.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1000 {
        return format!("{n} B");
    }
    #[expect(clippy::cast_precision_loss, reason = "a size shown to one decimal")]
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 999.95 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// A page number in `Ident`.
pub fn page(term: Term, number: u32) -> String {
    term.paint(Style::Ident, &number.to_string())
}

/// A rectangle as JSON: `[x0, y0, x1, y1]` in points.
#[derive(Serialize)]
pub struct JsonRect(pub [f64; 4]);

impl From<Rect> for JsonRect {
    fn from(r: Rect) -> Self {
        Self([r.x0, r.y0, r.x1, r.y1])
    }
}

/// A rectangle for a person: `x0 y0 x1 y1` to two decimals.
pub fn rect(r: Rect) -> String {
    format!("{:.2} {:.2} {:.2} {:.2}", r.x0, r.y0, r.x1, r.y1)
}

/// Bytes as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // Writing to a `String` cannot fail.
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// A page number for a person: 1-based.
pub fn page_number(index: pdfrum::PageIndex) -> u32 {
    u32::from(index) + 1
}

#[cfg(test)]
mod tests {
    use super::{Align, Table, bytes, fit, visible_width};

    #[test]
    fn a_table_aligns_numbers_right_and_drops_a_column_nobody_fills() {
        let mut table = Table::new(&[
            ("PAGE", Align::Right),
            ("NAME", Align::Left),
            ("NOTE", Align::Left),
            ("SIZE", Align::Right),
        ]);
        table.row(vec!["1".into(), "a".into(), String::new(), "3 B".into()]);
        table.row(vec![
            "10".into(),
            "bcd".into(),
            String::new(),
            "1.2 KB".into(),
        ]);
        // The layout, without going through stdout: the same fit() the
        // printer uses, over the same kept columns and widths.
        assert_eq!(fit("PAGE", 4, Align::Right), "PAGE");
        assert_eq!(fit("1", 4, Align::Right), "   1");
        assert_eq!(fit("a", 4, Align::Left), "a   ");
        assert!(!table.is_empty());
        let kept: Vec<usize> = (0..4)
            .filter(|&i| table.rows.iter().any(|r| !r[i].is_empty()))
            .collect();
        assert_eq!(kept, [0, 1, 3], "NOTE is empty in every row and goes");
    }

    #[test]
    fn sizes_read_like_du_h_and_escapes_take_no_width() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1000), "1.0 KB");
        assert_eq!(bytes(24332), "24.3 KB");
        assert_eq!(bytes(1_833_639), "1.8 MB");
        assert_eq!(bytes(999_950), "1.0 MB");
        assert_eq!(visible_width("page 3"), 6);
        assert_eq!(visible_width("\x1b[36mpage 3\x1b[0m"), 6);
        assert_eq!(visible_width("\x1b]8;;http://x\x1b\\p.2\x1b]8;;\x1b\\"), 3);
    }
}
