//! Opening a document: the header, the load ladder, and the page tree.
//!
//! # The ladder
//!
//! [`load`] is a sequence of attempts, each one falling back to a cruder
//! repair. Find the header; read the cross-reference information, rebuilding
//! it from a full-file scan if the structured paths fail; set up decryption;
//! then check that the trailer names a catalog and that the catalog names at
//! least one page. If that last check fails the reader *throws away the
//! table it just built* and rebuilds anyway, because a table that yields no
//! pages is more likely stale than the file is empty.
//!
//! That retry is why the ladder is written as a ladder rather than a straight
//! line: the same steps run twice with different inputs, and which rung a
//! file lands on decides whether it opens at all.
//!
//! # `/Root` must be a reference
//!
//! A trailer whose `/Root` is a dictionary written inline is treated as
//! having no catalog, even though the dictionary is right there. It reads as
//! damage, and damage triggers the rebuild — which on real files finds a
//! better catalog. Honoring the inline dictionary would skip that.
//!
//! # Counting pages without walking them
//!
//! A `/Pages` node's `/Count` is believed whenever it is positive and below
//! the cap, without checking it against the tree. Files whose counts are
//! wrong therefore report the wrong number — and lookups past the real end
//! fail individually, which is exactly what a reader that trusted the walk
//! instead would not reproduce.

use std::sync::{Arc, Mutex};

use pdfrum_common::{
    DiagKind, Diagnostics, LimitExceeded, Limits, Operation, PageIndex, PdfVersion, Severity,
};
use pdfrum_crypt::{Permissions, SecurityHandler};
use pdfrum_object::{ByteSpan, Dict, NoResolve, ObjRef, Object, Resolve, names};

use crate::error::Error;
use crate::store::ObjectStore;
use crate::xref::{Trailer, Xref, XrefShape};

/// How many bytes a `%PDF-1.7\n` header occupies, and the least a file can be.
const HEADER_SIZE: usize = 9;

/// Why a document could not be opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LoadError {
    /// No `%PDF` header within the first kilobyte, or a file too short to
    /// hold one.
    #[error("not a PDF file")]
    NotPdf,

    /// The document is encrypted and the password given does not open it.
    /// Distinct from the others because callers ask again rather than give
    /// up.
    #[error("wrong password")]
    WrongPassword,

    /// The document uses a security handler this reader does not implement.
    #[error("unsupported encryption: {0}")]
    UnsupportedEncryption(String),

    /// The file is damaged past what recovery could repair: no usable
    /// cross-reference information, or no catalog with pages in it.
    #[error("damaged beyond recovery: {0}")]
    Broken(String),

    /// The caller's `Limits::deadline` had passed when the open began, or
    /// passed during the cross-reference rebuild scan.
    #[error(transparent)]
    Limit(LimitExceeded),
}

/// How to open a document.
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// The password to try, as raw bytes. Not capped in length.
    pub password: Option<Vec<u8>>,
    /// Caps to enforce while reading.
    pub limits: Limits,
}

/// One page's dictionary, with the attributes it inherits already resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct PageDict {
    /// The page's own dictionary.
    pub dict: Dict,
    /// The reference it was reached through, when it had one. A page written
    /// inline in its parent's `/Kids` has none.
    pub reference: Option<ObjRef>,
}

impl PageDict {
    /// An attribute of this page, looked up through its `/Parent` chain
    /// (ISO 32000-1 §7.7.3.4).
    ///
    /// `/Resources`, `/MediaBox`, `/CropBox` and `/Rotate` are inheritable:
    /// a page that does not state one takes its parent's, or its
    /// grandparent's. The walk stops at the first node that states the key
    /// *directly* — a value that is itself a reference is resolved, but a
    /// `/Parent` that is not a dictionary ends the chain.
    #[must_use]
    pub fn inherited(&self, key: &pdfrum_object::Name, r: &impl Resolve) -> Option<Object> {
        let mut node = self.dict.clone();
        let mut seen: Vec<Dict> = Vec::new();
        for _ in 0..64 {
            if let Some(value) = node.get(key, r) {
                return Some(value.get().clone());
            }
            // A cycle in the parent chain would otherwise spin forever.
            if seen.contains(&node) {
                return None;
            }
            seen.push(node.clone());
            node = node.dict(names::PARENT, r)?;
        }
        None
    }
}

/// An opened document.
///
/// Holds the file, everything the reader learned about where its objects are,
/// and the lazy store that turns references into objects. `Send + Sync`, so
/// pages can be rendered in parallel.
#[derive(Debug)]
pub struct Document {
    /// The file from its header onwards; every offset indexes into this.
    bytes: ByteSpan,
    /// The trailer, merged across every section that contributed one.
    trailer: Trailer,
    /// The object store.
    store: Arc<ObjectStore>,
    /// The version the header declared, or `None` when it declared none.
    version: Option<PdfVersion>,
    /// Where the header was found in the original file.
    header_offset: u64,
    /// The shape of the cross-reference the load used, for the writer.
    xref_shape: XrefShape,
    /// The `/Encrypt` dictionary as the file wrote it, and whether the
    /// trailer held it directly rather than by reference.
    encrypt: Option<(Dict, bool)>,
    /// How many pages the catalog says there are.
    page_count: u32,
    /// Page dictionaries found so far, by index.
    pages: Mutex<PageCache>,
    /// Everything repaired while opening the file.
    pub diags: Diagnostics,
}

