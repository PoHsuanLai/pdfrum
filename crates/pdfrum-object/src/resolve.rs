//! Indirect-reference lookup: the one seam between the object model and
//! whatever holds the document's objects.
//!
//! **Resolution is one hop.** PDF lets an indirect object's body be itself a
//! reference (`7 0 obj 8 0 R endobj`); a chain like that reads as a *missing
//! value* rather than following to object 8. Every typed accessor in this
//! crate reproduces that — see [`Resolve`] and [`Resolved::as_direct`].
//!
//! Cycle safety is the store's problem, not this crate's: accessors resolve
//! one level by construction and cannot recurse.

use std::ops::Deref;
use std::sync::Arc;

use crate::{Error, ObjRef, Object};

/// A store that can produce the object behind a reference.
///
/// **Resolution is one hop: a reference to a reference is absent.** A typed
/// accessor that takes a `&impl Resolve` follows at most one `n g R`; if
/// what it finds there is itself a reference, the value reads as missing
/// rather than being chased further ([`Resolved::as_direct`]).
///
/// Implementations must look up by object *number* alone and ignore the
/// generation: a `12 3 R` in a real file resolves to whatever object 12 the
/// cross-reference table delivers, whatever generation it carries.
/// Unresolvable references — free entries, missing objects, cycles — return
/// [`Error::UnresolvedRef`] or [`Error::RefLoop`], which every typed accessor
/// then reads as absence.
pub trait Resolve {
    /// The object stored under `r`, or why it could not be produced.
    ///
    /// # Errors
    ///
    /// [`Error::UnresolvedRef`] when the store has no such object, and
    /// [`Error::RefLoop`] when the fetch re-entered one already in progress.
    fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, Error>;
}

impl<T: Resolve + ?Sized> Resolve for &T {
    fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, Error> {
        (**self).fetch(r)
    }
}

/// The result of resolving: a direct object stays borrowed, an indirect one
/// arrives shared from the store.
///
/// Behaves like the `Object` it holds through [`Deref`], so callers match on
/// it without caring which side it came from.
///
/// ```
/// use pdfrum_object::{Object, Resolved};
///
/// let obj = Object::Int(42);
/// let resolved = Resolved::Direct(&obj);
/// assert_eq!(resolved.as_int(), Some(42));
/// ```
#[derive(Debug, Clone)]
pub enum Resolved<'a> {
    /// The object was already direct; this borrows it in place.
    Direct(&'a Object),
    /// The object came from the store and is shared with it.
    Indirect(Arc<Object>),
}

impl Resolved<'_> {
    /// The object, whichever side it came from.
    #[must_use]
    pub fn get(&self) -> &Object {
        match self {
            Self::Direct(o) => o,
            Self::Indirect(o) => o,
        }
    }

    /// The object unless it is *itself* a reference.
    ///
    /// This is where [`Resolve`]'s one-hop rule is enforced: every typed
    /// accessor goes through here, so a reference-to-reference chain reads
    /// as absence.
    #[must_use]
    pub fn as_direct(&self) -> Option<&Object> {
        let obj = self.get();
        if matches!(obj, Object::Ref(_)) {
            None
        } else {
            Some(obj)
        }
    }

    /// Take ownership of the object, cloning a borrowed one.
    #[must_use]
    pub fn into_owned(self) -> Object {
        match self {
            Self::Direct(o) => o.clone(),
            Self::Indirect(o) => Arc::unwrap_or_clone(o),
        }
    }
}

impl Deref for Resolved<'_> {
    type Target = Object;

    fn deref(&self) -> &Object {
        self.get()
    }
}

impl PartialEq for Resolved<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

/// A resolver that knows nothing, for reading dictionaries whose references
/// are irrelevant (or before a store exists).
///
/// Every reference is unresolvable, which is exactly how a damaged file's
/// dangling references behave, so accessors keep working and return their
/// fallbacks.
///
/// ```
/// use pdfrum_object::{Dict, NoResolve, Object, names};
///
/// let dict = Dict::from_pairs([(names::LENGTH.clone(), Object::Int(7))]);
/// // Direct values still read fine without a store.
/// assert_eq!(dict.int(names::LENGTH, &NoResolve), Some(7));
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoResolve;

impl Resolve for NoResolve {
    fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, Error> {
        Err(Error::UnresolvedRef(r))
    }
}
