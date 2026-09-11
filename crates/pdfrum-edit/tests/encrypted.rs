//! Saving an encrypted document as encrypted.
//!
//! The unit tests in `pdfrum-crypt` say the cipher is its own inverse. These
//! say the *file* is: an encrypted document saved by this crate is a document
//! that opens with the same password and reads back the same objects, at every
//! standard security handler revision the corpus exercises.
//!
//! The three questions each fixture answers:
//!
//! 1. **Does it still need the password?** A saved file that opens with none
//!    is a decrypted file, however well-formed. That failure is invisible to
//!    a round-trip check that always supplies the password.
//! 2. **Does the content survive?** Every string and stream is deciphered,
//!    re-enciphered under a fresh vector, and deciphered again; the objects
//!    reachable from the catalog must compare equal to the originals.
//! 3. **Is the `/Encrypt` dictionary readable without the key?** A reader has
//!    to parse `/O`, `/U` and `/Perms` before it has one, so those strings
//!    must survive as plaintext.
//!
//! The corpus lives outside the repository, so every test skips when it is
//! absent rather than failing.

// The helpers below are shared by `#[test]` functions; `allow-expect-in-tests`
// does not reach them, so the allowance is stated once for the file.
#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdfrum_edit::{EditDoc, IdSource, SaveMode, SaveOptions, save};
use pdfrum_object::{ObjRef, Object, Resolve, names};
use pdfrum_parser::{Document, LoadOptions, load};

/// One encrypted fixture: its file name and a password that opens it.
///
/// The passwords are recovered from the oracle's own embedder tests.
/// `encrypted.pdf` is AESV2;
/// the `hello_world` family walks `/R` 2, 3, 5 and 6; `bug_644.pdf` is a
/// second `/R 5` shape whose `/P` masks to the same word for both roles.
const FIXTURES: [(&str, &[u8]); 7] = [
    ("encrypted_hello_world_r2.pdf", b"h\xf4tel"),
    ("encrypted_hello_world_r3.pdf", b"h\xf4tel"),
    ("encrypted_hello_world_r5.pdf", b"h\xf4tel"),
    ("encrypted_hello_world_r6.pdf", b"h\xf4tel"),
    ("encrypted.pdf", b"1234"),
    ("encrypted.pdf", b"5678"),
    ("bug_644.pdf", b"a"),
];

/// The read-only C++ PDFium checkout, resolved the one way every script and
/// test in this repository resolves it: `$PDFRUM_ORACLE_CHECKOUT`, else the
/// sibling `../pdfium-c++` directory README.md names.
///
/// Six lines rather than a shared module: forbids a `common`,
/// `util` or `helpers` module name, and an integration test in one crate
/// cannot reach another crate's test code anyway.
fn oracle_checkout() -> PathBuf {
    let checkout = std::env::var_os("PDFRUM_ORACLE_CHECKOUT").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pdfium-c++"),
        PathBuf::from,
    );
    if !checkout.is_dir() {
        // Said once per test process, so a run where every case below did
        // nothing says so rather than reporting a silent green.
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| {
            eprintln!(
                "skipping the oracle-corpus cases: no checkout at {} \
                 (set PDFRUM_ORACLE_CHECKOUT)",
                checkout.display()
            );
        });
    }
    checkout
}

/// Where the oracle's test files live, when this checkout has them.
fn resources() -> Option<PathBuf> {
    let path = oracle_checkout().join("testing/resources");
    path.is_dir().then_some(path)
}

/// Open one fixture, or `None` when the corpus is absent.
fn open(name: &str, password: &[u8]) -> Option<Document> {
    let bytes: Arc<[u8]> = Arc::from(std::fs::read(resources()?.join(name)).ok()?);
    let opts = LoadOptions {
        password: Some(password.to_vec()),
        ..LoadOptions::default()
    };
    Some(load(bytes, &opts).unwrap_or_else(|e| unreachable!("{name} failed to open: {e}")))
}

