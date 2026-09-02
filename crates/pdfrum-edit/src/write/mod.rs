//! Writing a document out (ISO 32000-1 §7.5).
//!
//! [`save`] is one straight-line function calling six steps. The C++ spells
//! the same sequence as a numbered stage machine driven by a resumable
//! `Continue()` loop — that machinery exists to support pausable saving
//! through its public API, a facility we do not offer, so the stage numbers
//! survive here only as the order the steps run in (divergence D2).
//!
//! ```text
//! header → old objects → new objects → encrypt dict → xref → trailer
//! ```
//!
//! # Full and incremental are one path, not two
//!
//! The difference is entirely in what each step does:
//!
//! | | full save | incremental save |
//! |---|---|---|
//! | header | `%PDF-1.N` + a binary comment | the original file, byte for byte |
//! | "new" objects | those the xref does not name, or names as free | *every* object in play |
//! | old objects | reachable objects, garbage-collected | none |
//! | cross-reference | a full table | a delta table, or a stream |
//! | trailer | no `/Prev` | `/Prev` naming the original's last section |
//!
//! Two conditions silently downgrade an incremental save to a full one, both
//! because appending would produce a file no reader could open: a **rebuilt**
//! cross-reference (there is no previous section to chain from) and a
//! **changed security key** (the appended objects would be keyed differently
//! from the bytes before them).
//!
//! # An encrypted document stays encrypted
//!
//! Objects reach the writer plaintext, because the parser deciphered them on
//! fetch. A save under a document's own security handler puts the cipher back
//! on with the same file key, so the saved file opens with the same password
//! (SPEC.md §11's M10 ruling; [`crate::encrypt`] holds the exemptions and the
//! initialisation-vector story). [`SaveOptions::remove_security`] is the
//! explicit opt-out, and turns the save into a plaintext rewrite with no
//! `/Encrypt` in the trailer.
//!
//! Two mechanics follow from `/Encrypt` having to be an indirect object
//! (ISO 32000-1 §7.6.1). A file that wrote it **inline** in the trailer has no
//! object number for it, so the writer promotes it to a fresh one past the
//! highest in play. And whichever number it ends up with, that object is the
//! one thing the encryptor never touches.
//!
//! # The garbage collection is the point
//!
//! A full save writes only what the trailer can still reach. Removing every
//! object from a page and regenerating its content really does produce a
//! smaller file, rather than one that still carries the images nothing points
//! at any more.

mod header;
pub mod id;
pub mod object;
pub mod reach;
pub mod stream;
pub mod trailer;
pub mod xref;

use std::io::Write;

use pdfrum_common::PdfVersion;
use pdfrum_object::{ObjRef, Object, Resolve, names};

use crate::doc::EditDoc;
use crate::encrypt;
use crate::error::Error;
use crate::write::id::{IdContext, IdSource};
use crate::write::xref::ObjectOffsets;

pub use header::write_header;

/// How a document is written back out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SaveMode {
    /// Rewrite the whole file, dropping anything nothing points at.
    #[default]
    Full,
    /// Append the changes after the original bytes, leaving them untouched.
    ///
    /// Downgraded to [`SaveMode::Full`] when the document's cross-reference
    /// was rebuilt or its security key changed; see the module docs.
    Incremental,
}

/// Everything a save may be asked to do differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveOptions {
    /// Whether to append or rewrite.
    pub mode: SaveMode,
    /// Keep the original bytes as the file's prefix. Only an incremental save
    /// reads this; clearing it there turns the save into a rewrite that keeps
    /// the appended shape.
    pub keep_original: bool,
    /// Drop the security handler and `/Encrypt`, writing the document in the
    /// clear.
    ///
    /// Off by default: an encrypted document saves encrypted under its own
    /// handler, and opens with the password it was opened with. Setting this
    /// is the explicit way to decrypt one on the way out — and it forces a
    /// full save, since plaintext cannot be appended behind ciphertext.
    ///
    /// Has no effect on an unencrypted document.
    pub remove_security: bool,
    /// Subset newly embedded fonts.
    pub subset_new_fonts: bool,
    /// The version to declare in the header. 1.0 through 1.7 are honoured;
    /// anything outside that range, and `None`, keep the document's own.
    ///
    /// Was `Option<u8>` in the `major × 10 + minor` packing
    /// (`docs/design/idiomatic-api.md` §WP1).
    pub version: Option<PdfVersion>,
    /// Where `/ID` and subset tags come from.
    pub id_source: IdSource,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self {
            mode: SaveMode::Full,
            keep_original: true,
            remove_security: false,
            subset_new_fonts: false,
            version: None,
            id_source: IdSource::Random,
        }
    }
}

