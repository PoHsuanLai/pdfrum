//! The commands that need a terminal: `preview`, `view`, `search`.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use pdfrum::{Document, FindOptions, RenderOptions, RenderSession, VelloCpuBackend};
use serde::Serialize;

use crate::out::outln;
use crate::term::{Graphics, Style, Term};
use crate::{out, pages, term};

/// The scale that fits a page of `width` points into `columns` cells at
/// `graphics`: pixel protocols get roughly eight pixels per cell, half-blocks
/// exactly one column per cell.
fn fit_scale(width: f64, columns: u16, graphics: Graphics) -> f64 {
    let pixels_per_cell = match graphics {
        Graphics::Halfblock => 1.0,
        _ => 8.0,
    };
    (f64::from(columns) * pixels_per_cell / width.max(1.0)).clamp(0.05, 8.0)
}

fn show(
    doc: &Document,
    index: u32,
    term: Term,
    columns: u16,
    session: &mut RenderSession,
) -> Result<()> {
    if term.graphics == Graphics::Off {
        bail!(
            "this terminal shows no pictures; use `render` to write a PNG, or `--graphics halfblock` to try anyway"
        );
    }
    let page = doc.page(index)?;
    let scale = fit_scale(page.width(), columns, term.graphics);
    let pixmap = page.render_on(
        &VelloCpuBackend::new(),
        &RenderOptions::scaled(scale),
        session,
    )?;
    let bytes = term::picture(&pixmap, term.graphics, columns);
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&bytes).context("cannot write to stdout")?;
    stdout.flush().context("cannot write to stdout")?;
    Ok(())
}

// ---- preview --------------------------------------------------------------

pub fn preview(
    file: &Path,
    password: Option<&str>,
    page: u32,
    width: Option<u16>,
    term: Term,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    if page == 0 || page > doc.page_count() {
        bail!("page {page} is not in 1..={}", doc.page_count());
    }
    let columns = width.unwrap_or_else(|| term::size().0.saturating_sub(1).max(10));
    let mut session = RenderSession::new();
    show(&doc, page - 1, term, columns, &mut session)?;
    Ok(ExitCode::SUCCESS)
}

// ---- view -----------------------------------------------------------------

/// A pager: one page at a time, keys to move, `/` to search, `q` to leave.
pub fn view(file: &Path, password: Option<&str>, start: u32, term: Term) -> Result<ExitCode> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, read};
    use crossterm::{cursor, execute, terminal};

    let doc = out::open(file, password)?;
    let count = doc.page_count();
    if count == 0 {
        bail!("the document has no pages");
    }
    if !term.interactive {
        bail!("`view` needs a terminal; use `render` or `extract text` in a pipe");
    }
    if term.graphics == Graphics::Off {
        bail!("this terminal shows no pictures; try `--graphics halfblock`");
    }
    let mut page = start.clamp(1, count) - 1;
    // Percent, so the arithmetic stays integral.
    let mut zoom: u32 = 100;
    let mut session = RenderSession::new();
    let mut needle = String::new();

    let mut stdout = std::io::stdout();
    terminal::enable_raw_mode().context("cannot put the terminal in raw mode")?;
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)
        .context("cannot switch screens")?;
    // Whatever happens below, the terminal comes back.
    let outcome = (|| -> Result<()> {
        loop {
            let (cols, _rows) = term::size();
            execute!(
                stdout,
                terminal::Clear(terminal::ClearType::All),
                cursor::MoveTo(0, 0)
            )?;
            let columns = u16::try_from(u32::from(cols.saturating_sub(1)) * zoom / 100)
                .unwrap_or(u16::MAX)
                .max(10);
            show(&doc, page, term, columns, &mut session)?;
            let status = format!(
                "page {}/{count}  zoom {zoom}%  j/k next/prev  g/G first/last  +/- zoom  /find  q quit{}",
                page + 1,
                if needle.is_empty() {
                    String::new()
                } else {
                    format!("  [/{needle}: n next]")
                }
            );
            write!(stdout, "\r\n{}", term.paint(Style::Bar, &status))?;
            stdout.flush()?;
            if let Event::Key(KeyEvent {
                code, modifiers, ..
            }) = read()?
            {
                match code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(());
                    }
                    KeyCode::Char('j' | ' ')
                    | KeyCode::Down
                    | KeyCode::PageDown
                    | KeyCode::Enter => {
                        page = (page + 1).min(count - 1);
                    }
                    KeyCode::Char('k' | 'b') | KeyCode::Up | KeyCode::PageUp => {
                        page = page.saturating_sub(1);
                    }
                    KeyCode::Char('g') | KeyCode::Home => page = 0,
                    KeyCode::Char('G') | KeyCode::End => page = count - 1,
                    KeyCode::Char('+' | '=') => zoom = (zoom * 5 / 4).min(400),
                    KeyCode::Char('-') => zoom = (zoom * 4 / 5).max(25),
                    KeyCode::Char('/') => {
                        needle = read_line(&mut stdout, term, "/")?;
                        if let Some(hit) = find_from(&doc, &needle, page, count) {
                            page = hit;
                        }
                    }
                    KeyCode::Char('n') if !needle.is_empty() => {
                        if let Some(hit) = find_from(&doc, &needle, (page + 1) % count, count) {
                            page = hit;
                        }
                    }
                    _ => {}
                }
            }
        }
    })();
    let _ = execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
    outcome?;
    Ok(ExitCode::SUCCESS)
}