/// A save whose identifiers come from a seed, so two runs share `/ID`.
/// Encrypted payloads are not pinned: AES vectors come from the OS.
fn fixed(mode: SaveMode) -> SaveOptions {
    SaveOptions {
        mode,
        id_source: IdSource::Fixed([0x5A; 16]),
        ..SaveOptions::default()
    }
}

/// Save `doc` and hand back the bytes.
fn saved(doc: &Document, opts: &SaveOptions) -> Vec<u8> {
    let edit = EditDoc::new(doc);
    let mut out = Vec::new();
    save(&edit, opts, &mut out).expect("the save succeeds");
    out
}

/// Load bytes with a password, reporting failure rather than panicking.
fn reload(bytes: &[u8], password: Option<&[u8]>) -> Option<Document> {
    let opts = LoadOptions {
        password: password.map(<[u8]>::to_vec),
        ..LoadOptions::default()
    };
    load(Arc::from(bytes), &opts).ok()
}

/// Every string in one object, in traversal order — the values encryption
/// actually touches.
fn strings(obj: &Object, into: &mut Vec<Vec<u8>>) {
    match obj {
        Object::Str(s) => into.push(s.as_bytes().to_vec()),
        Object::Array(a) => a.iter().for_each(|v| strings(v, into)),
        Object::Dict(d) => d.iter().for_each(|(_, v)| strings(v, into)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| strings(v, into)),
        _ => {}
    }
}

/// A fingerprint of what a document holds: for every object number, its
/// strings and its **decoded** stream bytes.
///
/// Decoded rather than raw, because the writer's own stream table
/// flate-compresses a payload that arrived without a `/Filter` — a change the
/// cipher knows nothing about, and one an unencrypted save makes too. What the
/// cipher must preserve is what the stream *means*, and that is what a decode
/// recovers.
fn contents(doc: &Document) -> Vec<(u32, Vec<Vec<u8>>, Vec<u8>)> {
    (1..=doc.xref().last_object_number())
        .filter_map(|num| {
            let obj = doc.fetch(ObjRef::new(num, 0)).ok()?;
            if obj.is_null() {
                return None;
            }
            let mut found = Vec::new();
            strings(&obj, &mut found);
            let data = match &*obj {
                Object::Stream(s) => pdfrum_parser::decoded_stream(
                    s,
                    doc,
                    &pdfrum_common::Limits::default(),
                    &mut pdfrum_common::Diagnostics::default(),
                ),
                _ => Vec::new(),
            };
            Some((num, found, data))
        })
        .collect()
}

// A saved encrypted document is still encrypted, and the same password
// opens it.
#[test]
fn every_revision_saves_encrypted_and_reopens_with_its_password() {
    for (name, password) in FIXTURES {
        let Some(doc) = open(name, password) else {
            return;
        };
        assert!(doc.is_encrypted(), "{name} was not encrypted to begin with");

        let out = saved(&doc, &fixed(SaveMode::Full));
        let reopened = reload(&out, Some(password))
            .unwrap_or_else(|| unreachable!("{name} saved unreadably under its own password"));
        assert!(
            reopened.is_encrypted(),
            "{name} saved decrypted despite remove_security being off"
        );
        assert_eq!(
            reopened.page_count(),
            doc.page_count(),
            "{name} lost pages in the save"
        );
    }
}

// The other half of "still encrypted": the wrong password, and no password at
// all, are both refused. Without this a writer that emitted `/Encrypt` over
// plaintext would pass every other test here.
#[test]
fn a_saved_encrypted_document_refuses_the_wrong_password() {
    for (name, password) in FIXTURES {
        let Some(doc) = open(name, password) else {
            return;
        };
        let out = saved(&doc, &fixed(SaveMode::Full));

        // `bug_644.pdf` and `encrypted.pdf` opened with the owner password
        // still have a user password, so "no password" is only refused where
        // the empty password is not itself a valid user password.
        assert!(
            reload(&out, Some(b"definitely-not-the-password")).is_none(),
            "{name} opened under a password it should not have"
        );
        assert!(
            reload(&out, None).is_none(),
            "{name} opened with no password at all"
        );
    }
}

