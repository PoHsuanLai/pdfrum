//! Viewer preferences (ISO 32000-1 §12.2) on the write side: how a document
//! asks to be displayed and printed.
//!
//! The reader is [`pdfrum_doc::ViewerPrefs`], and it is deliberately
//! permissive — each accessor has its own default, and two of those differ
//! between "no dictionary" and "an empty one". This writes the dictionary, so
//! a caller who wants a particular default has to say so; there is no way to
//! express "absent" other than clearing the key.

use pdfrum_object::{Dict, Name, Object, Resolve, names};

use crate::{doc::EditDoc, error::Error};

/// A preference write either applies or names why it could not.
type Result<T> = core::result::Result<T, Error>;

/// The preferences a document can ask for.
///
/// Every field is optional, and an unset one **leaves the key alone** rather
/// than writing a default — which is what lets this change one preference of
/// a document without flattening the rest.
///
/// ```
/// use pdfrum_edit::{Duplex, ViewerPreferences};
///
/// let prefs = ViewerPreferences::new()
///     .hide_toolbar(true)
///     .duplex(Duplex::DuplexFlipLongEdge)
///     .num_copies(2);
/// assert_eq!(prefs, prefs.clone());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewerPreferences {
    hide_toolbar: Option<bool>,
    hide_menubar: Option<bool>,
    hide_window_ui: Option<bool>,
    fit_window: Option<bool>,
    center_window: Option<bool>,
    display_doc_title: Option<bool>,
    direction_r2l: Option<bool>,
    print_scaling: Option<bool>,
    num_copies: Option<i64>,
    duplex: Option<Duplex>,
}

/// How a document asks to be printed on both sides (`/Duplex`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duplex {
    /// One side only (`/Simplex`).
    Simplex,
    /// Both sides, flipping about the short edge (`/DuplexFlipShortEdge`).
    DuplexFlipShortEdge,
    /// Both sides, flipping about the long edge (`/DuplexFlipLongEdge`).
    DuplexFlipLongEdge,
}

impl Duplex {
    /// The `/Duplex` name this setting writes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Simplex => "Simplex",
            Self::DuplexFlipShortEdge => "DuplexFlipShortEdge",
            Self::DuplexFlipLongEdge => "DuplexFlipLongEdge",
        }
    }
}

impl ViewerPreferences {
    /// Preferences that change nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `/HideToolbar`: hide the reader's toolbar while the document is open.
    #[must_use]
    pub fn hide_toolbar(mut self, yes: bool) -> Self {
        self.hide_toolbar = Some(yes);
        self
    }

    /// `/HideMenubar`: hide the reader's menu bar.
    #[must_use]
    pub fn hide_menubar(mut self, yes: bool) -> Self {
        self.hide_menubar = Some(yes);
        self
    }

    /// `/HideWindowUI`: hide the reader's own scroll bars and panes, leaving
    /// the page.
    #[must_use]
    pub fn hide_window_ui(mut self, yes: bool) -> Self {
        self.hide_window_ui = Some(yes);
        self
    }

    /// `/FitWindow`: resize the window to the first page.
    #[must_use]
    pub fn fit_window(mut self, yes: bool) -> Self {
        self.fit_window = Some(yes);
        self
    }

    /// `/CenterWindow`: centre the window on the screen.
    #[must_use]
    pub fn center_window(mut self, yes: bool) -> Self {
        self.center_window = Some(yes);
        self
    }

    /// `/DisplayDocTitle`: title the window with `/Info /Title` rather than
    /// the file name.
    ///
    /// PDF/UA requires this, since a file name is not an accessible name.
    #[must_use]
    pub fn display_doc_title(mut self, yes: bool) -> Self {
        self.display_doc_title = Some(yes);
        self
    }

    /// `/Direction`: `R2L` when true, `L2R` when false.
    ///
    /// This orders the *pages* in a spread, not the text in a line — a
    /// right-to-left script in a left-to-right document is the ordinary case
    /// and wants this left alone.
    #[must_use]
    pub fn direction_r2l(mut self, yes: bool) -> Self {
        self.direction_r2l = Some(yes);
        self
    }

    /// `/PrintScaling`: `AppDefault` when true, `None` when false.
    ///
    /// `false` is what a document asks for when its page size is the point —
    /// a form, a label sheet — and scaling to the paper would falsify it.
    #[must_use]
    pub fn print_scaling(mut self, yes: bool) -> Self {
        self.print_scaling = Some(yes);
        self
    }

    /// `/NumCopies`: the number of copies the print dialog offers by default.
    #[must_use]
    pub fn num_copies(mut self, copies: i64) -> Self {
        self.num_copies = Some(copies);
        self
    }

    /// `/Duplex`: how the print dialog offers two-sided printing.
    #[must_use]
    pub fn duplex(mut self, duplex: Duplex) -> Self {
        self.duplex = Some(duplex);
        self
    }

    /// Merges these preferences into an existing dictionary, keeping the keys
    /// this says nothing about.
    fn merge_into(&self, mut dict: Dict) -> Dict {
        let booleans = [
            ("HideToolbar", self.hide_toolbar),
            ("HideMenubar", self.hide_menubar),
            ("HideWindowUI", self.hide_window_ui),
            ("FitWindow", self.fit_window),
            ("CenterWindow", self.center_window),
            ("DisplayDocTitle", self.display_doc_title),
        ];
        for (key, value) in booleans {
            if let Some(value) = value {
                dict.insert(Name::from(key), Object::Bool(value));
            }
        }
        if let Some(r2l) = self.direction_r2l {
            let value = if r2l { "R2L" } else { "L2R" };
            dict.insert(
                Name::from("Direction"),
                Object::Name(Name::from(value.as_bytes())),
            );
        }
        if let Some(scaling) = self.print_scaling {
            // The reader tests for the *string* `None`; anything else, this
            // one included, means the application's default.
            let value = if scaling { "AppDefault" } else { "None" };
            dict.insert(
                Name::from("PrintScaling"),
                Object::Name(Name::from(value.as_bytes())),
            );
        }
        if let Some(copies) = self.num_copies {
            dict.insert(Name::from("NumCopies"), Object::Int(copies));
        }
        if let Some(duplex) = self.duplex {
            dict.insert(
                Name::from("Duplex"),
                Object::Name(Name::from(duplex.as_str().as_bytes())),
            );
        }
        dict
    }
}

