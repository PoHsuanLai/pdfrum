//! File specifications (ISO 32000-1 §7.11): naming a file inside or outside
//! the document.
//!
//! Five keys can hold a name and they are tried in a fixed precedence —
//! `/UF`, `/F`, `/DOS`, `/Mac`, `/Unix` — with two details that matter:
//!
//! - Each is **type-filtered to a string**, so a `/UF` written as a *name*
//!   contributes nothing. That filter is the fix for a real security bug: a
//!   name-valued `/UF /http://evil.org` used to read back as text.
//! - `/UF` decodes as PDF text (byte-order mark aware); every other key, and
//!   a bare string file specification, decodes as **Latin-1**. Two different
//!   functions, deliberately not unified.

use pdfrum_object::{Dict, Name, Object, Resolve, Stream, decode_text};

use crate::names;

/// A file specification: either a bare string or a dictionary of names.
#[derive(Debug, Clone, PartialEq)]
pub struct FileSpec {
    /// The object the specification was read from.
    pub object: Object,
}

/// The keys a file name can live under, in precedence order.
const NAME_KEYS: [&Name; 5] = [names::UF, names::F, names::DOS, names::MAC, names::UNIX];

impl FileSpec {
    /// Wraps an object as a file specification.
    ///
    /// ```
    /// use pdfrum_doc::FileSpec;
    /// use pdfrum_object::{NoResolve, Object, PdfString};
    ///
    /// let spec = FileSpec::new(Object::Str(PdfString::literal(b"report.pdf")));
    /// assert_eq!(spec.file_name(&NoResolve), "report.pdf");
    /// ```
    #[must_use]
    pub fn new(object: Object) -> FileSpec {
        FileSpec { object }
    }

    /// The file's name.
    ///
    /// An object that is neither a string nor a dictionary — a *name*, for
    /// instance — has no file name at all.
    ///
    /// ```
    /// use pdfrum_doc::FileSpec;
    /// use pdfrum_object::{Dict, Name, NoResolve, Object, PdfString};
    ///
    /// // `/UF` outranks `/F`.
    /// let dict = Dict::from_pairs([
    ///     (Name::from("F"), Object::Str(PdfString::literal(b"old.pdf"))),
    ///     (Name::from("UF"), Object::Str(PdfString::literal(b"new.pdf"))),
    /// ]);
    /// assert_eq!(FileSpec::new(Object::Dict(dict)).file_name(&NoResolve), "new.pdf");
    ///
    /// // Neither a string nor a dictionary: no file name at all.
    /// let named = FileSpec::new(Object::Name(Name::from("F")));
    /// assert_eq!(named.file_name(&NoResolve), "");
    /// ```
    #[must_use]
    pub fn file_name<R: Resolve>(&self, r: &R) -> String {
        match &self.object {
            Object::Str(text) => decode_file_name(&latin1(&text.bytes)),
            Object::Dict(dict) => decode_file_name(&dict_file_name(dict, r)),
            _ => String::new(),
        }
    }

    /// The embedded file's stream, when the specification carries one.
    ///
    /// The presence test runs on the **outer** dictionary's name keys while
    /// the stream comes from `/EF`, so a name with no matching `/EF` entry
    /// falls through to the next key rather than ending the search. A `/FS`
    /// of `URL` truncates the key list to `/UF` and `/F`.
    ///
    /// ```
    /// use pdfrum_doc::FileSpec;
    /// use pdfrum_object::{NoResolve, Object, PdfString};
    ///
    /// // A bare string names a file but embeds nothing.
    /// let spec = FileSpec::new(Object::Str(PdfString::literal(b"report.pdf")));
    /// assert!(spec.file_stream(&NoResolve).is_none());
    /// ```
    #[must_use]
    pub fn file_stream<R: Resolve>(&self, r: &R) -> Option<Stream> {
        let dict = self.object.as_dict()?;
        let embedded = dict.dict(names::EF, r)?;
        let keys = if is_url(dict, r) {
            &NAME_KEYS[..2]
        } else {
            &NAME_KEYS[..]
        };
        for key in keys {
            if dict.text(key, r).is_none_or(|text| text.is_empty()) {
                continue;
            }
            if let Some(stream) = embedded.stream(key, r) {
                return Some(stream);
            }
        }
        None
    }

