//! The extraction pipeline: collection, reordering, emission, and the
//! decisions between objects (`docs/design/pdfrum-text.md` §1.5–§1.9).
//!
//! # It reorders; it does not stream
//!
//! Text objects are not emitted where they are found. They are collected into
//! a batch, insertion-sorted by their transformed x, and flushed as a batch
//! whenever the next object's y jumps far enough to be a new line. Only then
//! are characters emitted, into a *staging* line, which a later stage
//! bidi-segments and moves into the final output. A character can be
//! reordered, merged, deleted, normalized into several, or dropped at four
//! different stages, and getting the stage order wrong changes the output even
//! when every individual threshold is right.
//!
//! # Spacing is geometry, not content
//!
//! Nothing here reads a space out of the content stream and treats it
//! specially. Every generated space comes from comparing a gap against a
//! threshold derived from the font's own space glyph — or, when that glyph is
//! untrustworthy, from the current character's width bucketed into quarters,
//! fifths and sixths. The thresholds are absolute numbers with no explanation
//! in the source and no test upstream; they are transcribed and pinned here.

use crate::charinfo::{
    CharBox, CharType, LooseBoundsInput, ObjectIndex, inverse_or_zero, loose_bounds, matrix_angle,
    transform_distance, transform_rect, transform_x_distance,
};
use crate::line::{Line, Output};
use crate::object::{Item, TextRun, ladder_char_width};
use crate::orientation::{Orientation, object_flow};
use crate::unicode::{is_alnum, is_alpha, is_print};
use kurbo::{Affine, Point, Rect};
use pdfrum_common::{DiagKind, Diagnostics, Severity};
use pdfrum_font::CharCode;
use pdfrum_object::{Name, Resolve};

/// The width below which a text object is not worth extracting at all, and
/// the height below which a character's box is rescued. In page space.
const SIZE_EPSILON: f64 = 0.01;

/// The font size a character with no text object reports.
const DEFAULT_FONT_SIZE: f32 = 1.0;

/// The `/ActualText` key, read two different ways by the two marked-content
/// passes — which is the whole point of §1.9.
fn actual_text_key() -> Name {
    Name::from("ActualText")
}

/// What to put between the previous object and this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generate {
    /// Nothing.
    None,
    /// A space.
    Space,
    /// A line break.
    LineBreak,
    /// A soft hyphen: the previous character becomes the sentinel and the
    /// line breaks.
    Hyphen,
}

/// What a marked-content scan decided about an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkState {
    /// Emit the object's glyphs normally.
    Pass,
    /// Emit nothing at all: either this object continues a mark already
    /// emitted, or its `/ActualText` holds nothing printable.
    Done,
    /// Emit the `/ActualText` string instead of the glyphs.
    Delay,
}

/// The previous object that actually emitted something.
#[derive(Debug, Clone)]
struct Previous {
    index: ObjectIndex,
}

/// The whole in-progress extraction.
///
/// One value, built by [`crate::extract`], driven, and destructured. Small on
/// purpose: the C++'s fourteen-member class splits into this plus a file of
/// free functions that take exactly what they need.
pub struct Builder<'a, R: Resolve> {
    /// The final output.
    pub out: Output,
    /// The staging line.
    line: Line,
    /// Every text object on the page, in walk order.
    runs: &'a [TextRun],
    /// The batch of objects awaiting a flush, as indices into
    /// [`runs`](Self::runs), sorted by transformed x.
    batch: Vec<usize>,
    /// The last object that emitted anything.
    previous: Option<Previous>,
    /// The page-global orientation guess.
    page_flow: Orientation,
    /// The union of the boxes of the objects on the current output line.
    line_rect: Rect,
    /// Page space to device space, which the batch split and one escape
    /// hatch measure in.
    display: Affine,
    /// `/ViewerPreferences /Direction (R2L)`.
    rtl: bool,
    resolver: &'a R,
}

impl<'a, R: Resolve> Builder<'a, R> {
    /// A builder over one page's text objects.
    pub fn new(
        runs: &'a [TextRun],
        page_flow: Orientation,
        display: Affine,
        rtl: bool,
        resolver: &'a R,
    ) -> Self {
        Self {
            out: Output::default(),
            line: Line::default(),
            runs,
            batch: Vec::new(),
            previous: None,
            page_flow,
            line_rect: Rect::ZERO,
            display,
            rtl,
            resolver,
        }
    }

