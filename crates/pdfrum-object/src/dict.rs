//! Dictionary objects (ISO 32000-1 §7.3.7) and the typed accessors every
//! other crate reads PDF structure through. Keys keep document order,
//! duplicates and all; the **last** entry with a key wins on lookup.
//!
//! Whether an accessor follows an indirect reference is deliberate, not an
//! optimization. [`Dict::get`], [`Dict::int`], [`Dict::number`],
//! [`Dict::dict`], [`Dict::array`], [`Dict::stream`], [`Dict::text`],
//! [`Dict::rect`], [`Dict::matrix`] and [`Dict::byte_string`] chase one
//! reference; [`Dict::raw`], [`Dict::direct_int`], [`Dict::name`],
//! [`Dict::bool`], [`Dict::number_obj`] and [`Dict::string`] read one as
//! absence.

// # Why a `Vec`, and why insertion order
//
// PDF dictionaries are small — a handful of keys, a few dozen at the extreme
// — so a linear scan beats a hash map on every real document, and the
// storage doubles as the writer's key order. PDFium uses a sorted map and
// therefore writes keys sorted; we keep document order in storage *and* in
// serialization. That difference is invisible to every behavior under test:
// lookup semantics are identical, and round-trip fidelity is judged by
// reparsing, not by byte-diffing the output.
//
// Duplicate keys are kept as parsed and the **last** one wins on lookup,
// which is what PDFium's overwrite-on-insert produces for a document read
// front to back.
//
// # Which accessors resolve
//
// Whether an accessor follows an indirect reference is not a detail — it is
// load-bearing recovery behavior. An indirect `/Prev` is *ignored* by the
// cross-reference reader while an indirect `/Length` *is* chased, and files
// in the wild depend on both. So this module offers each accessor in two
// flavours and callers pick deliberately. The non-resolving ones are not an
// optimization; they are the accessors whose C++ counterparts type-check
// *before* resolution, so a reference there reads as absence.

use pdfrum_common::kurbo::{Affine, Rect};

use crate::{Array, Name, Object, PdfString, Resolve, Resolved, Stream};

/// A PDF dictionary: key-value pairs in document order.
///
/// # Streams as values
///
/// ISO 32000-1 §7.3.8.1 forbids a *file* from writing a stream as a direct
/// dictionary value, and the reader drops one found inline while parsing.
/// That is a **file-format** constraint, not an in-memory invariant, and
/// this type does not police it:
/// [`Object::clone_direct`](crate::Object::clone_direct) flattens
/// references, so a `/Resources` whose `/XObject` entries are indirect
/// streams clones into a dictionary holding those streams directly, and
/// [`Dict::stream`] reads such a value back. Enforcing §7.3.8.1 is the
/// **writer's** job: `pdfrum-edit` hoists a direct stream to an indirect
/// object at serialization time.
///
/// ```
/// use pdfrum_object::{Dict, NoResolve, Object, names};
///
/// let dict = Dict::from_pairs([
///     (names::TYPE.clone(), Object::Name(names::PAGE.clone())),
///     (names::COUNT.clone(), Object::Int(3)),
/// ]);
/// assert_eq!(dict.name(names::TYPE), Some(names::PAGE));
/// assert_eq!(dict.int(names::COUNT, &NoResolve), Some(3));
/// assert_eq!(dict.len(), 2);
/// ```
// The inline-stream drop while parsing is `cpdf_syntax_parser.cpp:645-649`.
// `CPDF_Dictionary::CloneNonCyclic` produces the same flattened shape, since
// its loop writes straight into `map_` and bypasses the `CHECK(!IsStream())`
// that guards the ordinary setters; `Dict::stream` mirrors
// `CPDF_Dictionary::GetStreamFor` in reading it back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dict(Vec<(Name, Object)>);

impl Dict {
    /// An empty dictionary.
    #[must_use]
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// A dictionary from key-value pairs, keeping their order.
    #[must_use]
    pub fn from_pairs(pairs: impl IntoIterator<Item = (Name, Object)>) -> Self {
        pairs.into_iter().collect()
    }

    /// Append a pair, keeping any earlier entry with the same key.
    ///
    /// The later entry wins on lookup, so appending is how a reader records a
    /// duplicate key without losing what the file actually said.
    ///
    /// Any object, a stream included — see the type-level note on §7.3.8.1.
    pub fn push(&mut self, key: Name, value: Object) {
        self.0.push((key, value));
    }

    /// Number of stored pairs, duplicates included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the dictionary has no pairs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The pairs, in document order.
    pub fn iter(&self) -> impl Iterator<Item = &(Name, Object)> {
        self.0.iter()
    }

    /// The keys, in document order, duplicates included.
    pub fn keys(&self) -> impl Iterator<Item = &Name> {
        self.0.iter().map(|(k, _)| k)
    }

