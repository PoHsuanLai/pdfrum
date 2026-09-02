//! Appearance-stream generation (`cpdf_generateap`): turning an annotation's
//! dictionary into the content stream a viewer draws.
//!
//! # The overlay
//!
//! Upstream this **mutates the document**. Generating a sticky note's
//! appearance replaces its `/Rect` with a 20×20 box; generating an ink
//! annotation's inflates its `/Rect`; every annotation touched gains an
//! `/AP /N` pointing at a new stream and a marker key saying so. Everything
//! that reads the file afterwards — including the `--annot` dump the
//! conformance harness diffs byte for byte — sees the mutated state, which is
//! why the dump reports 20×20 rectangles for sticky notes whose files say
//! otherwise.
//!
//! Parsed objects here are values and the parser's store is immutable, so
//! [`generate_appearances`] returns an [`AnnotOverlay`] instead: one entry per
//! `/Annots` index recording the stream it produced and the dictionary edits
//! it implies. Readers consult the overlay before the dictionary.
//!
//! # Which annotations get one
//!
//! Ten subtypes have a generator, and a widget annotation with no `/AP`
//! dictionary gets its chrome from [`widget`] besides — plus, when the caller
//! has fonts to set text with, the field body [`field_body`] lays out.
//! Generation is refused
//! outright when the
//! annotation is hidden, or when `/AP /N` already reads as a dictionary —
//! and a **stream** answers as its own dictionary, so the ordinary "it
//! already has an appearance" case is covered by the same test. Only a
//! missing `/AP`, a missing `/N`, or a scalar `/N` leaves the door open.

mod border;
mod da;
pub(crate) mod emit;
pub mod field_body;
pub(crate) mod fmt;
pub mod font_map;
pub mod freetext;
mod markup;
pub(crate) mod popup;
mod shapes;
pub mod widget;

use kurbo::{Affine, Rect};
use pdfrum_common::{DiagKind, Diagnostics, Severity};
use pdfrum_object::{Array, Dict, Name, Object, Resolve, names as obj_names};

use crate::annot::{Subtype, appearance, quad};
use crate::names;
use crate::vt;

/// One generated appearance and the dictionary edits it implies.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedAp {
    /// The content-stream bytes.
    pub stream: Vec<u8>,
    /// The form `XObject`'s bounding box.
    pub bbox: Rect,
    /// Its matrix, which these generators always leave as the identity.
    pub matrix: Affine,
    /// Its resource dictionary.
    pub resources: Dict,
    /// A rewritten `/Rect`, when generation moved one.
    pub rect_override: Option<Rect>,
    /// A copied-down `/AS`, for the `/NeedAppearances` widget path.
    pub as_override: Option<Name>,
}

/// What an overlay says about one annotation.
///
/// Three states, not two, and the third is why this is an enum rather than an
/// `Option`. "Nothing was generated" and "this annotation draws nothing" are
/// different instructions: the first falls through to whatever `/AP` the file
/// carries, the second **suppresses** it. A field whose appearance has been
/// cleared — focus left it and it went back to drawing nothing — needs the
/// second, and expressing it as the absence of an entry would make it
/// indistinguishable from the first.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Appearance {
    /// Nothing to say. The file's own `/AP` is used, if it has one.
    #[default]
    Untouched,
    /// Draw this instead of the file's `/AP`.
    Generated(GeneratedAp),
    /// Draw nothing at all, even if the file carries an `/AP`.
    ///
    /// Nothing sets this yet. It exists so a cleared appearance has a
    /// spelling that is not "absent", which is what keeps the merge below
    /// able to express one later without changing shape.
    Suppressed,
}

/// The shape a focused widget's focus rectangle takes.
///
/// A widget being edited has a live control behind it, and what that control
/// answers when asked for a focus rectangle depends on which control it is —
/// three answers, not one, and two of the three are *no rectangle at all*:
///
/// - A **text field** and a **combo box** — editable or not — answer an empty
///   rectangle outright, so nothing is stroked over them. This is the common
///   case and it is why the focused text-field goldens carry a caret and
///   glyphs but no outline. `CPWL_ComboBox::GetFocusRect`
///   (`fpdfsdk/pwl/cpwl_combo_box.cpp:321-323`) returns an empty rectangle
///   with **no editability test in it**; a caller that inflates a read-only
///   combo strokes a box upstream never draws.
/// - A **check box**, a **radio button** and a **single-select list box**
///   answer their window rectangle inflated by one unit on every side, which
///   is [`FocusBox::Inflated`] — `CPWL_Wnd::GetFocusRect`
///   (`cpwl_wnd.cpp:713-719`), which the list box falls through to when it is
///   not multi-select.
/// - A **multi-select list box** answers the rectangle of the item its caret
///   sits on, clipped to the client area — a rectangle only the list control's
///   own scroll and caret state can name, so a caller that has it supplies it
///   as [`FocusBox::Rect`].
///
/// `annot_render`'s own table says the same thing; the two are kept in step
/// deliberately, because this is the one a `pdfrum-form` caller reads.
///
/// [`FocusBox::None`] is the empty answer and the default: a focused entry
/// that names it is still *focused* — it draws no tint — and simply strokes
/// nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum FocusBox {
    /// No rectangle: nothing is stroked. A text field and an editable combo
    /// box always answer this.
    #[default]
    None,
    /// The annotation's own rectangle, inflated by one unit on every side.
    Inflated,
    /// An explicit rectangle in page space, already in its final position.
    Rect(Rect),
}

/// Which annotation on the page holds the keyboard focus, and what its focus
/// rectangle is.
///
/// Both halves are needed and they are independent. The *index* alone decides
/// the tint: a widget with a live control is never tinted, focused or not, and
/// the focused one is the only widget a live control reaches in a
/// single-focus session. The *box* decides whether anything is stroked in its
/// place, which most field types answer with nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Focus {
    /// The raw `/Annots` index of the focused annotation — the same key space
    /// [`AnnotOverlay::set`] uses.
    pub annot: usize,
    /// The rectangle to stroke, in page space.
    pub box_: FocusBox,
}

impl Focus {
    /// Focus on one annotation with no rectangle to stroke — the answer a
    /// text field and an editable combo box give.
    #[must_use]
    pub fn at(annot: usize) -> Focus {
        Focus {
            annot,
            box_: FocusBox::None,
        }
    }
}