    fn run(&self, index: usize) -> Option<&'a TextRun> {
        self.runs.get(index)
    }

    /// Offers one text object to the batch (`ProcessTextObject`).
    ///
    /// The object is dropped outright when its box has no width, and dropped
    /// again — this time losing the object that *follows* a zero-glyph one,
    /// which is a real quirk — when the batch's last entry shows nothing. It
    /// otherwise either triggers a flush (the y jumped) or is
    /// insertion-sorted into place by its transformed x, which is the whole
    /// of the reading-order machinery.
    pub fn offer(&mut self, index: usize, diags: &mut Diagnostics) {
        let Some(run) = self.run(index) else { return };
        // `[oracle-bug]` Gate on the **advance**, not the glyph bounding box.
        // `cpdf_textpage.cpp:881` tests `GetRect().Width()`, which
        // `cpdf_textobject.cpp:305-331` builds from `GetCharBBox` — so an
        // object made only of spaces, whose boxes are empty but whose `w0`
        // per §9.4.3 is not, disappears before extraction. See
        // [`TextRun::advance`](crate::object::TextRun::advance).
        if run.advance < SIZE_EPSILON && run.rect.width().abs() < SIZE_EPSILON {
            diags.record(Severity::Recovered, DiagKind::TextObjectDegenerate, None);
            return;
        }
        let Some(&last) = self.batch.last() else {
            self.batch.push(index);
            return;
        };
        let Some(previous) = self.run(last) else {
            return;
        };
        if previous.count() == 0 {
            // The object *after* a zero-glyph one is discarded, not deferred.
            diags.record(Severity::Recovered, DiagKind::TextObjectDropped, None);
            return;
        }

        let previous_width = previous
            .item(previous.count() - 1)
            .map_or(0.0, |item| ladder_char_width(previous, Some(item.code)));
        let previous_width = f64::from(previous_width * previous.font_size / 1000.0).abs();
        let previous_width = transform_distance(previous.text_matrix, previous_width);

        let this_width = run
            .item(0)
            .map_or(0.0, |item| ladder_char_width(run, Some(item.code)));
        let this_width = f64::from(this_width * run.font_size / 1000.0).abs();
        let this_width = transform_distance(run.text_matrix, this_width);

        // Dimensionally inconsistent on purpose: the widths never pass through
        // the display matrix while the positions do. It is the behaviour.
        let threshold = previous_width.max(this_width) / 4.0;
        let previous_pos = self.display * previous.position;
        let this_pos = self.display * run.position;

        if (this_pos.y - previous_pos.y).abs() > threshold * 2.0 {
            self.flush(diags);
            self.batch.clear();
            self.batch.push(index);
            return;
        }

        // Scan backwards and insert after the first entry whose x is at or
        // below ours, which keeps equal-x objects in the order they arrived.
        for slot in (1..=self.batch.len()).rev() {
            let Some(&earlier) = self.batch.get(slot - 1) else {
                continue;
            };
            let Some(earlier) = self.run(earlier) else {
                continue;
            };
            if this_pos.x >= (self.display * earlier.position).x {
                self.batch.insert(slot, index);
                return;
            }
        }
        self.batch.insert(0, index);
    }

    /// Drains the batch, emitting characters in x-sorted order
    /// (`ProcessTransformedTextObjects`).
    pub fn flush(&mut self, diags: &mut Diagnostics) {
        let batch = std::mem::take(&mut self.batch);
        for index in batch {
            let Some(run) = self.run(index) else { continue };
            // Re-checked, because the batch may hold an object whose box
            // changed meaning since it was offered. `[oracle-bug]`: the
            // advance rescues a spaces-only object, as at `offer`.
            if run.advance < SIZE_EPSILON && run.rect.width().abs() < SIZE_EPSILON {
                continue;
            }
            let state = self.pre_marked_content(run, diags);
            if state == MarkState::Done {
                self.previous = Some(Previous { index: run.index });
                continue;
            }
            if self.previous.is_some() {
                let generate = self.decide(run);
                if generate == Generate::LineBreak {
                    self.line_rect = run.rect;
                } else {
                    self.line_rect = union(self.line_rect, run.rect);
                }
                if !self.apply(generate, run, diags) {
                    // The hyphen-cancel path: this object emits nothing and
                    // the previous object stays what it was, because nothing
                    // has replaced it.
                    continue;
                }
            } else {
                self.line_rect = run.rect;
            }
            if state == MarkState::Delay {
                self.process_marked_content(run, diags);
                self.previous = Some(Previous { index: run.index });
                continue;
            }
            self.previous = Some(Previous { index: run.index });
            let start = self.line.len();
            if self.emit_items(run, diags) {
                self.line.reverse_from(start);
            }
        }
    }

    /// Closes the staging line into the final output.
    pub fn close_line(&mut self) {
        crate::line::close(&mut self.line, &mut self.out, self.rtl);
    }

    // -- marked content ----------------------------------------------------

    /// Whether an object's `/ActualText` replaces, suppresses or leaves alone
    /// its glyphs (`PreMarkedContent`).
    ///
    /// Reads the key **without resolving references and without coercing
    /// types**: an indirect `/ActualText` is invisible here, so the object's
    /// glyphs are emitted normally and the resolving pass that would have
    /// replaced them never runs. That asymmetry with
    /// [`process_marked_content`](Self::process_marked_content) is real,
    /// exercised, and preserved.
    fn pre_marked_content(&self, run: &TextRun, diags: &mut Diagnostics) -> MarkState {
        let marks = run.marks.marks();
        if marks.is_empty() {
            return MarkState::Pass;
        }
        // Both the text and the dictionary are overwritten by every mark that
        // carries a parameter list, so the *last* one wins — and the
        // dictionary wins whether or not that mark had an `/ActualText`.
        let mut actual_text = None;
        let mut last_dict = None;
        for mark in marks {
            let Some(dict) = mark.properties.as_ref() else {
                continue;
            };
            last_dict = Some(dict);
            if let Some(string) = dict.raw(&actual_text_key()).and_then(|o| o.as_string()) {
                actual_text = Some(string.as_text().into_owned());
            }
        }
        let Some(actual_text) = actual_text else {
            return MarkState::Pass;
        };
        if let Some(previous) = self
            .previous
            .as_ref()
            .and_then(|previous| self.find_run(previous.index))
        {
            let previous_marks = previous.marks.marks();
            if previous_marks.len() == marks.len()
                && let (Some(a), Some(b)) = (previous_marks.last(), last_dict)
            {
                // Identity, not equality: two marks carrying the same
                // dictionary *value* are different marks.
                let same = a
                    .properties
                    .as_ref()
                    .is_some_and(|previous| std::sync::Arc::ptr_eq(previous, b));
                if same {
                    return MarkState::Done;
                }
            }
        }
        if actual_text.is_empty() {
            return MarkState::Pass;
        }
        let printable = actual_text
            .chars()
            .map(u32::from)
            .any(|code| (0x80 < code && code < 0xFFFD) || (code <= 0x80 && is_print(code)));
        if printable {
            MarkState::Delay
        } else {
            diags.record(
                Severity::Recovered,
                DiagKind::TextActualTextUnprintable,
                None,
            );
            MarkState::Done
        }
    }

    /// Emits an `/ActualText` string in place of an object's glyphs
    /// (`ProcessMarkedContent`).
    ///
    /// The string is read **with** one level of reference resolution and any
    /// object type, unlike the pass that decided to call this. And it is
    /// overwritten unconditionally by every mark carrying a parameter list,
    /// so a later mark with a parameter list but no `/ActualText` resets it to
    /// nothing and the object emits nothing at all.
    fn process_marked_content(&mut self, run: &TextRun, diags: &mut Diagnostics) {
        let mut actual_text = String::new();
        for mark in run.marks.marks() {
            let Some(dict) = mark.properties.as_ref() else {
                continue;
            };
            actual_text = dict
                .text(&actual_text_key(), self.resolver)
                .unwrap_or_default();
        }
        let chars: Vec<char> = actual_text.chars().collect();
        if chars.is_empty() {
            return;
        }
        let is_rtl = Self::object_is_rtl(run);
        let matrix = run.text_matrix;
        let mut rect = run.rect;
        // The width is read *after* one edge has already moved, so `step` is
        // the per-character width rather than the object's. Order matters.
        #[expect(
            clippy::cast_precision_loss,
            reason = "a string long enough to lose precision here cannot be laid out anyway"
        )]
        let count = chars.len() as f64;
        let step = if is_rtl {
            rect.x0 = rect.x1 - rect.width() / count;
            -rect.width()
        } else {
            rect.x1 = rect.x0 + rect.width() / count;
            rect.width()
        };

        for (offset, ch) in chars.iter().enumerate() {
            let mut code = u32::from(*ch);
            if code <= 0x80 && !is_print(code) {
                code = 0x20;
            }
            if code >= 0xFFFD {
                // Skipped, but the box still steps past it.
                diags.record(
                    Severity::Recovered,
                    DiagKind::TextActualTextCharDropped,
                    None,
                );
                continue;
            }
            #[expect(
                clippy::cast_precision_loss,
                reason = "an index this large cannot be laid out"
            )]
            let shift = offset as f64 * step;
            let char_box = Rect::new(rect.x0 + shift, rect.y0, rect.x1 + shift, rect.y1);
            let info = CharBox {
                char_type: CharType::ActualText,
                unicode: code,
                code: None,
                // Every synthesized character shares the object's origin;
                // only the box steps.
                origin: run.position,
                char_box,
                loose_char_box: char_box,
                matrix,
                object: Some(run.index),
                font_size: run.font_size,
                angle: matrix_angle(matrix),
            };
            self.line.push(code, info);
        }
    }

    // -- per-object emission ----------------------------------------------

    /// Emits one object's characters into the staging line
    /// (`ProcessTextObjectItems`).
    ///
    /// Returns whether the caller must reverse what was just staged, which is
    /// true for a right-to-left object drawn under a mirroring matrix.
    fn emit_items(&mut self, run: &TextRun, diags: &mut Diagnostics) -> bool {
        let matrix = run.text_matrix;
        let base_space = base_space(run, matrix) + base_space_adjustment(run, matrix);
        let mut spacing = 0.0f64;
        // The two per-character recoveries are counted and recorded once for
        // the object: a CJK page with a broken `/ToUnicode` would otherwise
        // record tens of thousands of entries and hide every other
        // diagnostic behind the sink's bound (design brief Q4).
        let mut unmapped = 0u32;

        for index in 0..run.count() {
            let Some(item) = run.item(index) else {
                continue;
            };

            // The gap *before* this glyph comes from the previous
            // adjustment, and only when the text so far does not already end
            // in a space.
            if index > 0 && run.kerning(index - 1) != 0.0 {
                let last = self
                    .line
                    .last_unit()
                    .or_else(|| self.out.text.last().copied());
                if last.is_some_and(|unit| unit != u32::from(b' ')) {
                    spacing = f64::from(-run.font_size_h * run.kerning(index - 1) / 1000.0);
                }
            }
            spacing -= base_space;

            if spacing != 0.0 && index > 0 {
                let threshold = space_threshold(run, item.code);
                if threshold != 0.0 && spacing >= threshold {
                    let origin = matrix * item.origin;
                    self.line.push(
                        u32::from(b' '),
                        CharBox {
                            char_type: CharType::Generated,
                            unicode: u32::from(b' '),
                            code: None,
                            origin,
                            char_box: Rect::new(origin.x, origin.y, origin.x, origin.y),
                            loose_char_box: Rect::new(origin.x, origin.y, origin.x, origin.y),
                            // The bare form matrix, not the composed one —
                            // which for us is the identity.
                            matrix: Affine::IDENTITY,
                            // But the *object* is recorded, unlike the
                            // inter-object spaces, which is what lets the
                            // previous-object search walk back to here.
                            object: Some(run.index),
                            font_size: run.font_size,
                            angle: 0.0,
                        },
                    );
                }
            }
            spacing = 0.0;

            let mut unicode: Vec<u32> = run
                .font
                .unicode_from_charcode(item.code)
                .into_iter()
                .map(u32::from)
                .collect();
            let mut char_type = CharType::Normal;
            if unicode.is_empty() && item.code.0 != 0 {
                // No mapping: the character code passes through as if it were
                // a code point, which is where `--txt` gets values that are
                // not Unicode scalars at all.
                unicode.push(item.code.0);
                char_type = CharType::NotUnicode;
                unmapped = unmapped.saturating_add(1);
            }

            let info = Self::build_char(run, item, char_type, matrix);

            if unicode.is_empty() {
                // Reachable only for character code zero, whose record keeps
                // a unicode of zero — a NUL in `--txt` — while the staged
                // text gets the sentinel.
                diags.record(Severity::Recovered, DiagKind::TextCharcodeZero, None);
                self.line.push(0xFFFE, info);
                continue;
            }

            // `[oracle-bug]` No character-level duplicate suppression runs
            // here; see `is_duplicate`, which is kept as the documented
            // description of what the oracle does and is no longer consulted.

            for code in unicode {
                let mut piece = info;
                piece.unicode = code;
                self.line.push(if code == 0 { 0xFFFE } else { code }, piece);
            }
        }

        if unmapped > 0 {
            diags.record(
                Severity::Recovered,
                DiagKind::TextCharcodesUnmapped(unmapped),
                None,
            );
        }
        // `[oracle-bug]` A44: nothing is deduplicated any more, so
        // `DiagKind::TextCharsDeduplicated` has no site. The variant is kept
        // in `pdfrum-common` because it is a public enum member and a future
        // opt-in to the oracle's rule would need it back.

        // A right-to-left object drawn under a mirroring matrix has already
        // been laid out backwards; the caller reverses what was staged.
        let is_rtl = Self::object_is_rtl(run);
        let [a, b, c, d, ..] = matrix.as_coeffs();
        is_rtl && (a * d - b * c) < 0.0
    }

    /// Builds one character's record, including its two boxes.
    fn build_char(run: &TextRun, item: Item, char_type: CharType, matrix: Affine) -> CharBox {
        let bbox = run.glyph_bbox(item.code);
        let scale = f64::from(run.font_size) / 1000.0;
        let mut char_box = Rect::new(
            bbox.x0 * scale + item.origin.x,
            bbox.y0 * scale + item.origin.y,
            bbox.x1 * scale + item.origin.x,
            bbox.y1 * scale + item.origin.y,
        );
        // The height rescue adds `font_size / 1000`, a thousandth of the font
        // size rather than the font size — it reads as a bug and is ported.
        if (char_box.y1 - char_box.y0).abs() < SIZE_EPSILON {
            char_box.y1 = char_box.y0 + scale;
        }
        // The width rescue uses the already-scaled advance, so the two
        // rescues end up in the same units after all.
        if (char_box.x1 - char_box.x0).abs() < SIZE_EPSILON {
            char_box.x1 = char_box.x0 + f64::from(run.scaled_char_width(item.code));
        }
        let char_box = transform_rect(matrix, char_box);
        let origin = matrix * item.origin;
        let loose = loose_bounds(&LooseBoundsInput {
            char_box,
            origin,
            matrix,
            code: Some(item.code),
            font: Some(&run.font),
            font_size: run.font_size,
            scaled_width: run.scaled_char_width(item.code),
        });
        CharBox {
            char_type,
            unicode: 0,
            code: Some(item.code),
            origin,
            char_box,
            loose_char_box: loose,
            matrix,
            object: Some(run.index),
            font_size: run.font_size,
            angle: matrix_angle(matrix),
        }
    }

    /// Whether a character repeats one of the last seven staged characters at
    /// effectively the same place (§1.7a).
    ///
    /// The epsilon is seven hundredths of the font size pushed through the
    /// matrix's **x** unit vector — not the averaged distance the rest of the
    /// crate uses, and the difference is observable. Fonts are compared by
    /// identity, so this only fires when the page layer really did share one
    /// font between the two objects.
    ///
    /// `[oracle-bug]` **No longer consulted.** `cpdf_textpage.cpp:1437-1458`
    /// sets `add_unicode = false` (`:1455`) when an entry within a **7-entry
    /// lookback** (`:1438`) shares the candidate's char code and font and sits
    /// within `0.07 × fontsize` on both axes. `bug_1769.in` draws one form
    /// `XObject` at 1000x and again at 1x; in the small instance the word
    /// collapses below the threshold, `r` and `l` fall inside the window, and
    /// the page extracts as `"wo d wo d"` (`crbug.com/42270780`). The spec has
    /// no notion of duplicate-glyph suppression — §8.2 composites coincident
    /// glyphs, and drawing one twice is how faux-bold is done — and pdf.js has
    /// no dedup at all (the only "identical" test in `evaluator.js` is
    /// font-state caching, `:3246`). It duplicates where PDFium deletes;
    /// deletion is the unrecoverable direction. Kept as the executable
    /// description of the oracle's rule, and as what a future option would
    /// switch back on.
    #[expect(
        dead_code,
        reason = "[oracle-bug] retained as the oracle's documented rule; see the note above"
    )]
    fn is_duplicate(&self, run: &TextRun, candidate: &CharBox) -> bool {
        let threshold = transform_x_distance(candidate.matrix, f64::from(0.07 * run.font_size));
        let staged = self.line.chars();
        let start = staged.len().saturating_sub(7);
        for earlier in staged.get(start..).unwrap_or_default().iter().rev() {
            if earlier.code != candidate.code {
                continue;
            }
            let Some(index) = earlier.object else {
                continue;
            };
            let Some(other) = self.find_run(index) else {
                continue;
            };
            if !std::sync::Arc::ptr_eq(&other.font, &run.font) {
                continue;
            }
            let dx = earlier.origin.x - candidate.origin.x;
            let dy = earlier.origin.y - candidate.origin.y;
            if dx.abs() < threshold && dy.abs() < threshold {
                return true;
            }
        }
        false
    }

    fn find_run(&self, index: ObjectIndex) -> Option<&'a TextRun> {
        self.runs.iter().find(|run| run.index == index)
    }

    /// Whether an object's characters read right to left (`IsRightToLeft`).
    fn object_is_rtl(run: &TextRun) -> bool {
        let codes: Vec<u32> = (0..run.count())
            .filter_map(|index| run.item(index))
            .filter_map(|item| {
                let unicode = run.font.unicode_from_charcode(item.code);
                let code = unicode.first().map_or(item.code.0, |ch| u32::from(*ch));
                (code != 0).then_some(code)
            })
            .collect();
        crate::bidi::is_right_to_left(&codes)
    }

    // -- inter-object decisions --------------------------------------------

    /// The previous object, re-derived from the last *character* rather than
    /// from the last object processed (`FindPreviousTextObject`).
    ///
    /// A generated inter-object space carries no object, so it leaves the
    /// answer alone; a generated *inter-word* space does carry one, so it can
    /// pull the answer back to its own object.
    fn previous_run(&self) -> Option<&'a TextRun> {
        let last = self.line.last_char().or_else(|| self.out.chars.last());
        let from_char = last
            .and_then(|info| info.object)
            .and_then(|index| self.find_run(index));
        from_char.or_else(|| {
            self.previous
                .as_ref()
                .and_then(|previous| self.find_run(previous.index))
        })
    }

    /// Decides what goes between the previous object and this one
    /// (`ProcessInsertObject`).
    fn decide(&self, run: &TextRun) -> Generate {
        let Some(previous) = self.previous_run() else {
            return Generate::None;
        };
        let mut mode = object_flow(run, self.page_flow);
        if mode == Orientation::Unknown {
            mode = object_flow(previous, self.page_flow);
        }
        let count = previous.count();
        if count == 0 {
            return Generate::None;
        }
        let (Some(previous_item), Some(item)) = (previous.item(count - 1), run.item(0)) else {
            return Generate::None;
        };
        let this_rect = run.rect;
        let previous_rect = previous.rect;
        let current_char = run
            .font
            .unicode_from_charcode(item.code)
            .first()
            .map_or(item.code.0, |ch| u32::from(*ch));

        // Only an object with a decided orientation can end a line on
        // geometry alone; an `Unknown` one runs neither test.
        let ends_line = match mode {
            Orientation::Horizontal => ends_horizontal_line(this_rect, previous_rect),
            Orientation::Vertical => {
                ends_vertical_line(this_rect, self.line_rect, run.font_size, previous.font_size)
            }
            Orientation::Unknown => false,
        };
        if ends_line {
            return self.hyphen_or_break(current_char);
        }

        let last_pos = previous_item.origin.x;
        let last_glyph_width = ladder_char_width(previous, Some(previous_item.code));
        let last_width = f64::from(last_glyph_width * previous.font_size / 1000.0).abs();
        let this_glyph_width = ladder_char_width(run, Some(item.code));
        let this_width = f64::from(this_glyph_width * run.font_size / 1000.0).abs();
        let mut threshold = last_width.max(this_width) / 4.0;

        let previous_inverse = inverse_or_zero(previous.text_matrix);
        let pos = previous_inverse * run.position;
        if last_width < this_width {
            threshold = transform_distance(previous_inverse, threshold);
        }

        if mode == Orientation::Horizontal && self.is_newline(previous, run, pos, threshold) {
            return self.hyphen_or_break(current_char);
        }

        if run.count() == 1 && is_hyphen_code(current_char) && self.is_hyphen(current_char) {
            return Generate::Hyphen;
        }
        if current_char == u32::from(b' ') {
            return Generate::None;
        }
        let previous_char = previous
            .font
            .unicode_from_charcode(previous_item.code)
            .last()
            .map_or(0, |ch| u32::from(*ch));
        if previous_char == u32::from(b' ') {
            return Generate::None;
        }

        // The space threshold is computed in **glyph units** and only then
        // scaled, which is why it is bucketed by the four-hundreds rather
        // than by anything in page space.
        let mut threshold2 = f64::from(last_glyph_width.max(this_glyph_width));
        threshold2 = normalize_threshold(threshold2, 400.0, 700.0, 800.0);
        if last_glyph_width >= this_glyph_width {
            threshold2 *= f64::from(previous.font_size.abs());
        } else {
            threshold2 *= f64::from(run.font_size.abs());
            threshold2 = transform_distance(run.text_matrix, threshold2);
            threshold2 = transform_distance(previous_inverse, threshold2);
        }
        threshold2 /= 1000.0;
        // Two exact-value hacks for font-and-size combinations that shipped
        // in real files. The windows are float-equality tests spelled as
        // ranges; transcribed literally.
        if (threshold2 < 1.4881 && threshold2 > 1.4879)
            || (threshold2 < 1.39001 && threshold2 > 1.38999)
        {
            threshold2 *= 1.5;
        }
        if generates_space(pos, last_pos, this_width, last_width, threshold2) {
            Generate::Space
        } else {
            Generate::None
        }
    }

    /// Whether the geometry says this object starts a new line (§1.8's
    /// `is_newline` block).
    fn is_newline(&self, previous: &TextRun, run: &TextRun, pos: Point, threshold: f64) -> bool {
        let rect = previous.rect;
        // The height is read **before** the box is normalized, so a box with
        // its corners the wrong way round reports a negative height here and
        // a positive one to `IsEmpty` — which is what the whole first clause
        // turns on.
        let height = rect.y1 - rect.y0;
        let normalized = normalize_rect(rect);
        let empty = normalized.x1 <= normalized.x0 || normalized.y1 <= normalized.y0;
        let jumped = (pos.y > threshold * 2.0 || pos.y < threshold * -3.0)
            && (pos.y.abs() >= 1.0 || pos.y.abs() > pos.x.abs());
        if !((empty && height > 5.0) || jumped) {
            return false;
        }
        if previous.count() <= 1 {
            return true;
        }
        let (Some(first), Some(last)) = (previous.item(0), previous.item(previous.count() - 1))
        else {
            return true;
        };
        let [da, db, dc, dd, ..] = self.display.as_coeffs();
        let [_, mb, mc, ..] = previous.text_matrix.as_coeffs();
        // An unrotated, y-flipped page whose previous object runs left to
        // right: the two objects may be cells of one table row rather than
        // two lines, which a vertical overlap inside a wide x band confirms.
        if last.origin.x > first.origin.x
            && da > 0.9
            && db < 0.1
            && dc < 0.1
            && dd < -0.9
            && mb < 0.1
            && mc < 0.1
        {
            let band = Rect::new(0.0, previous.rect.y0, 1000.0, previous.rect.y1);
            if contains(band, run.position) {
                return false;
            }
            let other = Rect::new(0.0, run.rect.y0, 1000.0, run.rect.y1);
            if contains(other, previous.position) {
                return false;
            }
        }
        true
    }

    fn hyphen_or_break(&self, current_char: u32) -> Generate {
        if self.is_hyphen(current_char) {
            Generate::Hyphen
        } else {
            Generate::LineBreak
        }
    }

    /// Whether the text so far ends in something that should become a soft
    /// hyphen (`IsHyphen`).
    ///
    /// Looks back past trailing spaces — but stops one short of the
    /// beginning, so an all-spaces buffer never finds a hyphen. Consults the
    /// **final** text when the staging line is empty, which is what makes the
    /// hyphen path reachable with nothing staged.
    fn is_hyphen(&self, current_char: u32) -> bool {
        let staged = self.line.text();
        let text: &[u32] = if staged.is_empty() {
            &self.out.text
        } else {
            staged
        };
        if text.is_empty() {
            return false;
        }
        let mut at = text.len() - 1;
        while at > 0 && text.get(at) == Some(&0x20) {
            at -= 1;
        }
        let Some(&candidate) = text.get(at) else {
            return false;
        };
        if !is_hyphen_code(candidate) {
            return false;
        }
        if at > 0
            && let Some(&before) = text.get(at - 1)
            && is_alpha(before)
            && is_alnum(current_char)
        {
            return true;
        }
        // Otherwise the previous *character record* decides: a piece or an
        // `/ActualText` character that is itself a hyphen counts.
        let previous = self.line.last_char().or_else(|| self.out.chars.last());
        previous.is_some_and(|info| {
            matches!(info.char_type, CharType::Piece | CharType::ActualText)
                && is_hyphen_code(info.unicode)
        })
    }

    /// Emits the decision (`ProcessGenerateCharacter`).
    ///
    /// Returns whether the caller should go on to emit this object's glyphs.
    fn apply(&mut self, generate: Generate, run: &TextRun, diags: &mut Diagnostics) -> bool {
        match generate {
            Generate::None => true,
            Generate::Space => {
                self.append_generated(u32::from(b' '), true);
                true
            }
            Generate::LineBreak => {
                self.close_line();
                // Guarded on the **final text**, not the character list, and
                // the line just closed may have populated it — so a break at
                // the very start of a page with a non-empty first line does
                // emit.
                if !self.out.text.is_empty() {
                    self.append_generated(u32::from('\r'), false);
                    self.append_generated(u32::from('\n'), false);
                }
                true
            }
            Generate::Hyphen => self.apply_hyphen(run, diags),
        }
    }

    fn apply_hyphen(&mut self, run: &TextRun, diags: &mut Diagnostics) -> bool {
        if run.count() == 1
            && let Some(item) = run.item(0)
        {
            let code = run
                .font
                .unicode_from_charcode(item.code)
                .first()
                .map_or(item.code.0, |ch| u32::from(*ch));
            if is_hyphen_code(code) {
                // An object that is *only* a hyphen is cancelled: it emits
                // nothing, and the previous object stays what it was.
                return false;
            }
        }
        while self.line.last_unit() == Some(0x20) {
            self.line.pop();
        }
        // [oracle-bug] cpdf_textpage.cpp:1357 is
        // `CharInfo& charinfo = temp_char_list_.back();` with **no emptiness
        // guard**, immediately after the `while` at `:1352-1356` that pops
        // trailing spaces from `temp_char_list_` and `temp_text_buf_`
        // together. Reaching the arm requires `IsHyphen` to have said yes,
        // and `IsHyphen` consults `text_buf_` — the *finished* text — when
        // the staging buffer is empty, so a crafted file can arrive here with
        // `temp_char_list_` empty and take `back()` on it. That is a `CHECK`
        // failure in debug and undefined behaviour in release. No reading of
        // §9.10 asks a text extractor to crash, and STYLE.md §3 forbids the
        // equivalent outright. pdf.js keeps no sentinel and no staging list of
        // this shape — its soft hyphen is normalised to `-`
        // (`unicode.js:57-58`) and rejoined at query time
        // (`pdf_find_controller.js:290-307`) — so there is nothing there to
        // dereference. We record a diagnostic and emit no hyphen. This is the
        // audit's A50, previously recorded as design brief D4 and Q5 — a
        // divergence flagged in case a release-mode oracle produced something
        // rather than crashing; the oracle-bug rule settles it without
        // needing that answer, since a crash writes no golden either way.
        let Some(last) = self.line.last_char_mut() else {
            // The C++ dereferences an empty list here, which is a crash on a
            // crafted file. We decline it (design brief D4): a crashing
            // oracle writes no golden, so there is nothing to match.
            diags.record(Severity::Suspicious, DiagKind::TextHyphenNoPrevChar, None);
            return true;
        };
        last.char_type = CharType::Hyphen;
        last.unicode = 0x2;
        // The record says 0x2 and the text says U+FFFE. Both are read.
        self.line.set_last_unit(0xFFFE);
        true
    }

    /// Appends a generated character, positioned just past the previous one
    /// (`AppendGeneratedCharacter` / `GenerateCharInfo`).
    ///
    /// A no-op when there is no previous character at all, which is why the
    /// very first thing on a page can never be a generated space or a line
    /// break.
    fn append_generated(&mut self, unicode: u32, staged: bool) {
        let Some(previous) = self.line.last_char().or_else(|| self.out.chars.last()) else {
            return;
        };
        let previous = *previous;
        let run = previous.object.and_then(|index| self.find_run(index));
        let width = match (run, previous.code) {
            (Some(run), Some(code)) => ladder_char_width(run, Some(code)),
            _ => 0.0,
        };
        let mut font_size = run.map_or_else(
            || {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "a page-space height narrowed to the f32 the C++ uses"
                )]
                let height = previous.char_box.height() as f32;
                height
            },
            |run| run.font_size,
        );
        // Exact float equality, and a negative size is deliberately not
        // rescued.
        if font_size == 0.0 {
            font_size = DEFAULT_FONT_SIZE;
        }
        let origin = Point::new(
            previous.origin.x + f64::from(width * font_size / 1000.0),
            previous.origin.y,
        );
        let info = CharBox {
            char_type: CharType::Generated,
            unicode,
            code: None,
            origin,
            char_box: Rect::new(origin.x, origin.y, origin.x, origin.y),
            loose_char_box: Rect::new(origin.x, origin.y, origin.x, origin.y),
            matrix: Affine::IDENTITY,
            // No object: this character belongs to the gap, not to either
            // side of it.
            object: None,
            font_size: DEFAULT_FONT_SIZE,
            angle: 0.0,
        };
        if staged {
            self.line.push(unicode, info);
        } else {
            self.out.text.push(unicode);
            self.out.chars.push(info);
        }
    }
}

