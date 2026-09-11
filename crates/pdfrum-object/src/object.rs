//! The [`Object`] enum and the coercions every accessor is built from.
//!
//! A coercion that does not apply to a variant yields that variant's
//! fallback rather than failing: an array has no string spelling, a name has
//! no numeric value.

// PDFium reaches these through virtual methods with defaults on the base
// class — `GetString()` on an array returns `""`, `GetInteger()` on a name
// returns `0`, and so on. Here they are exhaustive matches, so adding a
// variant makes every coercion fail to compile until it is considered.

use std::collections::HashSet;

use crate::number::{fmt_int, fmt_number, narrow_to_signed32, truncate_to_signed32, widen_to_f32};
use crate::{Array, Dict, Error, Name, PdfString, Resolve, Resolved, Stream};

/// An indirect object's identity: the pair a `N G obj` header carries and a
/// cross-reference entry keys.
///
/// A reference token (`12 3 R`) carries a generation too, but resolution
/// ignores it — see [`Resolve`]. The generation is kept because the
/// cross-reference table and the writer both need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjRef {
    /// The object number.
    pub num: u32,
    /// The generation number.
    ///
    /// Spelled out rather than `gen`, which Rust 2024 reserves.
    pub generation: u16,
}

impl ObjRef {
    // The object number no object may have; a cross-reference entry naming
    // it is a broken entry.
    //
    // **Private on purpose**: no sentinel belongs in a public surface.
    // PDFium exports this as `kInvalidObjNum`, but the PDF spec
    // reserves no such number — the 8388607-object limit simply puts it out
    // of reach — so a caller has nothing to compare against and no reason
    // to. `ObjRef::is_invalid` is the whole public surface: the sentinel
    // keeps existing in the parse, where the bytes contain it, and is asked
    // about rather than handed over.
    const INVALID_NUM: u32 = 0xFFFF_FFFF;

    /// A reference to `num` at `generation`.
    #[must_use]
    pub const fn new(num: u32, generation: u16) -> Self {
        Self { num, generation }
    }

    /// Whether the object number is the reserved invalid one.
    #[must_use]
    pub const fn is_invalid(self) -> bool {
        self.num == Self::INVALID_NUM
    }
}

/// A PDF object (ISO 32000-1 §7.3).
///
/// The eight basic types plus streams, plus a reference standing in for an
/// indirect object. `Int` and `Real` are separate variants because the
/// distinction is observable: an integer and a real that happen to be equal
/// serialize differently and read back differently through the integer
/// accessors.
///
/// ```
/// use pdfrum_object::{Object, PdfString};
///
/// assert_eq!(Object::Int(1245).as_int(), Some(1245));
/// assert_eq!(Object::Real(9.5).number(), Some(9.5));
/// assert_eq!(Object::Bool(true).as_bool(), Some(true));
/// // A name has no numeric value.
/// assert_eq!(Object::Name("Foo".into()).number(), None);
/// // ...but every object has a string spelling, empty for most.
/// assert_eq!(Object::Str(PdfString::literal(b"hi")).to_byte_string(), b"hi");
/// assert_eq!(Object::Null.to_byte_string(), b"");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum Object {
    /// The `null` object.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer.
    ///
    /// Holds the *mathematical* value. Every integer a conforming lexer can
    /// produce lies in `-2^31 ..= 2^32 - 1`
    /// (see [`INT_RANGE`](crate::INT_RANGE)) — larger literals fold to zero
    /// during parsing. Reading it back has two flavours,
    /// [`as_int`](Object::as_int) and [`number`](Object::number), which
    /// disagree above `i32::MAX`; see [`narrow_to_signed32`](crate::narrow_to_signed32).
    Int(i64),
    /// A real number. `f32` rather than `f64` to match the precision the
    /// oracle parses, formats and renders with.
    Real(f32),
    /// A string, in either syntax.
    Str(PdfString),
    /// A name.
    Name(Name),
    /// An array.
    Array(Array),
    /// A dictionary.
    Dict(Dict),
    /// A stream: a dictionary with bytes attached.
    ///
    /// Boxed because it is the one wide payload — a `Dict` plus a `ByteSpan`
    /// is 56 bytes where every other payload is 24 — and the one that never
    /// sits in the hot structures: ISO 32000-1 §7.3.8.1 forbids a stream as
    /// a direct array element or dictionary value, so the box is only
    /// dereferenced on the indirect-object path. It takes `Object` from 56
    /// bytes to 32 and a dictionary pair from 80 to 56.
    Stream(Box<Stream>),
    /// A reference to an indirect object.
    Ref(ObjRef),
}

