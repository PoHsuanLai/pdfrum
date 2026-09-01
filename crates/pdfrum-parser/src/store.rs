//! The lazy object store: the one thing that turns a reference into an
//! object.
//!
//! # Lazy, cached, and cycle-safe
//!
//! Objects are parsed the first time something asks for them and kept
//! forever after. Three guards make that safe on files designed to be
//! hostile:
//!
//! - An object whose fetch is already in progress resolves to nothing rather
//!   than recursing. This is what makes a self-referential `/Length` — a
//!   stream whose length lives in an object inside that same stream — end as
//!   a missing length instead of a stack overflow.
//! - An object number past the cross-reference table's end is refused, even
//!   when the bytes for it sit in the file. The table is the only index the
//!   reader trusts.
//! - The object header at a cross-reference offset must carry the number the
//!   table claimed. A mismatch means the table describes a version of the
//!   file that no longer exists, and the fetch fails rather than returning
//!   the wrong object.
//!
//! # Failures are not cached
//!
//! A fetch that fails leaves the slot empty, so a later fetch tries again.
//! That costs a re-parse on a broken object, and it is deliberate: whether an
//! object resolves can change as the reader learns more about the file, and a
//! negative cache would freeze the first answer.
//!
//! # Decryption happens here
//!
//! An encrypted document's strings and stream payloads are ciphertext until
//! the object holding them is fetched, and the key depends on the *enclosing
//! indirect object's* number, which only this layer knows. So the walk that
//! rewrites them lives here, including the rule that a signature
//! dictionary's `/Contents` is left alone — a detached signature covers the
//! raw bytes, and decrypting them would destroy it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_crypt::{CryptClass, SecurityHandler};
use pdfrum_object::{Array, Dict, Name, ObjRef, Object, PdfString, Resolve, Stream, names};

use crate::error::Error;
use crate::lexer::Lexer;
use crate::objstm::ObjStm;
use crate::syntax::{Context, Strictness, indirect};
use crate::xref::{Entry, Xref};

/// Everything needed to turn a reference into an object.
///
/// `Sync` on purpose: rendering pages in parallel means fetching objects in
/// parallel, so the caches are behind locks and the parsed objects are shared
/// immutably.
#[derive(Debug)]
pub struct ObjectStore {
    /// The file, from its header onwards.
    bytes: Arc<[u8]>,
    /// Where every object lives.
    xref: Xref,
    /// Caps.
    limits: Limits,
    /// Parsed objects, one slot per object number. The slot is created on
    /// first request and filled once.
    /// Keyed with [`pdfrum_common::FxBuildHasher`], not `std`'s `SipHash`: the
    /// key is an **object number**, a `u32` this crate assigns from the
    /// cross-reference table, and every resolve of every indirect reference in
    /// a document goes through this map. A file cannot choose the key — it can
    /// choose how many there are, which the `Limits` cap governs — so the
    /// collision resistance `SipHash` is paying for is not resistance to
    /// anything. See `pdfrum_common::FxBuildHasher`'s docs and
    /// `docs/status/M12.md`.
    cells: Mutex<Cells>,
    /// Object numbers whose parse is running right now.
    in_progress: Mutex<Vec<u32>>,
    /// Decoded object streams, keyed by their own object number.
    containers: Mutex<Containers>,
    /// How the document's strings and streams are encrypted.
    security: SecurityHandler,
    /// The object holding `/Root/Metadata` when it is exempt from
    /// decryption, which `/EncryptMetadata false` makes it.
    metadata_exempt: Option<u32>,
    /// Repairs recorded during fetches.
    diags: Mutex<Diagnostics>,
}

/// The parsed-object slots, keyed by object number.
///
/// A named type because the shape is three layers deep and reads badly inline:
/// an `Arc<OnceLock<_>>` per slot is what lets one thread create the slot and
/// another wait on the parse without holding the map's lock across it.
type Cells = HashMap<u32, Arc<OnceLock<Arc<Object>>>, pdfrum_common::FxBuildHasher>;

/// Decoded object streams, keyed by their own object number.
///
/// `None` records a container that was tried and could not be decoded, so a
/// broken object stream is parsed once rather than once per object in it.
type Containers = HashMap<u32, Option<Arc<ObjStm>>, pdfrum_common::FxBuildHasher>;

