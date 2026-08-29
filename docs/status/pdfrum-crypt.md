# `pdfrum-crypt` status

**Updated:** 2026-08-29 · **State:** implemented, all gates green

Third library crate of M1. Contract: SPEC.md §3; behavior:
`docs/design/pdfrum-crypt.md`.

Decryption only, per the brief's divergence D2: `/Encrypt` + password →
`SecurityHandler`, and every string and stream the parser reads → `decrypt`.
Encryption belongs to the save path (M7).

## What is implemented

Module plan followed as written in the brief §3, with one file added:
`lib.rs` · `standard.rs` · `object.rs` · `primitives.rs` · `rc4.rs` ·
**`key.rs`** (+ `test_fixtures.rs`, test-only). `SmallKey` got its own file
rather than living in `lib.rs`, because its whole reason to exist is a
hand-written redacting `Debug`, and that is a property worth isolating with its
own tests.

- **`SecurityHandler`**, the closed enum of SPEC §3 (D7): `Rc4V2` · `AesV4`
  (AESV2) · `AesV5` (AESV3) · `Identity`. Revision, permissions,
  `/EncryptMetadata`, the owner-unlocked flag and the password encoding are
  data inside the variants. `from_encrypt_dict` runs the C++'s ordering: a
  **non-empty** password is tried as the owner password first, an empty one is
  only ever a user password.
- **`EncryptParams` + `parse_encrypt_dict`** (brief §1.2/§1.3). The record/
  algorithm split is what makes the revision algorithms testable as KATs over
  literals instead of through whole documents.
- **Key-length resolution.** `/V 1` is 5 bytes with `/Length` ignored; `/V 2..3`
  divide `/Length` (default 40) by 8 with **no** promotion; `/V >= 4` apply the
  `nKeyBits < 40 ⇒ × 8` promotion, take the per-filter `/Length` at `/V 4` and
  the document's at `/V 5`, and leave the cipher as RC4 for any `/CFM` other
  than `AESV2`/`AESV3`.
- **Crypt filters.** `/StmF != /StrF` is a hard rejection, and both absent at
  `/V 4` is `MissingCryptFilter("")` — *not* the specification's `/Identity`
  default, which PDFium does not implement. `/EFF` is unimplemented upstream, so
  `CryptClass::Embedded` decrypts exactly like `Stream` (D1); the variant is
  matched explicitly rather than through a `_` arm.
- **Revisions 2 to 4** (brief §1.4–§1.6): the 32-byte pad, Algorithm 2's MD5
  order with its 50-round `copy_len`-prefix loop, Algorithm 4/5's 20 RC4 rounds
  and 16-byte comparison, Algorithm 7's owner recovery with its 50-round
  full-digest loop and the positional trailing-pad strip. The user check runs
  twice — honoring and then ignoring `/EncryptMetadata` — and keeps the key the
  *successful* call derived.
- **Revisions 5 and 6** (brief §1.7): Algorithm 2.A with its ≥48-byte `/O` and
  `/U` guards, the owner check's `/U[0..48]` vector, the zero-IV unwrap of
  `/OE`/`/UE`, and the `/Perms` block with its zero-padding of a short entry and
  its deliberately one-sided metadata comparison.
- **`revision6_hash`** (Algorithm 2.B) exactly as transcribed: the
  `BigOrder64BitsMod3` fold written as a fold, key and IV from the *current*
  `K`'s first 32 bytes regardless of `block_size`, the ×64 repetition that
  guarantees block alignment for any password length, and the stop test against
  the last byte of the **whole** ciphertext — 64 rounds minimum, 287 maximum.
- **Per-object keys** (brief §1.10): three bytes of object number and two of
  generation, both little-endian and both truncating; RC4's `min(key_len+5, 16)`
  cap; AESV2's `sAlT` suffix with the full 16-byte digest (the decrypt reading,
  which is the one this crate needs); and AESV3's rule that the file key is used
  **verbatim** with no derivation at all.
- **AES stream decrypt** (brief §1.11/§3.3): the one-block-lag semantics
  reproduced as an output rule (D6) — IV consumed, final block padding-stripped
  only when nothing followed it, a partial tail discarded while the block before
  it survives, no padding validation, and a pad byte of 0 keeping the whole
  block.
- **`is_signature_dict`**, exposed for the parser's decrypt walk. The walk and
  the deferral itself stay in `pdfrum-parser` (brief §3.4) because they need the
  object graph; this crate exposes the predicate and `encrypt_metadata()` so the
  parser does not rediscover either obligation.
- **Primitives.** RC4 in-crate (~50 lines with the empty-key rule);
  MD5/SHA-1/-256/-384/-512 and AES-CBC 128/192/256 over RustCrypto (D5). Every
  C++ `CHECK` — bad AES key length, non-block-multiple buffer — is a typed
  error (D4); at the `decrypt` boundary those become empty output, because
  PDFium never fails a decrypt.

## Tests

`cargo nextest run -p pdfrum-crypt`: **89** unit tests. `cargo test --doc`:
**6** doctests. Whole run is ~1.3 s.