    /// Whether any entry carries this key.
    #[must_use]
    pub fn contains_key(&self, key: &Name) -> bool {
        self.raw(key).is_some()
    }

    // ---- non-resolving accessors ----

    /// The stored value, whatever its type, without resolving references.
    ///
    /// The last entry with this key wins.
    #[must_use]
    pub fn raw(&self, key: &Name) -> Option<&Object> {
        self.0.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The value of a `Number`-typed entry in the C-integer view, without
    /// resolving.
    ///
    /// This is how the cross-reference reader reads `/Size`, `/Prev` and
    /// `/XRefStm`: an *indirect* value there is ignored rather than chased,
    /// which is deliberate recovery behavior in files whose trailer points at
    /// objects the table cannot yet describe.
    #[must_use]
    pub fn direct_int(&self, key: &Name) -> Option<i64> {
        self.raw(key)?.as_number()?.as_int()
    }

    /// The name a `Name`-typed entry holds, without resolving.
    ///
    /// A reference here reads as absent: the type check happens before any
    /// resolution, so `/Type 5 0 R` never names a type.
    #[must_use]
    pub fn name(&self, key: &Name) -> Option<&Name> {
        self.raw(key)?.as_name()
    }

    /// The value of a `Boolean`-typed entry, without resolving.
    ///
    /// An `Int(1)` is not a boolean and reads as absent.
    #[must_use]
    pub fn bool(&self, key: &Name) -> Option<bool> {
        self.raw(key)?.as_bool()
    }

    /// A `Number`-typed entry as an object, without resolving. Used where the
    /// distinction between "not a number" and "zero" matters, such as
    /// validating a cross-reference stream's `/Index`.
    #[must_use]
    pub fn number_obj(&self, key: &Name) -> Option<&Object> {
        self.raw(key)?.as_number()
    }

    /// The string a `String`-typed entry holds, without resolving.
    #[must_use]
    pub fn string(&self, key: &Name) -> Option<&PdfString> {
        self.raw(key)?.as_string()
    }

    // ---- resolving accessors ----

    /// The value, following one level of indirection.
    ///
    /// Returns `None` for a missing key *and* for a reference the store
    /// cannot produce — both are absence — and the store records the
    /// underlying failure in its diagnostics.
    #[must_use]
    pub fn get<'a>(&'a self, key: &Name, r: &impl Resolve) -> Option<Resolved<'a>> {
        self.raw(key)?.resolve(r).ok()
    }

    /// The integer value of an entry of any type, in the C-integer view.
    ///
    /// Coerces: booleans read as 0 and 1, reals truncate. A reference is
    /// followed one level, and a reference *to* a reference reads as absent.
    #[must_use]
    pub fn int(&self, key: &Name, r: &impl Resolve) -> Option<i64> {
        self.get(key, r)?.as_direct()?.as_int()
    }

    /// The numeric value of an entry, coercing integers to `f32`.
    #[must_use]
    pub fn number(&self, key: &Name, r: &impl Resolve) -> Option<f32> {
        self.get(key, r)?.as_direct()?.number()
    }

    /// The byte-string spelling of an entry of any type — see
    /// [`Object::to_byte_string`].
    #[must_use]
    pub fn byte_string(&self, key: &Name, r: &impl Resolve) -> Option<Vec<u8>> {
        Some(self.get(key, r)?.as_direct()?.to_byte_string())
    }

    /// An entry read as text — see [`Object::to_text`].
    #[must_use]
    pub fn text(&self, key: &Name, r: &impl Resolve) -> Option<String> {
        Some(self.get(key, r)?.to_text())
    }

    /// The dictionary an entry holds, following one level of indirection.
    ///
    /// A stream answers with its own dictionary, so `/Pages` pointing at
    /// either a dictionary or a stream reads the same way.
    ///
    /// Returns an owned clone because the dictionary may live inside an
    /// `Arc` the store owns; dictionaries are small and this keeps the
    /// borrow story simple for callers.
    #[must_use]
    pub fn dict(&self, key: &Name, r: &impl Resolve) -> Option<Dict> {
        self.get(key, r)?.as_direct()?.as_dict().cloned()
    }

    /// The array an entry holds, following one level of indirection.
    #[must_use]
    pub fn array(&self, key: &Name, r: &impl Resolve) -> Option<Array> {
        self.get(key, r)?.as_direct()?.as_array().cloned()
    }

    /// The stream an entry holds, following one level of indirection.
    #[must_use]
    pub fn stream(&self, key: &Name, r: &impl Resolve) -> Option<Stream> {
        self.get(key, r)?.as_direct()?.as_stream().cloned()
    }

    /// The reference an entry holds, without resolving it.
    #[must_use]
    pub fn reference(&self, key: &Name) -> Option<crate::ObjRef> {
        self.raw(key)?.as_ref_id()
    }

    /// A rectangle read from a four-element array — see [`Array::as_rect`].
    ///
    /// Missing or malformed yields the zero rectangle, never `None`: PDF
    /// consumers of `/MediaBox` and friends all want a rectangle.
    #[must_use]
    pub fn rect(&self, key: &Name, r: &impl Resolve) -> Rect {
        self.array(key, r)
            .map_or_else(|| Rect::new(0.0, 0.0, 0.0, 0.0), |a| a.as_rect())
    }

    /// A transformation matrix read from a six-element array — see
    /// [`Array::as_matrix`]. Missing or malformed yields the identity.
    #[must_use]
    pub fn matrix(&self, key: &Name, r: &impl Resolve) -> Affine {
        self.array(key, r)
            .map_or(Affine::IDENTITY, |a| a.as_matrix())
    }
}

