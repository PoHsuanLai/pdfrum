//! The commands that need a terminal: `preview`, `view`, `search`.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Condvar, Mutex};

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
    let bytes = term::picture(&pixmap, term.graphics, columns, 0);
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
    if out::is_stdin(file) {
        bail!("preview draws on a terminal, and reads no document from stdin; give a path");
    }
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

/// What one frame of the pager is: a page at a zoom on a screen of a size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    page: u32,
    zoom: u32,
    cols: u16,
    rows: u16,
}

/// Where a page lands on the screen.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Layout {
    /// Points to pixels.
    scale: f64,
    /// The picture's width and height in cells.
    cols: u16,
    rows: u16,
    /// The left margin that centres it.
    pad: u16,
}

/// A rendered frame, ready for the terminal.
struct Frame {
    layout: Layout,
    picture: Picture,
}

/// The bytes of a frame: a PNG for the pixel protocols (kitty stores it
/// once and places it by id; iTerm2 draws it inline, sized in cells), or
/// half-block rows carrying their own margin.
enum Picture {
    Png(Vec<u8>),
    Halfblock(Vec<u8>),
}

/// Frames rendered so far, a bell for the one being waited on, and the
/// word to stop.
#[derive(Default)]
struct Cache {
    frames: Mutex<HashMap<Key, Result<Frame, String>>>,
    ready: Condvar,
    stop: AtomicBool,
}

/// The page fits the screen at zoom 100 — the width less one column, the
/// height less the status row — and sits in the middle.
fn layout(page: (f64, f64), key: Key, screen: term::Screen, graphics: Graphics) -> Layout {
    let (page_w, page_h) = (page.0.max(1.0), page.1.max(1.0));
    let avail_cols = f64::from(key.cols.saturating_sub(1).max(10));
    let avail_rows = f64::from(key.rows.saturating_sub(1).max(4));
    let zoom = f64::from(key.zoom) / 100.0;
    let (scale, cols, rows) = if graphics == Graphics::Halfblock {
        // One pixel per column, two per row.
        let fit = avail_cols.min(2.0 * avail_rows * page_w / page_h);
        let columns = (fit * zoom).max(10.0).floor();
        (
            columns / page_w,
            columns,
            (columns * page_h / page_w / 2.0).ceil(),
        )
    } else {
        let fit =
            (avail_cols * screen.cell_width / page_w).min(avail_rows * screen.cell_height / page_h);
        let scale = (fit * zoom).clamp(0.05, 8.0);
        (
            scale,
            (page_w * scale / screen.cell_width).ceil(),
            (page_h * scale / screen.cell_height).ceil(),
        )
    };
    let cols = cols.min(f64::from(u16::MAX));
    let rows = rows.min(f64::from(u16::MAX));
    let pad = ((avail_cols - cols) / 2.0).max(0.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "cell counts: non-negative and bounded by the screen and the zoom cap"
    )]
    let (cols, rows, pad) = (cols as u16, rows as u16, pad as u16);
    Layout {
        scale,
        cols,
        rows,
        pad,
    }
}

fn render_frame(
    doc: &Document,
    key: Key,
    graphics: Graphics,
    session: &mut RenderSession,
) -> Result<Frame, String> {
    let page = doc.page(key.page).map_err(|e| e.to_string())?;
    let layout = layout((page.width(), page.height()), key, term::screen(), graphics);
    let pixmap = page
        .render_on(
            &VelloCpuBackend::new(),
            &RenderOptions::scaled(layout.scale),
            session,
        )
        .map_err(|e| e.to_string())?;
    let picture = if graphics == Graphics::Halfblock {
        Picture::Halfblock(term::picture(&pixmap, graphics, layout.cols, layout.pad))
    } else {
        Picture::Png(pixmap.encode_png().map_err(|e| e.to_string())?)
    };
    Ok(Frame { layout, picture })
}

/// The render thread: takes requests as they come, in the order they were
/// asked for, skips what is already cached, and rings the bell after each
/// frame.
fn render_worker(doc: &Document, requests: &Receiver<Key>, cache: &Cache, graphics: Graphics) {
    let mut session = RenderSession::new();
    while let Ok(first) = requests.recv() {
        let mut batch = vec![first];
        while let Ok(key) = requests.try_recv() {
            batch.push(key);
        }
        for key in batch {
            if cache.stop.load(Ordering::Relaxed) {
                return;
            }
            let cached = cache
                .frames
                .lock()
                .map_or(true, |frames| frames.contains_key(&key));
            if cached {
                continue;
            }
            let frame = render_frame(doc, key, graphics, &mut session);
            if let Ok(mut frames) = cache.frames.lock() {
                frames.insert(key, frame);
            }
            cache.ready.notify_all();
        }
    }
}

