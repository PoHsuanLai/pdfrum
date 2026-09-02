//! One page's annotations, read once into the shapes routing needs.
//!
//! # Why this exists as its own pass
//!
//! Every other module in this crate is a pure function over values, and stays
//! that way because *something* has to turn a document into those values.
//! This is that something: it walks a page's `/Annots` array once and answers
//! with `Candidate`s for the hit test, `Focusable`s for the tab ring, and
//! enough per-widget configuration to build a field's interaction state the
//! first time one is touched.
//!
//! Keeping the walk here rather than inside the router is what lets every
//! routing decision stay testable on hand-built values: the tests in `hit`,
//! `tab` and `field` never open a file, and the tests here never route an
//! event.
//!
//! # The index space is the raw array
//!
//! The walk is over `/Annots` **as the file writes it**, pop-ups included and
//! counted. That is what [`AnnotId`] promises and what the appearance overlay
//! a caller draws through is keyed by. `pdfrum-doc`'s own `AnnotList` drops
//! pop-ups and would renumber everything after the first one, so this does
//! not use it — it reads the array directly and keeps each entry's position.
//!
//! # Fields are identified by name, not by dictionary
//!
//! Two widgets can be two controls of one field — a radio group is the
//! ordinary case — and they must share one interaction state, or clicking the
//! second forgets what the first did. So a [`FieldId`] is allocated per
//! **fully qualified field name**, and two widgets that resolve to the same
//! name get the same id. A widget with no name at all is its own field, keyed
//! by its raw index, because nothing else can distinguish it.

use pdfrum_common::{Diagnostics, Limits, PageIndex};
use pdfrum_doc::form::{FieldFlags, FieldKind};
use pdfrum_doc::{Subtype, ap};
use pdfrum_object::{Dict, Name, Resolve, names as obj_names};

use crate::field::{ChoiceConfig, ChoiceOption, TextConfig};

/// `/MaxLen` — a text field's character cap.
///
/// Spelled here rather than imported: `pdfrum-doc`'s name table is private to
/// that crate, and three constants are cheaper than widening its surface.
const MAX_LEN: &Name = &Name::from_static(b"MaxLen");
/// `/TI` — the first visible row of a list box.
const TI: &Name = &Name::from_static(b"TI");
/// `/Fields` — the form's field array, under the catalog's `/AcroForm`.
const FIELDS: &Name = &Name::from_static(b"Fields");
/// `/MK` — a widget's appearance characteristics, which carry its rotation.
const MK: &Name = &Name::from_static(b"MK");
/// `/R` — the widget's rotation within its `/Rect`, in degrees.
const R: &Name = &Name::from_static(b"R");
/// `/Tabs` — the page's declared focus-traversal order.
///
/// Read from the page dictionary **directly**, not inherited from the page
/// tree: `CPDFSDK_AnnotIterator::GetTabOrder` calls `GetByteStringFor` on the
/// page's own dictionary, so a `/Tabs` on `/Pages` reaches no page.
const TABS: &Name = &Name::from_static(b"Tabs");
use crate::geom::Rotation;
use crate::hit::{Candidate, LayoutBand, WidgetHit};
use crate::session::{AnnotId, FieldId};
use crate::tab::{Focusable, Rect, TabOrder};

