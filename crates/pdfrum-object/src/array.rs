//! Array objects (ISO 32000-1 §7.3.6) and their typed accessors.
//!
//! The resolution rules mirror [`Dict`](crate::Dict)'s exactly, index for
//! key: the scalar accessors do not resolve (though a numeric coercion still
//! delegates through a reference one level), the composite ones do, and the
//! type-filtered ones read a reference as absence. An out-of-range index is
//! never an error — it is the same absence a missing key is.

use pdfrum_common::kurbo::{Affine, Rect};

use crate::{Dict, Name, ObjRef, Object, PdfString, Resolve, Resolved, Stream};

/// A PDF array: an ordered sequence of objects.
///
/// # Streams as elements
///
/// ISO 32000-1 §7.3.8.1 forbids a *file* from writing a stream as a direct
/// array element, and the reader drops one found inline while parsing. That
/// is a **file-format** constraint, not an in-memory invariant, and this
/// type does not police it:
/// [`Object::clone_direct`](crate::Object::clone_direct) flattens
/// references, so an array of indirect streams clones into one holding those
/// streams directly, and [`Array::stream_at`] reads such an element back.
/// Enforcing §7.3.8.1 is the **writer's** job: `pdfrum-edit` hoists a direct
/// stream to an indirect object at serialization time.
///
/// ```
/// use pdfrum_object::{Array, NoResolve, Object};
///
/// let a = Array::of([Object::Int(8902), Object::Name("address".into())]);
/// assert_eq!(a.int_at(0), Some(8902));
/// assert_eq!(a.name_at(1).and_then(|n| n.as_str()), Some("address"));
/// // Out of range is absence, never a panic.
/// assert_eq!(a.int_at(99), None);
/// ```
// The inline-stream drop while parsing is `cpdf_syntax_parser.cpp:591-596`.
// `CPDF_Dictionary::CloneNonCyclic` produces the same flattened shape, since
// its loop writes straight into `map_` and bypasses the `CHECK(!IsStream())`
// that guards the ordinary setters; `Array::stream_at` mirrors
// `CPDF_Array::GetStreamAt` in reading it back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Array(Vec<Object>);

impl Array {
    /// An empty array.
    #[must_use]
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// An array of these values, in order. The counterpart of
    /// [`Dict::from_pairs`].
    #[must_use]
    pub fn of(values: impl IntoIterator<Item = Object>) -> Self {
        values.into_iter().collect()
    }

    /// Append an element.
    ///
    /// Any object, a stream included — see the type-level note on §7.3.8.1.
    pub fn push(&mut self, value: Object) {
        self.0.push(value);
    }

    /// Inserts `value` at `index`, shifting later items; `index == len()`
    /// appends.
    ///
    /// # Panics
    ///
    /// When `index > len()`.
    pub fn insert(&mut self, index: usize, value: Object) {
        self.0.insert(index, value);
    }

    /// Removes and returns the item at `index`; `None` when out of range.
    pub fn remove(&mut self, index: usize) -> Option<Object> {
        (index < self.0.len()).then(|| self.0.remove(index))
    }

    /// Number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the array is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The elements, in order.
    pub fn iter(&self) -> impl Iterator<Item = &Object> {
        self.0.iter()
    }