/// The keys the pager answers to, for the status row.
const KEYS: &str = "j/k next/prev  g/G first/last  +/- zoom  / find  q quit";

/// The pager's state between keypresses.
struct Pager<'a> {
    doc: &'a Document,
    count: u32,
    page: u32,
    /// Percent, so the arithmetic stays integral.
    zoom: u32,
    needle: String,
    term: Term,
    /// Kitty: the image id each frame was stored under, and the id on
    /// screen now.
    stored: HashMap<Key, u32>,
    next_id: u32,
    placed: Option<u32>,
}

impl Pager<'_> {
    /// The status row: where we are, and what the keys do.
    fn status(&self, stdout: &mut std::io::Stdout, cols: u16, row: u16, note: &str) -> Result<()> {
        use crossterm::{cursor, execute};
        let text = format!(
            " page {}/{}  zoom {}%  {KEYS}{}{note}",
            self.page + 1,
            self.count,
            self.zoom,
            if self.needle.is_empty() {
                String::new()
            } else {
                format!("  [/{}: n next]", self.needle)
            }
        );
        // One cell short of the width: writing the last cell of the last
        // row arms the terminal's pending wrap, and the next byte scrolls.
        let width = usize::from(cols.saturating_sub(1));
        let line: String = text.chars().take(width).collect();
        execute!(stdout, cursor::MoveTo(0, row))?;
        write!(
            stdout,
            "{}",
            self.term.paint(Style::Bar, &format!("{line:<width$}"))
        )?;
        stdout.flush()?;
        Ok(())
    }

    /// Kitty: send the picture to the terminal if it is not there yet, and
    /// say which id it lives under.
    fn transmit(&mut self, stdout: &mut std::io::Stdout, key: Key, png: &[u8]) -> Result<u32> {
        if let Some(&id) = self.stored.get(&key) {
            return Ok(id);
        }
        self.next_id += 1;
        let id = self.next_id;
        stdout.write_all(&term::kitty_transmit(id, png))?;
        self.stored.insert(key, id);
        Ok(id)
    }

    /// Draw one frame in place of the last: no clear of the whole screen,
    /// one synchronized update, and only the rows below the picture wiped.
    fn draw(
        &mut self,
        stdout: &mut std::io::Stdout,
        key: Key,
        frame: &Frame,
        status_row: u16,
    ) -> Result<()> {
        use crossterm::{cursor, execute, terminal};
        execute!(stdout, terminal::BeginSynchronizedUpdate)?;
        // The picture may be at most one row short of the status row.
        let rows = frame.layout.rows.min(status_row);
        match &frame.picture {
            Picture::Png(png) if self.term.graphics == Graphics::Kitty => {
                let id = self.transmit(stdout, key, png)?;
                if let Some(old) = self.placed.replace(id)
                    && old != id
                {
                    // Take the old placement down but keep its picture.
                    stdout.write_all(format!("\x1b_Ga=d,d=i,q=2,i={old}\x1b\\").as_bytes())?;
                }
                execute!(stdout, cursor::MoveTo(frame.layout.pad, 0))?;
                stdout.write_all(&term::kitty_place(id, frame.layout.cols, rows))?;
            }
            Picture::Png(png) => {
                execute!(stdout, cursor::MoveTo(frame.layout.pad, 0))?;
                stdout.write_all(&term::iterm_cells(png, frame.layout.cols, rows))?;
            }
            Picture::Halfblock(bytes) => {
                execute!(stdout, cursor::MoveTo(0, 0))?;
                // Rows carry their margin; only as many as fit above the bar.
                for row in bytes
                    .split_inclusive(|&b| b == b'\n')
                    .take(usize::from(rows))
                {
                    stdout.write_all(row)?;
                }
            }
        }
        execute!(
            stdout,
            cursor::MoveTo(0, rows),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )?;
        execute!(stdout, terminal::EndSynchronizedUpdate)?;
        Ok(())
    }

    /// Kitty: while the terminal waits for a key, send the frames a
    /// keypress is likely to want, so placing them later is instant.
    fn transmit_ahead(
        &mut self,
        stdout: &mut std::io::Stdout,
        cache: &Cache,
        keys: &[Key],
    ) -> Result<()> {
        if self.term.graphics != Graphics::Kitty {
            return Ok(());
        }
        let Ok(frames) = cache.frames.lock() else {
            return Ok(());
        };
        let mut sent = 0;
        for &key in keys {
            if self.stored.contains_key(&key) {
                continue;
            }
            if let Some(Ok(Frame {
                picture: Picture::Png(png),
                ..
            })) = frames.get(&key)
            {
                self.transmit(stdout, key, png)?;
                sent += 1;
            }
            if sent == 2 {
                break;
            }
        }
        stdout.flush()?;
        Ok(())
    }

    /// Forget frames far from `key`, in the terminal too.
    fn prune(&mut self, stdout: &mut std::io::Stdout, cache: &Cache, key: Key) -> Result<()> {
        let far = |k: &Key| {
            k.page.abs_diff(key.page) > 2
                || k.zoom != key.zoom
                || k.cols != key.cols
                || k.rows != key.rows
        };
        if let Ok(mut frames) = cache.frames.lock()
            && frames.len() > 8
        {
            frames.retain(|k, _| !far(k));
        }
        let gone: Vec<Key> = self.stored.keys().filter(|k| far(k)).copied().collect();
        for k in gone {
            if let Some(id) = self.stored.remove(&k) {
                stdout.write_all(&term::kitty_delete(id))?;
            }
        }
        Ok(())
    }

    /// One keypress; `true` when it is time to leave.
    fn keypress(
        &mut self,
        code: crossterm::event::KeyCode,
        modifiers: crossterm::event::KeyModifiers,
        stdout: &mut std::io::Stdout,
        status_row: u16,
    ) -> Result<bool> {
        use crossterm::event::{KeyCode, KeyModifiers};
        use crossterm::{cursor, execute};
        let last = self.count - 1;
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),
            KeyCode::Char('j' | ' ' | 'l')
            | KeyCode::Down
            | KeyCode::Right
            | KeyCode::PageDown
            | KeyCode::Enter => self.page = (self.page + 1).min(last),
            KeyCode::Char('k' | 'b' | 'h') | KeyCode::Up | KeyCode::Left | KeyCode::PageUp => {
                self.page = self.page.saturating_sub(1);
            }
            KeyCode::Char('g') | KeyCode::Home => self.page = 0,
            KeyCode::Char('G') | KeyCode::End => self.page = last,
            KeyCode::Char('+' | '=') => self.zoom = (self.zoom * 5 / 4).min(400),
            KeyCode::Char('-') => self.zoom = (self.zoom * 4 / 5).max(25),
            KeyCode::Char('0') => self.zoom = 100,
            KeyCode::Char('/') => {
                execute!(stdout, cursor::MoveTo(0, status_row))?;
                self.needle = read_line(stdout, self.term, "/")?;
                if let Some(hit) = find_from(self.doc, &self.needle, self.page, self.count) {
                    self.page = hit;
                }
            }
            KeyCode::Char('n') if !self.needle.is_empty() => {
                let from = (self.page + 1) % self.count;
                if let Some(hit) = find_from(self.doc, &self.needle, from, self.count) {
                    self.page = hit;
                }
            }
            _ => {}
        }
        Ok(false)
    }
}