impl ObjectStore {
    /// Build a store over a file and its cross-reference table.
    pub(crate) fn new(
        bytes: Arc<[u8]>,
        xref: Xref,
        limits: Limits,
        security: SecurityHandler,
    ) -> Self {
        Self {
            bytes,
            xref,
            limits,
            cells: Mutex::new(HashMap::default()),
            in_progress: Mutex::new(Vec::new()),
            containers: Mutex::new(HashMap::default()),
            security,
            metadata_exempt: None,
            diags: Mutex::new(Diagnostics::default()),
        }
    }

    /// Exempt one object from decryption, for the metadata stream a document
    /// declared unencrypted.
    pub(crate) fn exempt_from_decryption(&mut self, num: u32) {
        self.metadata_exempt = Some(num);
    }

    /// The cross-reference table.
    pub(crate) fn xref(&self) -> &Xref {
        &self.xref
    }

    /// The caps in force.
    pub(crate) fn limits(&self) -> &Limits {
        &self.limits
    }

    /// How the document is encrypted.
    pub(crate) fn security(&self) -> &SecurityHandler {
        &self.security
    }

    /// Take the repairs recorded so far, leaving the sink empty.
    pub(crate) fn drain_diags(&self) -> Diagnostics {
        match self.diags.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(_) => Diagnostics::default(),
        }
    }

    /// The repairs recorded so far, *without* emptying the sink.
    ///
    /// The read behind [`Document::lazy_diagnostics`](crate::Document::lazy_diagnostics):
    /// a caller asking what the document has needed so far must be able to
    /// ask twice and get the same answer, which draining would break.
    pub(crate) fn peek_diags(&self) -> Diagnostics {
        match self.diags.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => Diagnostics::default(),
        }
    }

    /// Record a repair.
    fn note(&self, severity: Severity, what: DiagKind, at: Option<u64>) {
        if let Ok(mut guard) = self.diags.lock() {
            guard.record(severity, what, at);
        }
    }

    /// Run `f` with a diagnostics sink, folding what it recorded back in.
    fn with_diags<T>(&self, f: impl FnOnce(&mut Diagnostics) -> T) -> T {
        let mut local = Diagnostics::default();
        let out = f(&mut local);
        if let Ok(mut guard) = self.diags.lock() {
            guard.extend(&local);
        }
        out
    }

    /// Fetch an object, parsing it if this is the first request.
    ///
    /// # Errors
    ///
    /// [`Error::Unresolved`] when the table has nothing usable for this
    /// number, and [`Error::Cycle`] when the fetch re-entered one already
    /// running.
    pub fn get(&self, num: u32) -> Result<Arc<Object>, Error> {
        let reference = ObjRef::new(num, self.xref.generation(num));
        if num == 0 || num == ObjRef::INVALID_NUM {
            return Err(Error::Unresolved(reference));
        }
        // Numbers past the table's end name nothing, whatever the file holds.
        if !self.xref.is_valid_object_number(num) {
            return Err(Error::Unresolved(reference));
        }

        let cell = self.cell(num);
        if let Some(object) = cell.get() {
            return Ok(Arc::clone(object));
        }

        let guard = Guard::enter(self, num).ok_or(Error::Cycle(reference))?;
        // Another thread may have filled the slot while we waited.
        if let Some(object) = cell.get() {
            return Ok(Arc::clone(object));
        }

        // Every fetch already in flight is an object this parse is nested
        // inside, so the nesting budget continues from there rather than
        // restarting — an object reachable only through sixty-four layers of
        // `/Length` references is as unreachable as one nested that deep.
        let object = self
            .parse(num, guard.nesting())
            .ok_or(Error::Unresolved(reference))?;
        let object = Arc::new(object);
        // A race here is harmless: both values parse the same bytes.
        let _ = cell.set(Arc::clone(&object));
        Ok(cell.get().map_or(object, Arc::clone))
    }

    /// The slot for an object number, creating it if needed.
    fn cell(&self, num: u32) -> Arc<OnceLock<Arc<Object>>> {
        match self.cells.lock() {
            Ok(mut cells) => Arc::clone(cells.entry(num).or_default()),
            Err(_) => Arc::new(OnceLock::new()),
        }
    }

    /// Parse an object from wherever the table says it lives.
    ///
    /// `depth` is the nesting the *triggering* parse had already spent, so a
    /// fetch reached from inside a deep object continues that budget rather
    /// than getting a fresh one.
    fn parse(&self, num: u32, depth: u32) -> Option<Object> {
        match self.xref.entry(num)? {
            Entry::Offset(pos) if pos > 0 => self.parse_at(num, pos, depth),
            // A free slot, and an in-use one at offset zero — which is the
            // header, where no object can start.
            Entry::Free | Entry::Offset(_) => None,
            Entry::InObjStream { stream, index } => {
                self.parse_member(num, stream.num, index, depth)
            }
        }
    }

    /// Parse the indirect object at a byte offset.
    ///
    /// The header's object number is checked against the one asked for: a
    /// table pointing at the wrong bytes is the failure mode this catches.
    fn parse_at(&self, num: u32, pos: u64, depth: u32) -> Option<Object> {
        let pos = usize::try_from(pos).ok()?;
        if pos >= self.bytes.len() {
            return None;
        }
        let parsed = self.with_diags(|diags| {
            let mut ctx = Context {
                limits: &self.limits,
                diags,
                file: Some(&self.bytes),
                store: Some(self),
            };
            let mut lx = Lexer::at(&self.bytes, pos);
            indirect(&mut lx, &mut ctx, Strictness::Loose, depth).ok()
        })?;

        if parsed.num != num {
            self.note(
                Severity::Suspicious,
                DiagKind::ObjNumMismatch,
                Some(pos as u64),
            );
            return None;
        }
        Some(self.decrypted(ObjRef::new(parsed.num, parsed.generation), parsed.object))
    }

    /// Parse a member of an object stream.
    fn parse_member(&self, num: u32, archive: u32, index: u32, depth: u32) -> Option<Object> {
        let container = self.container(archive)?;
        // Members are already plaintext: the container was decrypted whole.
        self.with_diags(|diags| container.member(num, index, &self.limits, diags, self, depth))
    }

    /// The decoded object stream with this number, decoding it once.
    fn container(&self, archive: u32) -> Option<Arc<ObjStm>> {
        // Only an object something named as a container may be used as one.
        if !self.xref.is_object_stream(archive) {
            return None;
        }
        if let Ok(cache) = self.containers.lock()
            && let Some(hit) = cache.get(&archive)
        {
            return hit.clone();
        }

        let built = self.build_container(archive);
        if let Ok(mut cache) = self.containers.lock() {
            cache.insert(archive, built.clone());
        }
        built
    }

    /// Decode and index the object stream with this number.
    fn build_container(&self, archive: u32) -> Option<Arc<ObjStm>> {
        let Object::Stream(stream) = &*self.get(archive).ok()? else {
            return None;
        };
        self.with_diags(|diags| ObjStm::build(stream, &self.limits, diags, self).map(Arc::new))
    }

    /// Rewrite an object's strings and stream payloads as plaintext.
    ///
    /// A no-op for an unencrypted document, and for the metadata object a
    /// document declared exempt.
    fn decrypted(&self, obj: ObjRef, object: Object) -> Object {
        if matches!(self.security, SecurityHandler::Identity)
            || self.metadata_exempt == Some(obj.num)
        {
            return object;
        }
        let mut deferred = Vec::new();
        let out = decrypt_node(&self.security, obj, object, &mut deferred, false);
        // A `/Contents` under a dictionary carrying `/Type` or `/FT` was left
        // alone because those keys were ciphertext at the time. Now that the
        // parent is readable, the ones that are not signatures get decrypted
        // after all.
        resolve_deferred(&self.security, obj, out, &deferred)
    }
}