    /// The elements as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[Object] {
        &self.0
    }

    // ---- non-resolving accessors ----

    /// The element at `index`, whatever its type, without resolving.
    #[must_use]
    pub fn raw_at(&self, index: usize) -> Option<&Object> {
        self.0.get(index)
    }

    /// The integer value at `index` in the C-integer view, coercing any type
    /// that has one.
    #[must_use]
    pub fn int_at(&self, index: usize) -> Option<i64> {
        self.raw_at(index)?.as_int()
    }

    /// The numeric value at `index`, coercing integers to `f32`.
    #[must_use]
    pub fn number_at(&self, index: usize) -> Option<f32> {
        self.raw_at(index)?.number()
    }

    /// The numeric value at `index`, or 0.0 when it is missing or not a
    /// number. The fallback [`Array::as_rect`] and [`Array::as_matrix`] use.
    #[must_use]
    pub fn number_at_or_zero(&self, index: usize) -> f32 {
        self.number_at(index).unwrap_or(0.0)
    }

    /// The value of a `Boolean`-typed element. An `Int(1)` reads as absent.
    #[must_use]
    pub fn bool_at(&self, index: usize) -> Option<bool> {
        self.raw_at(index)?.as_bool()
    }

    /// The name at `index`, only for an actual name.
    #[must_use]
    pub fn name_at(&self, index: usize) -> Option<&Name> {
        self.raw_at(index)?.as_name()
    }

    /// The string at `index`, only for an actual string.
    #[must_use]
    pub fn string_at(&self, index: usize) -> Option<&PdfString> {
        self.raw_at(index)?.as_string()
    }

    /// A `Number`-typed element as an object, without resolving.
    ///
    /// This is how a cross-reference stream's `/Index` is validated: an
    /// indirect number there is *skipped*, not chased.
    #[must_use]
    pub fn number_obj_at(&self, index: usize) -> Option<&Object> {
        self.raw_at(index)?.as_number()
    }

    /// The byte-string spelling at `index` — see [`Object::to_byte_string`].
    #[must_use]
    pub fn byte_string_at(&self, index: usize) -> Option<Vec<u8>> {
        Some(self.raw_at(index)?.to_byte_string())
    }

    /// The element at `index` read as text — see [`Object::to_text`].
    #[must_use]
    pub fn text_at(&self, index: usize) -> Option<String> {
        Some(self.raw_at(index)?.to_text())
    }

    /// The reference at `index`, without resolving it.
    #[must_use]
    pub fn reference_at(&self, index: usize) -> Option<ObjRef> {
        self.raw_at(index)?.as_ref_id()
    }

    // ---- resolving accessors ----

    /// The element at `index`, following one level of indirection.
    #[must_use]
    pub fn get<'a>(&'a self, index: usize, r: &impl Resolve) -> Option<Resolved<'a>> {
        self.raw_at(index)?.resolve(r).ok()
    }

    /// The dictionary at `index`, following one level of indirection. A
    /// stream answers with its own dictionary.
    #[must_use]
    pub fn dict_at(&self, index: usize, r: &impl Resolve) -> Option<Dict> {
        self.get(index, r)?.as_direct()?.as_dict().cloned()
    }

    /// The array at `index`, following one level of indirection.
    #[must_use]
    pub fn array_at(&self, index: usize, r: &impl Resolve) -> Option<Array> {
        self.get(index, r)?.as_direct()?.as_array().cloned()
    }

    /// The stream at `index`, following one level of indirection.
    #[must_use]
    pub fn stream_at(&self, index: usize, r: &impl Resolve) -> Option<Stream> {
        self.get(index, r)?.as_direct()?.as_stream().cloned()
    }

    // ---- geometry ----

    /// The array read as a rectangle: exactly four elements, or the zero
    /// rectangle.
    ///
    /// The PDF order is left, bottom, right, top, mapping onto kurbo's
    /// `(x0, y0, x1, y1)` in that order. The result is **not** normalized:
    /// files write inverted boxes and consumers that care normalize
    /// themselves.
    ///
    /// ```
    /// use pdfrum_object::{Array, Object};
    /// use pdfrum_common::kurbo::Rect;
    ///
    /// let media_box = Array::of([0, 0, 612, 792].map(Object::from));
    /// assert_eq!(media_box.as_rect(), Rect::new(0.0, 0.0, 612.0, 792.0));
    /// ```
    #[must_use]
    pub fn as_rect(&self) -> Rect {
        if self.len() != 4 {
            return Rect::new(0.0, 0.0, 0.0, 0.0);
        }
        Rect::new(
            f64::from(self.number_at_or_zero(0)),
            f64::from(self.number_at_or_zero(1)),
            f64::from(self.number_at_or_zero(2)),
            f64::from(self.number_at_or_zero(3)),
        )
    }

    /// The array read as a transformation matrix: exactly six elements
    /// `[a b c d e f]`, or the identity.
    #[must_use]
    pub fn as_matrix(&self) -> Affine {
        if self.len() != 6 {
            return Affine::IDENTITY;
        }
        Affine::new([
            f64::from(self.number_at_or_zero(0)),
            f64::from(self.number_at_or_zero(1)),
            f64::from(self.number_at_or_zero(2)),
            f64::from(self.number_at_or_zero(3)),
            f64::from(self.number_at_or_zero(4)),
            f64::from(self.number_at_or_zero(5)),
        ])
    }

    /// The numbers in the array, missing or non-numeric elements reading as
    /// 0.0. Used wherever the specification says "an array of `n` numbers".
    #[must_use]
    pub fn to_numbers(&self) -> Vec<f32> {
        (0..self.len()).map(|i| self.number_at_or_zero(i)).collect()
    }
}