/// Per-annotation generated appearances, keyed by `/Annots` index.
///
/// Besides the per-annotation entries the overlay carries at most one
/// [`Focus`], because a session focuses one field at a time. It travels here
/// rather than as another parameter on the annotation pass for two reasons:
/// it is set by the same session that sets the appearances, from the same
/// index space, and adding it here left every existing caller compiling
/// unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnnotOverlay {
    entries: Vec<Appearance>,
    focus: Option<Focus>,
    hover: Option<usize>,
    live_edit: Option<usize>,
}

impl AnnotOverlay {
    /// An overlay with room for `count` annotations and nothing generated.
    #[must_use]
    pub fn with_capacity(count: usize) -> AnnotOverlay {
        AnnotOverlay {
            entries: vec![Appearance::Untouched; count],
            focus: None,
            hover: None,
            live_edit: None,
        }
    }

    /// Records which annotation holds the focus, and what to stroke over it.
    ///
    /// The index is a raw `/Annots` index. It is **not** bounded by the
    /// overlay's length: an overlay sized for the appearances it carries can
    /// still name a focused annotation past its end, and the annotation pass
    /// keys on the index rather than on an entry.
    pub fn set_focus(&mut self, focus: Focus) {
        self.focus = Some(focus);
    }

    /// Which annotation holds the focus, if any.
    #[must_use]
    pub fn focus(&self) -> Option<Focus> {
        self.focus
    }

    /// Records which annotation the pointer is inside.
    ///
    /// A raw `/Annots` index, like [`Self::set_focus`]'s, and equally
    /// unbounded by the overlay's length. Hover is a separate fact from focus
    /// and the two move independently: a pointer resting on an annotation
    /// leaves the keyboard focus wherever it was, and the annotation under the
    /// pointer need not be focusable at all — a highlight is the case that
    /// matters, since it is *only* reachable this way.
    ///
    /// What it decides is whether that annotation's synthesized pop-up note is
    /// **open**. A note card is drawn only while the pointer is inside its
    /// parent, and nothing a file can say opens one, so this is the whole of
    /// the signal.
    pub fn set_hover(&mut self, annot: usize) {
        self.hover = Some(annot);
    }

    /// Which annotation the pointer is inside, if any.
    #[must_use]
    pub fn hover(&self) -> Option<usize> {
        self.hover
    }

    /// Records that one annotation's supplied appearance is a **live edit's**
    /// — the field the session is currently typing in.
    ///
    /// A raw `/Annots` index, like [`Self::set_focus`]'s and equally unbounded
    /// by the overlay's length. At most one annotation can be under live edit,
    /// because a session focuses one field at a time; a second call replaces
    /// the first rather than accumulating.
    ///
    /// It is a separate signal from focus, and the two are **not**
    /// interchangeable. A field can hold the focus without being edited — it
    /// was tabbed to and nothing has been typed — in which case the session
    /// generates no appearance for it and there is nothing to mark. What this
    /// records is that the appearance carried at this index came from an
    /// editor, which is what makes the oracle draw its text with `ClearType`.
    pub fn set_live_edit(&mut self, annot: usize) {
        self.live_edit = Some(annot);
    }

    /// Which annotation's appearance is a live edit's, if any.
    #[must_use]
    pub fn live_edit(&self) -> Option<usize> {
        self.live_edit
    }

    /// Whether the appearance at one `/Annots` index came from a live edit.
    #[must_use]
    pub fn is_live_edit(&self, index: usize) -> bool {
        self.live_edit == Some(index)
    }

    /// Records a generated appearance at one `/Annots` index.
    pub fn set(&mut self, index: usize, generated: GeneratedAp) {
        self.set_appearance(index, Appearance::Generated(generated));
    }

    /// Records any of the three states at one `/Annots` index.
    pub fn set_appearance(&mut self, index: usize, appearance: Appearance) {
        if let Some(slot) = self.entries.get_mut(index) {
            *slot = appearance;
        }
    }

    /// What was generated at one `/Annots` index, if anything.
    ///
    /// A suppressed entry answers [`None`], the same as an untouched one —
    /// callers that only want a stream to draw need not distinguish them.
    /// [`AnnotOverlay::appearance`] is what tells them apart.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&GeneratedAp> {
        match self.appearance(index) {
            Appearance::Generated(generated) => Some(generated),
            Appearance::Untouched | Appearance::Suppressed => None,
        }
    }

    /// The full state at one `/Annots` index, suppression included.
    ///
    /// An index past the overlay's end reads as [`Appearance::Untouched`],
    /// which is what makes a short overlay safe to consult for any index.
    #[must_use]
    pub fn appearance(&self, index: usize) -> &Appearance {
        self.entries.get(index).unwrap_or(&Appearance::Untouched)
    }

    /// Lays `other`'s entries over this one's.
    ///
    /// Every entry `other` has anything to say about — generated **or**
    /// suppressed — replaces this overlay's, and its [`Appearance::Untouched`]
    /// entries leave this one's alone. So a caller-supplied overlay wins
    /// wherever it speaks and defers everywhere else, which is the merge a
    /// live edit needs: the session has an opinion about the one field being
    /// edited and none about the rest of the page.
    ///
    /// Indices are raw `/Annots` indices in both overlays. An entry of
    /// `other` past this overlay's end is dropped, because there is no
    /// annotation for it to apply to.
    ///
    /// `other`'s [`Focus`] and its hover each replace this overlay's when it
    /// has one, and leave it alone when it does not — the same "wins wherever
    /// it speaks" rule the entries follow. Unlike an entry, either one past
    /// this overlay's end survives: both name an annotation, not a slot.
    pub fn merge_over(&mut self, other: &AnnotOverlay) {
        for (index, entry) in other.entries.iter().enumerate() {
            if matches!(entry, Appearance::Untouched) {
                continue;
            }
            self.set_appearance(index, entry.clone());
        }
        if let Some(focus) = other.focus {
            self.focus = Some(focus);
        }
        if let Some(hover) = other.hover {
            self.hover = Some(hover);
        }
        if let Some(live_edit) = other.live_edit {
            self.live_edit = Some(live_edit);
        }
    }

    /// The rectangle an annotation should be read as having.
    #[must_use]
    pub fn rect(&self, index: usize, raw: Rect) -> Rect {
        self.get(index)
            .and_then(|generated| generated.rect_override)
            .unwrap_or(raw)
    }

    /// How many annotations the overlay covers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the overlay covers no annotations at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The font a text-bearing generator sets its text with.
