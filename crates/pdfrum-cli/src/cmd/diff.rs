//! `pdfrum diff`: what changed between two documents — the text, page by
//! page, and with `--visual` the pixels.

use std::fmt::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use pdfrum::{Document, Pixmap, RenderOptions, RenderSession, VelloCpuBackend};
use serde::Serialize;

use crate::out;
use crate::out::outln;
use crate::term::{Style, Term};

/// What the diff was asked for.
pub struct Request<'a> {
    pub left: &'a Path,
    pub right: &'a Path,
    pub password: Option<&'a str>,
    pub visual: bool,
    /// Where `--visual` writes its difference pictures, if anywhere.
    pub out_dir: Option<&'a Path>,
    pub dpi: f64,
    pub json: bool,
    pub term: Term,
}

#[derive(Serialize)]
struct PageDiff {
    page: u32,
    /// Lines only in the left document.
    removed: Vec<String>,
    /// Lines only in the right document.
    added: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pixels: Option<PixelDiff>,
}

#[derive(Serialize)]
struct PixelDiff {
    differing: u64,
    total: u64,
    /// Where the differences are, in pixels of the rendered page.
    #[serde(skip_serializing_if = "Option::is_none")]
    bbox: Option<[u32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    written: Option<String>,
}

#[derive(Serialize)]
struct Report {
    left: String,
    right: String,
    left_pages: u32,
    right_pages: u32,
    pages: Vec<PageDiff>,
}

pub fn run(req: &Request<'_>) -> Result<ExitCode> {
    let left = out::open(req.left, req.password)?;
    let right = out::open(req.right, req.password)?;
    let count = left.page_count().max(right.page_count());
    let mut pages = Vec::new();
    let backend = VelloCpuBackend::new();
    let mut session = RenderSession::new();
    if let Some(dir) = req.out_dir {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    for index in 0..count {
        let a = page_lines(&left, index);
        let b = page_lines(&right, index);
        let (removed, added) = line_diff(&a, &b);
        let pixels = if req.visual {
            Some(pixel_diff(
                &left,
                &right,
                index,
                req,
                backend,
                &mut session,
            )?)
        } else {
            None
        };
        pages.push(PageDiff {
            page: index + 1,
            removed,
            added,
            pixels,
        });
    }
    let report = Report {
        left: req.left.display().to_string(),
        right: req.right.display().to_string(),
        left_pages: left.page_count(),
        right_pages: right.page_count(),
        pages,
    };
    let differs = report.pages.iter().any(|p| {
        !p.removed.is_empty()
            || !p.added.is_empty()
            || p.pixels.as_ref().is_some_and(|x| x.differing > 0)
    });
    if req.json {
        out::json(&report)?;
    } else {
        print(&report, req.term);
    }
    Ok(if differs {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn page_lines(doc: &Document, index: u32) -> Vec<String> {
    let Ok(page) = doc.page(index) else {
        return Vec::new();
    };
    page.text()
        .slice(..)
        .replace("\r\n", "\n")
        .lines()
        .map(str::to_owned)
        .filter(|l| !l.trim().is_empty())
        .collect()
}

/// The lines of `a` not in `b`, and of `b` not in `a`, by a longest common
/// subsequence: what `diff` would print, without the context.
fn line_diff(left: &[String], right: &[String]) -> (Vec<String>, Vec<String>) {
    let (rows, cols) = (left.len(), right.len());
    let mut table = vec![vec![0u32; cols + 1]; rows + 1];
    for row in (0..rows).rev() {
        for col in (0..cols).rev() {
            table[row][col] = if left[row] == right[col] {
                table[row + 1][col + 1] + 1
            } else {
                table[row + 1][col].max(table[row][col + 1])
            };
        }
    }
    let (mut row, mut col) = (0, 0);
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    while row < rows && col < cols {
        if left[row] == right[col] {
            row += 1;
            col += 1;
        } else if table[row + 1][col] >= table[row][col + 1] {
            removed.push(left[row].clone());
            row += 1;
        } else {
            added.push(right[col].clone());
            col += 1;
        }
    }
    removed.extend(left[row..].iter().cloned());
    added.extend(right[col..].iter().cloned());
    (removed, added)
}

fn render(
    doc: &Document,
    index: u32,
    scale: f64,
    backend: VelloCpuBackend,
    session: &mut RenderSession,
) -> Result<Option<Pixmap>> {
    let Ok(page) = doc.page(index) else {
        return Ok(None);
    };
    Ok(Some(page.render_on(
        &backend,
        &RenderOptions::scaled(scale),
        session,
    )?))
}

fn pixel_diff(
    left: &Document,
    right: &Document,
    index: u32,
    req: &Request<'_>,
    backend: VelloCpuBackend,
    session: &mut RenderSession,
) -> Result<PixelDiff> {
    let scale = req.dpi / 72.0;
    let before = render(left, index, scale, backend, session)?;
    let after = render(right, index, scale, backend, session)?;
    // A page only one side has is compared against a blank one of the same
    // size, so what it carried shows as the change.
    let blank = |like: &Pixmap| Pixmap::filled(like.width(), like.height(), pdfrum::Color::WHITE);
    let (before, after) = match (before, after) {
        (Some(before), Some(after)) => (before, after),
        (Some(only), None) => {
            let other = blank(&only);
            (only, other)
        }
        (None, Some(only)) => {
            let other = blank(&only);
            (other, only)
        }
        (None, None) => {
            return Ok(PixelDiff {
                differing: 0,
                total: 0,
                bbox: None,
                written: None,
            });
        }
    };
    let width = before.width().max(after.width());
    let height = before.height().max(after.height());
    let mut differing = 0u64;
    let mut bbox: Option<[u32; 4]> = None;
    let mut picture = req
        .out_dir
        .map(|_| Pixmap::filled(width, height, pdfrum::Color::WHITE));
    for row in 0..height {
        for col in 0..width {
            let old = before.pixel(col, row).unwrap_or([255, 255, 255, 255]);
            let new = after.pixel(col, row).unwrap_or([255, 255, 255, 255]);
            if old == new {
                if let Some(canvas) = picture.as_mut() {
                    // The unchanged page, faded, so the changes have a place.
                    let faded = [200 + old[0] / 5, 200 + old[1] / 5, 200 + old[2] / 5, 255];
                    canvas.set_pixel(col, row, faded);
                }
                continue;
            }
            differing += 1;
            bbox = Some(
                bbox.map_or([col, row, col + 1, row + 1], |[x0, y0, x1, y1]| {
                    [x0.min(col), y0.min(row), x1.max(col + 1), y1.max(row + 1)]
                }),
            );
            if let Some(canvas) = picture.as_mut() {
                canvas.set_pixel(col, row, [255, 0, 0, 255]);
            }
        }
    }
    let written = match (picture, req.out_dir) {
        (Some(canvas), Some(dir)) if differing > 0 => {
            let path = dir.join(format!("diff-{}.png", index + 1));
            canvas
                .save_png(&path)
                .with_context(|| format!("cannot write {}", path.display()))?;
            Some(path.display().to_string())
        }
        _ => None,
    };
    Ok(PixelDiff {
        differing,
        total: u64::from(width) * u64::from(height),
        bbox,
        written,
    })
}

fn print(r: &Report, term: Term) {
    if r.left_pages != r.right_pages {
        out::record(
            term,
            &[(
                "pages",
                Some(format!("{} vs {}", r.left_pages, r.right_pages)),
            )],
        );
    }
    for p in &r.pages {
        let changed = !p.removed.is_empty()
            || !p.added.is_empty()
            || p.pixels.as_ref().is_some_and(|x| x.differing > 0);
        if !changed {
            continue;
        }
        out::heading(term, &format!("page {}", p.page));
        for l in &p.removed {
            outln!("  {}", term.paint(Style::Removed, &format!("- {l}")));
        }
        for l in &p.added {
            outln!("  {}", term.paint(Style::Added, &format!("+ {l}")));
        }
        if let Some(x) = &p.pixels
            && x.differing > 0
        {
            // Hundredths of a percent, in integers: the counts are exact.
            let basis_points = x.differing * 10_000 / x.total.max(1);
            let mut line = format!(
                "pixels {} of {} differ ({}.{:02}%)",
                x.differing,
                x.total,
                basis_points / 100,
                basis_points % 100
            );
            if let Some([x0, y0, x1, y1]) = x.bbox {
                let _ = write!(line, " in {x0} {y0} {x1} {y1}");
            }
            if let Some(w) = &x.written {
                let _ = write!(line, ", wrote {w}");
            }
            outln!("  {}", term.paint(Style::Muted, &line));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::line_diff;

    #[test]
    fn a_line_diff_names_what_left_and_what_arrived() {
        let a: Vec<String> = ["one", "two", "three", "four"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let b: Vec<String> = ["one", "deux", "three", "four", "five"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let (removed, added) = line_diff(&a, &b);
        assert_eq!(removed, ["two"]);
        assert_eq!(added, ["deux", "five"]);
        let (r, ad) = line_diff(&a, &a);
        assert!(r.is_empty() && ad.is_empty());
    }
}