impl FromIterator<(Name, Object)> for Dict {
    fn from_iter<I: IntoIterator<Item = (Name, Object)>>(iter: I) -> Self {
        let mut dict = Self::new();
        for (k, v) in iter {
            dict.push(k, v);
        }
        dict
    }
}

impl<'a> IntoIterator for &'a Dict {
    type Item = &'a (Name, Object);
    type IntoIter = std::slice::Iter<'a, (Name, Object)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use pdfrum_common::kurbo::{Affine, Rect};

    use crate::test_resolve::TestStore;
    use crate::{Array, Dict, Name, NoResolve, ObjRef, Object, PdfString, Stream, names};

    fn key(s: &str) -> Name {
        Name::from(s)
    }

    // From cpdf_dictionary_unittest.cpp:13-37, restated: PDFium's sorted map
    // iterates alphabetically, we iterate in document order (divergence #3 in
    // docs/design/pdfrum-object.md).
    #[test]
    fn iteration_follows_document_order_not_sort_order() {
        let dict = Dict::from_pairs([
            (key("the-dictionary"), Object::Dict(Dict::new())),
            (key("the-array"), Object::Array(Array::new())),
            (key("the-number"), Object::Int(42)),
        ]);
        let order: Vec<_> = dict.keys().filter_map(Name::as_str).collect();
        assert_eq!(order, ["the-dictionary", "the-array", "the-number"]);
    }

    #[test]
    fn last_duplicate_wins_on_lookup_and_both_are_kept() {
        let dict = Dict::from_pairs([
            (key("K"), Object::Int(1)),
            (key("K"), Object::Int(2)),
            (key("K"), Object::Int(3)),
        ]);
        assert_eq!(dict.raw(&key("K")), Some(&Object::Int(3)));
        assert_eq!(dict.direct_int(&key("K")), Some(3));
        assert_eq!(dict.len(), 3, "the file said it three times");
    }

    // From cpdf_object_unittest.cpp:289-311 (GetNameFor / GetByteStringFor).
    #[test]
    fn name_accessor_is_type_filtered_but_byte_string_coerces() {
        let dict = Dict::from_pairs([
            (key("bool"), Object::Bool(false)),
            (key("num"), Object::Real(0.23)),
            (key("string"), Object::Str(PdfString::literal(b"ium"))),
            (key("name"), Object::Name(key("Pdf"))),
        ]);

        assert_eq!(dict.name(&key("invalid")), None);
        assert_eq!(dict.name(&key("bool")), None);
        assert_eq!(dict.name(&key("num")), None);
        assert_eq!(dict.name(&key("string")), None);
        assert_eq!(dict.name(&key("name")), Some(&key("Pdf")));

        assert_eq!(dict.byte_string(&key("invalid"), &NoResolve), None);
        assert_eq!(
            dict.byte_string(&key("bool"), &NoResolve).as_deref(),
            Some(&b"false"[..])
        );
        assert_eq!(
            dict.byte_string(&key("num"), &NoResolve).as_deref(),
            Some(&b".23"[..])
        );
        assert_eq!(
            dict.byte_string(&key("string"), &NoResolve).as_deref(),
            Some(&b"ium"[..])
        );
        assert_eq!(
            dict.byte_string(&key("name"), &NoResolve).as_deref(),
            Some(&b"Pdf"[..])
        );
    }

    #[test]
    fn boolean_accessor_rejects_integers() {
        let dict = Dict::from_pairs([
            (key("flag"), Object::Bool(true)),
            (key("one"), Object::Int(1)),
        ]);
        assert_eq!(dict.bool(&key("flag")), Some(true));
        assert_eq!(dict.bool(&key("one")), None, "an Int(1) is not a boolean");
    }