impl FromIterator<Object> for Array {
    fn from_iter<I: IntoIterator<Item = Object>>(iter: I) -> Self {
        let mut array = Self::new();
        for value in iter {
            array.push(value);
        }
        array
    }
}

impl<'a> IntoIterator for &'a Array {
    type Item = &'a Object;
    type IntoIter = std::slice::Iter<'a, Object>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "these assertions pin exact bit patterns the oracle produces"
)]
mod tests {
    use pdfrum_common::kurbo::{Affine, Rect};

    use super::Array;
    use crate::test_resolve::TestStore;
    use crate::{Dict, Name, NoResolve, ObjRef, Object, PdfString, Stream, names};

    // From cpdf_array_unittest.cpp:18-37.
    #[test]
    fn boolean_accessor_rejects_integers() {
        let a = Array::of([
            Object::Bool(true),
            Object::Bool(false),
            Object::Int(0),
            Object::Int(1),
        ]);
        assert_eq!(a.bool_at(0), Some(true));
        assert_eq!(a.bool_at(1), Some(false));
        assert_eq!(a.bool_at(2), None);
        assert_eq!(a.bool_at(3), None);
        assert_eq!(a.bool_at(100), None);
    }

    #[test]
    fn out_of_range_is_absence_everywhere() {
        let a = Array::of([Object::Int(1)]);
        assert_eq!(a.raw_at(9), None);
        assert_eq!(a.int_at(9), None);
        assert_eq!(a.number_at(9), None);
        assert_eq!(a.number_at_or_zero(9), 0.0);
        assert_eq!(a.name_at(9), None);
        assert_eq!(a.string_at(9), None);
        assert_eq!(a.byte_string_at(9), None);
        assert_eq!(a.text_at(9), None);
        assert_eq!(a.reference_at(9), None);
        assert!(a.dict_at(9, &NoResolve).is_none());
        assert!(a.array_at(9, &NoResolve).is_none());
        assert!(a.stream_at(9, &NoResolve).is_none());
    }

    #[test]
    fn scalar_accessors_do_not_resolve_but_composite_ones_do() {
        let inner = Dict::from_pairs([(names::TYPE.clone(), Object::Name(names::PAGE.clone()))]);
        let store = TestStore::from_pairs([
            (1, Object::Int(42)),
            (2, Object::Dict(inner.clone())),
            (3, Object::Array(Array::of([Object::Int(1)]))),
            (
                4,
                Object::Stream(Box::new(Stream::new(inner.clone(), b"xyz".to_vec().into()))),
            ),
        ]);
        let a = Array::of([
            Object::Ref(ObjRef::new(1, 0)),
            Object::Ref(ObjRef::new(2, 0)),
            Object::Ref(ObjRef::new(3, 0)),
            Object::Ref(ObjRef::new(4, 0)),
        ]);

        // A reference is not a name and never will be.
        assert_eq!(a.name_at(0), None);
        assert_eq!(a.bool_at(0), None);
        // An indirect number in /Index is skipped, not chased.
        assert_eq!(a.number_obj_at(0), None);

        assert_eq!(a.dict_at(1, &store), Some(inner.clone()));
        assert_eq!(a.array_at(2, &store).map(|x| x.len()), Some(1));
        assert!(a.stream_at(3, &store).is_some());
        // A stream answers the dictionary accessor with its own dictionary.
        assert_eq!(a.dict_at(3, &store), Some(inner));
    }