// What the cipher wraps has to come back out of it. Every string and every
// stream payload the document holds is compared, object by object.
#[test]
fn a_saved_encrypted_document_holds_the_same_content() {
    for (name, password) in FIXTURES {
        let Some(doc) = open(name, password) else {
            return;
        };
        let out = saved(&doc, &fixed(SaveMode::Full));
        let reopened = reload(&out, Some(password)).expect("reopens");

        let before = contents(&doc);
        let after = contents(&reopened);
        assert!(!before.is_empty(), "{name} held nothing to compare");
        for (num, wanted, data) in &before {
            let Some((_, got, got_data)) = after.iter().find(|(n, _, _)| n == num) else {
                // An object the trailer can no longer reach is collected by a
                // full save, which is the writer working as designed.
                continue;
            };
            assert_eq!(got, wanted, "{name} object {num}: strings differ");
            assert_eq!(got_data, data, "{name} object {num}: stream bytes differ");
        }
    }
}

// ISO 32000-1 §7.6.1: a reader parses `/O`, `/U` and `/Perms` before it has a
// key, so the encryption dictionary is the one object never enciphered.
#[test]
fn the_encryption_dictionary_survives_as_plaintext() {
    for (name, password) in FIXTURES {
        let Some(doc) = open(name, password) else {
            return;
        };
        let Some((original, _)) = doc.encrypt_dict() else {
            unreachable!("{name} declares no /Encrypt");
        };
        let wanted: Vec<Vec<u8>> = original
            .iter()
            .filter_map(|(_, v)| v.as_string().map(|s| s.as_bytes().to_vec()))
            .collect();
        assert!(!wanted.is_empty(), "{name}'s /Encrypt holds no strings");

        let out = saved(&doc, &fixed(SaveMode::Full));
        let reopened = reload(&out, Some(password)).expect("reopens");
        let (written, _) = reopened
            .encrypt_dict()
            .unwrap_or_else(|| unreachable!("{name} saved without an /Encrypt"));
        let got: Vec<Vec<u8>> = written
            .iter()
            .filter_map(|(_, v)| v.as_string().map(|s| s.as_bytes().to_vec()))
            .collect();
        assert_eq!(got, wanted, "{name}'s /Encrypt was enciphered");
    }
}

// `/P` is what the document permits, and a save must not quietly widen or
// narrow it.
#[test]
fn permissions_survive_the_save() {
    for (name, password) in FIXTURES {
        let Some(doc) = open(name, password) else {
            return;
        };
        let out = saved(&doc, &fixed(SaveMode::Full));
        let reopened = reload(&out, Some(password)).expect("reopens");
        assert_eq!(
            reopened.permissions(),
            doc.permissions(),
            "{name} changed its permissions"
        );
        assert_eq!(
            reopened.owner_permissions(),
            doc.owner_permissions(),
            "{name} changed its owner permissions"
        );
        // And the raw `/P` word itself, which is what another reader reads.
        let p = |d: &Document| d.encrypt_dict().and_then(|(e, _)| e.direct_int(names::P));
        assert_eq!(p(&reopened), p(&doc), "{name} rewrote /P");
    }
}