// -- free helpers ---------------------------------------------------------

/// Whether a code point is one of the two hyphens the extractor recognizes:
/// ASCII hyphen-minus and SOFT HYPHEN.
///
/// **Not** U+2010 HYPHEN, which is why a document using the typographically
/// correct character gets no soft-hyphen handling at all.
#[must_use]
pub fn is_hyphen_code(code: u32) -> bool {
    code == 0x2D || code == 0xAD
}

/// The shared threshold bucketer (`NormalizeThreshold`).
///
/// Called with two different bucket sets — `(300, 500, 700)` for a space
/// glyph's width and `(400, 700, 800)` for an inter-object gap — and divides
/// by two, four, five or six accordingly.
#[must_use]
pub fn normalize_threshold(threshold: f64, t1: f64, t2: f64, t3: f64) -> f64 {
    if threshold < t1 {
        threshold / 2.0
    } else if threshold < t2 {
        threshold / 4.0
    } else if threshold < t3 {
        threshold / 5.0
    } else {
        threshold / 6.0
    }
}

/// The base spacing an object's character spacing implies
/// (`CalculateBaseSpace`).
///
/// Zero for an object with no character spacing or fewer than two glyphs, and
/// zero again for the two special cases at the end — a negative result, or
/// exactly two glyphs with any adjustment between them.
#[must_use]
pub fn base_space(run: &TextRun, matrix: Affine) -> f64 {
    let count = run.count();
    if run.char_space == 0.0 || count < 2 {
        return 0.0;
    }
    let spacing = transform_distance(matrix, f64::from(run.char_space));
    let mut base = spacing;
    let mut has_kerning = false;
    for kerning in &run.kernings {
        if *kerning != 0.0 {
            let adjusted = f64::from(-run.font_size_h * kerning / 1000.0);
            base = base.min(adjusted + spacing);
            has_kerning = true;
        }
    }
    if base < 0.0 || (count == 2 && has_kerning) {
        return 0.0;
    }
    base
}