/// Everything one page contributes to routing.
///
/// Built once per page per event replay. The three lists are parallel views
/// of the same walk rather than three walks: `candidates` is what the hit
/// test reads, `focusables` is what the tab ring reads, and `widgets` is what
/// a field's state is built from.
#[derive(Debug, Clone, Default)]
pub struct PageForm {
    /// Which page this describes.
    pub page: PageIndex,
    /// Every annotation, in raw `/Annots` order, for the hit test.
    pub(crate) candidates: Vec<Candidate>,
    /// Every annotation as a focus-ring candidate, paired with its subtype
    /// so the caller's `focusable` list can filter them.
    pub(crate) focusables: Vec<(Subtype, Focusable)>,
    /// The traversal order this page's `/Tabs` asks for.
    ///
    /// A property of the **page**, not of the session: `annotiter.pdf` is
    /// three pages of identical annotations under `/R`, `/C` and `/S`, and
    /// the first Tab lands on a different one on each. Defaults to
    /// [`TabOrder::Structure`], which is also what an unrecognized spelling
    /// means.
    pub tab_order: TabOrder,
    /// The widgets, with what a field's interaction state needs.
    pub widgets: Vec<WidgetInfo>,
    /// Every annotation's dictionary, keyed by its raw `/Annots` index.
    ///
    /// A `BTreeMap` rather than a `Vec` because the walk skips entries it
    /// cannot read as dictionaries, so the indices have gaps and a positional
    /// list would silently shift everything after one.
    pub dicts: std::collections::BTreeMap<u32, Dict>,
    /// The page's height in PDF units, for the one thing that needs it: how
    /// much room a combo box has to open its dropdown into.
    ///
    /// `CFFL_InteractiveFormFiller::QueryWherePopup`
    /// (`cffl_interactiveformfiller.cpp:679-680`) builds its page rectangle
    /// as `(0, GetPageHeight(), GetPageWidth(), 0)` normalized — from the
    /// **origin**, whatever the crop box says — and measures the widget's
    /// `/Rect` against it. So this is the display height and the comparison
    /// is against zero on the other side, reproduced rather than corrected:
    /// a page whose crop box starts away from the origin gets the oracle's
    /// answer, right or wrong, because the popup's position is what a golden
    /// pins.
    pub page_height: f32,
}

/// One widget annotation, read far enough to build its field's state.
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetInfo {
    /// Which annotation, by raw `/Annots` index.
    pub id: AnnotId,
    /// Which field it is a control of.
    pub field: FieldId,
    /// The field's fully qualified name, empty when it has none.
    pub name: String,
    /// What kind of field it is, when the classifier could name one.
    pub kind: Option<FieldKind>,
    /// The inherited `/Ff`.
    pub flags: FieldFlags,
    /// The widget's `/Rect` as written, in this crate's private `f32`.
    pub(crate) rect: Rect,
    /// The widget's `/MK /R`, as the quadrant the appearance stream is set
    /// into.
    ///
    /// Read the way `ap::widget::rotated_rect` reads it — `% 360`, and
    /// anything that is not `0`, `90`, `180` or `270` after that is upright.
    /// A rounding normalization that folded `37` down to upright would be
    /// wrong here, because routing and the generator must agree about which
    /// box a click lands in. `CPDFSDK_Widget::GetRotate`
    /// (`cpdfsdk_widget.cpp:458-461`) takes the same truncating modulo, so a
    /// `/R -90` is upright to both.
    pub rotation: Rotation,
    /// The widget's dictionary, for the readers that want the long tail.
    pub dict: Dict,
    /// The field dictionary the **value** is read from, when that is not the
    /// widget itself.
    pub valued: Dict,
}

impl WidgetInfo {
    /// The field's stored value.
    #[must_use]
    pub fn value<R: Resolve>(&self, r: &R) -> String {
        ap::field_body::field_value(&self.valued, r)
    }

    /// The field's options, for a choice field.
    #[must_use]
    pub fn options<R: Resolve>(&self, r: &R) -> Vec<ChoiceOption> {
        ap::field_body::options(&self.valued, r)
            .into_iter()
            .map(|choice| ChoiceOption {
                label: choice.label,
                value: choice.value,
            })
            .collect()
    }

    /// Which options the file says are selected, as **interaction** reads it.
    ///
    /// Deliberately not `ap::field_body::selected_indices`, and the two are
    /// both right. There are two producers of a list box's selection upstream
    /// and they disagree on purpose:
    ///
    /// - the **appearance** reader is `CPDFSDK_AppStream::SetAsListBox` →
    ///   `GetSelectedIndex` → `GetValueOrSelectedIndicesObject`, which takes
    ///   `/V` first and matches it as text. That is what draws a file with no
    ///   `/AP`, and it is what `ap::field_body::selected_indices` reproduces.
    /// - the **interaction** reader is `CPDF_FormField::IsItemSelected`
    ///   (`cpdf_formfield.cpp:546-554`), which consults `/I` first, as
    ///   integer indices, and falls back to `/V` only when `/I` is not
    ///   *usable* — `UseSelectedIndicesObject` (`:863-935`). That is what
    ///   `FORM_IsIndexSelected` answers and what a session's state must be
    ///   seeded from.
    ///
    /// They agree except on one shape — `/I` present, `/V` absent — where the
    /// first selects nothing and the second selects the rows `/I` names.
    /// `listbox_form.pdf`'s indices field is exactly that shape, and
    /// `CheckIfMultipleSelectedIndices` expects rows 1 and 3.
    #[must_use]
    pub fn selected<R: Resolve>(&self, r: &R) -> Vec<usize> {
        let values: Vec<String> = ap::field_body::options(&self.valued, r)
            .into_iter()
            .map(|choice| choice.value)
            .collect();
        pdfrum_doc::form::selected_indices_for_interaction(&self.valued, &values, r)
    }