// The interlock from , unchanged: an incremental save of an encrypted
// document appends freshly-keyed objects behind the original ciphertext,
// which is sound only because the key did not change. The original bytes must
// therefore be a prefix of the output.
#[test]
fn an_incremental_save_of_an_encrypted_document_appends() {
    for (name, password) in FIXTURES {
        let Some(doc) = open(name, password) else {
            return;
        };
        // A rebuilt table has no section to chain from, so the save
        // downgrades to a full one and the prefix property does not apply.
        if doc.xref_was_rebuilt() {
            continue;
        }
        let out = saved(&doc, &fixed(SaveMode::Incremental));
        let body = doc.bytes();
        assert_eq!(
            out.get(..body.len()),
            Some(body),
            "{name}: the incremental save rewrote the original bytes"
        );
        let text = String::from_utf8_lossy(&out);
        let before = String::from_utf8_lossy(body);
        assert!(
            text.matches("startxref").count() > before.matches("startxref").count(),
            "{name}: no appended cross-reference"
        );
        assert!(
            reload(&out, Some(password)).is_some(),
            "{name}: the appended file no longer opens"
        );
    }
}

// `remove_security` remains available, and does what it says: the output is
// plaintext, declares no `/Encrypt`, and opens with no password.
#[test]
fn remove_security_still_writes_a_decrypted_document() {
    for (name, password) in FIXTURES {
        let Some(doc) = open(name, password) else {
            return;
        };
        let opts = SaveOptions {
            remove_security: true,
            ..fixed(SaveMode::Full)
        };
        let out = saved(&doc, &opts);
        let reopened = reload(&out, None)
            .unwrap_or_else(|| unreachable!("{name} did not open without a password"));
        assert!(!reopened.is_encrypted(), "{name} stayed encrypted");
        assert!(reopened.encrypt_dict().is_none(), "{name} kept /Encrypt");
        assert_eq!(reopened.page_count(), doc.page_count(), "{name} lost pages");
    }
}

// Encrypted saves are not byte-reproducible: AES vectors come from the OS
// (see `encrypt.rs`). `IdSource::Fixed` still pins `/ID`, and two saves of
// one document still decipher to the same objects.
#[test]
fn two_saves_share_an_id_and_the_plaintext() {
    for (name, password) in FIXTURES {
        let Some(doc) = open(name, password) else {
            return;
        };
        let first = saved(&doc, &fixed(SaveMode::Full));
        let second = saved(&doc, &fixed(SaveMode::Full));

        let a = reload(&first, Some(password))
            .unwrap_or_else(|| unreachable!("{name}: first save unreadably"));
        let b = reload(&second, Some(password))
            .unwrap_or_else(|| unreachable!("{name}: second save unreadably"));

        let id = |d: &Document| d.trailer().raw(names::ID).cloned();
        assert_eq!(
            id(&a),
            id(&b),
            "{name}: /ID changed between two seeded saves"
        );
        assert_eq!(
            contents(&a),
            contents(&b),
            "{name}: two saves deciphered to different objects"
        );
    }
}

// And the ciphertext really is ciphertext: a save that emitted plaintext
// under an `/Encrypt` declaration would pass every reload test above (our own
// reader would decipher garbage into garbage consistently), so the check is
// that the saved bytes do *not* contain a string the original held in the
// clear only after deciphering.
#[test]
fn the_saved_body_is_not_the_plaintext() {
    for (name, password) in FIXTURES {
        let Some(doc) = open(name, password) else {
            return;
        };
        // A stream payload long enough that its appearance in the output
        // would be no coincidence.
        let Some(plaintext) = (1..=doc.xref().last_object_number()).find_map(|num| {
            match &*doc.fetch(ObjRef::new(num, 0)).ok()? {
                Object::Stream(s) if s.data.as_bytes().len() >= 32 => {
                    Some(s.data.as_bytes().to_vec())
                }
                _ => None,
            }
        }) else {
            continue;
        };
        let out = saved(&doc, &fixed(SaveMode::Full));
        assert!(
            !out.windows(plaintext.len()).any(|w| w == plaintext),
            "{name}: a stream's plaintext appears verbatim in the saved file"
        );
    }
}