/// The correction applied on top of [`base_space`]
/// (`CalculateBaseSpaceAdjustment`).
///
/// Sign-symmetric: a positive character spacing produces a negative
/// adjustment and a negative one a positive adjustment, both of the same
/// magnitude. Combined with [`base_space`] the two mostly cancel — but only
/// mostly, and only when nothing was adjusted.
#[must_use]
pub fn base_space_adjustment(run: &TextRun, matrix: Affine) -> f64 {
    let char_space = run.char_space;
    if char_space > 0.001 {
        return -transform_distance(matrix, f64::from(char_space));
    }
    if char_space < -0.001 {
        return transform_distance(matrix, f64::from(char_space.abs()));
    }
    0.0
}

/// The gap width below which an inter-glyph space is not generated
/// (`CalculateSpaceThreshold`).
///
/// Prefers the font's own space glyph — unless that glyph is wider than a
/// third of the font size, in which case it is not to be trusted and the
/// *current* character's width, bucketed, stands in for it.
#[must_use]
pub fn space_threshold(run: &TextRun, code: CharCode) -> f64 {
    let font_size_h = f64::from(run.font_size_h);
    let mut threshold = 0.0;
    if let Some(space) = run.font.char_code_from_unicode(' ') {
        threshold = font_size_h * f64::from(run.font.char_width(space)) / 1000.0;
    }
    if threshold > font_size_h / 3.0 {
        threshold = 0.0;
    } else {
        threshold /= 2.0;
    }
    if threshold == 0.0 {
        threshold = f64::from(ladder_char_width(run, Some(code)));
        threshold = normalize_threshold(threshold, 300.0, 500.0, 700.0);
        threshold = font_size_h * threshold / 1000.0;
    }
    threshold
}