    /// The embedded file stream's `/Params` dictionary.
    ///
    /// ```
    /// use pdfrum_doc::FileSpec;
    /// use pdfrum_object::{NoResolve, Object, PdfString};
    ///
    /// let spec = FileSpec::new(Object::Str(PdfString::literal(b"report.pdf")));
    /// assert!(spec.params(&NoResolve).is_none());
    /// ```
    #[must_use]
    pub fn params<R: Resolve>(&self, r: &R) -> Option<Dict> {
        self.file_stream(r)?.dict.dict(names::PARAMS, r)
    }
}

/// Whether the specification says its names are URLs.
fn is_url<R: Resolve>(dict: &Dict, r: &R) -> bool {
    dict.byte_string(names::FS, r).as_deref() == Some(b"URL")
}

/// Picks a name out of a dictionary file specification.
fn dict_file_name<R: Resolve>(dict: &Dict, r: &R) -> String {
    let as_string = |key: &Name| {
        dict.get(key, r)
            .and_then(|v| v.get().as_string().map(|s| s.bytes.to_vec()))
    };

    // `/UF` is PDF text; everything else is Latin-1.
    let mut name = as_string(names::UF)
        .map(|bytes| decode_text(&bytes).into_owned())
        .unwrap_or_default();
    if name.is_empty() {
        name = as_string(names::F)
            .map(|bytes| latin1(&bytes))
            .unwrap_or_default();
    }
    // A URL short-circuits **before** the platform translation, but after the
    // `/UF` and `/F` reads.
    if is_url(dict, r) {
        return name;
    }
    if name.is_empty() {
        // The loop stops at the first key whose value is a string — even an
        // empty one — so a present-but-empty `/DOS` shadows `/Mac`.
        for key in &NAME_KEYS[2..] {
            if let Some(bytes) = as_string(key) {
                name = latin1(&bytes);
                break;
            }
        }
    }
    name
}

/// Byte-for-code-point decoding.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| char::from(*b)).collect()
}

/// Translates a file name from PDF's platform-independent form.
///
/// On this platform it is the identity: the whole slash-translation machinery
/// is compiled only for Windows and macOS. The rules for those platforms are
/// recorded in the design brief so a future port has them; they are not
/// reproduced here, and the Windows branch in particular reads past the end
/// of a one- or two-character path, which we would not reproduce even if it
/// were enabled.
///
/// ```
/// use pdfrum_doc::nav::decode_file_name;
///
/// // The identity on this platform.
/// assert_eq!(decode_file_name("dir/report.pdf"), "dir/report.pdf");
/// ```
#[must_use]
pub fn decode_file_name(name: &str) -> String {
    name.to_owned()
}

/// The inverse of [`decode_file_name`]; also the identity here.
///
/// ```
/// use pdfrum_doc::nav::encode_file_name;
///
/// assert_eq!(encode_file_name("dir/report.pdf"), "dir/report.pdf");
/// ```
#[must_use]
pub fn encode_file_name(name: &str) -> String {
    name.to_owned()
}

#[cfg(test)]
mod tests {
    use super::{FileSpec, decode_file_name, encode_file_name};
    use pdfrum_object::{ByteSpan, Dict, Name, NoResolve, Object, PdfString, Stream};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn spec(pairs: &[(&str, Object)]) -> FileSpec {
        FileSpec::new(Object::Dict(dict(pairs)))
    }

    fn text(bytes: &[u8]) -> Object {
        Object::Str(PdfString::literal(bytes))
    }

    #[test]
    fn the_round_trip_is_the_identity_on_this_platform() {
        for path in [
            "./docs/test.pdf",
            "../test_docs/test.pdf",
            "/usr/local/home/test.pdf",
            "",
            "test.pdf",
        ] {
            assert_eq!(decode_file_name(path), path);
            assert_eq!(encode_file_name(path), path);
        }
    }