/// The page lookup's memory. The cache, not the index.
#[derive(Debug, Default)]
struct PageCache {
    /// One slot per page, filled as pages are found.
    slots: Vec<Option<PageDict>>,
    /// Whether the tree turned out to be deeper than the cap, which stops
    /// every later lookup as well.
    poisoned: bool,
}

/// Open a document.
///
/// `bytes` is the whole file. Every repair the reader performed is in
/// [`Document::diags`] afterwards, and a document that opened with a rebuilt
/// table reports so through [`Document::xref_was_rebuilt`].
///
/// # Errors
///
/// [`LoadError::NotPdf`] for a file with no header, [`LoadError::WrongPassword`]
/// and [`LoadError::UnsupportedEncryption`] for encryption the password or
/// the reader cannot handle, and [`LoadError::Broken`] for damage recovery
/// could not repair.
///
/// ```
/// use std::sync::Arc;
/// use pdfrum_parser::{LoadError, LoadOptions, load};
///
/// let not_a_pdf: Arc<[u8]> = Arc::from(&b"just some bytes"[..]);
/// assert_eq!(load(not_a_pdf, &LoadOptions::default()).err(), Some(LoadError::NotPdf));
/// ```
pub fn load(bytes: impl Into<ByteSpan>, opts: &LoadOptions) -> Result<Document, LoadError> {
    opts.limits
        .check_deadline(Operation::Open)
        .map_err(LoadError::Limit)?;
    let mut diags = Diagnostics::default();
    let bytes: ByteSpan = bytes.into();
    let header_offset = find_header(&bytes, &opts.limits).ok_or(LoadError::NotPdf)?;
    if bytes.len() < header_offset.saturating_add(HEADER_SIZE) {
        return Err(LoadError::NotPdf);
    }
    if header_offset > 0 {
        diags.record(
            Severity::Recovered,
            DiagKind::HeaderOffset,
            Some(header_offset as u64),
        );
    }

    // Everything before the header is invisible: offsets in the file are
    // relative to it, so the reader works on the slice from there on.
    //
    // A window, not a copy: junk before `%PDF` costs a refcount bump like
    // any other offset. Every stream is then a window into this one.
    let body = bytes
        .subspan(header_offset..bytes.len())
        .unwrap_or_else(|_| ByteSpan::empty());
    let version = read_version(&body);

    let (xref, mut trailer, mut xref_shape) =
        crate::xref::read_xref_full(&body, &opts.limits, &mut diags).map_err(|e| match e {
            crate::Error::Limit(limit) => LoadError::Limit(limit),
            other => LoadError::Broken(other.to_string()),
        })?;

    // First attempt: the catalog has to be reachable *and* have pages in it.
    // Shared from here on: the stores below only read the table, and a slot
    // vector is expensive to copy — see `ObjectStore`'s `xref` field. The
    // rebuild path below re-shares after it mutates.
    let mut shared_xref = Arc::new(xref);
    let mut security = build_security(&body, &shared_xref, &trailer.dict, opts, &mut diags)?;
    let mut store = build_store(&body, &shared_xref, opts, &trailer.dict, security);
    let mut page_count = catalog_page_count(&store, &trailer.dict, &opts.limits);

    if page_count.is_none() {
        if xref_shape.rebuilt {
            return Err(LoadError::Broken("no document catalog".into()));
        }
        // The table is the suspect, not the file: scan it and try again.
        diags.record(Severity::Recovered, DiagKind::RootRecovered, None);
        let mut fresh = Xref::new();
        let mut fresh_trailer = Trailer::default();
        let rebuilt = crate::xref::rebuild(
            &body,
            &mut fresh,
            &mut fresh_trailer,
            &opts.limits,
            &mut diags,
            &NoResolve,
        )
        .map_err(LoadError::Limit)?;
        if !rebuilt {
            return Err(LoadError::Broken("no document catalog".into()));
        }
        // The recovery path, so the copy this makes is not the common case:
        // the first attempt's store still holds a reference, and the merged
        // table has to be a fresh value for the second attempt's stores to
        // share in turn.
        let mut merged = (*shared_xref).clone();
        merged.merge_up(&fresh);
        crate::xref::merge_trailers(&mut trailer, &fresh_trailer);
        xref_shape = XrefShape::rebuilt();

        shared_xref = Arc::new(merged);
        security = build_security(&body, &shared_xref, &trailer.dict, opts, &mut diags)?;
        store = build_store(&body, &shared_xref, opts, &trailer.dict, security);
        // Second attempt asks only for a catalog. A rebuilt document whose
        // catalog is reachable but describes no pages still opens — it is
        // then a document of zero pages, which is a thing a file can be.
        if catalog(&store, &trailer.dict).is_none() {
            return Err(LoadError::Broken("no document catalog".into()));
        }
        page_count = catalog_page_count(&store, &trailer.dict, &opts.limits);
    }

    let page_count = page_count.unwrap_or(0);
    // Read last, from the trailer as it finally stands, so a rebuild that
    // replaced the trailer is reflected.
    let encrypt = encrypt_dict_located(&body, &shared_xref, &trailer.dict, &opts.limits);

    diags.extend(&store.drain_diags());

    Ok(Document {
        bytes: body,
        trailer,
        store,
        version,
        header_offset: header_offset as u64,
        xref_shape,
        encrypt,
        page_count,
        pages: Mutex::new(PageCache {
            slots: vec![None; usize::try_from(page_count).unwrap_or(0)],
            poisoned: false,
        }),
        diags,
    })
}