/// The frame for `key`, from the cache or after the worker rings; `waiting`
/// runs once if there is a wait. The frame stays in the cache.
fn wait_for(cache: &Cache, key: Key, mut waiting: impl FnMut() -> Result<()>) -> Result<()> {
    let poisoned = || anyhow::anyhow!("the render thread panicked");
    let mut frames = cache.frames.lock().map_err(|_| poisoned())?;
    let mut told = false;
    while !frames.contains_key(&key) {
        if !told {
            waiting()?;
            told = true;
        }
        frames = cache.ready.wait(frames).map_err(|_| poisoned())?;
    }
    Ok(())
}

/// A pager: one page at a time, centred and fitted to the screen; the
/// neighbouring pages render in the background so a keypress shows the
/// next page from the cache rather than after a wait.
pub fn view(file: &Path, password: Option<&str>, start: u32, term: Term) -> Result<ExitCode> {
    use crossterm::event::{Event, KeyEvent, read};
    use crossterm::{cursor, execute, terminal};

    if out::is_stdin(file) {
        bail!(
            "view reads its keys from stdin, so the document cannot come from there; give a path"
        );
    }
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
    let mut pager = Pager {
        doc: &doc,
        count,
        page: start.clamp(1, count) - 1,
        zoom: 100,
        needle: String::new(),
        term,
        stored: HashMap::new(),
        next_id: 0,
        placed: None,
    };

    let mut stdout = std::io::stdout();
    terminal::enable_raw_mode().context("cannot put the terminal in raw mode")?;
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)
        .context("cannot switch screens")?;
    let cache = Cache::default();
    let (requests, inbox) = mpsc::channel::<Key>();
    // The sender lives inside the scope: dropping it closes the channel,
    // which is what lets the render thread finish and the scope end.
    // Whatever happens below, the terminal comes back.
    let outcome = std::thread::scope(|scope| -> Result<()> {
        let requests = requests;
        let (doc_ref, cache_ref, graphics) = (&doc, &cache, term.graphics);
        scope.spawn(move || render_worker(doc_ref, &inbox, cache_ref, graphics));
        let result = (|| -> Result<()> {
            loop {
                let (cols, rows) = term::size();
                let key = Key {
                    page: pager.page,
                    zoom: pager.zoom,
                    cols,
                    rows,
                };
                // This page first, then the ones a keypress is likely to want.
                let ahead: Vec<Key> = [key.page + 1, key.page.wrapping_sub(1), key.page + 2]
                    .into_iter()
                    .filter(|&p| p < count)
                    .map(|p| Key { page: p, ..key })
                    .collect();
                for wanted in std::iter::once(key).chain(ahead.iter().copied()) {
                    let _ = requests.send(wanted);
                }
                let status_row = rows.saturating_sub(1);
                wait_for(&cache, key, || {
                    pager.status(&mut stdout, cols, status_row, "  rendering")
                })?;
                {
                    let frames = cache
                        .frames
                        .lock()
                        .map_err(|_| anyhow::anyhow!("the render thread panicked"))?;
                    match frames.get(&key) {
                        Some(Ok(frame)) => pager.draw(&mut stdout, key, frame, status_row)?,
                        Some(Err(e)) => {
                            execute!(
                                stdout,
                                terminal::Clear(terminal::ClearType::All),
                                cursor::MoveTo(0, 0)
                            )?;
                            write!(stdout, "cannot render page {}: {e}", key.page + 1)?;
                        }
                        None => {}
                    }
                }
                pager.status(&mut stdout, cols, status_row, "")?;
                pager.prune(&mut stdout, &cache, key)?;
                pager.transmit_ahead(&mut stdout, &cache, &ahead)?;
                if let Event::Key(KeyEvent {
                    code, modifiers, ..
                }) = read()?
                    && pager.keypress(code, modifiers, &mut stdout, status_row)?
                {
                    return Ok(());
                }
            }
        })();
        cache.stop.store(true, Ordering::Relaxed);
        drop(requests);
        result
    });
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
            .is_ok_and(|page| page.text().find(needle, options).next().is_some())
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
pub struct Hit {
    page: u32,
    /// The hit's character range in the page's text.
    start: usize,
    end: usize,
    /// The line the hit is on, with the hit itself unchanged.
    line: String,
    /// The hit's bounding boxes in page space, one per line it spans.
    rects: Vec<out::JsonRect>,
}

