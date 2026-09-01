//! Mapping between a point, a caret position and a character index.
//!
//! Four pure queries over an already-computed [`Layout`]. Nothing here lays
//! text out or reads a document; they are searches over the positions
//! [`crate::vt::layout`] already assigned, which is what keeps the editor
//! from growing a second layout engine.
//!
//! # What a [`Place`] is
//!
//! A caret sits **after** a character, and `word == -1` names the position
//! before the first character of its line. So a line of *n* characters has
//! *n + 1* caret positions, `-1` through `n - 1`, and the one place that is
//! not "after some character" is the line header. An empty line has only its
//! header.
//!
//! # The two coordinate spaces
//!
//! [`Layout`] positions are y-**down** from the plate's top-left corner.
//! Callers work in PDF user space, y-up, so [`place_at_point`] and
//! [`point_at_place`] take and return PDF points and flip on the way in and
//! out — the same flip [`Layout::to_pdf`] performs, which is why they take
//! the plate rather than assuming one.
//!
//! Between those two spaces sits one further shift, and it is the only thing
//! the editor adds over the layout: a **vertical alignment offset**. A
//! single-line field centres its one line in its plate, so the text is drawn
//! lower than the layout placed it and a click must be lifted by the same
//! amount before it is searched. Every query here therefore takes an
//! `offset`, the same `(dx, dy)` [`crate::vt::edit_ap::generate`] is given
//! when the appearance is written. Passing `(0.0, 0.0)` is the top-aligned
//! case. Getting this wrong is not a small error: a centred field's text
//! sits *below* its layout box, so an unshifted click misses every line and
//! falls out of the content entirely.
//!
//! # The tie-break, which is the whole point of this module
//!
//! A click lands after a character when it falls **strictly past that
//! character's horizontal midpoint**. Strictly: a click exactly on the
//! midpoint lands *before* the character, so a caret placed by clicking the
//! exact centre of a glyph goes to its left. Two details make this reproduce
//! upstream rather than merely resemble it:
//!
//! - The midpoint is half the character's **advance**, comb tail included —
//!   the same [`crate::vt::word_width`] the layout used — not half its
//!   inked extent.
//! - The comparison is a plain `>` on raw floats, with **no epsilon**, unlike
//!   the vertical comparisons below, which are epsilon-tolerant. That
//!   asymmetry is upstream's and is deliberate here: it decides which side of
//!   a boundary a click falls on, and softening it moves carets.
//!
//! The vertical searches — which section a point is in, then which line —
//! use [`crate::geom::is_float_bigger`] and its sibling, so a point within
//! `0.0001` of a section or line edge counts as *inside* it.
//!
//! # Out-of-content points never fail
//!
//! A point above every section yields the very first place; one below every
//! section yields the very last. A point beside a line — left of its first
//! character or right of its last — is resolved by the same midpoint rule
//! against that line's own characters, so it clamps to the line's header or
//! its final character rather than escaping to another line. There is no
//! "missed" answer, which is what lets a mouse drag off the widget and keep
//! extending a selection.

// `place` and `plate` are one letter apart and both are the domain's own
// words: a `Place` is a caret position, and the plate is the box the text is
// set into. Renaming either to satisfy the lint would make this module read
// against the spec that names them.
#![allow(clippy::similar_names)]

use kurbo::{Point, Rect};

use crate::geom;
use crate::vt::{Config, Layout, Metrics, Section, word_width};

/// A caret position: after character `word` of line `line` of section
/// `section`.
///
/// `word == -1` is the line header — the position before the line's first
/// character. Every other value names the character the caret sits after.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Place {
    /// Which paragraph.
    pub section: u32,
    /// Which line of that paragraph.
    pub line: u32,
    /// The character the caret follows, or `-1` for the line header.
    pub word: i32,
}

impl Place {
    /// A place, from the three indices.
    #[must_use]
    pub fn new(section: u32, line: u32, word: i32) -> Place {
        Place {
            section,
            line,
            word,
        }
    }

    /// The place every layout begins at: the header of its first line.
    ///
    /// A constant rather than a query, because it does not depend on the
    /// layout — the first caret position is `(0, 0, -1)` whatever the text
    /// is, including no text at all. [`begin_place`] is the same value,
    /// spelled for symmetry with [`end_place`], which does need the layout.
    pub const START: Place = Place {
        section: 0,
        line: 0,
        word: -1,
    };

    /// [`Place::START`], as a function.
    #[must_use]
    pub fn start() -> Place {
        Place::START
    }
}