/// Find `%PDF` within the first `limits.header_scan` bytes.
fn find_header(bytes: &[u8], limits: &Limits) -> Option<usize> {
    let window = usize::try_from(limits.header_scan).unwrap_or(usize::MAX);
    let last = bytes.len().checked_sub(4)?.min(window);
    (0..=last).find(|&i| bytes.get(i..i + 4) == Some(b"%PDF"))
}

/// Read the version digits out of `%PDF-M.N`.
///
/// Never validated: a header claiming version 9.9 opens like any other, and a
/// non-digit contributes nothing.
///
/// This is the one place in the workspace that knows the `major × 10 + minor`
/// packing the old `Document::version() -> u8` published: the digits are read,
/// packed, and immediately unpacked into a [`PdfVersion`]. The round trip is
/// kept rather than removed because `0` — the answer when neither digit is
/// readable — is what distinguishes "no version declared" from version 0.0,
/// and collapsing the two would change which documents the writer gives its
/// 1.7 fallback to.
fn read_version(body: &[u8]) -> Option<PdfVersion> {
    let digit = |i: usize| -> u8 {
        body.get(i)
            .filter(|b| b.is_ascii_digit())
            .map_or(0, |b| b - b'0')
    };
    match digit(5).saturating_mul(10).saturating_add(digit(7)) {
        0 => None,
        packed => Some(PdfVersion::new(packed / 10, packed % 10)),
    }
}

/// Build the security handler the trailer's `/Encrypt` calls for.
fn build_security(
    body: &ByteSpan,
    xref: &Arc<Xref>,
    trailer: &Dict,
    opts: &LoadOptions,
    diags: &mut Diagnostics,
) -> Result<SecurityHandler, LoadError> {
    let Some(encrypt) = encrypt_dict(body, xref, trailer, &opts.limits) else {
        return Ok(SecurityHandler::Identity);
    };
    // The handler name is type-checked before being resolved, so a `/Filter`
    // written as a string is not the standard handler however it spells it.
    if encrypt.name(names::FILTER) != Some(names::STANDARD) {
        return Err(LoadError::UnsupportedEncryption(
            encrypt
                .name(names::FILTER)
                .map_or_else(|| "unnamed".to_owned(), |n| n.as_text().into_owned()),
        ));
    }

    let file_id = trailer
        .array(names::ID, &NoResolve)
        .and_then(|a| a.string_at(0).map(|s| s.bytes.to_vec()))
        .unwrap_or_default();
    let password = opts.password.clone().unwrap_or_default();

    match SecurityHandler::from_encrypt_dict(&encrypt, &file_id, &password, &NoResolve) {
        Ok(handler) => {
            if handler.password_encoding() != pdfrum_crypt::PasswordEncoding::AsGiven {
                diags.record(Severity::Recovered, DiagKind::PasswordReencoded, None);
            }
            Ok(handler)
        }
        Err(pdfrum_crypt::Error::WrongPassword) => Err(LoadError::WrongPassword),
        Err(pdfrum_crypt::Error::UnsupportedHandler(name)) => Err(
            LoadError::UnsupportedEncryption(String::from_utf8_lossy(&name).into_owned()),
        ),
        Err(e) => Err(LoadError::UnsupportedEncryption(e.to_string())),
    }
}

/// The `/Encrypt` dictionary, written inline or reached through one
/// reference.
///
/// Most files write it indirectly, so the reference has to be chased — but
/// the real store does not exist yet, and could not read this dictionary if
/// it did, since it would try to decrypt it with the key this dictionary
/// defines. So the lookup goes through a throwaway store that decrypts
/// nothing. That is not a shortcut: the encryption dictionary is the one
/// object in a document that is always plaintext.
fn encrypt_dict(
    body: &ByteSpan,
    xref: &Arc<Xref>,
    trailer: &Dict,
    limits: &Limits,
) -> Option<Dict> {
    encrypt_dict_located(body, xref, trailer, limits).map(|(d, _)| d)
}

/// The same lookup, additionally reporting whether the trailer held the
/// dictionary **directly** rather than by reference.
///
/// A writer needs that second fact: an inline encryption dictionary has no
/// object number of its own, so it must be promoted to a fresh indirect
/// object before the trailer's `/Encrypt` can name it (ISO 32000-1 §7.6.1
/// requires `/Encrypt` be indirect).
fn encrypt_dict_located(
    body: &ByteSpan,
    xref: &Arc<Xref>,
    trailer: &Dict,
    limits: &Limits,
) -> Option<(Dict, bool)> {
    match trailer.raw(names::ENCRYPT)? {
        Object::Dict(d) => Some((d.clone(), true)),
        Object::Ref(r) => {
            let plain = ObjectStore::new(
                body.clone(),
                Arc::clone(xref),
                limits.clone(),
                SecurityHandler::Identity,
            );
            Some((plain.get(r.num).ok()?.as_dict().cloned()?, false))
        }
        _ => None,
    }
}