    #[test]
    fn a_bare_string_specification_reads_as_latin_one() {
        let spec = FileSpec::new(text(b"caf\xE9.pdf"));
        assert_eq!(spec.file_name(&NoResolve), "café.pdf");
    }

    #[test]
    fn a_name_object_has_no_file_name_at_all() {
        let spec = FileSpec::new(Object::Name(Name::from("test.pdf")));
        assert_eq!(spec.file_name(&NoResolve), "");
    }

    #[test]
    fn precedence_runs_unicode_then_file_then_the_three_platform_keys() {
        // Set in reverse precedence; each addition takes over.
        let mut pairs = vec![("Unix", text(b"unix.pdf"))];
        assert_eq!(spec(&pairs).file_name(&NoResolve), "unix.pdf");
        pairs.insert(0, ("Mac", text(b"mac.pdf")));
        assert_eq!(spec(&pairs).file_name(&NoResolve), "mac.pdf");
        pairs.insert(0, ("DOS", text(b"dos.pdf")));
        assert_eq!(spec(&pairs).file_name(&NoResolve), "dos.pdf");
        pairs.insert(0, ("F", text(b"f.pdf")));
        assert_eq!(spec(&pairs).file_name(&NoResolve), "f.pdf");
        pairs.insert(0, ("UF", text(b"uf.pdf")));
        assert_eq!(spec(&pairs).file_name(&NoResolve), "uf.pdf");
    }

    #[test]
    fn a_name_valued_key_contributes_nothing() {
        // The crbug.com/959183 fix: a name-typed `/UF` used to read back as
        // its text.
        let evil = spec(&[
            ("UF", Object::Name(Name::from("http://evil.org"))),
            ("F", text(b"safe.pdf")),
        ]);
        assert_eq!(evil.file_name(&NoResolve), "safe.pdf");

        for key in ["Unix", "Mac", "DOS", "F", "UF"] {
            let only = spec(&[(key, Object::Name(Name::from("x.pdf")))]);
            assert_eq!(only.file_name(&NoResolve), "", "{key}");
        }
    }

    #[test]
    fn a_present_but_empty_platform_key_shadows_the_next_one() {
        let shadowed = spec(&[("DOS", text(b"")), ("Mac", text(b"mac.pdf"))]);
        assert_eq!(shadowed.file_name(&NoResolve), "");
    }

    #[test]
    fn a_url_specification_still_reads_its_unicode_name() {
        let url = spec(&[
            ("FS", Object::Name(Name::from("URL"))),
            ("UF", text(b"http://example.com/x.pdf")),
            ("DOS", text(b"ignored.pdf")),
        ]);
        assert_eq!(url.file_name(&NoResolve), "http://example.com/x.pdf");
    }

    #[test]
    fn a_stream_needs_both_a_name_and_a_matching_embedded_entry() {
        let stream = Object::Stream(Box::new(Stream::new(
            dict(&[("Params", Object::Dict(dict(&[("Size", Object::Int(6))])))]),
            ByteSpan::from(b"hello!".to_vec()),
        )));
        // A name with no `/EF` entry falls through rather than ending the
        // search.
        let spec = spec(&[
            ("F", text(b"f.pdf")),
            ("Unix", text(b"unix.pdf")),
            ("EF", Object::Dict(dict(&[("Unix", stream)]))),
        ]);
        assert!(spec.file_stream(&NoResolve).is_some());
        assert_eq!(
            spec.params(&NoResolve)
                .and_then(|p| p.int(pdfrum_object::names::SIZE, &NoResolve)),
            Some(6)
        );
    }

    #[test]
    fn no_embedded_files_dictionary_means_no_stream() {
        assert!(FileSpec::new(text(b"x")).file_stream(&NoResolve).is_none());
        assert!(spec(&[("F", text(b"f"))]).file_stream(&NoResolve).is_none());
        assert!(
            spec(&[("F", text(b"f")), ("EF", Object::Dict(Dict::new()))])
                .file_stream(&NoResolve)
                .is_none()
        );
    }
}