impl Resolve for ObjectStore {
    fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, pdfrum_object::Error> {
        self.get(r.num).map_err(Into::into)
    }
}

/// Marks an object number as being parsed, and unmarks it on the way out.
struct Guard<'s> {
    store: &'s ObjectStore,
    num: u32,
    /// How many fetches were already in flight when this one started.
    nesting: u32,
}

impl<'s> Guard<'s> {
    /// Claim `num`, or `None` when a fetch of it is already running.
    fn enter(store: &'s ObjectStore, num: u32) -> Option<Self> {
        let mut running = store.in_progress.lock().ok()?;
        if running.contains(&num) {
            return None;
        }
        let nesting = u32::try_from(running.len()).unwrap_or(u32::MAX);
        running.push(num);
        drop(running);
        Some(Self {
            store,
            num,
            nesting,
        })
    }

    /// How deep the parse this guard protects is nested.
    fn nesting(&self) -> u32 {
        self.nesting
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if let Ok(mut running) = self.store.in_progress.lock() {
            running.retain(|&n| n != self.num);
        }
    }
}

/// Where a deferred `/Contents` sits, as a path of keys and indices from the
/// object's root.
#[derive(Debug, Clone)]
struct Deferred {
    /// The dictionary that held it, already decrypted.
    parent: Dict,
    /// How to reach the value again.
    path: Vec<Step>,
}