/// Record the metadata object as exempt from decryption when the document
/// says its metadata is not encrypted.
fn exempt_metadata(store: &mut ObjectStore, trailer: &Dict) {
    if store.security().encrypt_metadata() {
        return;
    }
    let Some(root) = trailer.reference(names::ROOT) else {
        return;
    };
    let Ok(catalog) = store.get(root.num) else {
        return;
    };
    if let Some(metadata) = catalog.as_dict().and_then(|d| d.reference(names::METADATA)) {
        store.exempt_from_decryption(metadata.num);
    }
}

/// Build a store over the table, with the metadata exemption applied.
fn build_store(
    body: &ByteSpan,
    xref: &Arc<Xref>,
    opts: &LoadOptions,
    trailer: &Dict,
    security: SecurityHandler,
) -> Arc<ObjectStore> {
    let mut store = ObjectStore::new(
        body.clone(),
        Arc::clone(xref),
        opts.limits.clone(),
        security,
    );
    exempt_metadata(&mut store, trailer);
    Arc::new(store)
}

/// The document catalog, if the trailer names one reachably.
///
/// A `/Root` written as anything but a reference does not name a catalog,
/// even when it is a perfectly good dictionary written inline: that reads as
/// damage, and damage is what triggers the rebuild that finds a better one.
fn catalog(store: &ObjectStore, trailer: &Dict) -> Option<Dict> {
    let root = trailer.reference(names::ROOT)?;
    store.get(root.num).ok()?.as_dict().cloned()
}

/// How many pages the catalog claims, or `None` when there is no usable
/// catalog or it describes no pages at all.
fn catalog_page_count(store: &ObjectStore, trailer: &Dict, limits: &Limits) -> Option<u32> {
    let catalog = catalog(store, trailer)?;
    let count = page_count_of(store, &catalog, limits);
    (count > 0).then_some(count)
}

/// The number of pages under a catalog.
///
/// A catalog without `/Pages` has none. A `/Pages` node without `/Kids` is
/// itself the single page — that is not a repair, it is what a file with one
/// page and no tree means.
fn page_count_of(store: &ObjectStore, catalog: &Dict, limits: &Limits) -> u32 {
    let Some(pages) = catalog.dict(names::PAGES, store) else {
        return 0;
    };
    if pages.raw(names::KIDS).is_none() {
        return 1;
    }
    // The root counts as an ancestor from the start, so a kid pointing back
    // at it is a loop rather than a subtree.
    let mut ancestors = vec![pages.clone()];
    count_subtree(store, &pages, limits, &mut ancestors).unwrap_or(0)
}

/// Count the leaves under a node, or `None` when the tree claims more pages
/// than a document may have.
///
/// `/Count` is believed whenever it is positive and under the cap, without
/// checking it against the tree — so a file that lies about its length
/// reports the lie, and the individual lookups past its real end are what
/// fail.
///
/// The `None` propagates all the way out rather than being absorbed as a
/// zero: a subtree that overflows makes the *whole* document uncountable,
/// which is why this returns an `Option` instead of saturating.
///
/// `ancestors` holds the nodes currently being descended through, and is the
/// cycle guard — the only one, since there is no depth cap here. Note what it
/// is *not*: a record of every node already seen. A node listed twice among
/// one parent's `/Kids` is counted twice, because the second listing is a
/// sibling rather than a loop, and a file whose tree shares subtrees that way
/// really does have that many pages.
fn count_subtree(
    store: &ObjectStore,
    node: &Dict,
    limits: &Limits,
    ancestors: &mut Vec<Dict>,
) -> Option<u32> {
    if let Some(count) = node.int(names::COUNT, store)
        && count > 0
        && count < i64::from(limits.max_page_count)
        && let Ok(count) = u32::try_from(count)
    {
        return Some(count);
    }

    let Some(kids) = node.array(names::KIDS, store) else {
        return Some(0);
    };
    let mut total: u32 = 0;
    for kid in kids.iter() {
        let Some(kid) = kid.resolve(store).ok().and_then(|k| k.as_dict().cloned()) else {
            continue;
        };
        // Only a kid that is already an ancestor would loop.
        if ancestors.contains(&kid) {
            continue;
        }
        total = total.saturating_add(match node_kind(&kid) {
            NodeKind::Branch => {
                ancestors.push(kid.clone());
                let under = count_subtree(store, &kid, limits, ancestors);
                ancestors.pop();
                under?
            }
            NodeKind::Leaf => 1,
        });
        if total >= limits.max_page_count {
            return None;
        }
    }
    Some(total)
}

/// What a page-tree node is.
enum NodeKind {
    /// An interior node whose `/Kids` hold more nodes.
    Branch,
    /// A page.
    Leaf,
}

/// Classify a node, guessing when `/Type` does not say.
///
/// A node with `/Kids` is a branch and one without is a page, whatever its
/// `/Type` claims — files write the wrong type often enough that the
/// structure is the more reliable witness.
fn node_kind(node: &Dict) -> NodeKind {
    match node.name(names::TYPE) {
        Some(t) if t == names::PAGES => NodeKind::Branch,
        Some(t) if t == names::PAGE => NodeKind::Leaf,
        _ => {
            if node.contains_key(names::KIDS) {
                NodeKind::Branch
            } else {
                NodeKind::Leaf
            }
        }
    }
}