///
/// Threaded in rather than loaded here, because loading one needs a font
/// cache the caller already owns, and because the layout engine is a pure
/// function of these numbers — which is what lets it be tested against a stub.
pub struct TextFont<'a> {
    /// The loaded font.
    pub font: &'a pdfrum_font::Font,
    /// Metrics derived from it, for the layout engine.
    pub metrics: vt::Metrics<'a>,
}

impl std::fmt::Debug for TextFont<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextFont")
            .field("metrics", &self.metrics)
            .finish_non_exhaustive()
    }
}

/// The character code a code point the face cannot map is written as.
///
/// A simple font's codes are one byte and `Font::append_char` truncates to
/// one, so the code the stream ends up carrying is the code point's **low
/// byte** — and the width has to be looked up under that same byte or the
/// layout advances by a glyph the stream does not name. A composite font
/// keeps the whole value, because its CMap decides the width itself.
fn unmapped_code(font: &pdfrum_font::Font, code: u32) -> pdfrum_font::CharCode {
    match font {
        pdfrum_font::Font::Type0(_) => pdfrum_font::CharCode(code),
        pdfrum_font::Font::Simple(_) | pdfrum_font::Font::Type3(_) => {
            pdfrum_font::CharCode(code & 0xff)
        }
    }
}

impl TextFont<'_> {
    /// How one code point is written into a content stream.
    ///
    /// A `Symbol` or `ZapfDingbats` font takes the code point's **low byte**
    /// verbatim, relying on the font's built-in encoding: there is no
    /// named-glyph table and no `/Encoding` consultation anywhere in this
    /// path. Anything else goes through the reverse `ToUnicode` mapping.
    ///
    /// **A code point the font cannot represent is still written**, as its own
    /// value taken for a character code. The glyph that draws is whatever that
    /// code happens to name in the chosen face and is usually wrong — but the
    /// text object exists, occupies the layout, and is what a reader sees.
    /// Dropping the character instead loses the object entirely, which on
    /// `bug_725389` — three Hebrew characters in a `/DA` naming Times-Roman —
    /// is the difference between six text objects and three.
    ///
    /// Upstream reaches the same place by a longer road: `CPDF_BAFontMap`
    /// would first look for a second face that knows the character, and only
    /// `CPWL_EditImpl::GetPDFWordString`'s fallthrough appends the raw value
    /// when none does. On a hermetic font set no second face is found, so the
    /// fallthrough is the whole of the observable behaviour, and the N-slot map
    /// is not built here for a result it does not change.
    #[must_use]
    pub fn encode(&self, code: u32) -> Vec<u8> {
        let name = self.font.base_font_name();
        if name == b"Symbol" || name == b"ZapfDingbats" {
            #[allow(clippy::cast_possible_truncation)]
            return vec![code as u8];
        }
        let mut out = Vec::new();
        let mapped = char::from_u32(code)
            .and_then(|ch| self.font.char_code_from_unicode(ch))
            .unwrap_or_else(|| unmapped_code(self.font, code));
        self.font.append_char(&mut out, mapped);
        out
    }

    /// One code point's width, in thousandths of an em.
    ///
    /// The width is the one the face gives whatever [`Self::encode`] wrote, so
    /// an unrepresentable code point measures the glyph its raw value names
    /// rather than nothing — the two have to agree or the layout advances past
    /// characters the stream still contains, and the line comes out the wrong
    /// length.
    ///
    /// A free function rather than a method because [`Self::metrics_of`] wants
    /// it as a `&dyn Fn` borrowed for the same lifetime as the font, which a
    /// closure over `self` cannot supply before `self` exists.
    #[must_use]
    pub fn char_width(font: &pdfrum_font::Font, code: u32) -> i32 {
        let charcode = char::from_u32(code)
            .and_then(|ch| font.char_code_from_unicode(ch))
            .unwrap_or_else(|| unmapped_code(font, code));
        #[allow(clippy::cast_possible_truncation)]
        {
            font.char_width(charcode) as i32
        }
    }

    /// The layout metrics a loaded font supplies.
    #[must_use]
    pub fn metrics_of<'a>(
        font: &'a pdfrum_font::Font,
        width: &'a dyn Fn(u32) -> i32,
    ) -> vt::Metrics<'a> {
        vt::Metrics {
            width,
            ascent: font.type_ascent(),
            descent: font.type_descent(),
        }
    }
}

/// One second face: the resource name it is filed under, the dictionary the
/// appearance's `/Resources /Font` carries, and the loaded face itself.
///
/// A borrow of what [`FormFonts`] already holds. The three travel together
/// because a generator needs all three to write one character — the alias for
/// the `Tf`, the face for the width, and the dictionary so the name resolves
/// when the stream is drawn.
#[derive(Debug, Clone, Copy)]
pub struct Substitute<'a> {
    /// The `Tf` name, and the key in the appearance's font resources.
    pub alias: &'a Name,
    /// The font dictionary that key maps to.
    pub dict: &'a Dict,
    /// The loaded face, for widths and for the codes it can write.
    pub font: &'a pdfrum_font::Font,
}

/// The faces a form's default resources name, loaded once for a page.
///
/// # Why the fonts are loaded rather than substituted for
///
/// A generator wants *metrics*, and it was tempting to hand every generator
/// one stock Helvetica on the reasoning that a non-embedded `/DA` font
/// substitutes to that face anyway. The metrics do not agree with that
/// reasoning, and the disagreement is visible: an ascent and descent taken
/// from the base-14 metric tables are 718 and −219, while the ones taken from
/// the **substituted face** — the size the layout engine actually stacks lines
/// by — are the face's own, and for the hermetic corpus's metric-compatible
/// Helvetica that is 905 and −211. On a list box the difference is the row
/// pitch: 11.24 units per row against 13.39, which is two extra rows in a
/// thirty-unit box.
///
/// So the font a widget's `/DA` names is loaded from the form's `/DR /Font`,
/// through the same loader and the same substitution options every other font
/// on the page goes through. A name the resources do not carry gets a stock
/// Helvetica, which is what the fallback is actually for.
pub struct FormFonts {
    /// Resource name and the face loaded under it, in `/DR /Font` order with
    /// the fallback last.
    entries: Vec<(Name, pdfrum_font::Font)>,
    /// The faces added for characters no declared font's charset covers, one
    /// per charset, keyed by the alias they are filed under.
    ///
    /// These are not in `/DR`, and no `/DA` names one: they are added when a
    /// field is asked to write a character its own font cannot, and they go
    /// into the **appearance stream's** own `/Resources /Font` rather than the
    /// form's. See [`font_map`].
    substitutes: Vec<(Name, Dict, pdfrum_font::Font)>,
}