/// What `search` was asked for.
pub struct Search<'a> {
    pub files: &'a [PathBuf],
    pub password: Option<&'a str>,
    pub needle: &'a str,
    pub ignore_case: bool,
    pub spec: Option<&'a str>,
    /// `-H` (`Some(true)`), `--no-filename` (`Some(false)`), or neither: a
    /// hit names its file when more than one file is searched.
    pub with_filename: Option<bool>,
    pub json: out::Json,
}

/// One file's hits: the JSON shape when several files are searched.
#[derive(Serialize)]
struct FileHits {
    file: String,
    hits: Vec<Hit>,
}

/// One hit with its file, for `--jsonl` when the text form would name it.
#[derive(Serialize)]
struct NamedHit<'a> {
    file: &'a str,
    #[serde(flatten)]
    hit: &'a Hit,
}

/// `grep` for documents: every hit with its line, the page it is on, and
/// — with several files, or `-H` — the file, as `a.pdf:page 2:…`. Exit 1
/// when no file had a hit.
pub fn search(req: &Search<'_>, term: Term) -> Result<ExitCode> {
    let options = FindOptions {
        match_case: !req.ignore_case,
        ..FindOptions::default()
    };
    let named = req.with_filename.unwrap_or(req.files.len() > 1);
    let (found, failed) = out::per_file(req.files, term, |file| {
        let doc = out::open(file, req.password)?;
        let hits = find(&doc, req.needle, options, req.spec)?;
        if req.json == out::Json::Lines {
            // As each file is done, like the text form.
            if named {
                let file = file.display().to_string();
                let lines: Vec<NamedHit<'_>> = hits
                    .iter()
                    .map(|hit| NamedHit { file: &file, hit })
                    .collect();
                out::items(&lines, req.json)?;
            } else {
                out::items(&hits, req.json)?;
            }
        } else if !req.json.is_on() {
            // Printed as each file is done, as `grep` does, so a long
            // list of files answers as it goes.
            let prefix = if named {
                format!("{}:", term.paint(Style::Ident, &file.display().to_string()))
            } else {
                String::new()
            };
            for hit in &hits {
                outln!(
                    "{prefix}{}:{}",
                    term.paint(Style::Ident, &format!("page {}", hit.page)),
                    painted(hit, req.needle, req.ignore_case, term)
                );
            }
        }
        Ok(FileHits {
            file: file.display().to_string(),
            hits,
        })
    });
    let any = found.iter().any(|f| !f.hits.is_empty());
    if req.json == out::Json::Document {
        // One file is the array of hits it always was; several files, or
        // a file asked for by name, are per-file documents.
        match found.as_slice() {
            [one] if req.files.len() == 1 && !named => out::json(&one.hits)?,
            _ => out::json(&found)?,
        }
    }
    Ok(out::exit(
        failed,
        if any {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        },
    ))
}