/// The first page at or after `from` (wrapping once) whose text contains
/// `needle`, case-insensitively.
fn find_from(doc: &Document, needle: &str, from: u32, count: u32) -> Option<u32> {
    if needle.is_empty() {
        return None;
    }
    let options = FindOptions {
        match_case: false,
        ..FindOptions::default()
    };
    (0..count).map(|i| (from + i) % count).find(|&i| {
        doc.page(i)
            .ok()
            .is_some_and(|page| page.text().find(needle, options).next().is_some())
    })
}

/// A line typed in raw mode, echoed after `prompt` on the status row.
fn read_line(stdout: &mut std::io::Stdout, term: Term, prompt: &str) -> Result<String> {
    use crossterm::event::{Event, KeyCode, KeyEvent, read};
    let mut line = String::new();
    loop {
        write!(stdout, "\r\x1b[K{}{line}", term.paint(Style::Bar, prompt))?;
        stdout.flush()?;
        if let Event::Key(KeyEvent { code, .. }) = read()? {
            match code {
                KeyCode::Enter => return Ok(line),
                KeyCode::Esc => return Ok(String::new()),
                KeyCode::Backspace => {
                    line.pop();
                }
                KeyCode::Char(c) => line.push(c),
                _ => {}
            }
        }
    }
}

// ---- search ---------------------------------------------------------------

#[derive(Serialize)]
struct Hit {
    page: u32,
    /// The hit's character range in the page's text.
    start: usize,
    end: usize,
    /// The line the hit is on, with the hit itself unchanged.
    line: String,
    /// The hit's bounding boxes in page space, one per line it spans.
    rects: Vec<out::JsonRect>,
}

/// `grep` for a document: every hit with its line, and the page it is on.
pub fn search(
    file: &Path,
    password: Option<&str>,
    needle: &str,
    ignore_case: bool,
    spec: Option<&str>,
    json: bool,
    term: Term,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let options = FindOptions {
        match_case: !ignore_case,
        ..FindOptions::default()
    };
    let mut hits = Vec::new();
    for index in pages::select(spec, doc.page_count())? {
        let page = doc.page(index)?;
        let text = page.text();
        let full = text.slice(..);
        let chars: Vec<char> = full.chars().collect();
        for range in text.find(needle, options) {
            let start = usize::from(range.start);
            let end = usize::from(range.end);
            let line_start = chars.get(..start).map_or(0, |before| {
                before
                    .iter()
                    .rposition(|&c| c == '\n' || c == '\r')
                    .map_or(0, |p| p + 1)
            });
            let line_end = chars
                .get(end..)
                .and_then(|after| after.iter().position(|&c| c == '\n' || c == '\r'))
                .map_or(chars.len(), |p| end + p);
            let line: String = chars
                .get(line_start..line_end)
                .unwrap_or(&[])
                .iter()
                .collect();
            // The find runs in text offsets; the boxes live in character
            // indices, and the page's map joins the two.
            let first = text.runs.char_index(range.start);
            let past = text
                .runs
                .char_index(range.end)
                .or_else(|| Some(pdfrum::CharIndex::from(text.char_count())));
            let rects = match (first, past) {
                (Some(a), Some(b)) => text.rects(a..b).into_iter().map(Into::into).collect(),
                _ => Vec::new(),
            };
            hits.push(Hit {
                page: out::page_number(page.index()),
                start,
                end,
                line,
                rects,
            });
        }
    }
    if json {
        out::json(&hits)?;
    } else {
        for h in &hits {
            let chars: Vec<char> = h.line.chars().collect();
            // The hit's offset within its line, to paint it.
            let full_len = chars.len();
            let hit_len = h.end - h.start;
            let painted = if term.color && hit_len <= full_len {
                let at = chars
                    .windows(hit_len)
                    .position(|w| {
                        let candidate: String = w.iter().collect();
                        if ignore_case {
                            candidate.eq_ignore_ascii_case(needle)
                        } else {
                            candidate == needle
                        }
                    })
                    .unwrap_or(0);
                let before: String = chars.get(..at).unwrap_or(&[]).iter().collect();
                let hit: String = chars.get(at..at + hit_len).unwrap_or(&[]).iter().collect();
                let after: String = chars.get(at + hit_len..).unwrap_or(&[]).iter().collect();
                format!("{before}{}{after}", term.paint(Style::Match, &hit))
            } else {
                h.line.clone()
            };
            outln!(
                "{}:{painted}",
                term.paint(Style::Ident, &format!("page {}", h.page))
            );
        }
    }
    Ok(if hits.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}