impl Document {
    /// How many pages the document has.
    ///
    /// A `u32`, deliberately, and not a
    /// [`PageIndex`](pdfrum_common::PageIndex): a count answers "how many"
    /// and an index answers "which one", and the last valid index of a
    /// three-page document is 2, not 3. Giving them one type would let each be
    /// passed where the other is meant, which is what the newtype exists to
    /// stop.
    #[must_use]
    pub fn page_count(&self) -> u32 {
        self.page_count
    }

    /// The page at `index`, counting from zero.
    ///
    /// The tree is walked in order and the pages found along the way are
    /// remembered, so reading a document front to back costs one traversal.
    /// A kid that will not load as a dictionary still **consumes its slot**:
    /// a missing page leaves a hole rather than shifting every page after it.
    ///
    /// Takes `impl Into<PageIndex>`, so `doc.page(0)` reads as it always has.
    ///
    /// # Errors
    ///
    /// [`Error::NoPage`] for an index past the count, or one the walk could
    /// not reach.
    pub fn page(&self, index: impl Into<PageIndex>) -> Result<PageDict, Error> {
        let index = index.into();
        let slot = usize::try_from(index.get()).unwrap_or(usize::MAX);
        if index.get() >= self.page_count {
            return Err(Error::NoPage(index));
        }
        let Ok(mut pages) = self.pages.lock() else {
            return Err(Error::NoPage(index));
        };
        if let Some(Some(found)) = pages.slots.get(slot) {
            return Ok(found.clone());
        }
        if pages.poisoned {
            return Err(Error::NoPage(index));
        }

        self.walk_pages(&mut pages);
        pages
            .slots
            .get(slot)
            .and_then(Clone::clone)
            .ok_or(Error::NoPage(index))
    }

    /// Walk the whole tree once, filling every slot it can reach.
    fn walk_pages(&self, pages: &mut PageCache) {
        let Some(root) = self.trailer.dict.reference(names::ROOT) else {
            return;
        };
        let Ok(catalog) = self.store.get(root.num) else {
            return;
        };
        let Some(node) = catalog
            .as_dict()
            .and_then(|d| d.dict(names::PAGES, &*self.store))
        else {
            return;
        };

        let mut next: usize = 0;
        let mut ancestors = Vec::new();
        let objref = catalog.as_dict().and_then(|d| d.reference(names::PAGES));
        self.visit(&node, objref, pages, &mut next, 0, &mut ancestors);
    }

    /// Depth-first, in order, filling slots as leaves are reached.
    ///
    /// `ancestors` is the cycle guard: the nodes on the path from the root to
    /// here. A node that reappears as a *sibling* is a second page, not a
    /// loop, so only an ancestor stops the descent.
    fn visit(
        &self,
        node: &Dict,
        reference: Option<ObjRef>,
        pages: &mut PageCache,
        next: &mut usize,
        depth: u32,
        ancestors: &mut Vec<Dict>,
    ) {
        if *next >= pages.slots.len() {
            return;
        }
        // A node without `/Kids` is where the walk stops, whatever it claims
        // to be — but a node that claims `/Type /Pages` and has no children
        // is describing a subtree that is not there, so no page comes of it.
        // (`page_count` still counts such a root as one page; the lookup is
        // what fails.)
        if node.raw(names::KIDS).is_none() {
            if matches!(node_kind(node), NodeKind::Branch) {
                return;
            }
            if let Some(slot) = pages.slots.get_mut(*next) {
                *slot = Some(PageDict {
                    dict: node.clone(),
                    reference,
                });
            }
            *next += 1;
            return;
        }

        // Only a node with children can be too deep, so the cap is checked
        // after the leaf case rather than on the way in. Exceeding it stops
        // every later lookup too, not just this one.
        if depth >= self.store.limits().max_page_tree_depth {
            // The oracle records the same fact in the same shape:
            // `reached_max_page_level_ = true` at `cpdf_document.cpp:281-283`,
            // after which its own later lookups fail too. `poisoned` is that
            // flag; the diagnostic is how a caller finds out *why* the pages
            // stopped resolving.
            self.store
                .note(Severity::Suspicious, DiagKind::PageTreeDepthExceeded, None);
            pages.poisoned = true;
            return;
        }

        let Some(kids) = node.array(names::KIDS, &*self.store) else {
            return;
        };
        ancestors.push(node.clone());
        for kid in kids.iter() {
            let kid_ref = kid.as_ref_id();
            let loaded = kid
                .resolve(&*self.store)
                .ok()
                .and_then(|k| k.as_dict().cloned());
            let Some(loaded) = loaded else {
                // A kid that will not load still costs a slot, so a missing
                // page leaves a hole rather than shifting every page after it.
                *next += 1;
                continue;
            };
            // Only a kid that is already an ancestor would loop; the same
            // node appearing twice as a sibling is two pages. PDFium skips a
            // kid it has already visited for the same reason
            // (`cpdf_document.cpp:87-88`), as part of the same pass that
            // rewrites a wrong `/Count` (`:111`) and guesses a missing `/Type`
            // (`:60`) — all of it "fix the in-memory representation for page
            // tree nodes that violate the spec".
            if ancestors.contains(&loaded) {
                self.store
                    .note(Severity::Recovered, DiagKind::PageTreeRepaired, None);
                continue;
            }
            self.visit(&loaded, kid_ref, pages, next, depth + 1, ancestors);
            if *next >= pages.slots.len() {
                break;
            }
        }
        ancestors.pop();
    }