/// The beginning of the text, not `(0, 0, 0)`.
///
/// `#[derive(Default)]` would zero all three fields, and a zero `word` is the
/// position *after* the first character — a different, valid-looking caret
/// that no layout ever means by "the default". `GetBeginWordPlace`
/// (`core/fpdfdoc/cpvt_variabletext.cpp:413-415`) is `(0, 0, -1)`, and a
/// `CPVT_WordPlace` left unset is `(-1, -1, -1)`, which is not a position at
/// all. Between the two, the one a caller reaching for a default wants is the
/// beginning.
impl Default for Place {
    fn default() -> Place {
        Place::START
    }
}

/// The first place in a layout: the header of its first line.
#[must_use]
pub fn begin_place(layout: &Layout) -> Place {
    let _ = layout;
    Place::start()
}

/// The last place in a layout: after the last character of its last line.
///
/// An empty layout, and one whose last section has no lines, both answer the
/// header of section zero — there is nowhere else for a caret to be.
#[must_use]
pub fn end_place(layout: &Layout) -> Place {
    let Some(section) = layout.sections.len().checked_sub(1) else {
        return Place::new(0, 0, -1);
    };
    let Some(last) = layout.sections.get(section) else {
        return Place::new(0, 0, -1);
    };
    end_of_section(last, section)
}

/// The last place of one section.
fn end_of_section(section: &Section, index: usize) -> Place {
    let Some(line_index) = section.lines.len().checked_sub(1) else {
        return Place::new(clamp_index(index), 0, -1);
    };
    let word = section
        .lines
        .get(line_index)
        .map_or(-1, |line| if line.begin < 0 { -1 } else { line.end });
    Place::new(clamp_index(index), clamp_index(line_index), word)
}

/// A `usize` index as the `u32` a [`Place`] holds, saturating rather than
/// wrapping on a layout too large to index — such a layout cannot be produced
/// by this crate, and saturating keeps the query total.
fn clamp_index(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// The caret position a click at `point` selects.
///
/// `point` is in PDF user space (y-up); `plate` is the box the layout was set
/// into, the same one [`Config::plate`] carried. The result is always a valid
/// place — see the module docs on out-of-content points.
///
/// # The rule
///
/// Within a line, the caret lands after the last character whose horizontal
/// midpoint the point is **strictly** past. A point exactly on a midpoint is
/// therefore *before* that character, not after it.
#[must_use]
pub fn place_at_point(
    layout: &Layout,
    plate: Rect,
    config: &Config,
    metrics: &Metrics<'_>,
    offset: (f32, f32),
    point: Point,
) -> Place {
    // Into layout space: undo the alignment offset the text was drawn with,
    // then x from the plate's left edge and y downward from its top. The
    // same transform `point_at_place` inverts.
    #[allow(clippy::cast_possible_truncation)]
    let x = point.x as f32 - offset.0 - geom::left(plate);
    #[allow(clippy::cast_possible_truncation)]
    let y = geom::top(plate) - (point.y as f32 - offset.1);

    match find_section(layout, y) {
        Found::Inside(index, section) => {
            let mut place = place_in_section(section, config, metrics, x, y);
            place.section = clamp_index(index);
            place
        }
        Found::Above => begin_place(layout),
        Found::Below => end_place(layout),
    }
}

/// Which section a y falls in, or which way it fell off.
enum Found<'a> {
    /// Inside this section, at this index.
    Inside(usize, &'a Section),
    /// Above every section.
    Above,
    /// Below every section, or there are none.
    Below,
}

/// Locates the section a layout-space y sits in.
///
/// Sections are laid out top to bottom and never overlap, so this is a scan
/// for the first whose box contains the point, with the two off-the-end
/// answers distinguished. A point in the seam between two sections — which
/// the epsilon makes possible — belongs to the first, because the scan stops
/// at the first match.
fn find_section(layout: &Layout, y: f32) -> Found<'_> {
    let mut any_above = false;
    for (index, section) in layout.sections.iter().enumerate() {
        let (top, bottom) = (geom::bottom(section.rect), geom::top(section.rect));
        if geom::is_float_smaller(y, top) {
            // Above this section. Since sections only descend from here, the
            // point is above everything unless an earlier one already claimed
            // to be above the point.
            return if any_above {
                Found::Below
            } else {
                Found::Above
            };
        }
        if geom::is_float_bigger(y, bottom) {
            any_above = true;
            continue;
        }
        return Found::Inside(index, section);
    }
    Found::Below
}

/// The place a point selects within one section, in layout space.
fn place_in_section(
    section: &Section,
    config: &Config,
    metrics: &Metrics<'_>,
    x: f32,
    y: f32,
) -> Place {
    let Some((index, line)) = find_line(section, y) else {
        // Above or below every line of a section the point is nonetheless
        // inside. Falling to the section's own ends keeps the answer a valid
        // place; an empty section has only its header.
        return if section.lines.is_empty() {
            Place::new(0, 0, -1)
        } else {
            end_of_section(section, 0)
        };
    };
    Place::new(
        0,
        clamp_index(index),
        word_at_x(section, line_range(line), config, metrics, x),
    )
}

