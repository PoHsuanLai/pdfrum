//! Viewer preferences (ISO 32000-1 §12.2): how a document asks to be
//! displayed and printed.
//!
//! Each accessor has its own default for a document with no
//! `/ViewerPreferences` at all, and two of those defaults are **not** the
//! value a present-but-empty dictionary produces: `num_copies` answers one
//! with no dictionary and zero with an empty one.

use pdfrum_object::{Array, Dict, Name, Object, Resolve};

use crate::names;

/// A document's viewer preferences.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViewerPrefs {
    /// The `/ViewerPreferences` dictionary, absent when the catalog has none.
    pub dict: Option<Dict>,
}

impl ViewerPrefs {
    /// Reads the catalog's preferences.
    #[must_use]
    pub fn read<R: Resolve>(catalog: &Dict, r: &R) -> ViewerPrefs {
        ViewerPrefs {
            dict: catalog.dict(names::VIEWER_PREFERENCES, r),
        }
    }

    /// Whether the reading order is right-to-left.
    #[must_use]
    pub fn is_direction_r2l<R: Resolve>(&self, r: &R) -> bool {
        self.dict
            .as_ref()
            .and_then(|dict| dict.byte_string(names::DIRECTION, r))
            .as_deref()
            == Some(b"R2L")
    }

    /// Whether the print dialog offers page scaling. **True** with no
    /// dictionary.
    #[must_use]
    pub fn print_scaling<R: Resolve>(&self, r: &R) -> bool {
        self.dict
            .as_ref()
            .and_then(|dict| dict.byte_string(names::PRINT_SCALING, r))
            .as_deref()
            != Some(b"None")
    }

    /// The default number of copies.
    ///
    /// One with no dictionary, **zero** with a dictionary that omits the key
    /// — the two paths genuinely differ.
    #[must_use]
    pub fn num_copies<R: Resolve>(&self, r: &R) -> i64 {
        match &self.dict {
            None => 1,
            Some(dict) => dict.int(names::NUM_COPIES, r).unwrap_or(0),
        }
    }

    /// The default printed page range, as raw index pairs. Duplicates are
    /// kept.
    #[must_use]
    pub fn print_page_range<R: Resolve>(&self, r: &R) -> Option<Array> {
        self.dict
            .as_ref()
            .and_then(|dict| dict.array(names::PRINT_PAGE_RANGE, r))
    }

    /// The duplex handling. `None` — the *string* — with no dictionary.
    #[must_use]
    pub fn duplex<R: Resolve>(&self, r: &R) -> Vec<u8> {
        self.dict
            .as_ref()
            .and_then(|dict| dict.byte_string(names::DUPLEX, r))
            .unwrap_or_else(|| b"None".to_vec())
    }

    /// Any preference whose value is a **name**.
    ///
    /// The type filter is the point: a Boolean `/HideToolbar` and an integer
    /// `/NumCopies` both answer nothing here even when present. Keys are
    /// case-sensitive.
    #[must_use]
    pub fn generic_name(&self, key: &Name) -> Option<Vec<u8>> {
        self.dict
            .as_ref()?
            .raw(key)
            .and_then(Object::as_name)
            .map(|name| name.as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::ViewerPrefs;
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object};

    fn prefs(pairs: &[(&str, Object)]) -> ViewerPrefs {
        ViewerPrefs {
            dict: Some(Dict::from_pairs(
                pairs
                    .iter()
                    .map(|(k, v)| (Name::from(*k), v.clone()))
                    .collect::<Vec<_>>(),
            )),
        }
    }

    #[test]
    fn a_document_with_no_preferences_uses_the_no_dictionary_defaults() {
        let none = ViewerPrefs::default();
        assert!(!none.is_direction_r2l(&NoResolve));
        assert!(none.print_scaling(&NoResolve));
        assert_eq!(none.num_copies(&NoResolve), 1);
        assert_eq!(none.duplex(&NoResolve), b"None");
        assert!(none.print_page_range(&NoResolve).is_none());
    }

    #[test]
    fn an_empty_dictionary_answers_zero_copies_not_one() {
        assert_eq!(prefs(&[]).num_copies(&NoResolve), 0);
    }

    #[test]
    fn the_generic_accessor_is_the_only_type_filtered_one() {
        let p = prefs(&[
            ("NumCopies", Object::Int(5)),
            ("Direction", Object::Name(Name::from("R2L"))),
            ("ViewArea", Object::Name(Name::from("CropBox"))),
            ("HideToolbar", Object::Bool(true)),
            ("Foo", Object::Name(Name::from("foo"))),
        ]);
        assert_eq!(p.num_copies(&NoResolve), 5);
        assert!(p.is_direction_r2l(&NoResolve));
        assert_eq!(
            p.generic_name(&Name::from("ViewArea")),
            Some(b"CropBox".to_vec())
        );
        // A Boolean and an integer are not names.
        assert_eq!(p.generic_name(&Name::from("HideToolbar")), None);
        assert_eq!(p.generic_name(&Name::from("NumCopies")), None);
        // Keys are case-sensitive.
        assert_eq!(p.generic_name(&Name::from("Foo")), Some(b"foo".to_vec()));
        assert_eq!(p.generic_name(&Name::from("foo")), None);
    }

    #[test]
    fn a_print_page_range_keeps_its_duplicates() {
        let p = prefs(&[(
            "PrintPageRange",
            Object::Array(Array::of([0, 2, 4, 4].map(Object::from))),
        )]);
        let range = p.print_page_range(&NoResolve).expect("present");
        assert_eq!(range.len(), 4);
        assert_eq!(range.int_at(3), Some(4));
    }
}