All 19 of the brief's test sets are covered, with every KAT vector transcribed
from the C++ (the byte arrays were extracted programmatically from
`fx_crypt_unittest.cpp`, not retyped):

| Set | Covered by |
|---|---|
| T1 MD5 | the RFC 1321 A.5 suite + the 10 MiB ramp in 4097-byte chunks |
| T2 SHA | SHA-1/-256 vectors, SHA-384/-512 empty + "simple test" + the 112-byte padding boundary |
| T3 AES-CBC | all twelve NIST SP 800-38A vectors, chained, round-tripped, per key size |
| T4 RC4 | all four ciphertext vectors (empty and `foobar` keys × short and 271-byte plaintexts) |
| T5–T10 | the five fixture `/Encrypt` dicts as literals: `encrypted_hello_world_r{2,3,5,6}.pdf`, `encrypted.pdf`, `bug_644.pdf`, each with its real passwords in both encodings |
| T11–T13 | truncated `/O`, `/U`, `/OE`, `/UE`, `/Perms` at every length from 0 to 63 |
| T14 | the 14-row key-length resolution table, plus the negative-`/Length` row |
| T15 | mismatched, absent-vs-named, and both-absent crypt filters |
| T16 | the missing-`/ID` case, pinned by the key it *changes* |
| T17 | object-key vectors, the 16-byte cap, objnum truncation, the AESV3 verbatim rule |
| T18 | every row of the `decrypt_aes_cbc` behavior table, plus pad bytes 0, 1, 15, 16, 255 |
| T19 | RC4 length preservation and round trip |

The brief's §4.5 fuzz targets are stated as **deterministic sweeps** rather than
`cargo-fuzz` targets: `libfuzzer-sys` is not in DEPS.md, and adding it is a
dependency-set decision outside this crate. Three tests sweep the shape space
that `fuzz_encrypt_dict`/`fuzz_decrypt` would — arbitrary `/V`, `/R`, `/Length`,
password-entry lengths, and every payload length from 0 to 79 through the public
`decrypt` — asserting only that nothing panics and that no payload grows. They
found one real issue during development (below). The `fuzz/` workspace lands
with the parser's targets; these sweeps stay regardless, since they run in 1.3 s
and name their failing input directly.

Gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -D
warnings`, `cargo nextest run` (397 workspace-wide), `cargo test --doc` (49),
`bash scripts/ci.sh` — all green.

## `[spec]` changes made

**None.** Every name and signature SPEC §3 fixes is implemented as written:
the four `SecurityHandler` variants, `from_encrypt_dict(dict, file_id,
password, r)`, `decrypt(&self, obj, class, data) -> Vec<u8>`, the three-variant
`CryptClass`, `WrongPassword` as a distinct variant, and the RustCrypto/in-crate
RC4 split. The orchestrator's decisions hold too: **no 127-byte password cap**
(Q4 resolved against the brief's proposal — `Limits` gains no field, and
`from_encrypt_dict` documents that the ISO limit is not enforced), and
divergences D1–D7 implemented as written.

The additions beyond SPEC §3 are all additive and none contradict it:
`EncryptParams`/`Cipher`/`parse_encrypt_dict` (the record/algorithm split the
brief §3.2 asks for), `SmallKey`, `PasswordEncoding`, `PAD`, `sha1`, `rc4`,
`is_signature_dict`, and the accessors `permissions`/`encrypt_metadata`/
`revision`/`owner_unlocked`/`password_encoding`.

## Brief corrections found while implementing

Four places where the brief was wrong or unresolved; the C++ was re-read in
each case and the code follows the C++.

1. **`GetPassCode` fills from the front of the pad** (brief §1.4). The brief
   says so correctly, but it is the single easiest thing to get wrong and the
   crate's first bug: the padding tail is `PAD[0..32-n]`, *not* `PAD[n..32]`.
   Pinned by `padding_fills_from_the_front_of_the_pad`.
2. **Q2 resolved — the Latin-1 ⇄ UTF-8 conversions.** The brief §3.2 proposed
   `U+FFFD` for invalid UTF-8 and `'?'` for scalars above 0xFF, flagging both as
   inferences. `core/fxcrt/widestring.cpp` says otherwise on both counts:
   `WideString::ToLatin1` is `wc & 0xff` (**truncation**, never `'?'`), and
   `UTF8Decode` (`:285-317`) is a lenient decoder that **drops** what it cannot
   use — a stray continuation byte and a truncated sequence contribute nothing,
   with no `U+FFFD` and no overlong or surrogate validation. Implemented as the
   C++ does; pinned by `utf8_narrowing_drops_invalid_bytes_silently` and
   `utf8_narrowing_keeps_the_low_byte`.
3. **Q3 confirmed — no promotion below `/V 4`.** `cpdf_security_handler.cpp:270`
   places `if (nKeyBits < 40) nKeyBits *= 8;` inside the `Version >= 4` branch,
   so `/V 2 /Length 8` really does resolve to a 1-byte key and a rejected
   document. The T14 row stands as the brief marked it.
4. **`bug_644.pdf`'s passwords are the reverse of what the brief says.** The
   brief T10 (following the C++ test *names* `OwnerPasswordVersion5` /
   `UserPasswordVersion5`) calls `"a"` the owner password and `"b"` the user
   one. Recomputing Algorithm 2.A against the fixture's real `/O`, `/U`, `/OE`,
   `/UE` shows the opposite: `"b"` validates as owner, `"a"` as user. The
   embedder test cannot tell them apart, because `/P 4092` masks to
   `0xFFFFFFFC` for both roles — which is presumably why that `/P` was chosen.
   The test is written the correct way round with the reason recorded beside it.

Also worth recording for the parser brief: the accessor kinds in
`cpdf_security_handler.cpp` are not uniform, and the difference is behavioral.
`GetNameFor("Filter")` and `GetBooleanFor("EncryptMetadata")` type-check
*before* resolving, so a reference or a wrong-typed value there reads as absent
(a string-valued `/Filter (Standard)` is **not** the standard handler, and an
`Int(0)` `/EncryptMetadata` does **not** turn metadata encryption off).
`GetIntegerFor` and `GetByteStringFor` coerce any type and follow one reference,
and `GetDictFor("CF")` resolves. The crate uses `pdfrum-object`'s non-resolving
`name`/`bool` and resolving `int`/`byte_string`/`dict` accordingly, and both
behaviors are pinned by tests.

## Cross-crate obligations recorded for `pdfrum-parser`

Both are documented on the crate's module docs so the parser brief does not
have to rediscover them:

1. **The signature exemption** (brief §1.12). The decrypt walk defers a
   `/Contents` whose parent dictionary has `/Type` or `/FT`; after the parent is
   decrypted, `pdfrum_crypt::is_signature_dict(parent)` decides whether the
   subtree stays undecrypted. The predicate reads the direct `/Type`, falling
   back to `/FT` only when `/Type` is absent entirely.
2. **The metadata exemption** (brief §1.12, Q1). The object `/Root/Metadata`
   points at is exempt when `encrypt_metadata()` is false. Q1's inverted
   predicate on the linearized path (`cpdf_parser.cpp:1287` vs `:305`) is the
   parser's call to make, not this crate's — the brief's proposal is to
   implement `:305`'s sense uniformly and escalate only if a linearized
   `/EncryptMetadata false` corpus file diffs against the oracle.

Q5 (should `decrypt` report damage?) stands as the brief proposed: the SPEC §3
signature has no `&mut Diagnostics`, the parser can compare input and output
lengths if it wants the tag, and the three swallowed recoveries — a dropped
final AES block, a discarded partial tail, an under-17-byte AES payload — are
documented on `decrypt` itself.

## The encrypt direction (M10, 2026-08-30)

D2's "decrypt only" is lifted; SPEC §3 records the ruling.
`SecurityHandler::encrypt(obj, class, iv, data)` is the inverse of `decrypt`,
and `pdfrum-edit` uses it to save an encrypted document encrypted under the
handler the original password opened.

Three things are worth knowing before touching it again:

1. **The object-key derivation is shared with decrypt, deliberately.** The
   C++'s `EncryptContent` truncates the AESV2 object key to the *file* key's
   length (`realkey.first(key_len_)`) where `DecryptStart` uses all sixteen
   bytes. The two agree at the only key length AESV2 ever has — 16 — and at 24
   the C++ would ask a 16-byte array for 24 bytes and abort, so there is no
   document on which the divergence is observable. Sharing one derivation is
   what makes encrypt-then-decrypt byte-exact, and the round-trip tests would
   fail loudly if it were split.
2. **The AES quirks are decrypt-only.** The one-block lag, the missing padding
   validation, the dropped partial tail (D6) are a *reader's* tolerance for
   files other producers wrote. The writer emits standard PKCS#7 with the
   caller's vector, which those rules accept exactly: their "last plaintext
   byte under sixteen strips that many" agrees with PKCS#7 on every value 1
   through 16, and the always-present pad block is what stops a 16-byte
   payload reading back as an empty one.
3. **An empty payload encrypts to empty.** The C++ tests for it in
   `CPDF_Encryptor::Encrypt`, one level above `EncryptContent`, so an empty
   string stays `()` rather than becoming 32 bytes of vector and padding. Easy
   to miss, and it changes the bytes of every document with an empty string in
   it.

Randomness stayed out of the crate: `Iv` is a caller argument, so there is no
global state and no `getrandom` in DEPS.md. `pdfrum-edit` derives vectors from
the document's bytes and a counter, which is what makes a save reproducible.

## Not in scope here

Building an `/Encrypt` dictionary — `OnCreate`, `AES256_SetPassword`,
`AES256_SetPerms`. v1 preserves passwords and never sets them, so `/O`, `/U`,
`/OE`, `/UE` and `/Perms` are copied from the file rather than derived.
Changing a document's password or its encryption mode is post-1.0 (PLAN.md
Phase 2's closing list).