/// Locates the line a section-space y sits in.
///
/// A y above the first line answers the first line, and one below the last
/// answers the last — clicking in a field's top or bottom margin puts the
/// caret on the nearest line rather than nowhere.
fn find_line(section: &Section, y: f32) -> Option<(usize, crate::vt::Line)> {
    let mut last: Option<(usize, crate::vt::Line)> = None;
    for (index, line) in section.lines.iter().enumerate() {
        // A line's box runs from its baseline less its ascent to its baseline
        // less its descent — descent being negative, so the bottom is below.
        let top = line.y - line.ascent;
        let bottom = line.y - line.descent;
        if geom::is_float_smaller(y, top) {
            // Above this line. The first line claims a point above the whole
            // section; otherwise the point sits in the gap above this one and
            // the line before it is the nearer.
            return Some(last.unwrap_or((index, *line)));
        }
        if geom::is_float_bigger(y, bottom) {
            last = Some((index, *line));
            continue;
        }
        return Some((index, *line));
    }
    last
}

/// The half-open range of word indices one line covers.
///
/// A line with `begin < 0` is the single line of an empty section and covers
/// nothing.
fn line_range(line: crate::vt::Line) -> std::ops::Range<usize> {
    if line.begin < 0 || line.end < line.begin {
        return 0..0;
    }
    let begin = usize::try_from(line.begin).unwrap_or(0);
    let end = usize::try_from(line.end).unwrap_or(0);
    begin..end.saturating_add(1)
}

/// Which character of a line's range a layout-space x lands after.
///
/// Returns `-1` for the line header. This is the tie-break the module docs
/// describe: strictly past a character's midpoint puts the caret after it.
fn word_at_x(
    section: &Section,
    range: std::ops::Range<usize>,
    config: &Config,
    metrics: &Metrics<'_>,
    x: f32,
) -> i32 {
    let mut answer: i32 = -1;
    for index in range {
        let Some(word) = section.words.get(index) else {
            break;
        };
        let midpoint = word.x + word_width(word, config, metrics, config.font_size) * 0.5;
        // Raw `>`, no epsilon: the boundary belongs to the character's left
        // half, so a click exactly on a midpoint lands before that character.
        if x > midpoint {
            answer = i32::try_from(index).unwrap_or(answer);
        } else {
            break;
        }
    }
    answer
}

/// Where a caret at `place` is drawn, in PDF user space.
///
/// The returned point is the caret's **top**: `x` is the trailing edge of the
/// character the place names (its leading edge, for a line header), and `y`
/// is the line's ascent line. [`caret_rect`] turns that into the rectangle
/// [`crate::ap::field_body::Highlight`] wants.
///
/// A place naming a section, line or character the layout does not have
/// answers that container's own start rather than failing — the layout can
/// shrink under an edit while a caret still points into it, and a caret
/// outside the text is not an error.
#[must_use]
pub fn point_at_place(
    layout: &Layout,
    plate: Rect,
    config: &Config,
    metrics: &Metrics<'_>,
    offset: (f32, f32),
    place: Place,
) -> Point {
    let (x, y) = caret_position(layout, config, metrics, place);
    let (x, y) = Layout::to_pdf(plate, x, y);
    Point::new(f64::from(x + offset.0), f64::from(y + offset.1))
}

/// The caret's rectangle: a hairline from the line's ascent to its descent.
///
/// `width` is the caret's thickness — [`crate::ap::field_body::CARET_WIDTH`]
/// for a drawn caret. The rectangle is in PDF user space, ready to hand to
/// [`crate::ap::field_body::Highlight`].
#[must_use]
pub fn caret_rect(
    layout: &Layout,
    plate: Rect,
    config: &Config,
    metrics: &Metrics<'_>,
    offset: (f32, f32),
    place: Place,
    width: f32,
) -> Rect {
    let (x, top) = caret_position(layout, config, metrics, place);
    let height = line_extent(layout, config, metrics, place);
    let (left, top) = Layout::to_pdf(plate, x, top);
    let (left, top) = (left + offset.0, top + offset.1);
    geom::rect(left, top - height, left + width, top)
}