impl Object {
    /// The boolean value, only for an actual boolean.
    ///
    /// `Int(1)` is deliberately *not* a boolean: the type check happens
    /// before any coercion, so a file that writes `1` for a flag reads as
    /// "absent, use the default".
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The integer value of any object that has one, in the C-integer view.
    ///
    /// Booleans count as 0 and 1, reals truncate toward zero (saturating, NaN
    /// to 0), and everything else has no integer value. Note this is not a
    /// type test — use [`Object::as_number`] for "is this a number".
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Bool(b) => Some(i64::from(*b)),
            Self::Int(v) => Some(narrow_to_signed32(*v)),
            Self::Real(v) => Some(truncate_to_signed32(*v)),
            Self::Null
            | Self::Str(_)
            | Self::Name(_)
            | Self::Array(_)
            | Self::Dict(_)
            | Self::Stream(_)
            | Self::Ref(_) => None,
        }
    }

    /// The numeric value of a number, coercing integers to `f32`.
    ///
    /// Only numbers have one — unlike [`Object::as_int`], a boolean does not
    /// count.
    #[must_use]
    pub fn number(&self) -> Option<f32> {
        match self {
            Self::Int(v) => Some(widen_to_f32(*v)),
            Self::Real(v) => Some(*v),
            Self::Null
            | Self::Bool(_)
            | Self::Str(_)
            | Self::Name(_)
            | Self::Array(_)
            | Self::Dict(_)
            | Self::Stream(_)
            | Self::Ref(_) => None,
        }
    }

    /// The number itself, for accessors that type-check before coercing.
    #[must_use]
    pub fn as_number(&self) -> Option<&Self> {
        match self {
            Self::Int(_) | Self::Real(_) => Some(self),
            _ => None,
        }
    }

    /// The string, only for an actual string object.
    #[must_use]
    pub fn as_string(&self) -> Option<&PdfString> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The name, only for an actual name object.
    #[must_use]
    pub fn as_name(&self) -> Option<&Name> {
        match self {
            Self::Name(n) => Some(n),
            _ => None,
        }
    }

    /// The array, only for an actual array.
    #[must_use]
    pub fn as_array(&self) -> Option<&Array> {
        match self {
            Self::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The dictionary — of a dictionary object, or of a stream.
    ///
    /// Streams answer with their own dictionary, which is what lets page-tree
    /// and cross-reference code read `/Type` off either kind of object
    /// without branching.
    #[must_use]
    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Self::Dict(d) => Some(d),
            Self::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    /// The stream, only for an actual stream.
    #[must_use]
    pub fn as_stream(&self) -> Option<&Stream> {
        match self {
            Self::Stream(s) => Some(s),
            _ => None,
        }
    }

    /// The reference, only for an actual reference.
    #[must_use]
    pub fn as_ref_id(&self) -> Option<ObjRef> {
        match self {
            Self::Ref(r) => Some(*r),
            _ => None,
        }
    }

    /// Whether this is the null object.
    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// The object's byte-string spelling.
    ///
    /// Booleans spell `true`/`false`, numbers spell as the writer would, a
    /// string yields its bytes and a name its decoded bytes. Everything else
    /// — null, arrays, dictionaries, streams, references — has no spelling
    /// and yields empty.
    #[must_use]
    pub fn to_byte_string(&self) -> Vec<u8> {
        match self {
            Self::Bool(true) => b"true".to_vec(),
            Self::Bool(false) => b"false".to_vec(),
            Self::Int(v) => fmt_int(*v).into_bytes(),
            Self::Real(v) => fmt_number(*v).into_bytes(),
            Self::Str(s) => s.as_bytes().to_vec(),
            Self::Name(n) => n.as_bytes().to_vec(),
            Self::Null | Self::Array(_) | Self::Dict(_) | Self::Stream(_) | Self::Ref(_) => {
                Vec::new()
            }
        }
    }

    /// The object read as text: strings and names decode, everything else
    /// yields empty.
    ///
    /// A stream's text needs its filters applied first, which this crate
    /// cannot do — the reader composes decoding with
    /// [`decode_text`](crate::decode_text) instead.
    #[must_use]
    pub fn to_text(&self) -> String {
        match self {
            Self::Str(s) => s.as_text().into_owned(),
            Self::Name(n) => n.as_text().into_owned(),
            Self::Null
            | Self::Bool(_)
            | Self::Int(_)
            | Self::Real(_)
            | Self::Array(_)
            | Self::Dict(_)
            | Self::Stream(_)
            | Self::Ref(_) => String::new(),
        }
    }

    /// Resolve one level: a reference becomes the object the store holds,
    /// anything else is already itself.
    ///
    /// The result may still be a reference — an indirect object whose body is
    /// `8 0 R` resolves to that reference and is *not* chased further, which
    /// is why typed accessors go through [`Resolved::as_direct`].
    ///
    /// # Errors
    ///
    /// Whatever the store reports for an unresolvable reference.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use pdfrum_object::{NoResolve, ObjRef, Object};
    /// let direct = Object::Real(1.5);
    /// assert_eq!(direct.resolve(&NoResolve).unwrap().number(), Some(1.5));
    /// // Without a store every reference is dangling.
    /// assert!(Object::Ref(ObjRef::new(4, 0)).resolve(&NoResolve).is_err());
    /// ```
    pub fn resolve<'a>(&'a self, r: &impl Resolve) -> Result<Resolved<'a>, Error> {
        match self {
            Self::Ref(id) => Ok(Resolved::Indirect(r.fetch(*id)?)),
            _ => Ok(Resolved::Direct(self)),
        }
    }

    /// Deep-copy the object with every reference replaced by what it points
    /// at, dropping the edges that would close a cycle. Only a reference back
    /// to an *ancestor* is a cycle; siblings may share substructure and both
    /// copies survive. A cut edge **disappears** — the key or element is
    /// omitted rather than becoming null — and an unresolvable reference
    /// disappears the same way, indistinguishably.
    ///
    /// A reference to a stream flattens into the stream itself, stored
    /// *directly* in the dictionary or array that held it, with its **raw**,
    /// still-encoded bytes and its `/Filter` intact: ISO 32000-1 §7.3.8.1
    /// constrains a *file*, not these in-memory types.
    ///
    /// ```
    /// # use std::collections::HashMap;
    /// # use std::sync::Arc;
    /// # use pdfrum_object::{Array, Error, ObjRef, Object, Resolve};
    /// # struct Store(HashMap<u32, Arc<Object>>);
    /// # impl Resolve for Store {
    /// #     fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, Error> {
    /// #         self.0.get(&r.num).cloned().ok_or(Error::UnresolvedRef(r))
    /// #     }
    /// # }
    /// let store = Store(HashMap::from([(7, Arc::new(Object::Int(42)))]));
    /// let array = Object::Array(Array::from_iter([Object::Ref(ObjRef::new(7, 0))]));
    /// assert_eq!(
    ///     array.clone_direct(&store),
    ///     Object::Array(Array::from_iter([Object::Int(42)])),
    /// );
    /// ```
    #[must_use]
    pub fn clone_direct(&self, r: &impl Resolve) -> Self {
        // A cut edge and a dangling reference are indistinguishable because
        // `CPDF_Reference::CloneNonCyclic` returns `nullptr` for both.
        // Flattening a stream into a container matches
        // `CPDF_Dictionary::CloneNonCyclic`, whose loop inserts into `map_`
        // directly and so bypasses the `CHECK(!IsStream())` that guards the
        // ordinary setters; the raw bytes are
        // `CPDF_Stream::CloneNonCyclic`'s `LoadAllDataRaw`.
        let mut ancestors = HashSet::new();
        clone_flattened(self, r, &mut ancestors).unwrap_or(Self::Null)
    }
}