/// One step of a path into an object.
#[derive(Debug, Clone)]
enum Step {
    /// Into a dictionary, by key.
    Key(Name),
    /// Into an array, by index.
    Index(usize),
}

/// Rewrite every string and stream payload as plaintext.
///
/// `deferred` collects the `/Contents` values that were skipped; `in_sig`
/// marks a subtree already known to be one, so nothing inside it is touched.
fn decrypt_node(
    handler: &SecurityHandler,
    obj: ObjRef,
    object: Object,
    deferred: &mut Vec<Deferred>,
    in_sig: bool,
) -> Object {
    match object {
        Object::Str(s) => {
            if in_sig {
                Object::Str(s)
            } else {
                let plain = handler.decrypt(obj, CryptClass::String, &s.bytes);
                Object::Str(PdfString::new(plain, syntax_of(&s)))
            }
        }
        Object::Array(a) => Object::Array(
            a.iter()
                .map(|v| decrypt_node(handler, obj, v.clone(), deferred, in_sig))
                .collect(),
        ),
        Object::Dict(d) => Object::Dict(decrypt_dict(handler, obj, &d, deferred, in_sig, &[])),
        Object::Stream(s) => {
            let dict = decrypt_dict(handler, obj, &s.dict, deferred, in_sig, &[]);
            // An embedded file stream is its own crypt-filter class
            // (ISO 32000-1 §7.6.5), and `/EFF` may name a different cipher
            // from `/StmF`'s. The class is read off `/Type` before the
            // decrypt, which is safe because a name is never enciphered.
            // Where `/EFF` is absent or names the stream filter — every file
            // in the corpus — the two classes are the same call.
            let class = if s.dict.name(names::TYPE) == Some(names::EMBEDDED_FILE) {
                CryptClass::Embedded
            } else {
                CryptClass::Stream
            };
            let plain = handler.decrypt(obj, class, &s.data);
            Object::Stream(Stream::new(dict, pdfrum_object::ByteSpan::from(plain)))
        }
        other => other,
    }
}

/// Rewrite a dictionary, deferring the `/Contents` of anything that might be
/// a signature.
fn decrypt_dict(
    handler: &SecurityHandler,
    obj: ObjRef,
    dict: &Dict,
    deferred: &mut Vec<Deferred>,
    in_sig: bool,
    path: &[Step],
) -> Dict {
    // A dictionary carrying either key *might* be a signature — but both are
    // still ciphertext, so the question cannot be settled yet.
    let suspicious = dict.contains_key(names::TYPE) || dict.contains_key(names::FT);
    let mut out = Dict::new();
    let mut skipped: Vec<(Name, Object)> = Vec::new();

    for (key, value) in dict.iter() {
        if suspicious && key == names::CONTENTS && !in_sig {
            skipped.push((key.clone(), value.clone()));
            out.push(key.clone(), value.clone());
            continue;
        }
        let mut child = path.to_vec();
        child.push(Step::Key(key.clone()));
        out.push(
            key.clone(),
            decrypt_value(handler, obj, value.clone(), deferred, in_sig, &child),
        );
    }

    for (key, _) in skipped {
        let mut child = path.to_vec();
        child.push(Step::Key(key));
        deferred.push(Deferred {
            parent: out.clone(),
            path: child,
        });
    }
    out
}

