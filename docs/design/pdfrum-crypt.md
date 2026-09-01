# Design Brief — `pdfrum-crypt`

**Behavior source (read-only oracle):** `/mnt/data2/pdfium/pdfium-c++`
- `core/fdrm/fx_crypt.h` / `.cpp` — RC4 + MD5
- `core/fdrm/fx_crypt_sha.h` / `.cpp` — SHA-1/256/384/512
- `core/fdrm/fx_crypt_aes.h` / `.cpp` — AES-CBC (128/192/256)
- `core/fpdfapi/parser/cpdf_security_handler.h` / `.cpp` — standard security handler, /R 2..6
- `core/fpdfapi/parser/cpdf_crypto_handler.h` / `.cpp` — per-object key derivation + stream/string decrypt
- `core/fpdfapi/parser/cpdf_parser.cpp` — wiring: when the handler is built, which objects are exempt
- Tests: `core/fdrm/fx_crypt_unittest.cpp`, `core/fpdfapi/parser/cpdf_security_handler_embeddertest.cpp`

**Contract:** SPEC.md §3. **Style:** STYLE.md (data + functions, enums, no panics, `forbid(unsafe_code)`).

This brief is written to be sufficient on its own: an implementing agent should
need SPEC.md + STYLE.md + this file, and never the C++.

---

## 1. Behavior inventory

### 1.1 Scope: what PDFium's standard security handler actually implements

Only `/Filter /Standard` is supported. `cpdf_parser.cpp:352-354`:

```
if (pEncryptDict->GetNameFor("Filter") != "Standard") { return HANDLER_ERROR; }
```

Note the asymmetry the C++ itself carries: the *check* uses `GetNameFor`
(name-typed), while `CPDF_SecurityHandler::GetPermissions`
(`cpdf_security_handler.cpp:224-229`) re-tests with `GetByteStringFor("Filter")
== "Standard"`, which also matches a *string*-typed `/Filter`. This only affects
the permissions mask, never whether the document opens — the parser has already
rejected non-`Standard` handlers by then.

Public-key (`/Filter /Adobe.PubSec`) handlers are **not implemented at all** —
the document simply fails to open with a handler error.

### 1.2 The `/Encrypt` dictionary → cipher + key length resolution

`LoadCryptInfo`, `cpdf_security_handler.cpp:233-292`. This is the single point
where `/V`, `/Length`, `/CF`, `/CFM` become a `(cipher, key_len_bytes)` pair.

For `/V < 4` (`:279-281`):
- cipher is always **RC4**.
- `key_len = /V > 1 ? /Length_default_40 / 8 : 5`.
  So `/V 1` ⇒ **5 bytes (40-bit), ignoring `/Length` entirely**; `/V 2` and
  `/V 3` ⇒ `/Length` in **bits**, defaulting to 40, integer-divided by 8.

For `/V >= 4` (`:241-278`):
- `/CF` must be present as a dictionary, else **fail** (`:242-245`).
- The filter name used is `/StrF` (see §1.3); if that name is literally
  `Identity`, cipher becomes **None** and `keylen` stays **0** (`:248-250`).
- Otherwise `/CF` must contain a sub-dictionary under that name, else **fail**
  (`:251-255`).
- Key bits (`:257-265`):
  - `/V == 4`: `nKeyBits = CF[name]/Length` (default 0); if 0, fall back to
    `/Encrypt/Length` with **default 128**.
  - `/V >= 5`: `nKeyBits = /Encrypt/Length` with **default 256**. Note the
    per-filter `/Length` is *ignored* at V5.
  - `nKeyBits < 0` ⇒ fail.
  - **`if (nKeyBits < 40) nKeyBits *= 8;`** (`:270-272`) — the damage-tolerance
    quirk that lets a file write `/Length 16` (meaning *bytes*) and still work:
    16 < 40 ⇒ 128 bits ⇒ 16 bytes. `encrypted.pdf` in the corpus relies on this
    (`/CF/StdCF/Length 16` with `/Encrypt/Length 128`).
  - `keylen = nKeyBits / 8`.
- Cipher from `/CF[name]/CFM`: `"AESV2"` or `"AESV3"` ⇒ **AES**; anything else
  (including `"V2"`, `"None"`, absent, or an unknown name) ⇒ stays **RC4**
  (`:274-277`). PDFium never distinguishes AESV2 from AESV3 by name — the
  distinction is carried entirely by `key_len == 32`.

Final validation (`:283-288`):
- `keylen < 0 || keylen > 32` ⇒ fail.
- `IsValidKeyLengthForCipher` (`:89-101`):
  - AES: `keylen ∈ {16, 24, 32}`
  - AES2 (unused by the load path): `keylen == 32`
  - RC4: `5 <= keylen <= 16`
  - None: any
  Failure here ⇒ `LoadDict` fails ⇒ `OnInit` returns false ⇒ the *document does
  not open* (reported by the parser as a password error, see §1.9).

### 1.3 Crypt filters `/StmF`, `/StrF`, `/EFF`

`LoadDict`, `cpdf_security_handler.cpp:294-310`:

```
if (version_ < 4) return LoadCryptInfo(dict, ByteString(), ...);   // name unused
stmf_name = dict->GetByteStringFor("StmF");
strf_name = dict->GetByteStringFor("StrF");
if (stmf_name != strf_name) return false;                          // HARD FAIL
return LoadCryptInfo(dict, strf_name, ...);
```

Load-bearing facts:

1. **PDFium refuses documents where `/StmF != /StrF`.** There is no
   per-class cipher; `CryptClass::Stream` and `CryptClass::String` always
   resolve to the same cipher and key. The comparison is on the raw bytes, so
   an absent `/StmF` (empty string) with an explicit `/StrF /StdCF` is a
   mismatch and the document fails to open.
2. When both are absent (`/V 4` with no `/StmF`/`/StrF`), the looked-up name is
   the **empty string**, which is neither `"Identity"` nor a key in `/CF` ⇒
   `LoadCryptInfo` fails at `:251-255`. Per ISO 32000 the default is
   `/Identity`; PDFium does **not** implement that default and rejects the file.
3. **`/EFF` is entirely unimplemented.** Grep of the whole `core/` tree finds no
   reference. Embedded-file streams are decrypted with the same
   `/StmF`-resolved cipher as any other stream. `CryptClass::Embedded` in our
   SPEC therefore behaves identically to `Stream` (see §2, Divergence D1).
4. `/AuthEvent` is read by nobody; it is inert.

### 1.4 `/R 2..4`: the MD5 key-derivation algorithm (ISO 32000 Alg. 2)

The padding string, `cpdf_security_handler.cpp:35-38` — 32 bytes, verbatim:

```
28 BF 4E 5E 4E 75 8A 41 64 00 4E 56 FF FA 01 08
2E 2E 00 B6 D0 68 3E 80 2F 0C A9 FE 64 53 69 7A
```

`GetPassCode` (`:40-48`): take `min(password.len, 32)` bytes of the password,
then fill the remainder from the *front* of the pad. A ≥32-byte password is
truncated to 32 with no padding.

`CalcEncryptKey` (`:50-87`) — feeds an MD5 in this exact order:
1. the 32-byte padded passcode,
2. `/O` **as raw bytes, whatever its length** (no 32-byte requirement here —
   a short or long `/O` is hashed verbatim; this is the crash fix behind
   `encrypted_hello_world_r{2,3}_bad_okey.pdf`),
3. `/P` as a **little-endian 4-byte `uint32`** — the value is read via
   `GetIntegerFor("P")` (a signed int, typically negative) and reinterpreted as
   `uint32`; on all supported platforms this is LE byte order of the two's
   complement,
4. the **first** element of the trailer `/ID` array as raw bytes, **only if
   non-empty** (`:68-70`). A missing `/ID` contributes nothing — not even a
   length marker,
5. if `!ignore_metadata && /R >= 3 && /EncryptMetadata == false`: four bytes
   `FF FF FF FF` (`:72-76`).

Then `digest = MD5.finish()` (16 bytes), `copy_len = min(key_len, 16)`, and for
`/R >= 3` **exactly 50 iterations** of `digest = MD5(digest[0..copy_len])`
(`:81-85`). Note the subtlety: each iteration hashes only the first `copy_len`
bytes of the previous digest but writes a full 16-byte digest. Finally the first
`key_len` bytes become the file encryption key.

`ignore_metadata` is the `bIgnoreEncryptMeta` flag threaded from
`CheckUserPassword`.

### 1.5 `/R 2..4`: user-password check (ISO 32000 Alg. 4/5)

`CheckUserPassword`, `cpdf_security_handler.cpp:471-508`.

Derive the key with `CalcEncryptKey(..., bIgnoreEncryptMeta, file_id)`, then:

- `/U` shorter than **16 bytes** ⇒ return false (damage guard, `:478-480`).
- **`/R == 2`** (`:483-488`): `buf = pad(32 bytes)`, `RC4_encrypt(buf, key)`,
  compare the **first 16 bytes** against `/U[0..16]`. (Note: only 16 of the 32
  bytes are compared even at R2.)
- **`/R >= 3`** (`:490-507`):
  1. `test[32] = {0}`; copy `min(32, /U.len)` bytes of `/U` into it.
  2. For `i` from **19 down to 0**: `tmpkey[j] = key[j] ^ i` for
     `j < key_len`; `RC4_crypt_in_place(test /* all 32 bytes */, tmpkey)`.
     RC4 is an involution, so this undoes the 20 encryption rounds.
  3. `expected = MD5(pad || file_id_if_nonempty)` (16 bytes).
  4. Compare `test[0..16]` against `expected` — **only the first 16 bytes**.

`CheckPasswordImpl` for `/R < 5` tries the user path **twice**
(`:467-468`): `CheckUserPassword(pw, false) || CheckUserPassword(pw, true)` —
i.e. once honoring `/EncryptMetadata` and once ignoring it. This is pure
damage tolerance for files that wrote `/EncryptMetadata false` but computed
`/U` without the `FF FF FF FF` tag. **The side effect matters: `encrypt_key_`
is left holding whatever key the *successful* call derived**, because
`CalcEncryptKey` writes into the member before the comparison.

### 1.6 `/R 2..4`: owner-password check (ISO 32000 Alg. 7)

`GetUserPassword` + `CheckOwnerPassword`, `cpdf_security_handler.cpp:510-556`.

`GetUserPassword(owner_password)`:
- `/O` shorter than **32 bytes** ⇒ return empty string immediately
  (`:512-516`). This is the `bad_okey` crash fix; the resulting empty "user
  password" then fails `CheckUserPassword`, so the document does not open.
- `digest = MD5(pad(owner_password))`; if `/R >= 3`, **50 iterations** of
  `digest = MD5(digest)` — here the *full 16 bytes* are re-hashed each round
  (contrast §1.4, which hashes `copy_len` bytes).
- `enckey[0..min(key_len,16)] = digest`.
- `okeybuf = /O[0..32]`.
- `/R == 2`: one `RC4_crypt(okeybuf, enckey[0..key_len])`.
  `/R >= 3`: for `i` from **19 down to 0**, `tempkey[j] = enckey[j] ^ i`,
  `RC4_crypt(okeybuf, tempkey[0..key_len])`.
- **Trailing-pad strip** (`:545-548`): while `len > 0 && pad[len-1] ==
  okeybuf[len-1]`, decrement `len`. This recovers the user password by
  stripping the padding *from the tail*, comparing against the pad bytes at the
  same index. It is not a general "strip pad" — a recovered password whose own
  last byte coincidentally equals the pad byte at that position gets one byte
  too many stripped. Behavior must be reproduced exactly.

`CheckOwnerPassword` then runs the recovered string through
`CheckUserPassword(pw, false) || CheckUserPassword(pw, true)`.

### 1.7 `/R 5` and `/R 6`: AES-256 (`/V 5`)

`AES256_CheckPassword`, `cpdf_security_handler.cpp:338-423`.

Length guards first: `/O` and `/U` must each be **≥ 48 bytes** (`:343-350`),
else false. Then, with `pkey` = `/U` for the user check and `/O` for the owner
check, and `vector` = `/U[0..48]` **only in the owner case** (`:351-356`):

- **Validation salt** = `pkey[32..40]` (8 bytes); **key salt** = `pkey[40..48]`.
- Hash 1 (`:358-370`): `/R >= 6` ⇒ `Revision6_Hash(password, validation_salt,
  vector)`; `/R == 5` ⇒ plain `SHA256(password || validation_salt || vector?)`.
  Compare against `pkey[0..32]`; mismatch ⇒ **false**.
- Hash 2 (`:375-386`): same construction with the **key salt** → 32-byte
  intermediate key.
- `/OE` (owner) or `/UE` (user) must be **≥ 32 bytes** (`:387-390`), else false.
- `encrypt_key_ = AES256_CBC_decrypt(key = hash2, iv = 16 zero bytes,
  ct = ekey[0..32])` — **no padding, no padding check**; the 32-byte
  ciphertext decrypts to exactly the 32-byte file key (`:392-396`).
- `/Perms` must be non-empty (`:399-402`); it is copied into a **16-byte buffer
  zero-initialized then filled with `min(16, perms.len)` bytes** — a short
  `/Perms` is zero-padded rather than rejected (`:404-406`).
- `buf = AES256_ECB-equivalent_decrypt(key = encrypt_key_, iv = zeros,
  ct = perms_buf)`. Note the C++ reuses the CBC routine with a zero IV over a
  single block, which is identical to ECB for one block (`:397-409`).
- Checks on the decrypted `buf` (`:410-422`):
  - `buf[9..12] == 'a','d','b'` else false;
  - `u32_LE(buf[0..4]) == permissions_` (the `/P` value as `uint32`) else false;
  - **relaxed metadata check:** `buf[8] == 'F' || IsMetadataEncrypted()`.
    A comment in the C++ explains this: some non-conforming producers disagree
    between `/Perms` and `/EncryptMetadata`; the buffer is treated as the
    truth, and the *only* rejected combination is
    "`/Perms` says metadata IS encrypted (`buf[8] != 'F'`) while
    `/EncryptMetadata` is false".

**`Revision6_Hash` — the hardened iterated hash (ISO 32000-2 Alg. 2.B).**
`cpdf_security_handler.cpp:114-183`. This is the trickiest routine in the crate.

```
K = SHA256(password || salt || vector?)          // 32 bytes
block_size = 32
i = 0
loop {
    // K1 = 64 repetitions of (password || K[0..block_size] || vector?)
    round_len = password.len + block_size + (vector ? 48 : 0)
    K1 = concat of 64 copies of (password || K[0..block_size] || vector?)
    // AES-128-CBC encrypt K1 with key = K[0..16], iv = K[16..32]
    E = AES128_CBC_encrypt(key = K[0..16], iv = K[16..32], K1)
    // select the next hash by the first 16 bytes of E, big-endian, mod 3
    match big_order_64bits_mod3(E[0..16]) {
        0 => { block_size = 32; K = SHA256(E) }
        1 => { block_size = 48; K = SHA384(E) }
        _ => { block_size = 64; K = SHA512(E) }
    }
    i += 1
    if !(i < 64 || i - 32 < E.last_byte()) { break }
}
hash = K[0..32]
```

Details that must be exact:

- **`BigOrder64BitsMod3`** (`:103-112`) is *not* the spec's "sum of the first 16
  bytes mod 3". It reads **four** big-endian `u32`s from `E[0..16]` and folds
  them as `ret = ((ret << 32) | word) % 3` after each word, with `ret` a
  `uint64`. Because `2^32 mod 3 == 1`, this is arithmetically equal to
  `(sum of the four u32 values) mod 3`, which in turn equals the byte-sum mod 3
  — but implement the loop as written to avoid any doubt.
- The **key and IV for each round come from the *current* `K`**: `key = K[0..16]`,
  `iv = K[16..32]`, refreshed after every round (`:176-178`).
- `K1` is built from `K[0..block_size]`, but the AES key/IV always come from
  `K[0..16]`/`K[16..32]` regardless of `block_size`.
- The loop condition is evaluated **after** `i` is incremented, and
  `E.last_byte()` is `encrypted_output.back()` — the last byte of the
  **full** `K1`-sized ciphertext (`64 * round_len` bytes), not of the digest.
  Comparison `i - 32 < E.last_byte()` is between a signed `int` and a
  `uint8_t` promoted to `int`; with `i >= 64` at that point, `i - 32 >= 32`,
  so it terminates once `E.last_byte() <= i - 32`. Minimum 64 rounds; maximum
  `32 + 255 = 287`.
- `K1` length is `64 * round_len`, which is a multiple of 16 only because AES
  CBC here is applied to the whole buffer at once — **it is not**
  necessarily a multiple of 16. The C++ `CRYPT_AESEncrypt` `CHECK`s
  `src.size() % 16 == 0` and would abort otherwise. In practice `round_len`
  is `pw_len + 32|48|64 (+48)`; if `pw_len % 16 != 0` this is a non-multiple
  and `64 * round_len` is still `64 *` that, i.e. a multiple of 64 ⇒ multiple
  of 16. So the ×64 repetition is what guarantees block alignment. Our
  implementation must preserve the ×64 structure for this reason.

### 1.8 Password encoding fallback (Latin-1 ⇄ UTF-8)

`CheckPassword`, `cpdf_security_handler.cpp:425-455`. Tried in order:

1. The password bytes **as given**. Success ⇒ conversion = `None`.
2. If the password `IsASCII()` ⇒ **stop, return false**. ASCII is a fixed point
   of both conversions, so retrying is pointless.
3. `/R >= 5`: retry as `Latin1→UTF-8` (interpret each byte as a Latin-1 code
   point, re-encode UTF-8). Success ⇒ conversion = `Latin1ToUtf8`.
   `/R < 5`: retry as `UTF-8→Latin-1` (decode UTF-8, re-encode each code point
   ≤ 0xFF as one byte). Success ⇒ conversion = `Utf8toLatin1`.

The direction flips at R5 because R2–R4 hash raw bytes (so a UTF-8 password
must be narrowed) while R5/R6 nominally take SASLprep'd UTF-8 (so a Latin-1
password must be widened). PDFium does **not** implement SASLprep.

The embeddertest fixtures pin this: `"âge"` and `"hôtel"` in both UTF-8
(`\xc3\xa2ge`, `h\xc3\xb4tel`) and Latin-1 (`\xe2ge`, `h\xf4tel`) forms open the
same documents at every revision (`cpdf_security_handler_embeddertest.cpp:24-38,
213-657`).

The recorded conversion is exposed as `GetEncodedPassword` (`:562-575`) and used
by the *save* path to re-encrypt with the same byte form. Since `pdfrum-crypt`
does not encrypt (§2, D2), we retain the enum only as reportable state.

### 1.9 Owner-vs-user ordering and permissions

`CheckSecurity` (`:213-219`):

```
if (!password.IsEmpty() && CheckPassword(password, /*bOwner=*/true)) {
    owner_unlocked_ = true;
    return true;
}
return CheckPassword(password, /*bOwner=*/false);
```

So: a **non-empty** password is tried as the owner password first; the empty
password is only ever tried as a user password. Both `/R<5` and `/R>=5` share
this ordering.

`GetPermissions(get_owner_perms)` (`:221-231`):
- base = `owner_unlocked_ && get_owner_perms ? 0xFFFFFFFF : permissions_`
  where `permissions_ = /P` with **default −1** (`:298`), read as `int` and
  stored as `uint32`.
- If the handler is `Standard`: `perm &= 0xFFFFFFFC; perm |= 0xFFFFF0C0;`
  — reserved bits 1–2 cleared, bits 7–12 and 13–32 forced set.

Pinned by embeddertest (`:107-139`):
| document | password | `GetDocPermissions` | `GetDocUserPermissions` |
|---|---|---|---|
| `about_blank.pdf` (unencrypted) | none / `"foobar"` | `0xFFFFFFFF` | `0xFFFFFFFF` |
| `encrypted.pdf` | `"1234"` (user) | `0xFFFFF2C0` | `0xFFFFF2C0` |
| `encrypted.pdf` | `"5678"` (owner) | `0xFFFFFFFC` | `0xFFFFF2C0` |
| `bug_644.pdf` (R5) | `"a"` (owner) or `"b"` (user) | `0xFFFFFFFC` | — |

Note `bug_644.pdf` reports owner permissions for the *user* password too: its
`/P` is 4092 = `0xFFC`, which after the mask is `0xFFFFFFFC` either way.

Failure of `CheckSecurity` ⇒ `OnInit` false ⇒ parser returns `PASSWORD_ERROR`
(`cpdf_parser.cpp:357-360`). A `LoadDict` failure (bad `/V`, `/CF`, key length,
`/StmF != /StrF`) also surfaces as `PASSWORD_ERROR`, not as a distinct
"unsupported" status — see Divergence D3.

### 1.10 Per-object keys (ISO 32000 Alg. 1)

`CPDF_CryptoHandler::PopulateKey`, `cpdf_crypto_handler.cpp:347-356`:

```
key[0..key_len]      = file_key
key[key_len + 0]     = objnum        & 0xFF
key[key_len + 1]     = (objnum >> 8) & 0xFF
key[key_len + 2]     = (objnum >> 16)& 0xFF
key[key_len + 3]     = gennum        & 0xFF
key[key_len + 4]     = (gennum >> 8) & 0xFF
```

Object number contributes **3 bytes**, generation **2 bytes**, both
little-endian, both **truncated** (an objnum ≥ 2^24 silently wraps).

Then, in `DecryptStart` (`:117-151`):
- **AES with `key_len == 32`** (AESV3 / `/V 5`): the per-object salting is
  **skipped entirely** — the file key is used directly (`:126-129`). This is
  correct per ISO 32000-2 and is the single most important branch to get right.
- **AES with `key_len != 32`** (AESV2): build a 48-byte scratch, `PopulateKey`,
  then write the four ASCII bytes **`sAlT`** (`0x73 0x41 0x6C 0x54`) at offset
  `key_len + 5`, and take `real_key = MD5(scratch[0 .. key_len + 9])`
  (16 bytes). The AES key is then `real_key` — **all 16 bytes**, because
  `CRYPT_AESSetKey` is called with the full `realkey` array (`:137`).
- **RC4**: `real_key = MD5(scratch[0 .. key_len + 5])`, and the RC4 key length
  is **`min(key_len + 5, 16)`** (`:145-146`) — i.e. capped at 16, which is why
  a 16-byte file key yields a 16-byte object key rather than 21.

The `EncryptContent` path (`:61-115`) has a subtly different key length:
`realkeylen = min(key_len + 5, 16)` is applied to **both** RC4 and the
non-32-byte AES case there, whereas `DecryptStart` uses the full 16 bytes for
AES. Since the AES branch of `EncryptContent` explicitly re-derives with
`realkey.first(key_len_)` (`:85-87`), encrypt and decrypt disagree about the AES
object-key length for AESV2 with `key_len != 16`. In practice AESV2 is always
`key_len == 16`, so the paths coincide. We implement the **decrypt** semantics.

### 1.11 Stream and string decryption

`DecryptStream` / `DecryptFinish`, `cpdf_crypto_handler.cpp:153-227`.

**RC4:** the whole payload is RC4'd in place with the object key. No length
transformation. Empty input ⇒ empty output.

**AES-CBC (both AESV2 and AESV3):**
- The **first 16 bytes of the ciphertext are the IV** and are consumed, not
  emitted.
- Remaining bytes are decrypted in 16-byte blocks, but with a **one-block
  lag**: a block is only emitted once *more input has arrived*
  (`:191` `else if (src_off < source.size())`). The final buffered block is
  handled by `DecryptFinish`.