impl std::fmt::Debug for FormFonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormFonts")
            .field(
                "names",
                &self.entries.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl FormFonts {
    /// Loads every font a document's interactive form declares, plus the
    /// fallback a name outside them resolves to.
    ///
    /// The fallback is loaded unconditionally and stored under an empty name,
    /// so a `/DA` naming nothing — or naming a font the resources lack — still
    /// has a face to measure with. That is the same substitution a viewer
    /// performs; it is only the *metric source* that this fixes.
    ///
    /// # Memoized on the context
    ///
    /// The faces are a pure function of the catalog's `/AcroForm`, and
    /// building them is expensive — the `/DR` walk constructs every font the
    /// form declares, encoding tables and substitution ladder included. The
    /// annotation overlay asks for them **once per page per render**, so the
    /// result is cached in the [`BuildContext`](pdfrum_page::BuildContext)
    /// beside the rest of the per-document font state, keyed on the
    /// `/AcroForm` reference. A caller threading one context through many
    /// renders of one document pays for this once;
    /// `docs/status/M13-perf-baseline.md` §5 measures what it used to cost.
    ///
    /// Nothing about *what* is built changed when the cache was added, which
    /// is what makes the appearance streams identical: the fallback still
    /// goes through the same loader, and the second faces are still loaded
    /// here rather than where a field discovers it needs one. Only the number
    /// of times moved.
    #[must_use]
    pub fn load<R: Resolve>(
        catalog: &Dict,
        r: &R,
        ctx: &mut pdfrum_page::BuildContext,
    ) -> std::sync::Arc<FormFonts> {
        let key = match catalog.raw(names::ACRO_FORM) {
            Some(pdfrum_object::Object::Ref(reference)) => {
                pdfrum_page::FormFontsKey::Form(*reference)
            }
            Some(_) => pdfrum_page::FormFontsKey::Direct,
            None => pdfrum_page::FormFontsKey::None,
        };
        ctx.form_fonts(key, |ctx| FormFonts::build(catalog, r, ctx))
    }

    /// [`Self::load`] without the cache: the faces, built now.
    #[must_use]
    fn build<R: Resolve>(catalog: &Dict, r: &R, ctx: &mut pdfrum_page::BuildContext) -> FormFonts {
        let (limits, mut diags) = (
            pdfrum_common::Limits::default(),
            pdfrum_common::Diagnostics::default(),
        );
        let mut load = |dict: &Dict| {
            pdfrum_font::load_with_options(
                dict,
                r,
                &ctx.fonts,
                &ctx.substitution,
                &limits,
                &mut diags,
            )
        };

        let mut entries = Vec::new();
        let fonts = catalog
            .dict(names::ACRO_FORM, r)
            .and_then(|form| form.dict(names::DR, r))
            .and_then(|resources| resources.dict(names::FONT, r));
        if let Some(fonts) = fonts {
            for key in fonts.keys() {
                let Some(dict) = fonts.dict(key, r) else {
                    continue;
                };
                if let Some(font) = load(&dict) {
                    entries.push((key.clone(), font));
                }
            }
        }
        // The fallback goes through the **same loader**, not the stock-metrics
        // constructor: the point of this type is that the ascent and descent
        // come from the face that is actually substituted, and a font built
        // from the base-14 tables would answer 718 and −219 where the face
        // answers its own. A field with no `/DR` at all is exactly where that
        // shows, because there is nothing else for it to measure with.
        entries.extend(load(&freetext::fallback_font()).map(|font| (Name::new(Vec::new()), font)));

        // The second faces, loaded here rather than where a field discovers it
        // needs one: loading needs the page's font cache, and the generators
        // are pure functions of the faces they are handed. There is one per
        // charset the font map can add for, which is one — see [`font_map`].
        let mut substitutes = Vec::new();
        for charset in font_map::SUBSTITUTABLE_CHARSETS {
            let Some(dict) = font_map::substitute_font_dict(*charset) else {
                continue;
            };
            if let Some(font) = load(&dict) {
                substitutes.push((Name::new(font_map::substitute_alias(*charset)), dict, font));
            }
        }
        FormFonts {
            entries,
            substitutes,
        }
    }

    /// The second face a character of `charset` is written in, with the alias
    /// it is filed under and the dictionary that goes into the appearance's
    /// own resources.
    ///
    /// Answers nothing for a charset with no encoding table, and for one whose
    /// face would not load — in both cases the caller leaves the character to
    /// the `/DA` font, which is the behaviour that predates this.
    #[must_use]
    pub fn substitute(&self, charset: pdfrum_font::Charset) -> Option<Substitute<'_>> {
        let alias = font_map::substitute_alias(charset);
        self.substitutes
            .iter()
            .find(|(name, _, _)| name.as_bytes() == alias)
            .map(|(name, dict, font)| Substitute {
                alias: name,
                dict,
                font,
            })
    }

    /// The face filed under one resource name, or the fallback.
    ///
    /// Answers nothing only if the fallback itself is missing, which
    /// [`Self::load`] makes impossible — the caller then generates chrome
    /// alone rather than being told a face exists that does not.
    #[must_use]
    pub fn face(&self, name: &[u8]) -> Option<&pdfrum_font::Font> {
        self.entries
            .iter()
            .find(|(key, _)| key.as_bytes() == name)
            .or_else(|| self.entries.last())
            .map(|(_, font)| font)
    }

    /// A [`TextFont`] over one resource name, with `width` borrowed for the
    /// same lifetime.
    ///
    /// The width closure cannot live inside the returned value — it has to be
    /// borrowed for the font's lifetime, which a closure over `self` cannot
    /// supply before `self` exists — so the caller keeps it and passes it in,
    /// the same shape [`TextFont::metrics_of`] already has.
    #[must_use]
    pub fn text_font<'a>(
        &'a self,
        name: &[u8],
        width: &'a dyn Fn(u32) -> i32,
    ) -> Option<TextFont<'a>> {
        let font = self.face(name)?;
        Some(TextFont {
            metrics: TextFont::metrics_of(font, width),
            font,
        })
    }
}