/// The caret's x and its line's ascent line, both in layout space.
fn caret_position(
    layout: &Layout,
    config: &Config,
    metrics: &Metrics<'_>,
    place: Place,
) -> (f32, f32) {
    let Some(section) = layout.sections.get(place.section as usize) else {
        return (0.0, 0.0);
    };
    let Some(line) = section.lines.get(place.line as usize) else {
        return (0.0, 0.0);
    };
    let top = line.y - line.ascent;
    if place.word < 0 {
        // The line header sits at the line's own left edge, which alignment
        // may have moved away from the plate's.
        return (line.x, top);
    }
    let index = usize::try_from(place.word).unwrap_or(0);
    let Some(word) = section.words.get(index) else {
        return (line.x, top);
    };
    (
        word.x + word_width(word, config, metrics, config.font_size),
        top,
    )
}

/// The height of the line a place sits on.
fn line_extent(layout: &Layout, config: &Config, metrics: &Metrics<'_>, place: Place) -> f32 {
    let fallback = || {
        crate::vt::font_ascent(metrics, config.font_size)
            - crate::vt::font_descent(metrics, config.font_size)
    };
    layout
        .sections
        .get(place.section as usize)
        .and_then(|section| section.lines.get(place.line as usize))
        .map_or_else(fallback, |line| line.ascent - line.descent)
}

/// How many characters precede `place` in the layout's text.
///
/// Counting the way the text itself does: every character of every earlier
/// section, **plus one for each section break**, plus the characters of this
/// place's own line up to and including the one it names. So the index of the
/// caret before the first character is zero, and the index after the last is
/// the text's length.
///
/// # A wrapped line's header is not a position of its own
///
/// `place_at_point` names the *line header* — `word == -1` — for a click left
/// of every midpoint on a line, and on a wrapped line that header is the same
/// caret position as the end of the line above it. Upstream folds the two
/// before counting: `WordPlaceToWordIndex`
/// (`core/fpdfdoc/cpvt_variabletext.cpp:361-379`) runs `UpdateWordPlace`
/// first, whose `PrevLineHeaderPlace` (`:715-720`) is
///
/// ```cpp
/// if (place.nWordIndex < 0 && place.nLineIndex > 0)
///     return GetPrevWordPlace(place);
/// ```
///
/// So the header of line 1 of a section whose first line holds characters
/// `0..=4` counts as **5**, not 0. Skipping the fold is not a rounding
/// difference: it sends a caret clicked at the start of a wrapped line to the
/// start of the whole field, and the next keystroke lands there.
///
/// # A place past the layout
///
/// Clamped to the end, and the clamp counts **no trailing section break** —
/// `WordPlaceToWordIndex`'s loop adds `kReturnLength` only when `i != sz - 1`
/// (`:372-374`), so an out-of-range section index yields the last section's
/// end rather than one past it.
#[must_use]
pub fn word_index_of_place(layout: &Layout, place: Place) -> usize {
    // The fold, before anything is counted. A header on the first line has no
    // line above it and stays where it is.
    if place.word < 0 && place.line > 0 {
        let previous = layout
            .sections
            .get(place.section as usize)
            .and_then(|section| section.lines.get(place.line as usize))
            .map(|line| Place::new(place.section, place.line - 1, line.begin - 1));
        if let Some(previous) = previous {
            return word_index_of_place(layout, previous);
        }
    }

    let target = place.section as usize;
    let last = layout.sections.len().saturating_sub(1);
    let mut index: usize = 0;
    for (position, section) in layout.sections.iter().enumerate() {
        if position >= target {
            break;
        }
        index = index.saturating_add(section.words.len());
        // The break between this section and the next counts as one
        // character, exactly as the tokenizer counted it on the way in — and
        // there is no break after the last section to count.
        if position != last {
            index = index.saturating_add(SECTION_BREAK_LENGTH);
        }
    }
    let Some(section) = layout.sections.get(target) else {
        return index;
    };
    let after = usize::try_from(place.word.saturating_add(1)).unwrap_or(0);
    index.saturating_add(after.min(section.words.len()))
}

/// A section break counts as one character in the flat index, the way the
/// line break it came from did.
const SECTION_BREAK_LENGTH: usize = 1;

/// Which caret position sits after `index` characters of the layout's text.
///
/// The inverse of [`word_index_of_place`]: for every place a layout can
/// produce, `place_of_word_index(layout, word_index_of_place(layout, p)) ==
/// p`. An index past the end answers the last place.
///
/// An index landing exactly *on* a section break — the position between a
/// paragraph's last character and the next paragraph's first — answers the
/// **end of the earlier** section, which is where a caret typed to that point
/// actually sits.
#[must_use]
pub fn place_of_word_index(layout: &Layout, index: usize) -> Place {
    let mut consumed: usize = 0;
    for (position, section) in layout.sections.iter().enumerate() {
        let end = consumed.saturating_add(section.words.len());
        if index <= end {
            let within = index.saturating_sub(consumed);
            let word = i32::try_from(within).unwrap_or(i32::MAX).saturating_sub(1);
            return place_of_word(section, position, word);
        }
        consumed = end.saturating_add(SECTION_BREAK_LENGTH);
    }
    end_place(layout)
}