/// Sets the catalog's `/ViewerPreferences`.
///
/// Preferences the builder leaves unset keep whatever the document had, so a
/// caller may change one without reading the rest. A document with no
/// `/ViewerPreferences` gains one.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog to hold
/// them.
///
/// ```
/// use std::sync::Arc;
/// use pdfrum_edit::{EditDoc, SaveOptions, ViewerPreferences, save, set_viewer_preferences};
/// use pdfrum_parser::{LoadOptions, load};
///
/// let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
/// let doc = load(bytes, &LoadOptions::default())?;
/// let mut edit = EditDoc::new(&doc);
///
/// set_viewer_preferences(
///     &mut edit,
///     &ViewerPreferences::new().display_doc_title(true),
/// )?;
///
/// let mut out = Vec::new();
/// save(&edit, &SaveOptions::default(), &mut out)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn set_viewer_preferences(
    dest: &mut EditDoc<'_>,
    preferences: &ViewerPreferences,
) -> Result<()> {
    let Some(root) = dest.base().trailer().reference(names::ROOT) else {
        return Err(Error::NoDestinationCatalog);
    };
    let Some(mut catalog) = dest
        .fetch(root)
        .ok()
        .and_then(|object| object.as_dict().cloned())
    else {
        return Err(Error::NoDestinationCatalog);
    };

    // An indirect dictionary is edited in place, so anything else naming it
    // keeps pointing at the one that now carries the change.
    match catalog.raw(names::VIEWER_PREFERENCES).cloned() {
        Some(Object::Ref(prefs_ref)) => {
            let existing = dest
                .fetch(prefs_ref)
                .ok()
                .and_then(|object| object.as_dict().cloned())
                .unwrap_or_default();
            dest.replace(prefs_ref, Object::Dict(preferences.merge_into(existing)));
        }
        existing => {
            let dict = match existing {
                Some(Object::Dict(dict)) => dict,
                _ => Dict::new(),
            };
            catalog.insert(
                names::VIEWER_PREFERENCES.clone(),
                Object::Dict(preferences.merge_into(dict)),
            );
            dest.replace(root, Object::Dict(catalog));
        }
    }
    Ok(())
}

/// Sets the catalog's `/OpenAction` to a destination on `page`.
///
/// This is the destination form of `/OpenAction`, which is what a "open at
/// this page" request means; the action-dictionary form runs a script or a
/// go-to and is not what a document usually wants on open.
///
/// # Errors
///
/// - [`Error::NoDestinationCatalog`] when the document has no catalog.
/// - [`Error::PageIndexOutOfRange`] / [`Error::InlinePage`] for a bad page.
///
/// ```
/// use std::sync::Arc;
/// use pdfrum_edit::{AnnotGoToView, EditDoc, SaveOptions, save, set_open_action};
/// use pdfrum_parser::{LoadOptions, load};
///
/// let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
/// let doc = load(bytes, &LoadOptions::default())?;
/// let mut edit = EditDoc::new(&doc);
///
/// set_open_action(&mut edit, 0u32, AnnotGoToView::Fit)?;
///
/// let mut out = Vec::new();
/// save(&edit, &SaveOptions::default(), &mut out)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn set_open_action(
    dest: &mut EditDoc<'_>,
    page: impl Into<pdfrum_common::PageIndex>,
    view: crate::AnnotGoToView,
) -> Result<()> {
    let page = page.into();
    let Some((page_ref, _, _)) = dest.page_state(page)? else {
        return Err(Error::InlinePage(page));
    };
    let Some(root) = dest.base().trailer().reference(names::ROOT) else {
        return Err(Error::NoDestinationCatalog);
    };
    let Some(mut catalog) = dest
        .fetch(root)
        .ok()
        .and_then(|object| object.as_dict().cloned())
    else {
        return Err(Error::NoDestinationCatalog);
    };
    catalog.insert(
        crate::names::OPEN_ACTION.clone(),
        Object::Array(crate::annot::goto_dest_array(page_ref, view)),
    );
    dest.replace(root, Object::Dict(catalog));
    Ok(())
}

/// Removes the catalog's `/OpenAction`, so the document opens at page one with
/// whatever view the reader prefers.
///
/// Answers whether there was one, matching
/// [`delete_attachment`](crate::delete_attachment): a delete that finds
/// nothing is not an error.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog.
pub fn clear_open_action(dest: &mut EditDoc<'_>) -> Result<bool> {
    let Some(root) = dest.base().trailer().reference(names::ROOT) else {
        return Err(Error::NoDestinationCatalog);
    };
    let Some(mut catalog) = dest
        .fetch(root)
        .ok()
        .and_then(|object| object.as_dict().cloned())
    else {
        return Err(Error::NoDestinationCatalog);
    };
    if !catalog.contains_key(crate::names::OPEN_ACTION) {
        return Ok(false);
    }
    catalog.remove(crate::names::OPEN_ACTION);
    dest.replace(root, Object::Dict(catalog));
    Ok(true)
}