    /// The text configuration, read from the flags and `/MaxLen`.
    #[must_use]
    pub fn text_config<R: Resolve>(&self, r: &R) -> TextConfig {
        let max_len =
            inherited_int(&self.dict, MAX_LEN, r).and_then(|value| u32::try_from(value).ok());
        TextConfig::read(self.flags, max_len)
    }

    /// The choice configuration, read from the flags.
    #[must_use]
    pub fn choice_config(&self) -> ChoiceConfig {
        ChoiceConfig::read(self.flags)
    }

    /// The row a list box starts drawing at, from `/TI`.
    #[must_use]
    pub fn top_index<R: Resolve>(&self, r: &R) -> usize {
        inherited_int(&self.dict, TI, r)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0)
    }
}

/// Reads one page's annotations.
///
/// `page_dict` is the page, `catalog` the document catalog — the form's
/// `/Fields` array is reached through it, which is what resolves a widget
/// that is a second control of an earlier field.
#[must_use]
pub fn read<R: Resolve>(
    page: impl Into<PageIndex>,
    page_dict: &Dict,
    catalog: &Dict,
    r: &R,
) -> PageForm {
    let page = page.into();
    let mut form = PageForm {
        page,
        tab_order: TabOrder::from_tabs(page_dict.byte_string(TABS, r).as_deref()),
        page_height: page_height(page_dict, r),
        ..PageForm::default()
    };
    let Some(annots) = page_dict.array(obj_names::ANNOTS, r) else {
        return form;
    };

    // A field id per distinct field name, so two controls of one field share
    // one interaction state. Allocated in first-seen order, which makes the
    // ids stable for a given file.
    let mut names: Vec<String> = Vec::new();

    for index in 0..annots.len() {
        let Some(dict) = annots.dict_at(index, r) else {
            continue;
        };
        let id = AnnotId::new(page, u32::try_from(index).unwrap_or(u32::MAX));
        let subtype =
            Subtype::from_bytes(&dict.byte_string(obj_names::SUBTYPE, r).unwrap_or_default());
        let rect = to_rect(dict.rect(obj_names::RECT, r));
        let band = band_of(subtype);

        let widget = (subtype == Subtype::Widget).then(|| read_widget(&dict, r));
        form.candidates.push(Candidate {
            id,
            rect,
            band,
            widget: widget.as_ref().map(|(hit, _)| *hit),
        });

        if let Some((_, info)) = widget {
            let name = pdfrum_doc::form::full_name(&dict, r);
            let field = field_id_of(&mut names, &name, index);
            let valued = value_dict_of(&dict, catalog, r).unwrap_or_else(|| dict.clone());
            form.widgets.push(WidgetInfo {
                id,
                field,
                name,
                kind: info.kind,
                flags: info.flags,
                rect,
                rotation: widget_rotation(&dict, r),
                dict: dict.clone(),
                valued,
            });
        }

        // The focus ring's membership is the caller's choice of subtypes, so
        // every annotation is offered here and the ring filters.
        form.focusables.push((subtype, Focusable { id, rect }));
        form.dicts.insert(id.index, dict);
    }
    form
}