/// Generates appearances for every annotation on a page that wants one.
///
/// The walk mirrors what a viewer does when it opens a page, because that
/// ordering is what the `--annot` contract describes: pop-ups written into
/// the file are skipped, everything else is offered to its generator, and the
/// results are keyed by position in `/Annots`.
///
/// The text-bearing generators are skipped here; [`generate_appearances_with_text`]
/// is the walk that enables them.
#[must_use]
pub fn generate_appearances<R: Resolve>(
    page: &Dict,
    r: &R,
    diags: &mut Diagnostics,
) -> AnnotOverlay {
    let Some(annots) = page.array(obj_names::ANNOTS, r) else {
        return AnnotOverlay::default();
    };
    let mut overlay = AnnotOverlay::with_capacity(annots.len());
    for index in 0..annots.len() {
        let Some(dict) = annots.dict_at(index, r) else {
            continue;
        };
        if crate::annot::is_popup(&dict, r) {
            continue;
        }
        if let Some(generated) = generate_one(&dict, r, diags) {
            overlay.set(index, generated);
        } else if let Some(generated) = widget::generate(&dict, r) {
            // A widget with no appearance dictionary gets its chrome built
            // when the page opens, whatever the form says about regenerating
            // appearances. See `widget` for how far that goes.
            diags.record(Severity::Recovered, DiagKind::AppearanceGenerated, None);
            overlay.set(index, generated);
        }
    }
    overlay
}

/// The same walk, with the text-bearing generators enabled.
///
/// The generators only produce an appearance when a font is in hand, so a
/// caller without one gets the same result as [`generate_appearances`].
///
/// Each annotation is measured with the face **its own** `/DA` names, looked
/// up in the form's default resources — not with one page-wide font. A page
/// whose fields name two different faces stacks their lines by two different
/// ascents, which is what a viewer does.
#[must_use]
pub fn generate_appearances_with_text<R: Resolve>(
    page: &Dict,
    catalog: &Dict,
    fonts: Option<&FormFonts>,
    r: &R,
    diags: &mut Diagnostics,
) -> AnnotOverlay {
    let Some(annots) = page.array(obj_names::ANNOTS, r) else {
        return AnnotOverlay::default();
    };
    let mut overlay = AnnotOverlay::with_capacity(annots.len());
    for index in 0..annots.len() {
        let Some(dict) = annots.dict_at(index, r) else {
            continue;
        };
        if crate::annot::is_popup(&dict, r) {
            continue;
        }
        // The width closure has to outlive the `TextFont` that borrows it, so
        // it is built here rather than inside the lookup.
        let named = fonts.and_then(|fonts| fonts.face(&font_name_of(&dict, catalog, r)));
        // A second face, for the characters this one's charset does not cover.
        // The **widths** have to know about it as well as the bytes: a run set
        // in two faces advances by two faces' metrics, and measuring it all
        // with the first gives a line the wrong length wherever the second one
        // writes. So the substitute enters through the width closure the
        // layout is built from, not only through the encoder.
        let da_charset = named.map_or(pdfrum_font::Charset::Ansi, font_map::font_charset);
        let substitute = fonts.and_then(|fonts| {
            font_map::SUBSTITUTABLE_CHARSETS
                .iter()
                .find(|charset| **charset != da_charset)
                .and_then(|charset| fonts.substitute(*charset))
        });
        let width = named.map(|font| {
            move |code: u32| match substitute {
                Some(sub) if !font_map::da_font_writes(font, da_charset, code) => {
                    font_map::substitute_width(sub.font, code)
                }
                _ => TextFont::char_width(font, code),
            }
        });
        let text_font = named.zip(width.as_ref()).map(|(font, width)| TextFont {
            metrics: TextFont::metrics_of(font, width),
            font,
        });
        let generated = generate_one(&dict, r, diags)
            .or_else(|| generate_text_bearing(&dict, catalog, text_font.as_ref(), r, diags))
            .or_else(|| {
                // A widget's own body needs the same font the free-text
                // generator wanted, so a caller with one gets the field's
                // value laid out and a caller without one gets the chrome
                // alone.
                match text_font.as_ref() {
                    Some(font) => widget::generate_with_text(&dict, catalog, font, substitute, r),
                    None => widget::generate(&dict, r),
                }
                .inspect(|_| {
                    diags.record(Severity::Recovered, DiagKind::AppearanceGenerated, None);
                })
            });
        if let Some(generated) = generated {
            overlay.set(index, generated);
        }
    }
    overlay
}

/// The `/DR /Font` resource name one annotation's default appearance names.
///
/// Falls back to the form's own `/DA`, then to nothing — and nothing resolves
/// to [`FormFonts`]'s fallback face rather than declining.
fn font_name_of<R: Resolve>(dict: &Dict, catalog: &Dict, r: &R) -> Vec<u8> {
    let form = catalog.dict(names::ACRO_FORM, r).unwrap_or_default();
    freetext::default_appearance(dict, &form, r)
        .map(|appearance| appearance.font_name)
        .unwrap_or_default()
}

/// The free-text generator, when its preconditions and a font allow.
fn generate_text_bearing<R: Resolve>(
    dict: &Dict,
    catalog: &Dict,
    text_font: Option<&TextFont<'_>>,
    r: &R,
    diags: &mut Diagnostics,
) -> Option<GeneratedAp> {
    if !should_generate(dict, r) {
        return None;
    }
    let subtype = Subtype::from_bytes(&dict.byte_string(obj_names::SUBTYPE, r).unwrap_or_default());
    if subtype != Subtype::FreeText {
        return None;
    }
    let font = text_font?;
    let generated =
        freetext::free_text(dict, catalog, r, &font.metrics, &|code| font.encode(code))?;
    diags.record(Severity::Recovered, DiagKind::AppearanceGenerated, None);
    Some(GeneratedAp {
        stream: generated.stream,
        bbox: dict.rect(obj_names::RECT, r),
        matrix: Affine::IDENTITY,
        resources: resources_dict(
            ext_gstate_dict(dict, false, r),
            generated.font_resources.clone(),
        ),
        rect_override: None,
        as_override: None,
    })
}

