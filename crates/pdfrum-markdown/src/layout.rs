//! Text that keeps its place on the page: columns stay columns, gaps stay
//! gaps, one character cell per half-em.
//!
//! Boxes are in PDF user space, y up: the top of the page is the largest
//! `y`, and rows are printed from there down.

use kurbo::Rect;

use crate::lines::Line;

/// The lines, indented by where they sit and separated by blank lines
/// where the page had space, so a two-column page reads as two columns.
///
/// A cell is half the body font size wide and one line tall; lines that
/// share a baseline are printed on one row, the right one placed by its
/// own x. Nothing is wrapped and nothing is joined.
#[must_use]
pub fn layout(lines: &[Line], page: Rect) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut sizes: Vec<f32> = lines
        .iter()
        .map(|l| l.font_size)
        .filter(|s| *s > 0.0)
        .collect();
    sizes.sort_by(f32::total_cmp);
    let body = f64::from(sizes.get(sizes.len() / 2).copied().unwrap_or(10.0)).max(1.0);
    let cell_w = body * 0.5;
    let row_h = body * 1.2;

    // Rows: lines grouped by baseline, top to bottom — y grows up the
    // page, so descending — then left to right.
    let mut ordered: Vec<&Line> = lines.iter().collect();
    ordered.sort_by(|a, b| {
        b.bbox
            .y0
            .partial_cmp(&a.bbox.y0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.bbox
                    .x0
                    .partial_cmp(&b.bbox.x0)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    let mut rows: Vec<Vec<&Line>> = Vec::new();
    for line in ordered {
        match rows.last_mut() {
            Some(row)
                if row
                    .first()
                    .is_some_and(|f| (f.bbox.y0 - line.bbox.y0).abs() < body * 0.5) =>
            {
                row.push(line);
            }
            _ => rows.push(vec![line]),
        }
    }

    let mut out = String::new();
    let mut previous_bottom: Option<f64> = None;
    for row in rows {
        let top = row.first().map_or(0.0, |l| l.bbox.y1);
        if let Some(prev) = previous_bottom {
            // One blank line per row height of empty page between the
            // previous row's bottom and this row's top.
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "clamped to 0..=3 before the cast"
            )]
            let blank = ((prev - top) / row_h).floor().clamp(0.0, 3.0) as u32;
            for _ in 0..blank {
                out.push('\n');
            }
        }
        previous_bottom = Some(row.first().map_or(top, |l| l.bbox.y0));
        let mut text = String::new();
        for line in row {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a non-negative cell count no wider than the page"
            )]
            let column = ((line.bbox.x0 - page.x0) / cell_w)
                .round()
                .clamp(0.0, 4096.0) as usize;
            let width = text.chars().count();
            if column > width {
                text.push_str(&" ".repeat(column - width));
            } else if width > 0 {
                text.push(' ');
            }
            text.push_str(&line.text);
        }
        out.push_str(text.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::layout;
    use crate::lines::Line;
    use kurbo::Rect;

    /// A line whose box has its bottom at `y`, y up.
    fn at(text: &str, x: f64, y: f64) -> Line {
        Line {
            text: text.to_owned(),
            bbox: Rect::new(x, y, x + 50.0, y + 10.0),
            font_size: 10.0,
            bold: false,
            mono: false,
            mcids: Vec::new(),
            segments: Vec::new(),
        }
    }

    #[test]
    fn two_columns_share_a_row_and_a_gap_makes_a_blank_line() {
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        // Reading order puts the lower line first; the page puts it last.
        let text = layout(
            &[
                at("below", 0.0, 60.0),
                at("left", 0.0, 100.0),
                at("right", 300.0, 100.0),
            ],
            page,
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0].trim_end(), format!("left{}right", " ".repeat(56)));
        // 30 pt of nothing between the first row's bottom and the next row's
        // top, at a 12 pt row height, is two empty rows.
        assert_eq!(lines[1], "");
        assert_eq!(lines[2], "");
        assert_eq!(lines[3], "below");
    }
}