/// A sink that remembers how many bytes have gone through it.
///
/// Every cross-reference offset is a byte count from the start of the output,
/// so the writer needs a running total. This is the C++'s buffered archive
/// minus its hand-rolled 32 KiB buffer — buffering is the caller's choice,
/// through a `BufWriter`.
struct Counting<W: Write> {
    inner: W,
    written: u64,
}

impl<W: Write> Counting<W> {
    fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.inner.write_all(bytes)?;
        self.written = self.written.saturating_add(bytes.len() as u64);
        Ok(())
    }

    /// The offset the next byte will land at.
    const fn offset(&self) -> u64 {
        self.written
    }
}

/// Write `doc` to `out`.
///
/// # Errors
///
/// [`Error::EncryptedSaveUnsupported`] when the document declares `/Encrypt`
/// but this reader never derived a key for it — an `/Identity` crypt filter,
/// or a handler we opened as [`pdfrum_crypt::SecurityHandler::Identity`] —
/// and `remove_security` was not set, because re-declaring a cipher over
/// plaintext would produce a file nothing could open. And [`Error::Io`] when
/// the sink refuses the bytes.
///
/// Damage in the input is not an error: an object that cannot be fetched is
/// dropped from both the body and the cross-reference, exactly as the C++
/// writer drops it.
pub fn save(doc: &EditDoc<'_>, opts: &SaveOptions, out: &mut impl Write) -> Result<(), Error> {
    let base = doc.base();

    // The document's own handler, when the save is to stay encrypted. A
    // `/Encrypt` we could not key — `/Identity`, or a filter this reader
    // answered with the identity handler — would be re-declared over
    // plaintext, which is the one shape that opens for nobody.
    let declared = base.encrypt_dict().is_some();
    let handler = base.security_handler();
    let keep_security = declared && !opts.remove_security;
    if keep_security && matches!(handler, pdfrum_crypt::SecurityHandler::Identity) {
        return Err(Error::EncryptedSaveUnsupported);
    }

    let id = id::build(
        IdContext {
            old: None,
            encrypt: base.encrypt_dict().map(|(d, _)| d),
            incremental: opts.mode == SaveMode::Incremental,
        }
        .with_old(base.trailer()),
        opts.id_source,
    );

    // Three things force a full save. A rebuilt cross-reference has no
    // previous section to name in `/Prev`; a rekey makes the original bytes
    // unreadable under the new key; and removing security means the appended
    // objects would be plaintext behind ciphertext.
    let forced_full = base.xref_was_rebuilt() || id.rekeyed || (declared && opts.remove_security);
    let incremental = opts.mode == SaveMode::Incremental && !forced_full;

    // ---- the security seam ----
    //
    // The number the `/Encrypt` dictionary will be written as decides two
    // things at once: which object the encryptor skips, and which one the
    // body loops leave to the dedicated stage below.
    let slot = keep_security.then(|| encrypt_slot(doc, base)).flatten();
    let encrypt_number = slot.as_ref().map(|s| s.number);
    let security = keep_security.then(|| encrypt::Security {
        handler,
        ivs: encrypt::IvSource::from_document(base.bytes()),
        encrypt_object: encrypt_number,
    });

    let mut sink = Counting::new(out);
    let mut offsets = ObjectOffsets::new();

    // ---- header, or the original bytes ----
    let mut body = Vec::new();
    if incremental && opts.keep_original {
        // The original is copied verbatim; nothing in it is ever rewritten.
        // That append discipline is what keeps signatures and byte-range
        // digests valid (invariant R7).
        sink.write(base.bytes())?;
    } else {
        write_header(&mut body, opts.version, base.version());
        sink.write(&body)?;
        body.clear();
    }

    // ---- partition ----
    let (old_nums, new_nums) = partition(doc, incremental);

    // ---- old objects, garbage-collected ----
    let reach = reach::walk(base.trailer(), base.trailer_object_number(), doc);
    for num in old_nums {
        // A full save keeps only what the trailer can still reach.
        if !reach.is_reachable(num) || encrypt_number == Some(num) {
            continue;
        }
        write_one(&mut sink, &mut offsets, doc, num, security.as_ref())?;
    }

    // ---- new objects, written whether or not anything points at them ----
    let mut new_nums = new_nums;
    for num in new_nums.iter().copied() {
        // A newly added object is written even when nothing references it:
        // the caller added it on purpose, and the sweep above cannot see an
        // intent that has not been wired up yet.
        if encrypt_number == Some(num) {
            continue;
        }
        write_one(&mut sink, &mut offsets, doc, num, security.as_ref())?;
    }

    // ---- the encrypt dictionary ----
    //
    // Written here rather than by the loops above, whether the file held it
    // inline or indirectly, for a reason that is not about encryption at all:
    // it must be written **from the plaintext copy the trailer lookup found**,
    // not from the object store. The store deciphers every string it hands
    // out and has no exemption for this object, so fetching `/Encrypt`
    // through it yields `/O` and `/U` run through a cipher keyed by the very
    // material they carry. The C++ never has to think about this — it keeps
    // the dictionary in a field beside the handler and writes that.
    //
    // A file that wrote the dictionary inline additionally needs the fresh
    // object number `encrypt_slot` minted, since ISO 32000-1 §7.6.1 requires
    // the trailer name it by reference.
    if let Some(EncryptSlot { number, dict }) = &slot {
        offsets.set(*number, sink.offset());
        let mut bytes = Vec::new();
        // And no encryptor, which is the rule ISO 32000-1 §7.6.1 states: a
        // reader parses this dictionary before it has a key.
        object::write_indirect(&mut bytes, *number, &Object::Dict(dict.clone()), None);
        sink.write(&bytes)?;
        if incremental && !new_nums.contains(number) {
            // Appended without re-sorting. Safe for a promoted dictionary
            // because its number is above everything already there, and for
            // an indirect one because the guard above kept it out.
            new_nums.push(*number);
        }
    }

    let last_written = offsets.last();

    // ---- cross-reference ----
    let xref_start = sink.offset();
    let as_stream = incremental && base.main_xref_is_stream();
    if !as_stream {
        let mut table = Vec::new();
        if incremental {
            let written: Vec<u32> = new_nums
                .iter()
                .copied()
                .filter(|n| offsets.contains(*n))
                .collect();
            xref::classic_delta(&mut table, &offsets, &written);
        } else {
            xref::classic_full(&mut table, &offsets, last_written);
        }
        sink.write(&table)?;
    }

    // ---- trailer ----
    let dict = trailer::build(trailer::TrailerParts {
        source: base.trailer(),
        id: &id.array,
        last_object_number: last_written,
        prev: (incremental && base.last_xref_offset() > 0).then(|| base.last_xref_offset()),
        encrypt: slot.as_ref().map(|s| s.number),
    });

    let mut tail = Vec::new();
    if as_stream {
        let written: Vec<u32> = new_nums
            .iter()
            .copied()
            .filter(|n| offsets.contains(*n))
            .collect();
        // The trailer object's own number comes from the document, not from
        // the highest object written, so it can sit above `/Size − 2`.
        let num = doc.last_object_number().saturating_add(1);
        trailer::write_stream(&mut tail, num, &dict, &offsets, &written);
    } else {
        trailer::write_classic(&mut tail, &dict);
    }
    trailer::write_tail(&mut tail, xref_start);
    sink.write(&tail)?;

    Ok(())
}