- `DecryptFinish` (`:216-224`): if exactly 16 bytes are buffered, decrypt them
  and inspect the last plaintext byte `p`:
  - `p < 16` ⇒ emit the first `16 - p` bytes (PKCS#7-style strip);
  - `p >= 16` ⇒ **emit nothing** — the entire final block is dropped.
  There is **no validation** that the padding bytes are all equal to `p`, and
  no error on a bad pad. `p == 0` emits all 16 bytes (a full block of "zero
  padding" is kept), which differs from strict PKCS#7 where 0 is invalid.
- A ciphertext whose length is **not** `16 + 16k` leaves a partial block in the
  buffer at finish time; `block_offset_ != 16` ⇒ **nothing is emitted for the
  tail**. Trailing partial bytes are silently discarded.
- A ciphertext of **exactly 16 bytes** yields empty output: the 16 bytes become
  the IV, nothing remains.

**AES streams shorter than 16 bytes are replaced by an empty stream** before
decryption is even attempted (`DecryptObjectTree`, `:295-298`):
```
if (IsCipherAES() && stream_access->GetSize() < 16) { stream->SetData({}); continue; }
```

If `DecryptStream`/`DecryptFinish` report failure the stream is likewise set
empty (`:307-312`). In practice they only fail on a null context.

`DecryptGetSize` (`:239-241`) is a size *estimate* used to pre-reserve the
output buffer: `AES ? src_size - 16 : src_size`. It underflows for
`src_size < 16`, but the `< 16` guard above prevents that path from being
reached with a real stream.

### 1.12 Which objects are decrypted, and the signature-dictionary dance

`DecryptObjectTree`, `cpdf_crypto_handler.cpp:247-327`, walks the whole
indirect object recursively:

- **Every `Str` is decrypted**, using the **top-level indirect object's**
  `(objnum, gennum)` — not any nested object's. Direct strings nested inside an
  indirect object share its key.
- **Every `Stream` is decrypted**, again with the top-level object's numbers.
- **Signature exemption:** when the walker reaches a key named `Contents`
  whose *parent dictionary* has either a `/Type` or an `/FT` key, that subtree
  is **deferred**, not decrypted, and the walk skips into it (`:267-279`). After
  the main walk, each deferred `(parent, contents)` pair is re-examined: if
  `IsSignatureDictionary(parent)` — i.e. the parent's direct `/Type`, or `/FT`
  if `/Type` is absent, has the string value `"Sig"` (`:48-59`) — the contents
  stay **undecrypted**; otherwise the walk resumes into them.
  The reason (per the C++ comment): at deferral time `/Type` and `/FT` are
  still encrypted strings, so the test cannot be made until the enclosing
  dictionary has been decrypted.
  Note the latent null-deref shape at `:268`: `parent_dict->KeyExist(...)` is
  called without checking `parent_dict != nullptr` — reachable only if the
  walker yields a `Contents` key with a non-dictionary parent.
- The loop processes **one deferred subtree per outer iteration**, popping from
  a LIFO stack until it finds a non-signature parent.

Parser-level exemptions (`cpdf_parser.cpp:1157-1163`):
```
should_decrypt = security_handler_ && crypto_handler && objnum != metadata_objnum_;
```
- The `/Root/Metadata` object is exempt when `/EncryptMetadata` is false
  (`:305-311`).
- **Bug worth noting:** the *linearized* main-xref path at `:1287-1292` uses the
  **opposite** predicate — `if (security_handler_ && IsMetadataEncrypted())` —
  so on that path the metadata object number is recorded (and thus exempted from
  decryption) precisely when metadata *is* supposed to be encrypted. This looks
  like an inverted condition. See Open Question Q1.
- Cross-reference **streams and object streams are decrypted like any other
  object** by this code path — but per ISO 32000 the xref stream is never
  encrypted. PDFium gets away with it because the xref stream is parsed before
  the security handler exists.

### 1.13 Primitive-level facts (`core/fdrm`)

These bind the RustCrypto choices in SPEC §3.

**RC4** (`fx_crypt.cpp:131-162`). Textbook KSA/PRGA, `kPermutationLength = 256`
(`fx_crypt.h:18`). One quirk (`:142`): an **empty key** is handled as
`key[i % 0]` ⇒ the code substitutes `0`, so `CryptArcFourSetup(ctx, {})`
produces a well-defined permutation (pinned by a unittest KAT, §4.1). We must
reproduce that rather than reject an empty key. The permutation is stored as
`int32_t[256]` in the C++; that is an implementation artifact, not behavior.

**MD5** (`fx_crypt.cpp:164-225`) — standard RFC 1321. RustCrypto `md-5` matches;
KATs in §4.1.

**SHA** (`fx_crypt_sha.h:30-56`): SHA-1 (20-byte digest), SHA-256 (32),
SHA-384 (48), SHA-512 (64). All four are needed: SHA-1 only for
`AES256_SetPassword` (encryption, out of scope) and for `CPDF_StreamAcc`'s
non-cryptographic content digest; SHA-256/384/512 for R6.

**AES** (`fx_crypt_aes.cpp:529-632`):
- `CRYPT_AESSetKey` hard-`CHECK`s `key.size() ∈ {16, 24, 32}` (`:530`) —
  a process abort in the C++, which we replace with a typed error.
  `Nr = 6 + key_len/4` ⇒ 10 / 12 / 14 rounds.
- `CRYPT_AESDecrypt` `CHECK`s `src.size() % 16 == 0` **and**
  `src.size() == dest.size()` (`:593-594`); `CRYPT_AESEncrypt` `CHECK`s only
  the block-multiple (`:619`) — an asymmetry, not a behavior we need.
- **CBC chaining state lives in the context across calls.** Encrypt aliases
  `ctx->iv` directly and leaves the last ciphertext block there (`:620-630`);
  decrypt copies it out, chains locally, and writes back at `:613`. Consecutive
  calls therefore continue one CBC stream. Every place we use AES in this crate
  either sets a fresh IV first or decrypts a single block, so a stateless
  RustCrypto `cbc::Decryptor` is a faithful replacement — with one exception:
  §1.11's streaming AES decrypt must chain across the whole stream.

---

## 2. Divergences

**D1 — `CryptClass::Embedded` collapses onto `Stream`.** SPEC §3 names a
three-variant `CryptClass`. PDFium ignores `/EFF` entirely (§1.3), so
`Embedded` must decrypt exactly like `Stream` to stay oracle-faithful. We keep
the variant (it documents the PDF concept and gives us a place to implement
`/EFF` later without a signature change) but `decrypt` matches `Stream |
Embedded` in one arm. A doc comment records the reason.

**Reversed 2026-09-02 (oracle-divergence audit, A26): `/EFF` is implemented,
and the collapse was reproducing an oracle bug.** `grep '"EFF"' core/
fpdfsdk/` over the oracle returns **zero hits**;
`CPDF_SecurityHandler::LoadDict` (`cpdf_security_handler.cpp:303-311`) takes
one filter name and builds one `CPDF_CryptoHandler`, so an embedded file
stream decrypts with the *stream* filter whatever `/EFF` says — and a
document whose `/EFF` names an AES filter while `/StmF` names an RC4 one
silently produces garbage for every attachment. ISO 32000-1 §7.6.5 table 20
defines `/EFF` as a distinct default for embedded file streams, independent
of `/StmF`; pdf.js carries it separately, reading it with the `/StmF` default
at `crypto.js:1120` (`eff = dict.get("EFF") || stmf`), consulting it at
`:1206` and handing it to the cipher transform as `embeddedFilterName` at
`:1336`. PLAN.md's oracle-bug rule (§212–229) therefore obliges the correct
behaviour, and the D1 paragraph's own "place to implement `/EFF` later
without a signature change" is exactly what was used.

*What shipped:* `standard::embedded_cipher` resolves `/EFF` — the key's
absence, a name equal to `/StmF`'s, and a `/CFM` resolving to the same cipher
all mean "no override", which is table 20's default written three ways;
`/Identity` gives `Cipher::None`; a name `/CF` does not carry falls back
rather than refusing the document, since the streams still decrypt.
`EncryptParams` and each `SecurityHandler` variant carry
`embedded_cipher: Option<Cipher>`, and `decrypt`/`encrypt` route
`CryptClass::Embedded` through it. Only the **cipher** can differ: §7.6.5
gives every `/CF` entry the one file encryption key, so the override runs a
different algorithm over the same key. `pdfrum-parser` passes
`CryptClass::Embedded` for a stream whose `/Type` is `/EmbeddedFile`, read
off the dictionary before the decrypt — safe, because a name is never
enciphered. Blast radius **0 rows**: no corpus file carries an `/EFF` at all,
so the six tests build their fixture on top of `encrypted.pdf`'s dictionary
rather than taking one from the corpus. The site carries `// [oracle-bug]`
with both citations.

**D2 — decryption only; no encryption.** *(Half-lifted in M10, 2026-08-30.)*
PDFium's `OnCreate`, `AES256_SetPassword`, `AES256_SetPerms` and
`EncryptContent` exist to write encrypted files. This brief inventories the
encrypt paths only where they clarify a decrypt path (§1.10).

M10 needed the payload half and took it, as the `[spec]` change to SPEC §3
this paragraph anticipated: `SecurityHandler::encrypt(obj, class, iv, data)`
is now `EncryptContent`'s counterpart, with the object-key derivation shared
with decrypt (§1.10's note on the AESV2 length disagreement resolves in the
decrypt reading, since the two coincide at every reachable key length), a
standard PKCS#7 pad rather than the decrypt side's quirks (§1.11's rules are
what a *reader* tolerates), a caller-supplied `Iv` so no randomness enters
this crate, and an empty payload short-circuiting to empty as
`CPDF_Encryptor::Encrypt` does above the cipher.

The **dictionary** half stays deferred and is now post-1.0: `OnCreate`,
`AES256_SetPassword` and `AES256_SetPerms` write `/O`, `/U`, `/OE`, `/UE` and
`/Perms`, which only a caller that *chose* the passwords can do. v1 preserves
passwords, so it copies those entries instead. Re-keying and
encryption-mode conversion are on PLAN.md Phase 2's explicit post-1.0 list.

**D3 — richer failure typing.** The C++ collapses "unsupported handler",
"malformed `/Encrypt`", "`/StmF != /StrF`", "bad key length" and "wrong
password" into `PASSWORD_ERROR` / `HANDLER_ERROR` at the parser boundary. SPEC
§3 requires `WrongPassword` to be distinct; we go further and give each of the
above its own `Error` variant, because the CLI and the conformance harness both
need to tell "needs a password" from "we can't do this crypto". The *set of
documents that open* is unchanged — only the diagnosis differs.

**D4 — no `CHECK`-style aborts.** Every C++ `CHECK` in the crypto primitives
(`CRYPT_AESSetKey` on a bad key length, `CRYPT_AESDecrypt` on a
non-block-multiple) becomes an `Error` return. Per STYLE.md §3 there are no
panics in library code, and per SPEC §3 all of these are reachable from
attacker-controlled bytes.

**D5 — primitives come from RustCrypto, not hand-rolled.** `aes` + `cbc`,
`md-5`, `sha1`, `sha2` per DEPS.md. RC4 is ~30 lines in-crate (no suitable
maintained crate, and DEPS.md forbids adding one for that size). The C++'s
`int32_t[256]` permutation becomes `[u8; 256]`; behavior is identical.

**D6 — no streaming decrypt context.** The C++ exposes
`DecryptStart`/`DecryptStream`/`DecryptFinish` with a `void*` context so it can
decrypt while reading. SPEC §3 mandates a single `decrypt(&self, obj, class,
data) -> Vec<u8>`. The buffering behavior of §1.11 (one-block lag, final-block
padding strip, discarded partial tail) is reproduced *as an output rule*, not as
a state machine.

**D7 — `SecurityHandler` is a closed enum, not a class.** Per SPEC §3 and
STYLE.md §1. `Rc4V2` covers `/V 1..4` with RC4; `AesV4` covers AESV2 (16/24-byte
key with `sAlT`); `AesV5` covers AESV3 (`/V 5`, 32-byte key, no per-object
salting); `Identity` covers `/StrF /Identity` and the no-`/Encrypt` case.
The revision number is data inside the variant, not a subclass.

---

## 3. Module plan

```
crates/pdfrum-crypt/src/
├── lib.rs          // public surface + Error; the whole API in one screen
├── rc4.rs          // the ~30-line stream cipher
├── primitives.rs   // thin typed wrappers over RustCrypto: md5, sha*, aes_cbc_decrypt
├── standard.rs     // /Encrypt dict -> SecurityHandler; the /R 2..6 algorithms
└── object.rs       // per-object key derivation + stream/string decryption
```

### 3.1 `lib.rs` — public surface

```rust
#![forbid(unsafe_code)]

pub use standard::{EncryptParams, SecurityHandler};
pub use object::CryptClass;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the supplied password is not the user or owner password")]
    WrongPassword,
    #[error("/Filter {0:?} is not the standard security handler")]
    UnsupportedHandler(Box<[u8]>),
    #[error("unsupported /V {version} / /R {revision}")]
    UnsupportedRevision { version: i64, revision: i64 },
    #[error("/StmF and /StrF name different crypt filters")]
    MismatchedCryptFilters,
    #[error("crypt filter {0:?} is not present in /CF")]
    MissingCryptFilter(Box<[u8]>),
    #[error("/Encrypt is malformed: {0}")]
    MalformedEncryptDict(&'static str),
    #[error("key length {len} bytes is invalid for {cipher}")]
    CipherKeyLength { cipher: &'static str, len: usize },
}
```

`SecurityHandler`, per SPEC §3:

```rust
#[derive(Debug, Clone)]
pub enum SecurityHandler {
    /// /V 1..4 with RC4. `key` is `key_len` bytes (5..=16).
    Rc4V2 { key: SmallKey, revision: u8, permissions: u32, owner_unlocked: bool,
            encrypt_metadata: bool },
    /// AESV2: 16- or 24-byte key, per-object MD5 salting with "sAlT".
    AesV4 { key: SmallKey, revision: u8, permissions: u32, owner_unlocked: bool,
            encrypt_metadata: bool },
    /// AESV3 (/V 5, /R 5 or 6): 32-byte key used directly, no per-object salt.
    AesV5 { key: [u8; 32], revision: u8, permissions: u32, owner_unlocked: bool,
            encrypt_metadata: bool },
    /// No encryption, or /StrF /Identity.
    Identity,
}

/// A file encryption key of 5..=32 bytes. Invariant: `len <= 32`.
#[derive(Clone)]
pub struct SmallKey { bytes: [u8; 32], len: u8 }
```

`SmallKey` derives `Debug` **manually**, printing `SmallKey(<n bytes redacted>)`
— key material must never reach a log or a `Diagnostics` entry.

Entry points:

```rust
impl SecurityHandler {
    /// Build a handler from the trailer's /Encrypt dict.
    /// `file_id` is the FIRST element of the trailer /ID array, raw bytes;
    /// pass `&[]` when /ID is absent (the C++ contributes nothing in that case).
    /// `password` is the raw user-supplied bytes.
    pub fn from_encrypt_dict(
        dict: &Dict, file_id: &[u8], password: &[u8], r: &impl Resolve,
    ) -> Result<Self, Error>;

    /// Decrypt one string or stream payload belonging to indirect object `obj`.
    /// Infallible by design: PDFium never fails a decrypt, it produces a
    /// best-effort (possibly empty) result. See `decrypt` behavior table.
    #[must_use]
    pub fn decrypt(&self, obj: ObjRef, class: CryptClass, data: &[u8]) -> Vec<u8>;

    /// Permissions as the C++ reports them; `owner` selects the
    /// owner-unlocked override.
    #[must_use]
    pub fn permissions(&self, owner: bool) -> u32;

    #[must_use]
    pub fn encrypt_metadata(&self) -> bool;

    #[must_use]
    pub fn revision(&self) -> u8;
}

/// Which crypt filter class a payload belongs to (/StmF, /StrF, /EFF).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptClass { Stream, String, Embedded }
```

`decrypt` takes `&self` and returns an owned `Vec<u8>`; for `Identity` it is a
plain copy of `data`. (SPEC §3 fixes this signature; a borrowed-return variant
would be a `[spec]` change.)

### 3.2 `standard.rs` — the revision algorithms

Data first: parsing the dictionary yields a record, and the algorithms are free
functions over it.

```rust
/// The parsed /Encrypt dictionary. Purely a record of what the file said.
#[derive(Debug, Clone)]
pub struct EncryptParams {
    pub version: i64,             // /V, default 0
    pub revision: i64,            // /R, default 0
    pub permissions: u32,         // /P as u32, default 0xFFFF_FFFF (-1)
    pub cipher: Cipher,           // resolved from /V, /CF, /CFM
    pub key_len: usize,           // bytes, 0..=32
    pub encrypt_metadata: bool,   // /EncryptMetadata, default true
    pub o: Box<[u8]>, pub u: Box<[u8]>,
    pub oe: Box<[u8]>, pub ue: Box<[u8]>, pub perms: Box<[u8]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cipher { None, Rc4, Aes }

pub fn parse_encrypt_dict(dict: &Dict, r: &impl Resolve) -> Result<EncryptParams, Error>;

/// ISO 32000 Alg. 2 — the /R 2..4 file key from a padded password.
fn file_key_r234(p: &EncryptParams, password: &[u8], file_id: &[u8],
                 ignore_metadata: bool) -> SmallKey;

/// ISO 32000 Alg. 4/5 — returns the derived key on success.
fn check_user_password_r234(p: &EncryptParams, password: &[u8], file_id: &[u8],
                            ignore_metadata: bool) -> Option<SmallKey>;

/// ISO 32000 Alg. 7 — recover the user password from the owner password.
fn recover_user_password(p: &EncryptParams, owner_password: &[u8]) -> Vec<u8>;

/// ISO 32000-2 Alg. 2.A — /R 5 and /R 6. Returns the 32-byte file key.
fn check_password_aes256(p: &EncryptParams, password: &[u8], owner: bool)
    -> Option<[u8; 32]>;

/// ISO 32000-2 Alg. 2.B — the hardened iterated hash. `vector` is Some(/U[0..48])
/// for owner checks, None for user checks.
fn revision6_hash(password: &[u8], salt: &[u8; 8], vector: Option<&[u8; 48]>) -> [u8; 32];

pub const PAD: [u8; 32] = [ /* the 32 bytes of §1.4 */ ];
```

`from_encrypt_dict` orchestrates §1.9's ordering:

```
params = parse_encrypt_dict(dict)?          // may fail: unsupported / malformed
if params.cipher == None { return Ok(Identity) }
if !password.is_empty() {
    if let Some(key) = try_password(&params, password, owner=true, file_id) {
        return Ok(handler(params, key, owner_unlocked = true))
    }
}
match try_password(&params, password, owner=false, file_id) {
    Some(key) => Ok(handler(params, key, owner_unlocked = false)),
    None => Err(Error::WrongPassword),
}
```

`try_password` implements the encoding fallback of §1.8: try raw; if the
password is pure ASCII give up; otherwise try the revision-appropriate
re-encoding. The Latin-1 ⇄ UTF-8 conversions are ~15 lines each and stay in
this file (no `encoding_rs` dependency — DEPS.md is closed):

```rust
/// Reinterpret each byte as a Latin-1 scalar and re-encode as UTF-8.
fn latin1_to_utf8(bytes: &[u8]) -> Vec<u8>;
/// Decode UTF-8 (lossy: invalid sequences map to U+FFFD, matching WideString);
/// re-encode each scalar <= 0xFF as one byte, others as '?'.
fn utf8_to_latin1(bytes: &[u8]) -> Vec<u8>;
```

The `'?'` substitution matches `WideString::ToLatin1`'s narrowing of
out-of-range scalars; verify against the oracle during implementation
(Open Question Q2).

### 3.3 `object.rs` — per-object keys and payload decryption

```rust
/// ISO 32000 Alg. 1. Returns the key actually fed to the cipher.
fn object_key(handler_key: &SmallKey, obj: ObjRef, aes: bool) -> ObjectKey;

/// Strip the leading IV and CBC-decrypt, reproducing PDFium's buffering rules.
fn decrypt_aes_cbc(key: &[u8], data: &[u8]) -> Vec<u8>;
```

`decrypt_aes_cbc` behavior table (from §1.11) — this is the acceptance spec:

| `data.len()` | output |
|---|---|
| `0..16` | empty |
| exactly `16` | empty (all IV) |
| `16 + 16k`, `k >= 1`, last plaintext byte `p < 16` | `16(k-1) + (16 - p)` bytes |
| `16 + 16k`, `k >= 1`, last plaintext byte `p >= 16` | `16(k-1)` bytes (final block dropped) |
| `16 + 16k + m`, `0 < m < 16` | `16k` bytes; the `m` trailing bytes are discarded, **and the last full block is also dropped** (it never leaves the lag buffer) |

The last row deserves care: with a partial tail, `block_offset_ != 16` at
finish, so the C++ emits nothing for the buffered block *or* the partial bytes.
Restated: output is `16 * max(0, k - 1)`… no — precisely, the lag means the
block at index `k-1` is emitted only if bytes arrived after it. With a partial
tail those bytes *did* arrive, so block `k-1` **is** emitted, and only the
partial `m` bytes are lost. Implement by simulation and pin with a unit test
per row (Test T7).

`decrypt` dispatch:

```rust
match (self, class) {
    (Identity, _) => data.to_vec(),
    (Rc4V2 { key, .. }, _) => rc4(&object_key_rc4(key, obj), data),
    (AesV4 { key, .. }, _) => decrypt_aes_cbc(&object_key_aes_v4(key, obj), data),
    (AesV5 { key, .. }, _) => decrypt_aes_cbc(key, data),
}
```

`CryptClass` is matched but never branches (D1); the parameter exists so a
future `/EFF` implementation is not a signature change. Silence the unused-arm
warning with an explicit `Stream | String | Embedded => …` rather than `_`, per
STYLE.md §1.

### 3.4 Data flow, and what the parser must do

```
trailer /Encrypt  ─┐
trailer /ID[0]     ├─→ SecurityHandler::from_encrypt_dict ─→ SecurityHandler
LoadOptions.password ┘                                             │
                                                                   ▼
      fetched indirect object ──→ walk ──→ every Str / Stream ──→ decrypt(obj, class, bytes)
```

Two responsibilities stay in **`pdfrum-parser`**, not here, because they need
the object graph:

1. **The signature exemption** (§1.12). `pdfrum-crypt` has no `Object` walker
   and must not grow one. The parser's decrypt walk implements the deferral: a
   `Contents` value whose parent dict has `/Type` or `/FT` is queued; after the
   walk the parent is tested for `Type == Sig || FT == Sig` and, if it is not a
   signature, the subtree is decrypted in a second pass. The brief for
   `pdfrum-parser` must carry this; it is recorded here because the behavior is
   crypto-driven.
2. **The metadata exemption** (§1.12) — skipping decryption of the object
   `/Root/Metadata` points to when `/EncryptMetadata` is false. `pdfrum-crypt`
   exposes `encrypt_metadata()` for the parser to consult.

Both are noted as cross-crate obligations so the parser brief does not
rediscover them.

---

## 4. Test plan

Crypt is the crate with the strongest known-answer-test potential in the whole
project: every algorithm is deterministic, and the C++ ships literal vectors.
All tests are `#[cfg(test)]` units run by `cargo nextest run`.

### 4.1 Primitive KATs — ported verbatim from `fx_crypt_unittest.cpp`

**T1 — MD5** (`fx_crypt_unittest.cpp:51-192`). RFC 1321 A.5 suite:

| input | MD5 |
|---|---|
| `""` | `d41d8cd98f00b204e9800998ecf8427e` |
| `"a"` | `0cc175b9c0f1b6a831c399e269772661` |
| `"abc"` | `900150983cd24fb0d6963f7d28e17f72` |
| `"message digest"` | `f96b697d7cb7938d525a2f31aaf161d0` |
| `"abcdefghijklmnopqrstuvwxyz"` | `c3fcd3d76192e4007dfb496cca67e13b` |
| `A–Z a–z 0–9` (62 chars) | `d174ab98d277d9f5a5611c2c9f419d9f` |
| `"1234567890"` × 8 (80 chars) | `57edf4a22be3c955ac49da2e2107b67a` |
| `(i & 0xFF for i in 0..10*1024*1024+1)` | `90bd6ad90acef5adaa92203e21c7a13e` |

The long-data case (`:79-92`, `:104-130`) also exercises chunked update with a
deliberately non-power-of-two chunk length of **4097**; port it as a streaming
test. These pin our RustCrypto wiring, not our code — they are cheap insurance
that we hashed the right bytes in the right order.

**T2 — SHA** (`:194-260`, `:506-600`):

| fn | input | digest |
|---|---|---|
| SHA-1 | `""` | `da39a3ee5e6b4b0d3255bfef95601890afd80709` |
| SHA-1 | `"abc"` | `a9993e364706816aba3e25717850c26c9cd0d89d` |
| SHA-1 | FIPS 180-2 A.2 multi-block | `84983e441c3bd26ebaae4aa1f95129e5e54670f1` |
| SHA-256 | `""` | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| SHA-256 | `"abc"` | `ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad` |
| SHA-256 | FIPS 180-2 B.2 | `248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1` |
| SHA-384 | `""` | `38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b` |
| SHA-512 | `""` | `cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e` |

Plus the 112-byte `"a"×112` padding-boundary cases for SHA-384
(`187d4e07cb306103c69967bf544d0dfbe90425 77599c73c330abc0cb64c61236d5ed565ee19119d8c31779a38f791fcd`)
and SHA-512
(`c01d080efd492776a1c43bd23dd99d0a2e626d481e16782e75d54c2503b5dc32bd05f0f1ba33e568b88fd2d970929b719ecbb152f58f130a407c8830604b70ca`)
— they catch the 112-vs-128 padding rule for the 1024-bit block variants.

**T3 — AES-CBC** (`:602-730`), the BoringSSL NIST SP 800-38A vectors. Each is
one 16-byte block; the four cases per key size chain (each IV is the previous
ciphertext), which also exercises our CBC state handling:

AES-128, key `2b7e151628aed2a6abf7158809cf4f3c`:
| IV | plaintext | ciphertext |
|---|---|---|
| `000102030405060708090a0b0c0d0e0f` | `6bc1bee22e409f96e93d7e117393172a` | `7649abac8119b246cee98e9b12e9197d` |
| `7649abac8119b246cee98e9b12e9197d` | `ae2d8a571e03ac9c9eb76fac45af8e51` | `5086cb9b507219ee95db113a917678b2` |
| `5086cb9b507219ee95db113a917678b2` | `30c81c46a35ce411e5fbc1191a0a52ef` | `73bed6b8e3c1743b7116e69e22229516` |
| `73bed6b8e3c1743b7116e69e22229516` | `f69f2445df4f9b17ad2b417be66c3710` | `3ff1caa1681fac09120eca307586e1a7` |

AES-192, key `8e73b0f7da0e6452c810f32b809079e562f8ead2522c6b7b`: same four
plaintexts, ciphertexts `4f021db243bc633d7178183a9fa071e8`,
`b4d9ada9ad7dedf4e5e738763f69145a`, `571b242012fb7ae07fa9baac3df102e0`,
`08b0e27988598881d920a9e64f5615cd` (each IV = previous ciphertext, first IV
`000102…0f`).

AES-256, key
`603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4`: ciphertexts
`f58c4c04d6e5f1ba779eabfb5f7bfbd6`, `9cfc4e967edb808d679f777bc6702c7d`,
`39f23369a9d9bacfa530e26304231461`, `b2eb05e2c39be9fcda6c19078c6a9d1b`.

Test both directions (encrypt-then-decrypt round trip), as the C++ does at
`:624-637`.

**T4 — RC4.** The C++ KATs (`:262-504`) assert on the *internal permutation
state* after setup and after crypting, which is implementation detail we should
not pin. Port the **ciphertext** assertions only:

- Key = **empty**, plaintext
  `"The Quick Fox Jumped Over The Lazy Brown Dog."` **including its NUL
  terminator** (45 bytes: the C++ uses `std::begin/end` over a `uint8_t[]`
  string literal) ⇒
  `8a70ec61f2423459e12658082f4ed818aa6a1ac7d0839df2370b195a42b613ffd2b555451ff0ceab613ecaac1efc`.
- Key = `"foobar"`, same plaintext ⇒
  `3bc175cea736da07e5d6bc375acdc4192472c7daa16b7a776aa72caff07bc066aea769bbca46795111 1e058a74a6`
  (spaces for readability only; the C++ decimal array is
  `59,193,117,206,167,54,218,7,229,214,188,55,90,205,196,25,36,114,199,218,161,107,122,119,106,167,44,175,240,123,192,102,174,167,105,187,202,70,121,81,17,30,5,138,116,166`).
- The long-plaintext cases (`:321-326`, > 256 bytes, exercising permutation
  wrap-around) are worth porting for both keys; transcribe the byte arrays from
  `fx_crypt_unittest.cpp:368-388` and `:457-477`.
- **Empty-key behavior** is itself an assertion: `rc4(&[], data)` must not panic
  and must produce the empty-key vector above (§1.13).

### 4.2 Algorithm KATs — derived from the corpus fixtures

The C++ has no unit tests for the `/R 2..6` algorithms; it tests them only
end-to-end through `cpdf_security_handler_embeddertest.cpp`. We can do better,
because the fixture `/Encrypt` dictionaries are small and self-contained. Embed
them as literals and assert on the derived key and on the accept/reject verdict.

**T5 — `/R 2` (`testing/resources/encrypted_hello_world_r2.pdf`).**
```
/V 1  /R 2  /P -64
/O = 65b4d14434c8434aeb2e2ddd3922e3233f4fdf4a527f179a3a5cca0563d6249e
/U = 4219bd5bea1f046782e698112d6b80b2295e4b19e58f8690486800550c59e63e
/ID[0] = 2b778de1bcef1733b35e680882812409
```
- key length must resolve to **5 bytes** (`/V 1`, `/Length` absent).
- Owner password `"\xe2ge"` (Latin-1 `âge`) accepted; `"\xc3\xa2ge"` (UTF-8)
  accepted via the UTF-8→Latin-1 fallback.
- User password `"h\xf4tel"` and `"h\xc3\xb4tel"` accepted.
- `"tiger"` rejected with `Error::WrongPassword`.

**T6 — `/R 3` (`encrypted_hello_world_r3.pdf`).**
```
/V 2  /R 3  /Length 128  /P -3904
/O = 894b1d3a9003e3bc172d8ff9277bc931a520f52c2d1f206e49d3ee74a901e408
/U = a923680e625d8922366aced0a070775e00000000000000000000000000000000
/ID[0] = 9b744068bb5efbe920baaba6da63c2bf
```
- key length **16 bytes**; the 50-iteration MD5 loop is exercised.
- Same four password encodings accepted.
- Note `/U`'s trailing 16 zero bytes — pins that only `/U[0..16]` is compared.

**T7 — `/V 4` AESV2 (`testing/resources/encrypted.pdf`).**
```
/V 4  /R 4  /Length 128  /CF<</StdCF<</CFM/AESV2 /Length 16>>>>
/StmF /StdCF  /StrF /StdCF  /P -3392
/O = f4dac5619702f34666f7e8e1af03e4660072a021cdf7ec908bdb3c78f9c9633c
/U = 3a15f3b2387a77be7080464022951ed100000000000000000000000000000000
/ID[0] = 1B0FD0F5E29AD84DBF67775E9E3B009F
```
- **`/CF/StdCF/Length 16` must be promoted to 128 bits** by the `< 40` rule
  (§1.2) ⇒ key length 16. Assert this explicitly; it is the highest-value
  damage-tolerance assertion in the crate.
- Cipher resolves to `Aes` with `key_len == 16` ⇒ variant `AesV4`.
- User password `"1234"` ⇒ `permissions(false) == 0xFFFFF2C0`.
- Owner password `"5678"` ⇒ `permissions(true) == 0xFFFFFFFC` and
  `permissions(false) == 0xFFFFF2C0`.
- No password ⇒ `WrongPassword`; `"tiger"` ⇒ `WrongPassword`
  (embeddertest `:121-139`).

**T8 — `/R 5` (`encrypted_hello_world_r5.pdf`).**
```
/V 5 /R 5 /Length 256 /CF<</StdCF<</CFM/AESV3 /Length 32>>>> /P -4
/O    = a770489a67f9076d8edbd86032fc3f926295c2d04707d2a1b007d44a2b64f6f7b10bb0c97ec06e945e33ad56d5e8cca4
/OE   = e111339fe969ac0851e4f9542f4e3e3600be8f4b2fee27f572e92edc59378640
/U    = 3e5d54b881ae698f01104dfe4a36fccd5a94d913c575ebd4d44f43e6366269067bdef258bb4b37deb90db87904f84917
/UE   = 8608d184b6f3cd03ebf896946cd9e9420b361fa380d5de9c94c2a819aa5a0638
/Perms= c954c264d796dfd131ddb784f5a8b1bf
/ID[0]= 7ca64129d20fc9745f1bfc0e4166590a
```
- Owner `"\xe2ge"` / `"\xc3\xa2ge"`, user `"h\xf4tel"` / `"h\xc3\xb4tel"` all
  accepted. At `/R 5` the fallback direction is **Latin-1 → UTF-8**.
- Assert the recovered 32-byte file key is stable across the four password
  spellings that unlock the *same* role.
- Assert the `/Perms` check runs: mutate `/Perms` byte 9 from `'a'` and expect
  `WrongPassword`.
- `/ID[0]` is present but **unused** at `/R >= 5`; assert acceptance with the
  `/ID` removed (the embeddertest's `RemoveTrailerIdFromDocument` case,
  `:329-335`, proves R5 is ID-independent).

**T9 — `/R 6` (`encrypted_hello_world_r6.pdf`).** Same shape, exercising
`revision6_hash`:
```
/V 5 /R 6 /P -4
/O    = d80e6106fd39478c8860c9145e896f126c3fa0c9125ad2096d242c075fa621ac992241608cc3d397135a2c0aec96db3b
/OE   = 9046a22c32d33286559594eaef09b9cf49228b58d02bf5ddc3383df9263282e2
/U    = 0798ab4b1c93d360f96b8ba41d1add5b7eaf4b110f014de88a57615fdd6f677c1a5e059d15ed6eed88d94c0349583f86
/UE   = 9e304c9fff647b71536db3684c72914cae3882885eb8cf9cfbf3026dae2e1f35
/Perms= 030e31486569c2d3c03f01e57ece54fc
```
- Owner `"âge"`, user `"hôtel"` (both encodings) accepted.
- This is the *only* test that exercises the ≥64-round loop, the SHA-384/512
  branches and the mod-3 selector. If it passes, `revision6_hash` is right.

**T10 — `/R 5` alternate (`testing/resources/bug_644.pdf`).**
```
/V 5 /R 5 /P 4092 /CF<</StdCF<</CFM/AESV3 /Length 32>>>>
/O    = B6C711683D98F878929688EF497A0BB928E1F0013A0B5C357BE701E42DC4A6A9E124B0C505DDDA91562C5EA791E2B7AC
/OE   = 26B337B3B635C18262B4915289F1D353EB432D7E7FF6BE5450C82D690202A093
/U    = 69F20E0450E8B2A8ACA6AF1DE1284DB11EC4E38F6E7CB2B9AE9A1CFF6F95BA6CD83783C4ED8B31D933482CBB7A791290
/UE   = 5104E81C113D43246A264580FE82D2890B7B8CEEF4A3D667B81A32EED62D8C54
/Perms= 3D62C200CDB31A603EF202E12993AE13
```
- Owner password `"a"`, user password `"b"` (embeddertest `:183-199`); no
  password and `"tiger"` rejected. `/P 4092` ⇒ `permissions(_) == 0xFFFFFFFC`.
- Both passwords are pure ASCII ⇒ the encoding fallback must **not** fire
  (§1.8 step 2). Assert `Err(WrongPassword)` for `"tiger"` reaches that early
  return rather than attempting a conversion.

### 4.3 Damage-tolerance tests

**T11 — short `/O`** (`encrypted_hello_world_r2_bad_okey.pdf`,
`encrypted_hello_world_r3_bad_okey.pdf`; embeddertest `:201-211`,
crbug.com/42270437). `/O` shorter than 32 bytes ⇒ `recover_user_password`
returns empty and the whole open fails with `WrongPassword`, **without
panicking or indexing out of bounds**. Construct the dicts synthetically with
`/O` of length 0, 1, 31 and assert `Err(WrongPassword)` for each.

**T12 — short `/U`.** `/U` of length 0..15 ⇒ `WrongPassword` at every revision.
`/U` of length 16..31 at `/R >= 3`: accepted into the 32-byte `test` buffer
zero-padded (§1.5), so it *can* still succeed — assert no panic and that the
comparison is over the first 16 bytes.

**T13 — `/V 5` short strings.** `/O` or `/U` < 48 bytes, or `/OE`/`/UE` < 32
bytes, or empty `/Perms` ⇒ `WrongPassword`, no panic (§1.7).

**T14 — key-length validation.** Table-driven over `LoadCryptInfo`'s rules
(§1.2):

| `/V` | `/Length` | `/CF/StdCF/Length` | `/CFM` | expected |
|---|---|---|---|---|
| 1 | absent | — | — | RC4, 5 bytes |
| 1 | 128 | — | — | RC4, **5 bytes** (`/Length` ignored) |
| 2 | absent | — | — | RC4, 5 bytes (40 default / 8) |
| 2 | 40 | — | — | RC4, 5 bytes |
| 2 | 128 | — | — | RC4, 16 bytes |
| 2 | 256 | — | — | `CipherKeyLength` (32 > RC4's 16) |
| 2 | 8 | — | — | RC4, 8 bytes (`8/8 = 1`? no: `/V<4` path skips the `<40` promotion ⇒ 1 byte ⇒ **`CipherKeyLength`**, `1 < 5`) |
| 4 | 128 | absent | `V2` | RC4, 16 bytes |
| 4 | 128 | 16 | `AESV2` | **AES, 16 bytes** (the `<40` promotion) |
| 4 | 128 | 128 | `AESV2` | AES, 16 bytes |
| 4 | absent | absent | `AESV2` | AES, 16 bytes (`/Length` default 128) |
| 4 | 128 | 40 | `AESV2` | `CipherKeyLength` (5 bytes for AES) |
| 4 | — | — | `Identity` name | `Identity` handler |
| 5 | 256 | 32 | `AESV3` | AES, **32 bytes** (per-filter `/Length` ignored) |
| 5 | absent | — | `AESV3` | AES, 32 bytes (default 256) |
| 4 | 128 | −8 | `AESV2` | `MalformedEncryptDict` (negative bits) |

Note the `/V 2, /Length 8` row: the `nKeyBits < 40 ⇒ ×8` promotion lives only
in the `/V >= 4` branch (`cpdf_security_handler.cpp:270-272`), so the `/V < 4`
path does a bare `/8`. Confirm this against the oracle during implementation —
it is a plausible place for the brief to be wrong (Open Question Q3).

**T15 — `/StmF` ≠ `/StrF`.** ⇒ `Error::MismatchedCryptFilters`. Also: both
absent at `/V 4` ⇒ `MissingCryptFilter("")`, not `Identity` (§1.3 point 2).

**T16 — missing `/ID`.** `/R 2..4` with `file_id = &[]` must skip the ID
contribution in both `CalcEncryptKey` and the Alg. 5 comparison hash. Pin with
a synthetic fixture built by re-encrypting a known key, or by asserting that
the two code paths agree with `file_id = &[]`.

### 4.4 Per-object key and payload tests

**T17 — `object_key` vectors.** Hand-computed, checked against the algorithm:
- RC4, `key_len = 5`, obj `(1, 0)`: scratch = `key || 01 00 00 00 00`,
  `real = MD5(scratch[0..10])`, RC4 key length `min(10, 16) = 10`.
- RC4, `key_len = 16`, obj `(1, 0)`: `real = MD5(scratch[0..21])`, RC4 key
  length `min(21, 16) = **16**`.
- AESV2, `key_len = 16`, obj `(1, 0)`: scratch =
  `key || 01 00 00 00 00 || 73 41 6C 54`, `real = MD5(scratch[0..25])`, AES key
  = all 16 bytes of `real`.
- AESV3, `key_len = 32`, any obj: key is the file key **verbatim**; assert the
  object number does not affect it.
- Objnum truncation: `(0x0100_0001, 0)` and `(1, 0)` must produce the **same**
  key (only 3 bytes of objnum are used).

**T18 — AES stream decrypt table.** One test per row of §3.3's table, plus:
- exactly 32 bytes with last plaintext byte `0x10` ⇒ empty output;
- exactly 32 bytes with last plaintext byte `0x00` ⇒ 16 bytes output
  (zero is *not* treated as invalid padding);
- 16 + 16 + 5 bytes ⇒ 16 bytes output (the partial tail is dropped, the
  preceding full block is not).

**T19 — RC4 stream decrypt.** Empty input ⇒ empty output. Round-trip:
`rc4(k, rc4(k, data)) == data`.

### 4.5 Conformance & fuzz

- **Conformance cluster `encryption`** (PLAN.md §5 `--triage` tag). The corpus
  files are `encrypted*.pdf`, `bug_644.pdf`, `bug_1124998.pdf`
  (embeddertest `:659-661`, opens with password `"test"`), and
  `bug_424613308.pdf` (`:663-666`, must **fail to open without crashing** — the
  C++ note is "test passes if `CHECK()` does not fail", i.e. it is a
  key-length/`CHECK` regression fixture; for us it must return an `Error`).
  Exit criterion for the cluster: every one of these files reaches the same
  open/reject verdict as the oracle, and the decrypted page content produces a
  Tier-A-identical `--txt` dump.
- **Fuzz target `fuzz_encrypt_dict`**: bytes → a synthetic `/Encrypt` dict
  (fields sliced from the input) + a password slice →
  `SecurityHandler::from_encrypt_dict`. Must never panic. The `/R 6` loop is
  bounded at 287 rounds by construction, but each round allocates
  `64 * (pw_len + 64 + 48)` bytes — cap the password length in the target
  (say 4 KiB) so the fuzzer does not merely find an OOM. Note this as a real
  DoS property of the format, not a bug in our code (Open Question Q4).
- **Fuzz target `fuzz_decrypt`**: `(handler_from_fixed_key, obj, class, bytes)`
  → `decrypt`. Must never panic for any input length, in particular
  0..17 bytes with AES.

---

## 5. Open questions

**Q1 — the inverted metadata predicate.** `cpdf_parser.cpp:305` uses
`!IsMetadataEncrypted()` to record `metadata_objnum_`, while the linearized
path at `:1287` uses `IsMetadataEncrypted()`. One of the two must be wrong;
`:305` matches the spec (exempt the metadata object *when it is not
encrypted*). Proposal: implement `:305`'s sense uniformly and note the
divergence. **Escalate if** a corpus file with a linearized, `/EncryptMetadata
false` document diffs against the oracle — then we replicate the bug instead.
Resolve by building the oracle and diffing `--show-metadata` on such a file
before implementation lands.

**Q2 — the exact Latin-1 ⇄ UTF-8 conversions.** PDFium routes through
`WideString::FromUTF8`/`ToLatin1`, whose behavior on invalid UTF-8 and on
scalars > 0xFF is not documented in the files this brief covers. The proposed
`U+FFFD` / `'?'` handling in §3.2 is an inference. Resolve by reading
`core/fxcrt/widestring.cpp` during implementation, or by a differential test
against the oracle with a deliberately invalid-UTF-8 password. Low risk: every
corpus fixture uses well-formed input in both encodings.

**Q3 — the `/V < 4` key-length path.** §1.2 reads the `nKeyBits < 40 ⇒ ×8`
promotion as living only in the `/V >= 4` branch, which makes `/V 2 /Length 8`
resolve to a 1-byte key and thus a rejected document. Confirm from
`cpdf_security_handler.cpp:266-281` that no promotion happens for `/V < 4`
(the code reads that way; the T14 row is marked accordingly). If a corpus file
depends on the other reading, the table changes and this brief gets a `[spec]`-
adjacent amendment.

**Q4 — R6 cost bounds as a policy question.** A hostile file can force up to
287 rounds of `SHA-512(AES-128-CBC(64 × (pw + 64 + 48) bytes))`. With a
user-supplied 1 MiB password that is ~19 GiB of AES. PDFium has no guard
because its password comes from an application, not a file. Since our password
also comes from the caller, the exposure is small — but `Limits` is the natural
home for a `max_password_len`. Proposal: cap at **127 bytes** (the ISO 32000-2
limit for `/R 6`), returning `WrongPassword` for longer input, and record it in
`Limits`. **Needs a decision** because it is a behavioral difference from the
C++ (which would accept a longer password) and because adding a field to
`Limits` touches SPEC §1.

**Q5 — should `decrypt` report damage?** STYLE.md §3 says recoveries are
recorded, never silent. `decrypt` currently swallows several: a dropped final
AES block, a discarded partial tail, an AES payload under 16 bytes. SPEC §3's
signature has no `&mut Diagnostics`. Proposal: leave the signature alone for M1
(the parser can compare input and output lengths if it wants a diagnostic) and
revisit if the conformance harness needs the tag. Flagging it because it is the
one place this crate knowingly departs from the "never silently swallow a
recovery" rule.