    /// The trailer dictionary, merged across every section.
    #[must_use]
    pub fn trailer(&self) -> &Dict {
        &self.trailer.dict
    }

    /// The object number the trailer came from; zero for a bare `trailer`
    /// dictionary.
    #[must_use]
    pub fn trailer_object_number(&self) -> u32 {
        self.trailer.object_number
    }

    /// The document catalog.
    ///
    /// # Errors
    ///
    /// [`Error::NoCatalog`] when the trailer names none.
    pub fn catalog(&self) -> Result<Dict, Error> {
        let root = self
            .trailer
            .dict
            .reference(names::ROOT)
            .ok_or(Error::NoCatalog)?;
        self.store
            .get(root.num)
            .ok()
            .and_then(|c| c.as_dict().cloned())
            .ok_or(Error::NoCatalog)
    }

    /// The version the header declared: [`PdfVersion::PDF_1_7`] for
    /// `%PDF-1.7`.
    ///
    /// Never validated — a header claiming 9.9 opens like any other and
    /// reports 9.9. `None` means the header carried no readable digits at
    /// all, which a file with no `%PDF` line and one with `%PDF-x.y` both
    /// produce; the writer's fallback for that case is 1.7.
    #[must_use]
    pub fn version(&self) -> Option<PdfVersion> {
        self.version
    }

    /// Where the `%PDF` header sat in the original file. Non-zero means
    /// everything before it was ignored.
    #[must_use]
    pub fn header_offset(&self) -> u64 {
        self.header_offset
    }

    /// What the object store has repaired since the file opened, as a running
    /// total — read it *after* the work, not at load.
    ///
    /// [`Document::diags`] is the load-time snapshot and never changes. This
    /// one grows, because the store is lazy: a wrong `/Length` or a bad table
    /// offset is only discovered when a caller first reaches that object. It
    /// clones rather than draining, so asking twice between two fetches gives
    /// the same answer twice.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use pdfrum_parser::{LoadOptions, load};
    /// let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/minimal.pdf")[..]);
    /// let doc = load(bytes, &LoadOptions::default())?;
    /// // A clean file repairs nothing, before or after its pages are read.
    /// let _ = doc.page(0);
    /// assert!(doc.lazy_diagnostics().is_empty());
    /// # Ok::<(), pdfrum_parser::LoadError>(())
    /// ```
    #[must_use]
    pub fn lazy_diagnostics(&self) -> Diagnostics {
        self.store.peek_diags()
    }

    /// Whether the cross-reference table came from the recovery scan rather
    /// than the file's own sections. An incremental save is unsafe when it
    /// did.
    #[must_use]
    pub fn xref_was_rebuilt(&self) -> bool {
        self.xref_shape.rebuilt
    }

    /// Byte offset of the newest cross-reference section the load chained
    /// from, or **0** when the table was rebuilt by scanning.
    ///
    /// This is what an incremental update writes as its `/Prev`, so the zero
    /// carries meaning rather than being an absence: a rebuilt document has
    /// no previous section worth naming, and the writer answers by emitting a
    /// full table after the original bytes instead of a delta.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use pdfrum_parser::{LoadOptions, load};
    /// let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/minimal.pdf")[..]);
    /// let doc = load(bytes, &LoadOptions::default())?;
    /// assert!(doc.last_xref_offset() > 0);
    /// assert!(!doc.xref_was_rebuilt());
    /// # Ok::<(), pdfrum_parser::LoadError>(())
    /// ```
    #[must_use]
    pub fn last_xref_offset(&self) -> u64 {
        self.xref_shape.last_offset
    }

    /// Whether the document's **main** cross-reference — the newest section,
    /// the one `startxref` names — was a stream rather than a classic table.
    ///
    /// Not "the chain contained a stream somewhere": a hybrid file whose
    /// newest section is a classic table answers `false`. The writer reads it
    /// to decide whether an incremental update appends a classic delta table
    /// or folds the cross-reference into a stream object, and matching the
    /// original keeps a reader that only understands one of the two working.
    #[must_use]
    pub fn main_xref_is_stream(&self) -> bool {
        self.xref_shape.main_is_stream
    }

    /// The `/Encrypt` dictionary as the file wrote it, and whether the
    /// trailer held it **directly** rather than by reference.
    ///
    /// Returned raw and undecrypted, because the encryption dictionary is the
    /// one object in a document that is always plaintext. `Some` here does
    /// not imply the document opened encrypted — a file can declare a handler
    /// this reader answered with [`SecurityHandler::Identity`].
    #[must_use]
    pub fn encrypt_dict(&self) -> Option<(&Dict, bool)> {
        self.encrypt.as_ref().map(|(d, inline)| (d, *inline))
    }

    /// What the document permits, for the password that opened it.
    ///
    /// The owner's own unrestricted view is
    /// [`Document::owner_permissions`]. An unencrypted document permits
    /// everything.
    #[must_use]
    pub fn permissions(&self) -> Permissions {
        self.store.security().permissions()
    }

    /// What the document permits under the owner's view.
    ///
    /// Every permission, for a document the owner password opened; otherwise
    /// the same answer as [`Document::permissions`].
    #[must_use]
    pub fn owner_permissions(&self) -> Permissions {
        self.store.security().owner_permissions()
    }