/// Generates one annotation's appearance, if it should have one.
#[must_use]
pub(crate) fn generate_one<R: Resolve>(
    dict: &Dict,
    r: &R,
    diags: &mut Diagnostics,
) -> Option<GeneratedAp> {
    if !should_generate(dict, r) {
        return None;
    }
    let subtype = Subtype::from_bytes(&dict.byte_string(obj_names::SUBTYPE, r).unwrap_or_default());
    let generated = match subtype {
        Subtype::Circle => markup::circle(dict, r),
        Subtype::Highlight => markup::highlight(dict, r),
        Subtype::Ink => markup::ink(dict, r)?,
        Subtype::Square => markup::square(dict, r),
        Subtype::Squiggly => markup::squiggly(dict, r),
        Subtype::StrikeOut => markup::strike_out(dict, r),
        Subtype::Text => markup::text(dict, r),
        Subtype::Underline => markup::underline(dict, r),
        // Everything else has no generator. The two text-bearing subtypes —
        // free text and pop-ups — do have one upstream, but it needs the
        // layout engine and is built on top of this dispatch rather than
        // inside it, so they answer the same way here.
        _ => return None,
    };
    diags.record(Severity::Recovered, DiagKind::AppearanceGenerated, None);

    // The bounding box is the annotation's rectangle **as it stands after
    // this generator ran** — so a sticky note's is the 20×20 box it just
    // produced, not the one the file declared.
    let rect = generated
        .rect_override
        .unwrap_or_else(|| dict.rect(obj_names::RECT, r));
    let bbox = if generated.is_text_markup {
        quad::bounding_rect_from_quad_points(dict.array(names::QUAD_POINTS, r).as_ref())
    } else {
        rect
    };
    Some(GeneratedAp {
        stream: generated.stream,
        bbox,
        matrix: Affine::IDENTITY,
        resources: resources_dict(
            ext_gstate_dict(dict, generated.blend_multiply, r),
            generated.font_resources.clone(),
        ),
        rect_override: generated.rect_override,
        as_override: None,
    })
}

/// Whether an annotation is eligible for a generated appearance.
///
/// Two gates. A **dictionary-valued** `/AP /N` suppresses generation, and a
/// stream answers as its own dictionary — so the common "it already has an
/// appearance" case and the multi-state checkbox case are the same test. And
/// a hidden annotation never generates.
#[must_use]
pub(crate) fn should_generate<R: Resolve>(dict: &Dict, r: &R) -> bool {
    if appearance::has_appearance(dict, r) {
        return false;
    }
    let flags = crate::annot::AnnotFlags::from_bits(dict.int(names::F, r).unwrap_or(0));
    !flags.is_hidden()
}

/// The graphics-state dictionary a generated appearance names.
///
/// Both alphas take the annotation's `/CA` when the key is present, whatever
/// its type reads as, and one otherwise. Only the highlight generator asks
/// for a blend mode other than normal.
#[must_use]
pub(crate) fn ext_gstate_dict<R: Resolve>(dict: &Dict, multiply: bool, r: &R) -> Dict {
    let opacity = if dict.contains_key(names::CA) {
        dict.number(names::CA, r).unwrap_or(0.0)
    } else {
        1.0
    };
    let blend = if multiply {
        names::MULTIPLY_BLEND
    } else {
        obj_names::NORMAL
    };
    let state = Dict::from_pairs([
        (
            obj_names::TYPE.clone(),
            Object::Name(names::EXT_GSTATE.clone()),
        ),
        (names::CA.clone(), Object::Real(opacity)),
        (names::CA_LOWER.clone(), Object::Real(opacity)),
        (names::AIS.clone(), Object::Bool(false)),
        (names::BM.clone(), Object::Name(blend.clone())),
    ]);
    Dict::from_pairs([(names::GS.clone(), Object::Dict(state))])
}

/// The appearance stream's `/Resources`, omitting either half when absent.
#[must_use]
pub(crate) fn resources_dict(ext_gstate: Dict, font: Option<Dict>) -> Dict {
    let mut resources = Dict::new();
    resources.push(names::EXT_GSTATE.clone(), Object::Dict(ext_gstate));
    if let Some(font) = font {
        resources.push(names::FONT.clone(), Object::Dict(font));
    }
    resources
}

/// The stream dictionary a generated appearance is stored under.
#[must_use]
pub fn stream_dict(generated: &GeneratedAp) -> Dict {
    Dict::from_pairs([
        (names::FORM_TYPE.clone(), Object::Int(1)),
        (
            obj_names::TYPE.clone(),
            Object::Name(names::XOBJECT.clone()),
        ),
        (
            obj_names::SUBTYPE.clone(),
            Object::Name(names::FORM.clone()),
        ),
        (
            names::MATRIX.clone(),
            Object::Array(matrix_array(generated.matrix)),
        ),
        (
            names::BBOX.clone(),
            Object::Array(rect_array(generated.bbox)),
        ),
        (
            names::RESOURCES.clone(),
            Object::Dict(generated.resources.clone()),
        ),
        (
            names::LENGTH.clone(),
            Object::Int(i64::try_from(generated.stream.len()).unwrap_or(0)),
        ),
    ])
}

/// A transform as its six numbers.
///
/// Narrowed to single precision because that is what a PDF real is; the
/// transforms these generators write are all exactly representable anyway.
#[allow(clippy::cast_possible_truncation)]
fn matrix_array(matrix: Affine) -> Array {
    Array::of(
        matrix
            .as_coeffs()
            .into_iter()
            .map(|value| Object::Real(value as f32)),
    )
}