    #[test]
    fn direct_int_ignores_indirection_while_int_follows_it() {
        let store = TestStore::from_pairs([(3, Object::Int(99))]);
        let dict = Dict::from_pairs([
            (names::PREV.clone(), Object::Ref(ObjRef::new(3, 0))),
            (names::LENGTH.clone(), Object::Ref(ObjRef::new(3, 0))),
        ]);
        // How the cross-reference reader reads /Prev: an indirect value is
        // ignored, not chased.
        assert_eq!(dict.direct_int(names::PREV), None);
        // How the stream reader reads /Length: chased.
        assert_eq!(dict.int(names::LENGTH, &store), Some(99));
    }

    #[test]
    fn resolution_stops_after_one_level() {
        let store =
            TestStore::from_pairs([(1, Object::Ref(ObjRef::new(2, 0))), (2, Object::Int(7))]);
        let dict = Dict::from_pairs([(key("K"), Object::Ref(ObjRef::new(1, 0)))]);
        // Object 1's body is itself a reference; the value reads as absent.
        assert_eq!(dict.int(&key("K"), &store), None);
        assert_eq!(dict.number(&key("K"), &store), None);
        // ...but the resolution itself succeeded and produced that reference.
        assert_eq!(
            dict.get(&key("K"), &store).as_deref(),
            Some(&Object::Ref(ObjRef::new(2, 0)))
        );
    }

    #[test]
    fn dangling_references_read_as_absent() {
        let store = TestStore::default();
        let dict = Dict::from_pairs([(key("K"), Object::Ref(ObjRef::new(9, 0)))]);
        assert!(dict.get(&key("K"), &store).is_none());
        assert_eq!(dict.int(&key("K"), &store), None);
        assert_eq!(dict.dict(&key("K"), &store), None);
    }

    // From cpdf_object_unittest.cpp:271-288 (GetDict): a stream answers with
    // its own dictionary, directly or through a reference.
    #[test]
    fn dict_accessor_accepts_a_stream() {
        let inner = Dict::from_pairs([(names::LENGTH.clone(), Object::Int(3))]);
        let stream = Stream::new(inner.clone(), b"abc".to_vec().into());
        let store = TestStore::from_pairs([(5, Object::Stream(stream))]);
        let dict = Dict::from_pairs([(key("S"), Object::Ref(ObjRef::new(5, 0)))]);

        assert_eq!(dict.dict(&key("S"), &store), Some(inner));
        assert!(dict.stream(&key("S"), &store).is_some());
        assert_eq!(dict.array(&key("S"), &store), None);
    }

    // From cpdf_object_unittest.cpp:471-507.
    #[test]
    fn rect_and_matrix_need_exactly_the_right_element_count() {
        let four = Object::Array(Array::of([
            Object::Int(1),
            Object::Int(2),
            Object::Int(3),
            Object::Int(4),
        ]));
        let three = Object::Array(Array::of([Object::Int(1), Object::Int(2), Object::Int(3)]));
        let six = Object::Array(
            (1..=6)
                .map(|i| Object::Int(i64::from(i)))
                .collect::<Array>(),
        );

        let dict = Dict::from_pairs([
            (key("four"), four),
            (key("three"), three),
            (key("six"), six),
        ]);

        assert_eq!(
            dict.rect(&key("four"), &NoResolve),
            Rect::new(1.0, 2.0, 3.0, 4.0)
        );
        assert_eq!(
            dict.rect(&key("three"), &NoResolve),
            Rect::new(0.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(
            dict.rect(&key("missing"), &NoResolve),
            Rect::new(0.0, 0.0, 0.0, 0.0)
        );

        assert_eq!(
            dict.matrix(&key("six"), &NoResolve),
            Affine::new([1.0, 2.0, 3.0, 4.0, 5.0, 6.0])
        );
        assert_eq!(dict.matrix(&key("four"), &NoResolve), Affine::IDENTITY);
        assert_eq!(dict.matrix(&key("missing"), &NoResolve), Affine::IDENTITY);
    }

    #[test]
    fn missing_keys_read_as_their_fallbacks_everywhere() {
        let dict = Dict::new();
        let absent = key("nope");
        assert!(dict.is_empty());
        assert!(!dict.contains_key(&absent));
        assert_eq!(dict.raw(&absent), None);
        assert_eq!(dict.int(&absent, &NoResolve), None);
        assert_eq!(dict.number(&absent, &NoResolve), None);
        assert_eq!(dict.name(&absent), None);
        assert_eq!(dict.bool(&absent), None);
        assert_eq!(dict.string(&absent), None);
        assert_eq!(dict.text(&absent, &NoResolve), None);
        assert_eq!(dict.reference(&absent), None);
    }

    #[test]
    fn parsed_nulls_are_stored_like_any_other_value() {
        let dict = Dict::from_pairs([(key("K"), Object::Null)]);
        assert!(dict.contains_key(&key("K")));
        assert_eq!(dict.raw(&key("K")), Some(&Object::Null));
        assert_eq!(dict.int(&key("K"), &NoResolve), None);
    }
}