/// Whether a gap is wide enough to be a space (`GenerateSpace`).
///
/// Three clauses, the middle of which is nearly dead: it can only fire when
/// `threshold + last_width` comes out negative, which needs a negative
/// threshold, which the width ladder cannot produce. Ported anyway, because
/// that reasoning rests on invariants of other crates.
#[must_use]
pub fn generates_space(
    pos: Point,
    last_pos: f64,
    this_width: f64,
    last_width: f64,
    threshold: f64,
) -> bool {
    if (last_pos + last_width - pos.x).abs() <= threshold {
        return false;
    }
    let threshold_pos = threshold + last_width;
    let difference = pos.x - last_pos;
    if difference.abs() > threshold_pos {
        return true;
    }
    if pos.x < 0.0 && -threshold_pos > difference {
        return true;
    }
    difference > this_width + last_width
}

/// Whether two objects fail to overlap vertically, i.e. sit on different
/// lines (`EndHorizontalLine`).
///
/// Short objects never end a line: anything under four and a half units tall
/// is treated as decoration rather than as a line of text.
#[must_use]
pub fn ends_horizontal_line(this_rect: Rect, previous_rect: Rect) -> bool {
    if this_rect.height() <= 4.5 || previous_rect.height() <= 4.5 {
        return false;
    }
    let top = this_rect.y1.min(previous_rect.y1);
    let bottom = this_rect.y0.max(previous_rect.y0);
    bottom >= top
}