// x : a page edited *and* saved goes through the cipher like anything
// else, because the regenerated stream is written the same way as the rest of
// the body. The failure this guards against is specific and silent — a
// regenerated stream added to the overlay after the encryptor was set up, and
// so written in the clear under an `/Encrypt` that claims otherwise. Such a
// file opens, and its edited page renders as garbage.
#[test]
fn a_mutated_encrypted_document_saves_re_encrypted() {
    for (name, password) in FIXTURES {
        let Some(doc) = open(name, password) else {
            return;
        };
        let Ok(page_dict) = doc.page(0) else {
            continue;
        };
        let Some(page_ref) = page_dict.reference else {
            continue;
        };
        let resources = page_dict
            .inherited(names::RESOURCES, &doc)
            .and_then(|object| object.resolve(&doc).ok()?.as_dict().cloned())
            .unwrap_or_default();

        // Rebuild the page and dirty every object, which is the mutation with
        // the largest regenerated stream and therefore the most to leak.
        let mut ctx = pdfrum_page::BuildContext::new();
        let mut diags = pdfrum_common::Diagnostics::default();
        let limits = pdfrum_common::Limits::default();
        let bytes = page_contents(&doc, &page_dict.dict, &limits, &mut diags);
        let ops = pdfrum_page::parse_content(&bytes, &limits, &mut diags);
        let mut page = pdfrum_page::build_page_streams(
            &ops,
            &pdfrum_page::StreamBounds::default(),
            &page_dict.dict,
            |key| page_dict.inherited(key, &doc),
            &pdfrum_page::Resources::for_page(Some(resources.clone())),
            &doc,
            &mut ctx,
            &limits,
            &mut diags,
        );
        if page.objects().is_empty() {
            continue;
        }
        for index in 0..page.objects().len() {
            page.object_mut(index);
        }

        let mut edit = EditDoc::new(&doc);
        let rewrite =
            pdfrum_edit::regenerate(&page, &resources, &doc).expect("every object is dirty");
        let shared = pdfrum_edit::shared_objects(&edit);
        pdfrum_edit::apply_rewrite(&mut edit, page_ref, &page_dict.dict, &rewrite, &shared);

        let mut out = Vec::new();
        save(&edit, &fixed(SaveMode::Full), &mut out).expect("the save succeeds");

        // It still needs the password.
        assert!(
            reload(&out, None).is_none(),
            "{name}: a mutated save opened with no password"
        );
        let reopened = reload(&out, Some(password)).expect("reopens with its password");
        assert_eq!(reopened.page_count(), doc.page_count());

        // And the regenerated operators are not sitting in the file in the
        // clear. `q\n` opens every stream we write, and the prologue's
        // literal is long enough that finding it would be no coincidence.
        let prologue = b"0 0 0 RG 0 0 0 rg 1 w 0 J 0 j";
        assert!(
            !out.windows(prologue.len()).any(|w| w == prologue),
            "{name}: a regenerated stream was written in the clear"
        );
    }
}

/// A page's `/Contents`, decoded and joined — the tool's `assemble`, inlined
/// so this test needs nothing from the binary crate.
fn page_contents(
    doc: &Document,
    page: &pdfrum_object::Dict,
    limits: &pdfrum_common::Limits,
    diags: &mut pdfrum_common::Diagnostics,
) -> Vec<u8> {
    let Some(contents) = page.get(names::CONTENTS, doc) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut push = |object: &Object, out: &mut Vec<u8>| {
        if let Some(stream) = object.as_stream() {
            out.extend_from_slice(
                &pdfrum_filters::decode_chain(stream, 0, doc, limits, diags).data,
            );
            out.push(b' ');
        }
    };
    let Some(direct) = contents.as_direct() else {
        return out;
    };
    match direct {
        Object::Stream(_) => push(direct, &mut out),
        Object::Array(array) => {
            for element in array.iter() {
                if let Ok(resolved) = element.resolve(doc) {
                    push(resolved.get(), &mut out);
                }
            }
        }
        _ => {}
    }
    out
}