    // From cpdf_object_unittest.cpp:471-507.
    #[test]
    fn rect_and_matrix_need_exactly_the_right_element_count() {
        let numbers = |n: usize| {
            (1..=n)
                .map(|i| Object::Real(f32::from(u8::try_from(i).unwrap_or(0))))
                .collect::<Array>()
        };
        assert_eq!(numbers(4).as_rect(), Rect::new(1.0, 2.0, 3.0, 4.0));
        assert_eq!(numbers(3).as_rect(), Rect::new(0.0, 0.0, 0.0, 0.0));
        assert_eq!(numbers(5).as_rect(), Rect::new(0.0, 0.0, 0.0, 0.0));
        assert_eq!(Array::new().as_rect(), Rect::new(0.0, 0.0, 0.0, 0.0));

        assert_eq!(
            numbers(6).as_matrix(),
            Affine::new([1.0, 2.0, 3.0, 4.0, 5.0, 6.0])
        );
        assert_eq!(numbers(5).as_matrix(), Affine::IDENTITY);
        assert_eq!(numbers(7).as_matrix(), Affine::IDENTITY);
    }

    #[test]
    fn rect_is_not_normalized() {
        // A file that writes its box inverted keeps it inverted.
        let inverted = Array::of([
            Object::Int(612),
            Object::Int(792),
            Object::Int(0),
            Object::Int(0),
        ]);
        assert_eq!(inverted.as_rect(), Rect::new(612.0, 792.0, 0.0, 0.0));
    }

    #[test]
    fn malformed_geometry_elements_read_as_zero() {
        let a = Array::of([
            Object::Int(1),
            Object::Name(Name::from("nope")),
            Object::Null,
            Object::Real(4.0),
        ]);
        assert_eq!(a.as_rect(), Rect::new(1.0, 0.0, 0.0, 4.0));
        assert_eq!(a.to_numbers(), [1.0, 0.0, 0.0, 4.0]);
    }

    // From cpdf_object_unittest.cpp:210-236, restated over an array.
    #[test]
    fn byte_string_spelling_per_element_type() {
        let a = Array::of([
            Object::Bool(false),
            Object::Bool(true),
            Object::Int(1245),
            Object::Real(9.003_45),
            Object::Str(PdfString::literal(b"A simple test")),
            Object::Name(Name::from("space")),
            Object::Array(Array::new()),
            Object::Dict(Dict::new()),
            Object::Null,
        ]);
        let spellings: Vec<Vec<u8>> = (0..a.len())
            .map(|i| a.byte_string_at(i).unwrap_or_default())
            .collect();
        assert_eq!(
            spellings,
            [
                &b"false"[..],
                b"true",
                b"1245",
                b"9.00345",
                b"A simple test",
                b"space",
                b"",
                b"",
                b"",
            ]
        );
    }

    // From cpdf_array_unittest.cpp:207-241: PDFium's Find/Contains compare
    // resolved pointer identity; over values, structural equality is the
    // equivalent question and the one callers can actually ask.
    #[test]
    fn membership_is_structural() {
        let a = Array::of([Object::Int(1), Object::Name(Name::from("x"))]);
        assert!(a.iter().any(|o| o == &Object::Int(1)));
        assert!(!a.iter().any(|o| o == &Object::Int(2)));
        assert_eq!(a.as_slice().len(), 2);
    }

    #[test]
    fn array_insert_shifts_and_appends_at_len() {
        let mut a = Array::new();
        a.push(Object::Int(1));
        a.push(Object::Int(3));
        a.insert(1, Object::Int(2));
        assert_eq!(
            a.as_slice(),
            [Object::Int(1), Object::Int(2), Object::Int(3)]
        );
        a.insert(3, Object::Int(4));
        assert_eq!(a.raw_at(3), Some(&Object::Int(4)));
    }

    #[test]
    fn array_remove_out_of_range_is_none() {
        let mut a = Array::of([Object::Int(1), Object::Int(2)]);
        assert_eq!(a.remove(5), None);
        assert_eq!(a.remove(0), Some(Object::Int(1)));
        assert_eq!(a.as_slice(), [Object::Int(2)]);
    }
}