    /// Whether the document is encrypted.
    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        !matches!(self.store.security(), SecurityHandler::Identity)
    }

    /// The security handler the password opened this document with.
    ///
    /// [`SecurityHandler::Identity`] for an unencrypted document, and for one
    /// whose crypt filter is `/Identity`.
    ///
    /// The writer needs this to save an encrypted document *as encrypted*: it
    /// re-enciphers every string and stream under the same handler, so the
    /// result opens with the same password. It carries the file key, so it
    /// is deliberately not `Clone`-friendly to hold onto — borrow it for the
    /// length of a save and let it go.
    #[must_use]
    pub fn security_handler(&self) -> &SecurityHandler {
        self.store.security()
    }

    /// The file, from its header onwards.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The object store, for fetching references.
    #[must_use]
    pub fn store(&self) -> &Arc<ObjectStore> {
        &self.store
    }

    /// Where every object lives.
    #[must_use]
    pub fn xref(&self) -> &Xref {
        self.store.xref()
    }
}

impl Resolve for Document {
    fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, pdfrum_object::Error> {
        self.store.fetch(r)
    }
}

#[cfg(test)]
mod tests {
    use super::{LoadError, LoadOptions, find_header, load, read_version};
    use pdfrum_common::{DiagKind, Limits, PdfVersion};
    use pdfrum_crypt::Permissions;
    use pdfrum_object::{Name, names};
    use std::sync::Arc;

    fn open(bytes: &[u8]) -> Result<super::Document, LoadError> {
        load(Arc::from(bytes), &LoadOptions::default())
    }

    /// A document with `count` pages under one `/Pages` node.
    fn build(count: usize) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.7\n");
        let mut offsets = vec![0usize];

        offsets.push(out.len());
        out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        offsets.push(out.len());
        let kids: Vec<String> = (0..count).map(|i| format!("{} 0 R", i + 3)).collect();
        out.extend_from_slice(
            format!(
                "2 0 obj\n<< /Type /Pages /Count {count} /Kids [{}] /MediaBox [0 0 612 792] >>\nendobj\n",
                kids.join(" ")
            )
            .as_bytes(),
        );

        for i in 0..count {
            offsets.push(out.len());
            out.extend_from_slice(
                format!(
                    "{} 0 obj\n<< /Type /Page /Parent 2 0 R /PageNumber {i} >>\nendobj\n",
                    i + 3
                )
                .as_bytes(),
            );
        }