/// The page's display height, the one number a dropdown's placement needs.
///
/// `/MediaBox` and `/CropBox` are inheritable (ISO 32000-1 §7.7.3.4), so the
/// walk climbs `/Parent` for a page that states neither — `derive_boxes`
/// takes the closure that does the climbing, and applies `/Rotate` after it,
/// because a quarter-turned page's *height* is its crop box's width and the
/// popup's room is measured on the page as shown.
fn page_height<R: Resolve>(page_dict: &Dict, r: &R) -> f32 {
    let inherited = |key: &Name| -> Option<pdfrum_object::Object> {
        let mut node = page_dict.clone();
        // The same bound `PageDict::inherited` uses; a `/Parent` cycle in a
        // damaged file would otherwise spin here.
        for _ in 0..64 {
            if let Some(value) = node.get(key, r) {
                return Some(value.get().clone());
            }
            node = node.dict(obj_names::PARENT, r)?;
        }
        None
    };
    // The boxes are derived, not read: a missing or degenerate `/MediaBox` is
    // US Letter rather than nothing, which is the size the oracle would have
    // measured the room against too.
    let mut diags = Diagnostics::default();
    let (_, height) = pdfrum_page::display_size_from_dict(page_dict, inherited, r, &mut diags);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a page taller than f32 has already lost meaning; the value \
                  only decides which side a dropdown opens on"
    )]
    let height = height as f32;
    height
}

/// A widget's `/MK /R`, as the quadrant its appearance stream is set into.
///
/// The truncating `% 360` is deliberate and is `ap::widget::rotated_rect`'s;
/// see [`WidgetInfo::rotation`].
fn widget_rotation<R: Resolve>(dict: &Dict, r: &R) -> Rotation {
    let degrees = dict.dict(MK, r).and_then(|mk| mk.int(R, r)).unwrap_or(0);
    match degrees % 360 {
        90 => Rotation::Quarter,
        180 => Rotation::Half,
        270 => Rotation::ThreeQuarter,
        _ => Rotation::None,
    }
}

/// What reading a widget's own dictionary answers.
struct WidgetRead {
    kind: Option<FieldKind>,
    flags: FieldFlags,
}

/// Reads the four hit-test gates and the field classification.
fn read_widget<R: Resolve>(dict: &Dict, r: &R) -> (WidgetHit, WidgetRead) {
    let flags = FieldFlags::from_bits(inherited_int(dict, obj_names::FF, r).unwrap_or(0));
    let field_type = inherited_name(dict, obj_names::FT, r).unwrap_or_default();
    let kind = FieldKind::classify(&field_type, flags);
    let annot_flags = pdfrum_doc::AnnotFlags::from_bits(dict.int(obj_names::F, r).unwrap_or(0));

    let hit = WidgetHit {
        signature: kind == Some(FieldKind::Signature),
        // Any of the three "do not show this" bits, which is the oracle's own
        // disjunction rather than the hidden bit alone.
        hidden: annot_flags.is_hidden()
            || annot_flags.no_view()
            || annot_flags.contains(pdfrum_doc::AnnotFlags::INVISIBLE),
        read_only: flags.is_read_only(),
        push_button: kind == Some(FieldKind::Button),
    };
    (hit, WidgetRead { kind, flags })
}

/// Which band a subtype sorts into.
fn band_of(subtype: Subtype) -> LayoutBand {
    match subtype {
        Subtype::Popup => LayoutBand::Popup,
        Subtype::Widget => LayoutBand::Widget,
        _ => LayoutBand::Other,
    }
}

/// The field id for a name, allocating one the first time it is seen.
///
/// A widget with no name cannot be grouped with anything, so it becomes its
/// own field keyed by its raw index — offset past the named ids so the two
/// spaces cannot collide.
fn field_id_of(names: &mut Vec<String>, name: &str, index: usize) -> FieldId {
    if name.is_empty() {
        // Unnamed widgets are their own fields. The offset keeps them out of
        // the named range, which grows from zero.
        return FieldId(u32::MAX - u32::try_from(index).unwrap_or(0));
    }
    if let Some(at) = names.iter().position(|known| known == name) {
        return FieldId(u32::try_from(at).unwrap_or(0));
    }
    names.push(name.to_string());
    FieldId(u32::try_from(names.len() - 1).unwrap_or(0))
}