/// Every hit of `needle` on the selected pages.
pub fn find(
    doc: &Document,
    needle: &str,
    options: FindOptions,
    spec: Option<&str>,
) -> Result<Vec<Hit>> {
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
    Ok(hits)
}

/// The hit's line with the hit itself in `Match` when colour is on.
fn painted(hit: &Hit, needle: &str, ignore_case: bool, term: Term) -> String {
    let chars: Vec<char> = hit.line.chars().collect();
    // The hit's offset within its line, to paint it.
    let full_len = chars.len();
    let hit_len = hit.end - hit.start;
    if !term.color || hit_len > full_len {
        return hit.line.clone();
    }
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
    let found: String = chars.get(at..at + hit_len).unwrap_or(&[]).iter().collect();
    let after: String = chars.get(at + hit_len..).unwrap_or(&[]).iter().collect();
    format!("{before}{}{after}", term.paint(Style::Match, &found))
}

#[cfg(test)]
mod tests {
    use super::{Key, layout};
    use crate::term::{Graphics, Screen};

    #[test]
    fn a_page_fits_the_screen_at_zoom_100_and_sits_in_the_middle() {
        let screen = Screen {
            cell_width: 10.0,
            cell_height: 20.0,
        };
        let key = Key {
            page: 0,
            zoom: 100,
            cols: 101,
            rows: 41,
        };
        // A portrait page on a 100x40-cell screen (the last column and the
        // status row are kept free): height is the limit, 40 rows of 20 px
        // = 800 px for 842 pt -> scale 0.95, 566 px wide = 57 cells, so
        // (100 - 57) / 2 = 21 cells of margin.
        let l = layout((595.0, 842.0), key, screen, Graphics::Kitty);
        assert!((l.scale - 800.0 / 842.0).abs() < 1e-9, "{}", l.scale);
        assert_eq!((l.cols, l.rows, l.pad), (57, 40, 21));
        // Half-blocks: one column per pixel, two rows per... two pixels per
        // row, so 40 rows fit 80 px of height, 80 * 595 / 842 = 56 columns.
        let l = layout((595.0, 842.0), key, screen, Graphics::Halfblock);
        assert_eq!((l.cols, l.rows, l.pad), (56, 40, 22));
        assert!((l.scale - 56.0 / 595.0).abs() < 1e-9, "{}", l.scale);
        // Zooming in doubles the width and the margin goes.
        let zoomed = Key { zoom: 200, ..key };
        let l = layout((595.0, 842.0), zoomed, screen, Graphics::Kitty);
        assert_eq!((l.cols, l.pad), (114, 0));
    }
}
