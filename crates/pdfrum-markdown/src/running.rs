//! Running headers and footers, seen across pages.
//!
//! One page cannot tell a running header from a short line at its top; a
//! document can: the same words in the same band on page after page are
//! furniture. A line whose centre is in the top or bottom 12% of its page
//! is compared, band for band, with every other page's by its text with
//! each run of digits made `#` — `Page 3 of 11` and `Page 7 of 11` are
//! one line, `3 / 11 www.example.com` and `4 / 11 www.example.com` are
//! one — and a line found on three pages or more, and on more than half of
//! them, goes from every page it is on. A two-page document keeps
//! everything: two of anything is not yet a pattern.

use std::collections::{HashMap, HashSet};

use kurbo::Rect;

use crate::lines::Line;

/// The fraction of the page height at each edge where a running line sits.
const BAND: f64 = 0.12;
/// The fewest pages a line must repeat on.
const MIN_PAGES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Band {
    Top,
    Bottom,
}

/// Which of each page's lines are running headers or footers: one `bool`
/// per line of each `(lines, crop box)` pair, `true` for a line to drop.
#[must_use]
pub fn mask(pages: &[(&[Line], Rect)]) -> Vec<Vec<bool>> {
    let keys: Vec<Vec<Option<(Band, String)>>> = pages
        .iter()
        .map(|(lines, page)| {
            lines
                .iter()
                .map(|line| band(line, *page).map(|b| (b, key(&line.text))))
                .collect()
        })
        .collect();
    // How many pages each line is on: a page that draws it twice counts once.
    let mut on_pages: HashMap<&(Band, String), usize> = HashMap::new();
    for page in &keys {
        let distinct: HashSet<&(Band, String)> = page.iter().flatten().collect();
        for k in distinct {
            *on_pages.entry(k).or_default() += 1;
        }
    }
    let running = |k: &(Band, String)| {
        on_pages
            .get(k)
            .is_some_and(|&n| n >= MIN_PAGES && n * 2 > pages.len())
    };
    keys.iter()
        .map(|page| {
            page.iter()
                .map(|k| k.as_ref().is_some_and(running))
                .collect()
        })
        .collect()
}

/// The band a line's centre is in, if it is in one.
fn band(line: &Line, page: Rect) -> Option<Band> {
    if page.height() <= 0.0 || line.bbox.area() <= 0.0 {
        return None;
    }
    let reach = page.height() * BAND;
    let center = f64::midpoint(line.bbox.y0, line.bbox.y1);
    if center > page.y1 - reach {
        Some(Band::Top)
    } else if center < page.y0 + reach {
        Some(Band::Bottom)
    } else {
        None
    }
}

/// The text with every run of digits made `#` and its spacing collapsed.
fn key(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        let mut in_digits = false;
        for c in word.chars() {
            if c.is_ascii_digit() {
                if !in_digits {
                    out.push('#');
                }
                in_digits = true;
            } else {
                out.push(c);
                in_digits = false;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{key, mask};
    use crate::lines::Line;
    use kurbo::Rect;

    /// A line whose box has its *top* at `top`, y up, 10 pt tall.
    fn line(text: &str, top: f64) -> Line {
        Line {
            text: text.to_owned(),
            bbox: Rect::new(72.0, top - 10.0, 300.0, top),
            font_size: 10.0,
            bold: false,
            bold_prefix: 0,
            mono: false,
            mcids: Vec::new(),
            segments: Vec::new(),
        }
    }

    const PAGE: Rect = Rect::new(0.0, 0.0, 595.0, 842.0);

    /// The guide's furniture on page `n` of `total`, with one body line.
    fn guide_page(n: usize, total: usize) -> Vec<Line> {
        vec![
            line("Foxit MobilePDF", 805.0),
            line("Quick Guide", 786.0),
            line(&format!("Body text of page {n}"), 700.0),
            line(&format!("{n} / {total} www.foxitsoftware.com"), 79.7),
        ]
    }

    #[test]
    fn digits_are_one_mark_and_spacing_is_collapsed() {
        assert_eq!(key("Page 3 of 11"), "Page # of #");
        assert_eq!(key("3 / 11"), "# / #");
        assert_eq!(key("  Quick   Guide "), "Quick Guide");
        assert_eq!(key("v2.10a"), "v#.#a");
    }

    #[test]
    fn lines_repeated_in_a_band_on_most_pages_go_and_the_body_stays() {
        let pages: Vec<Vec<Line>> = (1..=11).map(|n| guide_page(n, 11)).collect();
        let input: Vec<(&[Line], Rect)> = pages.iter().map(|p| (p.as_slice(), PAGE)).collect();
        let got = mask(&input);
        assert_eq!(got.len(), 11);
        for page in &got {
            assert_eq!(page, &[true, true, false, true]);
        }
    }

    #[test]
    fn two_pages_are_not_a_pattern() {
        let pages: Vec<Vec<Line>> = (1..=2).map(|n| guide_page(n, 2)).collect();
        let input: Vec<(&[Line], Rect)> = pages.iter().map(|p| (p.as_slice(), PAGE)).collect();
        for page in mask(&input) {
            assert_eq!(page, &[false, false, false, false]);
        }
    }

    #[test]
    fn a_repeat_needs_a_majority_and_a_band() {
        // Six pages: a header on three of them is not a majority, and a
        // line repeated in the body is body.
        let pages: Vec<Vec<Line>> = (1..=6)
            .map(|n| {
                let mut page = vec![line("Chapter One", 700.0), line("Body", 650.0)];
                if n <= 3 {
                    page.push(line("Draft", 830.0));
                }
                if n <= 4 {
                    page.push(line("Confidential", 20.0));
                }
                page
            })
            .collect();
        let input: Vec<(&[Line], Rect)> = pages.iter().map(|p| (p.as_slice(), PAGE)).collect();
        let got = mask(&input);
        assert_eq!(got[0], [false, false, false, true]);
        assert_eq!(got[3], [false, false, true]);
        assert_eq!(got[5], [false, false]);
    }
}