/// The dictionary a widget's field **value** is read from.
///
/// Usually the widget itself. The exception is two `/Fields` entries sharing
/// a `/T` with no parent between them: the second is a second *control* of
/// the first's field, and both show the first dictionary's value.
fn value_dict_of<R: Resolve>(dict: &Dict, catalog: &Dict, r: &R) -> Option<Dict> {
    if dict.contains_key(obj_names::PARENT) {
        return None;
    }
    let name = dict.byte_string(obj_names::T, r)?;
    let form = catalog.dict(obj_names::ACRO_FORM, r)?;
    let fields = form.array(FIELDS, r)?;
    let first = (0..fields.len())
        .filter_map(|index| fields.dict_at(index, r))
        .find(|entry| entry.byte_string(obj_names::T, r).as_deref() == Some(name.as_slice()))?;
    (first != *dict).then_some(first)
}

/// An inherited integer field attribute.
fn inherited_int<R: Resolve>(dict: &Dict, key: &pdfrum_object::Name, r: &R) -> Option<i64> {
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    pdfrum_doc::form::field_attr(dict, key, r, &limits, &mut diags)?.as_int()
}

/// An inherited name-valued field attribute, as bytes.
fn inherited_name<R: Resolve>(dict: &Dict, key: &pdfrum_object::Name, r: &R) -> Option<Vec<u8>> {
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    Some(pdfrum_doc::form::field_attr(dict, key, r, &limits, &mut diags)?.to_byte_string())
}

/// A `kurbo` rectangle onto this crate's. The rect half of `Point::narrow`.
pub(crate) fn to_rect(rect: kurbo::Rect) -> Rect {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "page coordinates beyond f32 have already lost meaning, and every \
                  geometric query in this crate is f32"
    )]
    Rect::new(
        rect.x0 as f32,
        rect.y0 as f32,
        rect.x1 as f32,
        rect.y1 as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unnamed_widget_is_its_own_field() {
        let mut names = Vec::new();
        let first = field_id_of(&mut names, "", 0);
        let second = field_id_of(&mut names, "", 1);
        assert_ne!(first, second, "two unnamed widgets are two fields");
        assert!(names.is_empty(), "and neither claims a named id");
    }

    /// The rule a radio group depends on: two controls of one field share one
    /// interaction state, so clicking the second remembers what the first
    /// did.
    #[test]
    fn two_widgets_with_one_name_are_one_field() {
        let mut names = Vec::new();
        let first = field_id_of(&mut names, "Group", 0);
        let second = field_id_of(&mut names, "Group", 3);
        assert_eq!(first, second);
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn distinct_names_take_distinct_ids_in_first_seen_order() {
        let mut names = Vec::new();
        assert_eq!(field_id_of(&mut names, "A", 0), FieldId(0));
        assert_eq!(field_id_of(&mut names, "B", 1), FieldId(1));
        assert_eq!(field_id_of(&mut names, "A", 2), FieldId(0));
        assert_eq!(field_id_of(&mut names, "C", 3), FieldId(2));
    }

    /// The named and unnamed id spaces must not collide, or an unnamed widget
    /// would share a field with a named one.
    #[test]
    fn the_unnamed_id_space_does_not_meet_the_named_one() {
        let mut names = Vec::new();
        let loose = field_id_of(&mut names, "", 0);
        for index in 0..64 {
            let titled = field_id_of(&mut names, &format!("field{index}"), index);
            assert_ne!(titled, loose);
        }
    }

    #[test]
    fn subtypes_sort_into_the_three_bands() {
        assert_eq!(band_of(Subtype::Popup), LayoutBand::Popup);
        assert_eq!(band_of(Subtype::Widget), LayoutBand::Widget);
        assert_eq!(band_of(Subtype::Link), LayoutBand::Other);
        assert_eq!(band_of(Subtype::Highlight), LayoutBand::Other);
    }

    /// An empty page answers with empty lists rather than declining.
    #[test]
    fn a_page_with_no_annots_reads_as_empty() {
        let page = Dict::new();
        let catalog = Dict::new();
        let form = read(0, &page, &catalog, &pdfrum_object::NoResolve);
        assert!(form.candidates.is_empty());
        assert!(form.widgets.is_empty());
        assert!(form.focusables.is_empty());
    }
}