/// Whether an object fails to overlap the current line horizontally
/// (`EndVerticalLine`).
///
/// Note the asymmetry with the horizontal test: this one compares against the
/// **accumulated line box**, not against the previous object.
#[must_use]
pub fn ends_vertical_line(
    this_rect: Rect,
    line_rect: Rect,
    font_size: f32,
    previous_font_size: f32,
) -> bool {
    if this_rect.width() <= f64::from(font_size) * 0.1
        || line_rect.width() <= f64::from(previous_font_size) * 0.1
    {
        return false;
    }
    let left = this_rect.x0.max(line_rect.x0);
    let right = this_rect.x1.min(line_rect.x1);
    right <= left
}

fn union(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.x0.min(b.x0),
        a.y0.min(b.y0),
        a.x1.max(b.x1),
        a.y1.max(b.y1),
    )
}

fn normalize_rect(rect: Rect) -> Rect {
    Rect::new(
        rect.x0.min(rect.x1),
        rect.y0.min(rect.y1),
        rect.x0.max(rect.x1),
        rect.y0.max(rect.y1),
    )
}

fn contains(rect: Rect, point: Point) -> bool {
    point.x >= rect.x0 && point.x <= rect.x1 && point.y >= rect.y0 && point.y <= rect.y1
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::unreadable_literal,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::*;

    #[test]
    fn the_threshold_bucketer_divides_by_two_four_five_and_six() {
        // Just below each edge takes the smaller divisor; at the edge, the
        // larger. The comparison is strict.
        assert_eq!(normalize_threshold(299.0, 300.0, 500.0, 700.0), 149.5);
        assert_eq!(normalize_threshold(300.0, 300.0, 500.0, 700.0), 75.0);
        assert_eq!(normalize_threshold(499.0, 300.0, 500.0, 700.0), 124.75);
        assert_eq!(normalize_threshold(500.0, 300.0, 500.0, 700.0), 100.0);
        assert_eq!(normalize_threshold(699.0, 300.0, 500.0, 700.0), 139.8);
        assert_eq!(normalize_threshold(700.0, 300.0, 500.0, 700.0), 700.0 / 6.0);
        // The other bucket set.
        assert_eq!(normalize_threshold(399.0, 400.0, 700.0, 800.0), 199.5);
        assert_eq!(normalize_threshold(400.0, 400.0, 700.0, 800.0), 100.0);
        assert_eq!(normalize_threshold(700.0, 400.0, 700.0, 800.0), 140.0);
        assert_eq!(normalize_threshold(800.0, 400.0, 700.0, 800.0), 800.0 / 6.0);
        // Zero and negatives fall in the first bucket.
        assert_eq!(normalize_threshold(0.0, 300.0, 500.0, 700.0), 0.0);
        assert_eq!(normalize_threshold(-10.0, 300.0, 500.0, 700.0), -5.0);
    }

    #[test]
    fn hyphen_codes_are_the_two_the_cpp_lists() {
        assert!(is_hyphen_code(0x2D));
        assert!(is_hyphen_code(0xAD));
        // U+2010 HYPHEN is deliberately not one, which is why a hard hyphen
        // survives into the output.
        assert!(!is_hyphen_code(0x2010));
        assert!(!is_hyphen_code(0x2D + 1));
    }

    #[test]
    fn a_short_object_never_ends_a_horizontal_line() {
        // Four and a half units is the floor, and the test is `<=`.
        let short = Rect::new(0.0, 0.0, 10.0, 4.5);
        let tall = Rect::new(0.0, 20.0, 10.0, 30.0);
        assert!(!ends_horizontal_line(short, tall));
        assert!(!ends_horizontal_line(tall, short));
        // Just over the floor, with no vertical overlap, does end the line.
        let just_tall = Rect::new(0.0, 0.0, 10.0, 4.6);
        assert!(ends_horizontal_line(just_tall, tall));
        // Overlapping boxes do not.
        let overlapping = Rect::new(0.0, 25.0, 10.0, 35.0);
        assert!(!ends_horizontal_line(overlapping, tall));
    }

    #[test]
    fn a_narrow_object_never_ends_a_vertical_line() {
        // A tenth of the font size is the floor.
        let narrow = Rect::new(0.0, 0.0, 1.0, 100.0);
        let line = Rect::new(50.0, 0.0, 60.0, 100.0);
        assert!(!ends_vertical_line(narrow, line, 10.0, 10.0));
        // Wide enough, and disjoint in x, ends the line.
        let wide = Rect::new(0.0, 0.0, 20.0, 100.0);
        assert!(ends_vertical_line(wide, line, 10.0, 10.0));
        // Overlapping in x does not.
        let overlapping = Rect::new(55.0, 0.0, 80.0, 100.0);
        assert!(!ends_vertical_line(overlapping, line, 10.0, 10.0));
    }

    #[test]
    fn space_generation_has_three_independent_clauses() {
        let at = |x: f64| Point::new(x, 0.0);
        // Clause zero: a gap inside the threshold is never a space.
        assert!(!generates_space(at(10.5), 0.0, 5.0, 10.0, 1.0));
        // Clause one: a difference past `threshold + last_width`.
        assert!(generates_space(at(20.0), 0.0, 5.0, 5.0, 1.0));
        // The opening clause guards everything after it: a gap that lands
        // within the threshold of where the previous glyph ended is never a
        // space, however large the absolute difference is.
        assert!(!generates_space(at(11.0), 0.0, 2.0, 3.0, 20.0));
        // Clause three: a difference past both widths, once the opening
        // guard has been cleared.
        assert!(generates_space(at(11.0), 0.0, 2.0, 3.0, 1.0));
        // Clause two, which needs a negative x and a difference below the
        // negated bound. No fixture upstream exercises it.
        assert!(generates_space(at(-30.0), 0.0, 5.0, 5.0, 1.0));
    }

    #[test]
    fn the_two_magic_float_bands_are_exclusive_ranges() {
        let in_band = |t: f64| (t < 1.4881 && t > 1.4879) || (t < 1.39001 && t > 1.38999);
        assert!(in_band(1.4880));
        assert!(in_band(1.3900));
        // The bounds themselves are outside.
        assert!(!in_band(1.4879));
        assert!(!in_band(1.4881));
        assert!(!in_band(1.38999));
        assert!(!in_band(1.39001));
        // And so is anything between the two bands.
        assert!(!in_band(1.44));
    }
}