/// Split the objects in play into the ones written from the file's own table
/// and the ones written as additions.
///
/// A **full** save treats an object as new when the cross-reference does not
/// name it, or names its slot free. Everything else is old — even an object
/// the caller replaced, because the old path re-fetches through the overlay
/// and so sees the replacement anyway.
///
/// An **incremental** save treats every object in play as new, because the
/// appended section must carry a fresh copy of anything that changed. That is
/// why an incremental save's size grows with how much of the document has
/// been touched.
fn partition(doc: &EditDoc<'_>, incremental: bool) -> (Vec<u32>, Vec<u32>) {
    let base = doc.base();
    let xref = base.xref();

    if incremental {
        let mut new: Vec<u32> = doc.edited().map(|(n, _)| n).collect();
        new.sort_unstable();
        new.dedup();
        return (Vec::new(), new);
    }

    let last = xref.last_object_number();
    let old: Vec<u32> = (1..=last)
        .filter(|n| !doc.is_removed(*n))
        .filter(|n| !matches!(xref.entry(*n), None | Some(pdfrum_parser::Entry::Free)))
        .collect();

    let mut new: Vec<u32> = doc
        .edited()
        .map(|(n, _)| n)
        .filter(|n| {
            !xref.is_valid_object_number(*n)
                || matches!(xref.entry(*n), None | Some(pdfrum_parser::Entry::Free))
        })
        .collect();
    new.sort_unstable();
    new.dedup();
    (old, new)
}

