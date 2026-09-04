//! Opening the document and shaping output: the two things every command
//! shares.

use std::path::Path;

use anyhow::{Context, Result};
use pdfrum::{Document, Rect};
use serde::Serialize;

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
    match password {
        Some(password) => Document::open_with_password(file, password.as_bytes()),
        None => Document::open(file),
    }
    .with_context(|| format!("cannot open {}", file.display()))
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