/// Rewrite one value, keeping arrays' paths so a deferral inside one can be
/// found again.
fn decrypt_value(
    handler: &SecurityHandler,
    obj: ObjRef,
    value: Object,
    deferred: &mut Vec<Deferred>,
    in_sig: bool,
    path: &[Step],
) -> Object {
    match value {
        Object::Dict(d) => Object::Dict(decrypt_dict(handler, obj, &d, deferred, in_sig, path)),
        Object::Array(a) => {
            let mut out = Array::new();
            for (i, v) in a.iter().enumerate() {
                let mut child = path.to_vec();
                child.push(Step::Index(i));
                out.push(decrypt_value(
                    handler,
                    obj,
                    v.clone(),
                    deferred,
                    in_sig,
                    &child,
                ));
            }
            Object::Array(out)
        }
        other => decrypt_node(handler, obj, other, deferred, in_sig),
    }
}

/// Decrypt the deferred `/Contents` values whose parents turned out not to be
/// signature dictionaries.
fn resolve_deferred(
    handler: &SecurityHandler,
    obj: ObjRef,
    object: Object,
    deferred: &[Deferred],
) -> Object {
    let mut out = object;
    for entry in deferred {
        if pdfrum_crypt::is_signature_dict(&entry.parent) {
            // A real signature: its `/Contents` covers the raw bytes and
            // must stay exactly as written.
            continue;
        }
        out = rewrite_at(handler, obj, out, &entry.path);
    }
    out
}

/// Decrypt the value at `path`, leaving everything else alone.
fn rewrite_at(handler: &SecurityHandler, obj: ObjRef, object: Object, path: &[Step]) -> Object {
    let Some((step, rest)) = path.split_first() else {
        let mut ignored = Vec::new();
        return decrypt_node(handler, obj, object, &mut ignored, false);
    };
    match (object, step) {
        (Object::Dict(d), Step::Key(key)) => {
            Object::Dict(Dict::from_pairs(d.iter().map(|(k, v)| {
                if k == key {
                    (k.clone(), rewrite_at(handler, obj, v.clone(), rest))
                } else {
                    (k.clone(), v.clone())
                }
            })))
        }
        (Object::Stream(s), Step::Key(key)) => {
            let dict = Dict::from_pairs(s.dict.iter().map(|(k, v)| {
                if k == key {
                    (k.clone(), rewrite_at(handler, obj, v.clone(), rest))
                } else {
                    (k.clone(), v.clone())
                }
            }));
            Object::Stream(Stream::new(dict, s.data))
        }
        (Object::Array(a), Step::Index(index)) => Object::Array(
            a.iter()
                .enumerate()
                .map(|(i, v)| {
                    if i == *index {
                        rewrite_at(handler, obj, v.clone(), rest)
                    } else {
                        v.clone()
                    }
                })
                .collect(),
        ),
        (other, _) => other,
    }
}

/// Keep a string's source spelling across the rewrite, since the writer
/// round-trips it.
fn syntax_of(s: &PdfString) -> pdfrum_object::StringSyntax {
    if s.hex {
        pdfrum_object::StringSyntax::Hex
    } else {
        pdfrum_object::StringSyntax::Literal
    }
}

#[cfg(test)]
mod tests {
    use super::ObjectStore;
    use crate::error::Error;
    use crate::xref::Xref;
    use pdfrum_common::Limits;
    use pdfrum_crypt::SecurityHandler;
    use pdfrum_object::{ObjRef, Resolve, names};
    use std::sync::Arc;

    fn store(file: &[u8], build: impl FnOnce(&mut Xref)) -> ObjectStore {
        let mut xref = Xref::new();
        build(&mut xref);
        ObjectStore::new(
            Arc::from(file),
            xref,
            Limits::default(),
            SecurityHandler::Identity,
        )
    }

