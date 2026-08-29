//! Appearance-stream generation.

pub mod fmt;

use kurbo::{Affine, Rect};
use pdfrum_object::{Dict, Name};

/// One generated appearance and the dictionary edits it implies.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedAp {
    /// The content-stream bytes.
    pub stream: Vec<u8>,
    /// The form `XObject`'s bounding box.
    pub bbox: Rect,
    /// Its matrix.
    pub matrix: Affine,
    /// Its resource dictionary.
    pub resources: Dict,
    /// A rewritten `/Rect`, when generation moved one.
    pub rect_override: Option<Rect>,
    /// A copied-down `/AS`, for the widget path.
    pub as_override: Option<Name>,
}

/// Per-annotation generated appearances, keyed by `/Annots` index.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnnotOverlay {
    entries: Vec<Option<GeneratedAp>>,
}

impl AnnotOverlay {
    /// An overlay with room for `count` annotations and nothing generated.
    #[must_use]
    pub fn with_capacity(count: usize) -> AnnotOverlay {
        AnnotOverlay {
            entries: vec![None; count],
        }
    }

    /// Records a generated appearance at one `/Annots` index.
    pub fn set(&mut self, index: usize, generated: GeneratedAp) {
        if let Some(slot) = self.entries.get_mut(index) {
            *slot = Some(generated);
        }
    }

    /// What was generated at one `/Annots` index, if anything.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&GeneratedAp> {
        self.entries.get(index).and_then(Option::as_ref)
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