/// Where the `/Encrypt` dictionary goes on this save, and what to write there.
///
/// `dict` is the **plaintext** dictionary, taken from the trailer lookup that
/// reads it through a store deciphering nothing — see the writing stage for
/// why fetching it the ordinary way would corrupt it.
#[derive(Debug, Clone)]
struct EncryptSlot {
    /// The object number the trailer's `/Encrypt` will point at.
    number: u32,
    /// The dictionary to write there.
    dict: pdfrum_object::Dict,
}

/// Decide the `/Encrypt` dictionary's object number for this save.
///
/// A trailer naming it by reference already answers the question. One holding
/// it inline does not, so the number is minted one past everything in play —
/// which is what makes the incremental append-without-sorting sound, and what
/// ISO 32000-1 §7.6.1 requires, since the trailer must name it by reference.
///
/// `None` when the trailer's `/Encrypt` is neither a dictionary nor a
/// reference: there is no dictionary to point at, so the save writes no
/// `/Encrypt`, and `save`'s plaintext check has already refused the one shape
/// where that would produce an unopenable file.
fn encrypt_slot(doc: &EditDoc<'_>, base: &pdfrum_parser::Document) -> Option<EncryptSlot> {
    let (dict, inline) = base.encrypt_dict()?;
    let number = if inline {
        doc.last_object_number().saturating_add(1)
    } else {
        base.trailer().reference(names::ENCRYPT)?.num
    };
    Some(EncryptSlot {
        number,
        dict: dict.clone(),
    })
}

/// Write one indirect object, recording where it landed.
///
/// The offset is recorded **before** the fetch and erased if the fetch fails,
/// so a broken object vanishes from the body and the cross-reference together
/// rather than leaving a table entry pointing at the next object's header.
fn write_one<W: Write>(
    sink: &mut Counting<W>,
    offsets: &mut ObjectOffsets,
    doc: &EditDoc<'_>,
    num: u32,
    security: Option<&encrypt::Security<'_>>,
) -> Result<(), Error> {
    offsets.set(num, sink.offset());
    let Ok(obj) = doc.fetch(ObjRef::new(num, 0)) else {
        offsets.erase(num);
        return Ok(());
    };
    // A null carries no information a reader needs; the C++ writes it, but a
    // free slot reads identically and costs nothing.
    if obj.is_null() {
        offsets.erase(num);
        return Ok(());
    }

    // `for_object` is what refuses the `/Encrypt` dictionary its encryptor,
    // so the rule lives in one place rather than at every call site.
    let enc = security.and_then(|s| s.for_object(num));
    let mut bytes = Vec::new();
    object::write_indirect(&mut bytes, num, &obj, enc.as_ref());
    sink.write(&bytes)
}

impl<'a> IdContext<'a> {
    /// Fill in the trailer's own `/ID`, when it has one.
    fn with_old(mut self, trailer: &'a pdfrum_object::Dict) -> Self {
        self.old = match trailer.raw(names::ID) {
            Some(Object::Array(a)) => Some(a),
            _ => None,
        };
        self
    }
}