/// The place naming character `word` of a section, with the line it falls on
/// resolved.
fn place_of_word(section: &Section, index: usize, word: i32) -> Place {
    let line = line_of_word(section, word);
    Place::new(clamp_index(index), line, word)
}

/// Which line of a section a character index falls on.
///
/// A `-1` word is the first line's header. A character past the section's
/// last line answers that last line, which keeps the place valid.
fn line_of_word(section: &Section, word: i32) -> u32 {
    if word < 0 {
        return 0;
    }
    for (index, line) in section.lines.iter().enumerate() {
        if line.begin < 0 {
            continue;
        }
        if word >= line.begin && word <= line.end {
            return clamp_index(index);
        }
    }
    clamp_index(section.lines.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::{
        Place, begin_place, caret_rect, end_place, place_at_point, place_of_word_index,
        point_at_place, word_index_of_place,
    };
    use crate::geom;
    use crate::vt::{self, Config, Metrics};
    use kurbo::Point;

    /// A single-line plate a hundred wide and thirty tall, the shape
    /// `text_form.pdf`'s field has.
    fn config() -> Config {
        Config {
            plate: geom::rect(101.0, 101.0, 199.0, 129.0),
            font_size: 12.0,
            ..Config::default()
        }
    }

    /// Helvetica's advance widths, per mille, for the characters the tests
    /// use. Hand-transcribed so the arithmetic below can be checked by hand
    /// rather than against whatever the font loader answers.
    fn helvetica(ch: u32) -> i32 {
        match char::from_u32(ch) {
            Some('A' | 'B' | 'E') => 667,
            Some('C' | 'D' | 'H') => 722,
            Some('F') => 611,
            Some('G') => 778,
            Some(' ') => 278,
            _ => 556,
        }
    }

    fn metrics() -> Metrics<'static> {
        Metrics {
            width: &helvetica,
            ascent: 723,
            descent: -207,
        }
    }

    /// Every character ten wide, which makes midpoints land on fives.
    fn ten(_: u32) -> i32 {
        10
    }

    fn tens() -> Metrics<'static> {
        Metrics {
            width: &ten,
            ascent: 1000,
            descent: -200,
        }
    }

    /// The offset a single-line field is drawn with: its one line centred in
    /// its plate. The same expression `ap::field_body::vertical_offset`
    /// computes, restated here so the tests do not depend on that module.
    fn centred(layout: &vt::Layout, config: &Config) -> (f32, f32) {
        let content = layout.content_rect_pdf(config.plate);
        (
            0.0,
            (geom::height(content) - geom::height(config.plate)) * 0.5,
        )
    }

    fn at(config: &Config, metrics: &Metrics<'_>, text: &str, x: f32, y: f32) -> Place {
        let layout = vt::layout(text, config, metrics);
        let offset = centred(&layout, config);
        place_at_point(
            &layout,
            config.plate,
            config,
            metrics,
            offset,
            Point::new(f64::from(x), f64::from(y)),
        )
    }

    /// The acceptance geometry: `text_form.pdf`'s field, Helvetica 12, the
    /// eight characters the embeddertest types, and the click it makes.
    ///
    /// The field's `/Rect` is `[100 100 200 130]` and its default border is
    /// one unit, so the plate starts at x = 101. Helvetica 12 sets
    /// `ABCDEFGH` at these advances:
    ///
    /// ```text
    ///   A 101.000 .. 109.004   midpoint 105.002
    ///   B 109.004 .. 117.008   midpoint 113.006
    ///   C 117.008 .. 125.672   midpoint 121.340
    ///   D 125.672 .. 134.336   midpoint 130.004
    ///   E 134.336 .. 142.340   midpoint 138.338
    /// ```
    ///
    /// A click at x = 134 is past D's midpoint and short of E's, so the caret
    /// lands after D — after exactly four characters, which is what
    /// `InsertTextInPopulatedTextFieldMiddle` observes when it inserts
    /// `Hello` and reads back `ABCDHelloEFGH`.
    #[test]
    fn the_embeddertests_middle_click_lands_after_four_characters() {
        let config = config();
        let metrics = metrics();
        let layout = vt::layout("ABCDEFGH", &config, &metrics);
        let place = place_at_point(
            &layout,
            config.plate,
            &config,
            &metrics,
            centred(&layout, &config),
            Point::new(134.0, 115.0),
        );
        assert_eq!(place.word, 3, "the caret sits after D");
        assert_eq!(
            word_index_of_place(&layout, place),
            4,
            "four characters precede the caret"
        );
    }

    /// The two neighbouring embeddertests, which calibrate the same rule at
    /// the ends: a click at the field's left edge inserts in front of
    /// everything, and one at x = 166 inserts behind it.
    #[test]
    fn the_same_field_clicked_at_either_end_gives_the_two_extremes() {
        let config = config();
        let metrics = metrics();
        let layout = vt::layout("ABCDEFGH", &config, &metrics);
        let offset = centred(&layout, &config);
        let begin = place_at_point(
            &layout,
            config.plate,
            &config,
            &metrics,
            offset,
            Point::new(102.0, 115.0),
        );
        assert_eq!(begin.word, -1, "before the first character");
        assert_eq!(word_index_of_place(&layout, begin), 0);

        let end = place_at_point(
            &layout,
            config.plate,
            &config,
            &metrics,
            offset,
            Point::new(166.0, 115.0),
        );
        assert_eq!(end.word, 7, "after the last character");
        assert_eq!(word_index_of_place(&layout, end), 8);
    }

    /// The tie-break itself, on a plate where the midpoints are round
    /// numbers. Every character is ten wide, so the first sits at 101..111
    /// with its midpoint at 106.
    #[test]
    fn a_click_exactly_on_a_midpoint_lands_before_that_character() {
        let config = config();
        let metrics = tens();
        // Ten wide at size twelve is 10 * 12 * 0.001 = 0.12 per character,
        // so use a size that makes the arithmetic whole.
        let config = Config {
            font_size: 1000.0,
            ..config
        };
        // Each character is now ten units wide: 101..111, 111..121, ...
        assert_eq!(at(&config, &metrics, "abc", 106.0, 115.0).word, -1);
        // A hair past it, and the caret moves after the character.
        assert_eq!(at(&config, &metrics, "abc", 106.001, 115.0).word, 0);
        // A hair before, and it does not.
        assert_eq!(at(&config, &metrics, "abc", 105.999, 115.0).word, -1);
        // The second character's midpoint, 116, behaves the same way.
        assert_eq!(at(&config, &metrics, "abc", 116.0, 115.0).word, 0);
        assert_eq!(at(&config, &metrics, "abc", 116.001, 115.0).word, 1);
    }

    /// Left of, right of, above and below the text. None of these fail, and
    /// each clamps to the nearest place rather than to nothing.
    #[test]
    fn points_outside_the_content_clamp_to_its_ends() {
        let config = config();
        let metrics = metrics();
        let layout = vt::layout("ABCDEFGH", &config, &metrics);
        let offset = centred(&layout, &config);
        let ask = |x: f32, y: f32| {
            place_at_point(
                &layout,
                config.plate,
                &config,
                &metrics,
                offset,
                Point::new(f64::from(x), f64::from(y)),
            )
        };
        // Far left of the first character, on the line.
        assert_eq!(ask(-1000.0, 115.0).word, -1);
        // Far right of the last, on the line.
        assert_eq!(ask(1000.0, 115.0).word, 7);
        // Above everything: the layout's first place.
        assert_eq!(ask(134.0, 10_000.0), begin_place(&layout));
        // Below everything: its last.
        assert_eq!(ask(134.0, -10_000.0), end_place(&layout));
    }

    /// An empty field has exactly one place, and every click finds it.
    #[test]
    fn an_empty_field_answers_its_only_place_from_anywhere() {
        let config = config();
        let metrics = metrics();
        let layout = vt::layout("", &config, &metrics);
        for (x, y) in [
            (102.0, 115.0),
            (198.0, 115.0),
            (134.0, 10_000.0),
            (134.0, -10_000.0),
            (-500.0, 115.0),
        ] {
            let place = place_at_point(
                &layout,
                config.plate,
                &config,
                &metrics,
                centred(&layout, &config),
                Point::new(x, y),
            );
            assert_eq!(place.word, -1, "at ({x}, {y})");
            assert_eq!(word_index_of_place(&layout, place), 0);
        }
    }

    /// A multi-line field: which line a y picks, and that an empty paragraph
    /// between two full ones is reachable.
    #[test]
    fn a_blank_line_between_paragraphs_is_its_own_place() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 200.0, 200.0),
            font_size: 10.0,
            multi_line: true,
            ..Config::default()
        };
        let metrics = tens();
        let layout = vt::layout("ab\n\ncd", &config, &metrics);
        assert_eq!(layout.sections.len(), 3, "three paragraphs");
        assert!(
            layout.sections.get(1).is_some_and(|s| s.words.is_empty()),
            "the middle one is empty"
        );
        // The empty paragraph's only place, reached through the index pair.
        let blank = place_of_word_index(&layout, 3);
        assert_eq!(blank, Place::new(1, 0, -1));
        assert_eq!(word_index_of_place(&layout, blank), 3);
    }

    /// A defaulted `Place` is the beginning of the text, not the position
    /// after the first character.
    #[test]
    fn a_default_place_is_the_start_and_not_a_zero_word() {
        assert_eq!(Place::default(), Place::START);
        assert_eq!(Place::default(), Place::start());
        assert_eq!(Place::START.word, -1, "a zero word is after character 0");
    }

    /// A wrapped line's header is the previous line's end, and it is a
    /// *caret position*, not zero.
    ///
    /// `PrevLineHeaderPlace` (`cpvt_variabletext.cpp:715-720`) folds
    /// `word < 0 && line > 0` onto `GetPrevWordPlace` before
    /// `WordPlaceToWordIndex` counts anything. Without the fold a click at
    /// the start of the second visual line reports index 0 — the start of the
    /// whole field — and the next keystroke is inserted there.
    #[test]
    fn a_wrapped_lines_header_indexes_to_the_end_of_the_line_above() {
        // `tens()` is ten thousandths of an em, so at 10pt each character
        // advances 0.1 units: a 0.45-unit plate holds four per line and
        // "abcdefgh" wraps after "abcd".
        let config = Config {
            plate: geom::rect(0.0, 0.0, 0.45, 200.0),
            font_size: 10.0,
            multi_line: true,
            auto_return: true,
            ..Config::default()
        };
        let layout = vt::layout("abcdefgh", &config, &tens());
        let section = layout.sections.first().expect("one section");
        assert!(section.lines.len() >= 2, "the text must wrap: {section:?}");
        let first_line_end = section.lines.first().expect("a first line").end;

        // The second line's header, which is what a click left of its first
        // midpoint produces.
        let header = Place::new(0, 1, -1);
        assert_eq!(
            word_index_of_place(&layout, header),
            word_index_of_place(&layout, Place::new(0, 0, first_line_end)),
            "the header must be the line above's end, not the field's start"
        );
        assert_ne!(word_index_of_place(&layout, header), 0);
    }

    /// A place naming a section the layout does not have is the end, and the
    /// end is not one past it.
    ///
    /// `WordPlaceToWordIndex`'s loop adds `kReturnLength` only when
    /// `i != sz - 1` (`cpvt_variabletext.cpp:372-374`), so the clamp counts no
    /// trailing section break.
    #[test]
    fn a_place_past_the_last_section_indexes_to_the_very_end() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 200.0, 200.0),
            font_size: 10.0,
            multi_line: true,
            ..Config::default()
        };
        for text in ["abc", "ab\ncd", "a\nb\nc"] {
            let layout = vt::layout(text, &config, &tens());
            assert_eq!(
                word_index_of_place(&layout, Place::new(9, 0, 0)),
                word_index_of_place(&layout, end_place(&layout)),
                "{text:?}"
            );
        }
    }

    /// The round trip brief §4.4's P6 states: every place a layout can name
    /// survives being turned into an index and back.
    #[test]
    fn every_place_survives_the_index_round_trip() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 200.0, 200.0),
            font_size: 10.0,
            multi_line: true,
            ..Config::default()
        };
        let metrics = tens();
        for text in ["", "a", "abc", "ab\ncd", "ab\n\ncd", "a\nb\nc\nd"] {
            let layout = vt::layout(text, &config, &metrics);
            for (s, section) in layout.sections.iter().enumerate() {
                for (l, line) in section.lines.iter().enumerate() {
                    let last = if line.begin < 0 { -1 } else { line.end };
                    for word in -1..=last {
                        let place = Place::new(
                            u32::try_from(s).unwrap_or(0),
                            u32::try_from(l).unwrap_or(0),
                            word,
                        );
                        let index = word_index_of_place(&layout, place);
                        // A later line's header is not a caret position of
                        // its own: it collapses to the end of the line above,
                        // which is where the round trip lands. Asserting the
                        // collapse *target* is the point — skipping the case
                        // is what let it collapse to zero unnoticed.
                        let expected = if word < 0 && l > 0 {
                            let previous = section.lines.get(l - 1).expect("a line above");
                            place_of_word_index(
                                &layout,
                                word_index_of_place(
                                    &layout,
                                    Place::new(
                                        u32::try_from(s).unwrap_or(0),
                                        u32::try_from(l - 1).unwrap_or(0),
                                        previous.end,
                                    ),
                                ),
                            )
                        } else {
                            place
                        };
                        assert_eq!(
                            place_of_word_index(&layout, index),
                            expected,
                            "{text:?} at {place:?} (index {index})"
                        );
                    }
                }
            }
        }
    }

    /// An index past the text's end is the last place, not a panic.
    #[test]
    fn an_index_past_the_end_answers_the_last_place() {
        let config = config();
        let metrics = metrics();
        let layout = vt::layout("ABC", &config, &metrics);
        assert_eq!(place_of_word_index(&layout, 3), end_place(&layout));
        assert_eq!(place_of_word_index(&layout, 9999), end_place(&layout));
    }

    /// Clicking where a caret is drawn puts the caret back where it was:
    /// `place_at_point` and `point_at_place` agree at every position.
    #[test]
    fn a_caret_clicked_at_its_own_position_does_not_move() {
        let config = config();
        let metrics = metrics();
        let layout = vt::layout("ABCDEFGH", &config, &metrics);
        let offset = centred(&layout, &config);
        for word in -1..8 {
            let place = Place::new(0, 0, word);
            let point = point_at_place(&layout, config.plate, &config, &metrics, offset, place);
            // A click *at* the caret is exactly on a character boundary,
            // which is a half-width past the previous character's midpoint —
            // strictly past it, so the caret stays put.
            let back = place_at_point(
                &layout,
                config.plate,
                &config,
                &metrics,
                offset,
                Point::new(point.x, point.y - 1.0),
            );
            assert_eq!(back, place, "caret at {word} moved");
        }
    }

    /// The caret rectangle a focused field draws: as wide as asked, as tall
    /// as its line, and starting where the character it follows ends.
    #[test]
    fn the_caret_rectangle_spans_its_line() {
        let config = config();
        let metrics = metrics();
        let layout = vt::layout("ABCDEFGH", &config, &metrics);
        let rect = caret_rect(
            &layout,
            config.plate,
            &config,
            &metrics,
            (0.0, 0.0),
            Place::new(0, 0, 3),
            0.4,
        );
        // After D, whose advance ends at 134.336.
        assert!(
            (geom::left(rect) - 134.336).abs() < 0.01,
            "{:?}",
            geom::left(rect)
        );
        assert!((geom::width(rect) - 0.4).abs() < 1e-5);
        // As tall as Helvetica 12's ascent plus descent.
        let expected = (723.0 + 207.0) * 12.0 * 0.001;
        assert!(
            (geom::height(rect) - expected).abs() < 0.01,
            "{:?} vs {expected}",
            geom::height(rect)
        );
    }

    /// A place naming something the layout does not have answers a valid
    /// point rather than panicking — the caret can outlive an edit that
    /// shortened the text.
    #[test]
    fn a_place_past_the_layout_still_answers_a_point() {
        let config = config();
        let metrics = metrics();
        let layout = vt::layout("AB", &config, &metrics);
        for place in [
            Place::new(9, 0, 0),
            Place::new(0, 9, 0),
            Place::new(0, 0, 99),
            Place::new(u32::MAX, u32::MAX, i32::MAX),
        ] {
            let point = point_at_place(&layout, config.plate, &config, &metrics, (0.0, 0.0), place);
            assert!(point.x.is_finite() && point.y.is_finite(), "{place:?}");
            // The index of an out-of-range place is whatever the walk
            // accumulated before it ran out — a number, never a panic — and
            // feeding it back always lands on a place the layout does hold.
            let index = word_index_of_place(&layout, place);
            let back = place_of_word_index(&layout, index);
            assert!(
                (back.section as usize) < layout.sections.len(),
                "{place:?} came back as {back:?}"
            );
        }
    }

    /// A comb field's cells: the tail each character carries widens its
    /// advance, so the midpoints are the cells' midpoints and not the
    /// glyphs'.
    #[test]
    fn comb_cells_hit_test_on_the_cell_rather_than_the_glyph() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 100.0, 20.0),
            font_size: 10.0,
            char_array: 4,
            ..Config::default()
        };
        let metrics = tens();
        let layout = vt::layout("ab", &config, &metrics);
        // Four cells across a hundred units: 25 each. The first character's
        // advance therefore runs a full cell, so its midpoint is far right of
        // the glyph's own centre.
        let first = layout
            .sections
            .first()
            .and_then(|s| s.words.first())
            .copied()
            .expect("a first character");
        let width = vt::word_width(&first, &config, &metrics, config.font_size);
        assert!(width > 20.0, "the cell tail widened the advance: {width}");
        let mid = first.x + width * 0.5;
        let before = place_at_point(
            &layout,
            config.plate,
            &config,
            &metrics,
            (0.0, 0.0),
            Point::new(f64::from(mid) - 0.01, 10.0),
        );
        let after = place_at_point(
            &layout,
            config.plate,
            &config,
            &metrics,
            (0.0, 0.0),
            Point::new(f64::from(mid) + 0.01, 10.0),
        );
        assert_eq!(before.word, -1);
        assert_eq!(after.word, 0);
    }
}
