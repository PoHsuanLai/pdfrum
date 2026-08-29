//! Visual reordering, over the Unicode bidirectional algorithm.
//!
//! One call per section, then one query per line. The paragraph direction is
//! settable in principle but **only the automatic setting is reachable**
//! upstream — nothing outside its own tests ever sets another — so the two
//! forcing modes exist here for the ported assertions and carry no
//! conformance risk.
//!
//! The fallback matters as much as the algorithm. A line the resolver reports
//! zero runs for — which happens for a paragraph separator standing alone —
//! is laid out as a single left-to-right run rather than dropped.

/// Which way a paragraph runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// Detected from the text, defaulting to left-to-right.
    #[default]
    Auto,
    /// Forced left-to-right.
    Ltr,
    /// Forced right-to-left.
    Rtl,
}

impl Direction {
    /// The paragraph level the resolver is given, or none to detect one.
    fn level(self) -> Option<unicode_bidi::Level> {
        match self {
            Direction::Auto => None,
            Direction::Ltr => Some(unicode_bidi::Level::ltr()),
            Direction::Rtl => Some(unicode_bidi::Level::rtl()),
        }
    }
}

/// One visual run: where it starts in the logical order, how long it is, and
/// which way it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// First logical index.
    pub start: usize,
    /// How many characters.
    pub length: usize,
    /// Whether the run reads right to left.
    pub is_rtl: bool,
}

/// A section's resolved bidi state, queried per line.
#[derive(Debug)]
pub struct Resolver {
    text: String,
    /// Byte offset of each character, plus a final end offset, so a character
    /// range converts to a byte range without rescanning.
    offsets: Vec<usize>,
    direction: Direction,
}

impl Resolver {
    /// Resolves a section's characters.
    #[must_use]
    pub fn new(chars: &[u32], direction: Direction) -> Resolver {
        let mut text = String::new();
        let mut offsets = Vec::with_capacity(chars.len() + 1);
        for code in chars {
            offsets.push(text.len());
            text.push(char::from_u32(*code).unwrap_or(char::REPLACEMENT_CHARACTER));
        }
        offsets.push(text.len());
        Resolver {
            text,
            offsets,
            direction,
        }
    }

    /// The visual runs covering one line, in visual order.
    ///
    /// An empty result means the caller should fall back to a single
    /// left-to-right run — which is what a line holding nothing but a
    /// paragraph separator produces.
    #[must_use]
    pub fn visual_runs(&self, start: usize, length: usize) -> Vec<Run> {
        if length == 0 || start >= self.offsets.len().saturating_sub(1) {
            return Vec::new();
        }
        let end = (start + length).min(self.offsets.len() - 1);
        let (Some(from), Some(to)) = (self.offsets.get(start), self.offsets.get(end)) else {
            return Vec::new();
        };

        let info = unicode_bidi::BidiInfo::new(&self.text, self.direction.level());
        let Some(paragraph) = info.paragraphs.first() else {
            return Vec::new();
        };
        let (levels, ranges) = info.visual_runs(paragraph, *from..*to);

        ranges
            .into_iter()
            .filter_map(|range| {
                let run_start = self.char_index(range.start)?;
                let run_end = self.char_index(range.end)?;
                let is_rtl = levels
                    .get(range.start)
                    .is_some_and(unicode_bidi::Level::is_rtl);
                (run_end > run_start).then_some(Run {
                    start: run_start,
                    length: run_end - run_start,
                    is_rtl,
                })
            })
            .collect()
    }

    /// The character index a byte offset begins.
    fn char_index(&self, byte: usize) -> Option<usize> {
        self.offsets.iter().position(|offset| *offset == byte)
    }
}

#[cfg(test)]
mod tests {
    use super::{Direction, Resolver};

    /// The logical indices in visual order, which is what the ported
    /// assertions are written as.
    fn order(text: &str, direction: Direction) -> Vec<usize> {
        let chars: Vec<u32> = text.chars().map(|c| c as u32).collect();
        let resolver = Resolver::new(&chars, direction);
        let mut out = Vec::new();
        for run in resolver.visual_runs(0, chars.len()) {
            let span = run.start..run.start + run.length;
            if run.is_rtl {
                out.extend(span.rev());
            } else {
                out.extend(span);
            }
        }
        out
    }

    // The six orderings pinned upstream, over a Latin run and a Hebrew one
    // separated by underscores.
    const LATIN_FIRST: &str = "A_B_C_\u{5D0}_\u{5D1}";
    const HEBREW_FIRST: &str = "\u{5D0}_\u{5D1}_\u{5D2}_A_B";

    #[test]
    fn latin_first_reads_left_to_right_until_the_hebrew_run() {
        assert_eq!(
            order(LATIN_FIRST, Direction::Auto),
            [0, 1, 2, 3, 4, 5, 8, 7, 6]
        );
        assert_eq!(
            order(LATIN_FIRST, Direction::Ltr),
            [0, 1, 2, 3, 4, 5, 8, 7, 6]
        );
        assert_eq!(
            order(LATIN_FIRST, Direction::Rtl),
            [8, 7, 6, 5, 0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn hebrew_first_puts_the_whole_paragraph_the_other_way_round() {
        assert_eq!(
            order(HEBREW_FIRST, Direction::Auto),
            [6, 7, 8, 5, 4, 3, 2, 1, 0]
        );
        assert_eq!(
            order(HEBREW_FIRST, Direction::Ltr),
            [4, 3, 2, 1, 0, 5, 6, 7, 8]
        );
        assert_eq!(
            order(HEBREW_FIRST, Direction::Rtl),
            [6, 7, 8, 5, 4, 3, 2, 1, 0]
        );
    }

    #[test]
    fn an_empty_line_reports_no_runs_at_all() {
        let resolver = Resolver::new(&[], Direction::Auto);
        assert!(resolver.visual_runs(0, 0).is_empty());
        let resolver = Resolver::new(&[65], Direction::Auto);
        assert!(resolver.visual_runs(0, 0).is_empty());
        assert!(resolver.visual_runs(4, 2).is_empty());
    }
}