/// A rectangle as its four corner numbers, in PDF's ordering.
fn rect_array(rect: Rect) -> Array {
    use crate::geom;
    Array::of(
        [
            geom::left(rect),
            geom::bottom(rect),
            geom::right(rect),
            geom::top(rect),
        ]
        .map(Object::Real),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AnnotOverlay, Appearance, GeneratedAp, ext_gstate_dict, generate_appearances, generate_one,
        should_generate,
    };
    use crate::geom;
    use pdfrum_common::Diagnostics;
    use pdfrum_object::{Array, ByteSpan, Dict, Name, NoResolve, Object, Stream};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn numbers(values: &[f32]) -> Object {
        Object::Array(Array::of(values.iter().copied().map(Object::from)))
    }

    fn sticky_note() -> Dict {
        dict(&[
            ("Subtype", Object::Name(Name::from("Text"))),
            ("Rect", numbers(&[10.0, 20.0, 200.0, 300.0])),
        ])
    }

    #[test]
    fn an_annotation_with_an_appearance_stream_generates_nothing() {
        let with_ap = dict(&[
            ("Subtype", Object::Name(Name::from("Text"))),
            (
                "AP",
                Object::Dict(dict(&[(
                    "N",
                    Object::Stream(Stream::new(Dict::new(), ByteSpan::from(b"x".to_vec()))),
                )])),
            ),
        ]);
        assert!(!should_generate(&with_ap, &NoResolve));
        assert!(should_generate(&sticky_note(), &NoResolve));
    }

    #[test]
    fn a_hidden_annotation_never_generates() {
        let mut hidden = sticky_note();
        hidden.push(Name::from("F"), Object::Int(2));
        assert!(!should_generate(&hidden, &NoResolve));
    }

    #[test]
    fn a_character_the_face_cannot_map_is_still_written() {
        // `bug_725389` shows three Hebrew characters through a `/DA` naming
        // Times-Roman, which has no glyph for any of them. Dropping them loses
        // the text objects entirely — six become three — where the oracle
        // writes the raw code point as a character code and draws whatever it
        // names. Wrong glyph, right object count, right layout.
        let font = pdfrum_font::Font::load_standard(
            pdfrum_font::StandardFont::Times,
            &pdfrum_font::FontCache::new(),
        );
        let width = |code: u32| super::TextFont::char_width(&font, code);
        let text = super::TextFont {
            metrics: super::TextFont::metrics_of(&font, &width),
            font: &font,
        };
        // Hebrew bet, which no standard Latin face encodes.
        assert_eq!(text.encode(0x05D1), vec![0xD1]);
        // And a character it does encode still round-trips through the
        // `ToUnicode` mapping rather than through the fallthrough.
        assert_eq!(text.encode(u32::from('A')), vec![b'A']);
        // The width follows whatever `encode` wrote, so the layout advances by
        // the same glyph the stream names.
        assert_eq!(
            super::TextFont::char_width(&font, 0x05D1),
            super::TextFont::char_width(&font, 0xD1)
        );
    }

    #[test]
    fn a_sticky_notes_rectangle_override_reaches_the_overlay() {
        let page = dict(&[(
            "Annots",
            Object::Array(Array::of([Object::Dict(sticky_note())])),
        )]);
        let mut diags = Diagnostics::default();
        let overlay = generate_appearances(&page, &NoResolve, &mut diags);
        assert_eq!(overlay.len(), 1);
        let raw = geom::rect(10.0, 20.0, 200.0, 300.0);
        assert_eq!(overlay.rect(0, raw), geom::rect(10.0, 20.0, 30.0, 40.0));
        // The bounding box follows the rewritten rectangle, not the file's.
        assert_eq!(
            overlay.get(0).map(|generated| generated.bbox),
            Some(geom::rect(10.0, 20.0, 30.0, 40.0))
        );
    }

    #[test]
    fn a_pop_up_written_into_the_file_is_skipped_by_the_walk() {
        let page = dict(&[(
            "Annots",
            Object::Array(Array::of([Object::Dict(dict(&[(
                "Subtype",
                Object::Name(Name::from("Popup")),
            )]))])),
        )]);
        let mut diags = Diagnostics::default();
        let overlay = generate_appearances(&page, &NoResolve, &mut diags);
        // The slot still exists — indices stay aligned with `/Annots` — but
        // nothing was generated into it.
        assert_eq!(overlay.len(), 1);
        assert!(overlay.get(0).is_none());
    }

    #[test]
    fn a_subtype_with_no_generator_produces_nothing() {
        let stamp = dict(&[("Subtype", Object::Name(Name::from("Stamp")))]);
        let mut diags = Diagnostics::default();
        assert!(generate_one(&stamp, &NoResolve, &mut diags).is_none());
    }

    #[test]
    fn a_text_markup_bounding_box_comes_from_the_quadrilaterals() {
        let highlight = dict(&[
            ("Subtype", Object::Name(Name::from("Highlight"))),
            ("Rect", numbers(&[0.0, 0.0, 5.0, 5.0])),
            (
                "QuadPoints",
                numbers(&[10.0, 20.0, 30.0, 20.0, 10.0, 10.0, 30.0, 10.0]),
            ),
        ]);
        let mut diags = Diagnostics::default();
        let got = generate_one(&highlight, &NoResolve, &mut diags).expect("generates");
        assert_eq!(got.bbox, geom::rect(10.0, 10.0, 30.0, 20.0));
    }

    #[test]
    fn the_graphics_state_takes_its_alpha_from_the_opacity_key() {
        let opaque = ext_gstate_dict(&Dict::new(), false, &NoResolve);
        let state = opaque
            .dict(&Name::from("GS"), &NoResolve)
            .expect("one entry");
        assert_eq!(state.number(&Name::from("CA"), &NoResolve), Some(1.0));
        assert_eq!(
            state.name(&Name::from("BM")).map(Name::as_bytes),
            Some(&b"Normal"[..])
        );

        let half = dict(&[("CA", Object::from(0.5_f32))]);
        let state = ext_gstate_dict(&half, true, &NoResolve)
            .dict(&Name::from("GS"), &NoResolve)
            .expect("one entry");
        assert_eq!(state.number(&Name::from("ca"), &NoResolve), Some(0.5));
        assert_eq!(
            state.name(&Name::from("BM")).map(Name::as_bytes),
            Some(&b"Multiply"[..])
        );
    }

    /// A generated appearance, distinguishable by its stream.
    fn made(stream: &str) -> GeneratedAp {
        GeneratedAp {
            stream: stream.as_bytes().to_vec(),
            bbox: geom::rect(0.0, 0.0, 1.0, 1.0),
            matrix: kurbo::Affine::IDENTITY,
            resources: Dict::default(),
            rect_override: None,
            as_override: None,
        }
    }

    /// The merge's whole contract in one test: the supplied overlay wins
    /// where it speaks, defers where it does not, and can say "draw nothing"
    /// as a value rather than as an absence.
    #[test]
    fn a_supplied_overlay_wins_only_where_it_has_something_to_say() {
        let mut base = AnnotOverlay::with_capacity(4);
        base.set(0, made("base zero"));
        base.set(1, made("base one"));
        base.set(2, made("base two"));

        let mut supplied = AnnotOverlay::with_capacity(4);
        supplied.set(1, made("live one"));
        supplied.set_appearance(2, Appearance::Suppressed);
        // Index 0 and 3 are untouched and must not disturb the base.

        base.merge_over(&supplied);

        assert_eq!(
            base.get(0).map(|g| g.stream.clone()),
            Some(b"base zero".to_vec()),
            "an untouched entry leaves the generated one alone"
        );
        assert_eq!(
            base.get(1).map(|g| g.stream.clone()),
            Some(b"live one".to_vec()),
            "a supplied entry replaces the generated one"
        );
        assert_eq!(
            base.appearance(2),
            &Appearance::Suppressed,
            "suppression survives the merge as a value"
        );
        assert_eq!(base.get(2), None, "a suppressed entry has no stream");
        assert_eq!(base.appearance(3), &Appearance::Untouched);
    }

    /// Suppression and absence read the same to `get` and differently to
    /// `appearance` — which is the distinction the enum exists to carry.
    #[test]
    fn suppressed_and_untouched_differ_only_where_it_matters() {
        let mut overlay = AnnotOverlay::with_capacity(2);
        overlay.set_appearance(0, Appearance::Suppressed);
        assert_eq!(overlay.get(0), None);
        assert_eq!(overlay.get(1), None);
        assert_ne!(overlay.appearance(0), overlay.appearance(1));
        // An index past the end is untouched rather than a panic, so a short
        // overlay is safe to consult for any annotation.
        assert_eq!(overlay.appearance(99), &Appearance::Untouched);
    }

    /// An entry past the end of the overlay being merged into is dropped:
    /// there is no annotation for it to apply to.
    #[test]
    fn a_supplied_entry_past_the_end_is_dropped() {
        let mut base = AnnotOverlay::with_capacity(1);
        let mut supplied = AnnotOverlay::with_capacity(5);
        supplied.set(4, made("nowhere"));
        base.merge_over(&supplied);
        assert_eq!(base.len(), 1);
        assert_eq!(base.get(4), None);
    }

    #[test]
    fn only_the_named_annotation_is_a_live_edit() {
        let mut overlay = AnnotOverlay::with_capacity(3);
        overlay.set(0, made("a"));
        overlay.set(1, made("b"));
        assert_eq!(overlay.live_edit(), None);
        assert!(!overlay.is_live_edit(0));
        overlay.set_live_edit(1);
        assert_eq!(overlay.live_edit(), Some(1));
        assert!(overlay.is_live_edit(1));
        // Every other annotation's appearance is an ordinary one, which is
        // what keeps ClearType off the rest of the page.
        assert!(!overlay.is_live_edit(0));
        assert!(!overlay.is_live_edit(2));
    }

    #[test]
    fn a_session_editing_a_second_field_replaces_the_first() {
        // One field is edited at a time, so this is a replacement rather than
        // a set: a stale mark would draw a field's committed text with
        // ClearType long after the editor left it.
        let mut overlay = AnnotOverlay::with_capacity(3);
        overlay.set_live_edit(0);
        overlay.set_live_edit(2);
        assert_eq!(overlay.live_edit(), Some(2));
        assert!(!overlay.is_live_edit(0));
    }

    #[test]
    fn merging_carries_the_live_edit_mark_over() {
        // The mark has to survive `merge_over` or it would be lost exactly
        // where it matters — the supplied overlay is the session's, and the
        // base is what the annotation pass generated.
        let mut base = AnnotOverlay::with_capacity(2);
        let mut supplied = AnnotOverlay::with_capacity(2);
        supplied.set(1, made("edited"));
        supplied.set_live_edit(1);
        base.merge_over(&supplied);
        assert!(base.is_live_edit(1));
        // And a merge that says nothing about it leaves the mark alone.
        let untouched = AnnotOverlay::with_capacity(2);
        base.merge_over(&untouched);
        assert!(base.is_live_edit(1));
    }

    #[test]
    fn the_form_faces_are_built_once_per_document_and_shared() {
        // The regression this pins: `FormFonts::load` used to rebuild every
        // `/DR` font, the fallback, and the synthesized second faces on every
        // call, and the annotation overlay calls it once per page per render.
        // See docs/status/M13-perf-baseline.md §4-5.
        let catalog = dict(&[("AcroForm", Object::Ref(pdfrum_object::ObjRef::new(7, 0)))]);
        let mut ctx = pdfrum_page::BuildContext::new();
        let first = super::FormFonts::load(&catalog, &NoResolve, &mut ctx);
        let second = super::FormFonts::load(&catalog, &NoResolve, &mut ctx);
        assert!(
            std::sync::Arc::ptr_eq(&first, &second),
            "a second load of one document's form must hit the cache"
        );
    }

    #[test]
    fn a_catalog_with_no_form_shares_one_set_of_faces() {
        // The case that made this a whole-corpus regression rather than a
        // forms one: a document with an annotation but no `/AcroForm` paid
        // the fallback load and the substitute synthesis on every render.
        // With no form there is nothing document-specific to build, so one
        // slot serves every such document a context is threaded through.
        let mut ctx = pdfrum_page::BuildContext::new();
        let first = super::FormFonts::load(&dict(&[]), &NoResolve, &mut ctx);
        let second = super::FormFonts::load(
            &dict(&[("Type", Object::Name(Name::from("Catalog")))]),
            &NoResolve,
            &mut ctx,
        );
        assert!(std::sync::Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn two_documents_do_not_share_one_contexts_form_faces() {
        // A `BuildContext` may legitimately be threaded through two
        // documents, so the cache keys on the `/AcroForm` reference the way
        // every other cache on it keys on the reference that named its value.
        let mut ctx = pdfrum_page::BuildContext::new();
        let one = super::FormFonts::load(
            &dict(&[("AcroForm", Object::Ref(pdfrum_object::ObjRef::new(7, 0)))]),
            &NoResolve,
            &mut ctx,
        );
        let two = super::FormFonts::load(
            &dict(&[("AcroForm", Object::Ref(pdfrum_object::ObjRef::new(8, 0)))]),
            &NoResolve,
            &mut ctx,
        );
        assert!(!std::sync::Arc::ptr_eq(&one, &two));
    }

    #[test]
    fn a_direct_form_dictionary_is_not_cached() {
        // It has no reference to key on, and its content is
        // document-specific, so it re-derives rather than risking one
        // document's faces standing in for another's.
        let catalog = dict(&[("AcroForm", Object::Dict(dict(&[])))]);
        let mut ctx = pdfrum_page::BuildContext::new();
        let first = super::FormFonts::load(&catalog, &NoResolve, &mut ctx);
        let second = super::FormFonts::load(&catalog, &NoResolve, &mut ctx);
        assert!(!std::sync::Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn an_overlay_that_marks_no_live_edit_leaves_every_index_ordinary() {
        // The default, and the whole corpus outside the form-events rows.
        let mut overlay = AnnotOverlay::with_capacity(4);
        overlay.set(2, made("generated"));
        assert_eq!(overlay.live_edit(), None);
        for index in 0..6 {
            assert!(!overlay.is_live_edit(index), "index {index}");
        }
    }
}