    #[test]
    fn fetches_and_caches() {
        let file = b"%PDF-1.7\n1 0 obj << /Type /Page >> endobj\n";
        let s = store(file, |x| {
            x.add_normal(1, 0, false, 9, &Limits::default());
        });
        let first = s.get(1).expect("object");
        assert!(first.as_dict().is_some());
        // The same Arc comes back.
        let second = s.get(1).expect("object");
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn nested_fetches_share_one_nesting_budget() {
        // Each object is a stream whose `/Length` lives in the next object,
        // so reading object 1 forces a fetch of 2, which forces 3, and so on.
        // These fetches are genuinely nested — each is suspended while the
        // next runs — and the budget is spent across the whole chain rather
        // than reset per object, so the deepest links are unreachable.
        let count: u32 = 80;
        let mut file = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for i in 1..=count {
            offsets.push(file.len());
            if i == count {
                file.extend_from_slice(format!("{i} 0 obj 4 endobj\n").as_bytes());
            } else {
                file.extend_from_slice(
                    format!(
                        "{i} 0 obj << /Length {} 0 R >> stream\nDATA\nendstream endobj\n",
                        i + 1
                    )
                    .as_bytes(),
                );
            }
        }
        let s = store(&file, |x| {
            for (i, offset) in offsets.iter().enumerate() {
                let num = u32::try_from(i).unwrap_or(0) + 1;
                x.add_normal(num, 0, false, *offset as u64, &Limits::default());
            }
        });

        // The outermost object still reads: a `/Length` that cannot be
        // resolved falls back to scanning for `endstream`, so exhausting the
        // budget costs accuracy on the innermost links, not the whole read.
        let first = s.get(1).expect("object 1");
        assert_eq!(&*first.as_stream().expect("stream").data, b"DATA");
    }

    #[test]
    fn object_zero_never_resolves() {
        let s = store(b"", |x| {
            x.add_normal(1, 0, false, 0, &Limits::default());
        });
        assert!(matches!(s.get(0), Err(Error::Unresolved(_))));
    }

    #[test]
    fn numbers_past_the_table_are_unfetchable() {
        let file = b"%PDF-1.7\n1 0 obj 5 endobj\n9 0 obj 7 endobj\n";
        let s = store(file, |x| {
            x.add_normal(1, 0, false, 9, &Limits::default());
        });
        // Object 9's bytes are right there, but the table ends at 1.
        assert!(matches!(s.get(9), Err(Error::Unresolved(_))));
    }

    #[test]
    fn a_free_entry_resolves_to_nothing() {
        let s = store(b"1 0 obj 5 endobj", |x| {
            x.add_normal(2, 0, false, 0, &Limits::default());
            x.set_free(1, 1);
        });
        assert!(matches!(s.get(1), Err(Error::Unresolved(_))));
    }

    #[test]
    fn a_header_naming_another_object_fails_the_fetch() {
        let file = b"%PDF-1.7\n7 0 obj << >> endobj\n";
        let s = store(file, |x| {
            // The table says object 1 lives at the offset where 7 does.
            x.add_normal(1, 0, false, 9, &Limits::default());
        });
        assert!(matches!(s.get(1), Err(Error::Unresolved(_))));
        assert!(
            s.drain_diags()
                .contains(&pdfrum_common::DiagKind::ObjNumMismatch)
        );
    }

    #[test]
    fn a_self_referential_length_ends_as_a_keyword_scan() {
        // Object 1's /Length points at object 1.
        let file = b"%PDF-1.7\n1 0 obj << /Length 1 0 R >> stream\nDATA\nendstream endobj\n";
        let s = store(file, |x| {
            x.add_normal(1, 0, false, 9, &Limits::default());
        });
        let obj = s.get(1).expect("object");
        assert_eq!(&*obj.as_stream().expect("stream").data, b"DATA");
    }

    #[test]
    fn a_failed_fetch_is_retried_rather_than_remembered() {
        let file = b"%PDF-1.7\nnot an object\n";
        let s = store(file, |x| {
            x.add_normal(1, 0, false, 9, &Limits::default());
        });
        assert!(s.get(1).is_err());
        assert!(s.get(1).is_err());
    }

    #[test]
    fn resolving_reads_through_the_trait() {
        let file = b"%PDF-1.7\n1 0 obj << /Count 4 >> endobj\n";
        let s = store(file, |x| {
            x.add_normal(1, 0, false, 9, &Limits::default());
        });
        let fetched = s.fetch(ObjRef::new(1, 0)).expect("object");
        assert_eq!(
            fetched.as_dict().and_then(|d| d.direct_int(names::COUNT)),
            Some(4)
        );
    }

    #[test]
    fn the_store_is_send_and_sync() {
        fn assert_both<T: Send + Sync>() {}
        assert_both::<ObjectStore>();
    }
}