        let xref_at = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets.iter().skip(1) {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
                offsets.len()
            )
            .as_bytes(),
        );
        out
    }

    #[test]
    fn finds_a_header_at_the_start_or_after_junk() {
        assert_eq!(find_header(b"%PDF-1.7\n", &Limits::default()), Some(0));
        assert_eq!(find_header(b"junk%PDF-1.7\n", &Limits::default()), Some(4));
        assert_eq!(find_header(b"no header", &Limits::default()), None);
    }

    #[test]
    fn version_digits_are_read_not_validated() {
        assert_eq!(read_version(b"%PDF-1.7\n"), Some(PdfVersion::PDF_1_7));
        assert_eq!(read_version(b"%PDF-2.0\n"), Some(PdfVersion::PDF_2_0));
        assert_eq!(read_version(b"%PDF-x.y\n"), None);
        // The private packing round-trips every digit pair the header can
        // spell, and only `0.0` — which is unreachable, since a `0` packed
        // value is reported as "no version" — is not a `Some`.
        for major in 0..=9u8 {
            for minor in 0..=9u8 {
                let header = format!("%PDF-{major}.{minor}\n");
                let expected = (major, minor) != (0, 0);
                assert_eq!(
                    read_version(header.as_bytes()),
                    expected.then(|| PdfVersion::new(major, minor)),
                    "header {header:?}"
                );
            }
        }
    }

    #[test]
    fn a_file_without_a_header_is_not_a_pdf() {
        assert_eq!(open(b"just some bytes").err(), Some(LoadError::NotPdf));
        // A header at the very end with no room for a document.
        assert_eq!(open(b"%PDF").err(), Some(LoadError::NotPdf));
    }

    #[test]
    fn opens_a_document_and_counts_its_pages() {
        let doc = open(&build(3)).expect("document");
        assert_eq!(doc.page_count(), 3);
        assert_eq!(doc.version(), Some(PdfVersion::PDF_1_7));
        assert!(!doc.xref_was_rebuilt());
        assert!(!doc.is_encrypted());
        assert_eq!(doc.permissions(), Permissions::ALL);
    }

    #[test]
    fn reads_pages_in_order() {
        let doc = open(&build(5)).expect("document");
        let number = Name::from("PageNumber");
        for i in 0..5u32 {
            let page = doc.page(i).expect("page");
            assert_eq!(page.dict.direct_int(&number), Some(i64::from(i)));
        }
        assert!(doc.page(5).is_err());
    }

    #[test]
    fn reads_pages_in_reverse_and_out_of_order() {
        let doc = open(&build(5)).expect("document");
        let number = Name::from("PageNumber");
        for i in (0..5u32).rev() {
            assert_eq!(
                doc.page(i).expect("page").dict.direct_int(&number),
                Some(i64::from(i))
            );
        }
        // An out-of-range lookup must not poison the ones after it.
        assert!(doc.page(99).is_err());
        assert_eq!(doc.page(3).expect("page").dict.direct_int(&number), Some(3));
    }

    #[test]
    fn a_count_larger_than_the_tree_reports_the_lie() {
        let text = String::from_utf8_lossy(&build(3)).replace("/Count 3", "/Count 9");
        let doc = open(text.as_bytes()).expect("document");
        // The claimed count is what the document reports...
        assert_eq!(doc.page_count(), 9);
        // ...and the pages that exist still resolve.
        assert!(doc.page(0).is_ok());
        assert!(doc.page(2).is_ok());
        // The ones past the real tree do not.
        assert!(doc.page(3).is_err());
        assert!(doc.page(8).is_err());
        // And the real ones still work afterwards.
        assert!(doc.page(2).is_ok());
    }

    #[test]
    fn a_kids_less_pages_node_counts_as_a_page_it_cannot_produce() {
        // Counting and looking up disagree here, and both are right. The
        // count treats a `/Pages` node with no `/Kids` as the document's one
        // page, but the walk refuses to hand back a node that calls itself a
        // branch — so the document reports one page and has none.
        let file = b"%PDF-1.7\n\
                     1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
                     2 0 obj\n<< /Type /Pages /Count 3 >>\nendobj\n\
                     trailer\n<< /Root 1 0 R >>\nstartxref\n0\n%%EOF\n";
        let doc = open(file).expect("document");
        assert_eq!(doc.page_count(), 1);
        assert!(doc.page(0).is_err());
        assert!(doc.page(1).is_err());
    }

    #[test]
    fn a_kids_less_node_that_does_not_claim_to_be_a_branch_is_a_page() {
        // The same shape without the `/Type /Pages` claim: the node has no
        // children, so it is the page itself and the lookup succeeds.
        let file = b"%PDF-1.7\n\
                     1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
                     2 0 obj\n<< /MediaBox [0 0 10 10] >>\nendobj\n\
                     trailer\n<< /Root 1 0 R >>\nstartxref\n0\n%%EOF\n";
        let doc = open(file).expect("document");
        assert_eq!(doc.page_count(), 1);
        assert!(doc.page(0).is_ok());
    }

    #[test]
    fn a_catalog_without_pages_will_not_open() {
        let file = b"%PDF-1.7\n\
                     1 0 obj\n<< /Type /Catalog >>\nendobj\n\
                     trailer\n<< /Root 1 0 R >>\nstartxref\n0\n%%EOF\n";
        assert!(matches!(open(file), Err(LoadError::Broken(_))));
    }

    #[test]
    fn a_root_written_inline_does_not_name_a_catalog() {
        let file = b"%PDF-1.7\n\
                     1 0 obj\n<< /Type /Page >>\nendobj\n\
                     trailer\n<< /Root << /Type /Catalog /Pages 2 0 R >> >>\n\
                     startxref\n0\n%%EOF\n";
        assert!(matches!(open(file), Err(LoadError::Broken(_))));
    }

    #[test]
    fn a_broken_start_xref_still_opens_the_document() {
        let text = String::from_utf8_lossy(&build(2)).into_owned();
        let broken = text
            .replace("%%EOF", "")
            .replace("startxref\n", "startxref\n1\n");
        let doc = open(broken.as_bytes()).expect("document");
        assert!(doc.xref_was_rebuilt());
        assert_eq!(doc.page_count(), 2);
        assert!(doc.diags.contains(&DiagKind::XrefRebuilt));
    }

    #[test]
    fn a_header_after_junk_shifts_every_offset() {
        let mut file = vec![b'x'; 100];
        file.extend_from_slice(&build(2));
        let doc = open(&file).expect("document");
        assert_eq!(doc.header_offset(), 100);
        assert_eq!(doc.page_count(), 2);
        assert!(doc.diags.contains(&DiagKind::HeaderOffset));
    }

    #[test]
    fn inheritable_attributes_come_from_the_parent() {
        let doc = open(&build(2)).expect("document");
        let page = doc.page(0).expect("page");
        // The page states no /MediaBox; its /Pages parent does.
        let inherited = page
            .inherited(names::MEDIA_BOX, &doc)
            .expect("inherited media box");
        let array = inherited.as_array().expect("array");
        assert_eq!(array.number_at(2), Some(612.0));
        assert_eq!(array.number_at(3), Some(792.0));
        // A key nobody states is absent.
        assert!(page.inherited(names::ROTATE, &doc).is_none());
    }

    #[test]
    fn a_pages_reference_is_reported() {
        let doc = open(&build(1)).expect("document");
        assert_eq!(doc.page(0).expect("page").reference.map(|r| r.num), Some(3));
    }

    #[test]
    fn documents_are_send_and_sync() {
        fn assert_both<T: Send + Sync>() {}
        assert_both::<super::Document>();
    }

    #[test]
    fn never_panics_on_arbitrary_bytes() {
        let seeds: &[&[u8]] = &[
            b"",
            b"%PDF",
            b"%PDF-1.7",
            b"%PDF-1.7\nstartxref\n0\n%%EOF",
            b"%PDF-1.7\ntrailer<</Root 1 0 R>>",
            b"%PDF-1.7\n1 0 obj<</Length 1 0 R>>stream\n",
            b"%PDF-1.7\n\x00\xff\x80\x0b",
        ];
        for seed in seeds {
            let _ = open(seed);
        }
    }
}