/// Deep-copy `obj` flattening references, cutting edges back to `ancestors`.
/// `None` means "this edge closes a cycle or dangles" — the caller drops it.
fn clone_flattened(
    obj: &Object,
    r: &impl Resolve,
    ancestors: &mut HashSet<ObjRef>,
) -> Option<Object> {
    match obj {
        Object::Ref(id) => {
            if !ancestors.insert(*id) {
                return None;
            }
            let target = r.fetch(*id).ok();
            let cloned = target
                .as_deref()
                .and_then(|t| clone_flattened(t, r, &mut ancestors.clone()));
            ancestors.remove(id);
            cloned
        }
        Object::Array(a) => Some(Object::Array(
            a.iter()
                .filter_map(|e| clone_flattened(e, r, &mut ancestors.clone()))
                .collect(),
        )),
        Object::Dict(d) => Some(Object::Dict(
            d.iter()
                .filter_map(|(k, v)| {
                    clone_flattened(v, r, &mut ancestors.clone()).map(|v| (k.clone(), v))
                })
                .collect(),
        )),
        Object::Stream(s) => {
            let dict = s
                .dict
                .iter()
                .filter_map(|(k, v)| {
                    clone_flattened(v, r, &mut ancestors.clone()).map(|v| (k.clone(), v))
                })
                .collect();
            Some(Object::Stream(Box::new(Stream::new(dict, s.data.clone()))))
        }
        other => Some(other.clone()),
    }
}

impl From<bool> for Object {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<i64> for Object {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<i32> for Object {
    fn from(v: i32) -> Self {
        Self::Int(i64::from(v))
    }
}

impl From<f32> for Object {
    fn from(v: f32) -> Self {
        Self::Real(v)
    }
}

impl From<Name> for Object {
    fn from(v: Name) -> Self {
        Self::Name(v)
    }
}

impl From<&Name> for Object {
    fn from(v: &Name) -> Self {
        Self::Name(v.clone())
    }
}

impl From<u32> for Object {
    fn from(v: u32) -> Self {
        Self::Int(i64::from(v))
    }
}

impl From<PdfString> for Object {
    fn from(v: PdfString) -> Self {
        Self::Str(v)
    }
}

impl From<Array> for Object {
    fn from(v: Array) -> Self {
        Self::Array(v)
    }
}

impl From<Dict> for Object {
    fn from(v: Dict) -> Self {
        Self::Dict(v)
    }
}

impl From<Stream> for Object {
    fn from(v: Stream) -> Self {
        Self::Stream(Box::new(v))
    }
}

impl From<ObjRef> for Object {
    fn from(v: ObjRef) -> Self {
        Self::Ref(v)
    }
}
