# pdfrum-crypt

The standard security handler (ISO 32000-1 §7.6), revisions 2 through 6: an
`/Encrypt` dictionary plus a password produce a [`SecurityHandler`], and every
string and stream in the file decrypts through it, keyed by the indirect
object it belongs to.

```rust
use pdfrum_crypt::{CryptClass, SecurityHandler};
use pdfrum_object::ObjRef;

let handler = SecurityHandler::Identity;
assert_eq!(handler.decrypt(ObjRef::new(1, 0), CryptClass::Stream, b"raw"), b"raw");
```

**The object reference is not optional context — it is part of the key.**
Revisions 2 to 4 derive an RC4 or AES-128 key by an MD5 ladder over the padded
password *and the object's number and generation*, so the same bytes at a
different object number decrypt to something else. Revisions 5 and 6 verify a
SHA-2 hash and unwrap a 32-byte AES-256 key the file stores directly, and only
there is the key object-independent. [`CryptClass`] distinguishes strings from
streams because `/StmF` and `/StrF` can name different crypt filters.

The AES-CBC initialisation vector is an argument rather than something this
crate mints, because a decrypting caller reads it off the ciphertext and only
an encrypting one has to produce it. `pdfrum-edit` draws file keys and
vectors from the operating system for every encrypted save; a fixed seed
still pins `/ID` bytes, not the secrets.

Two things are deliberately out of scope. Building a new `/Encrypt`
dictionary is revision 6 only (`standard_r6`), because `/O`, `/U`, `/OE`,
`/UE` and `/Perms` are written by whoever chose the passwords. And graph
walks stay in the caller — whether a signature's `/Contents` is exempt, or
`/EncryptMetadata` applies, is a question about the document, not about a
cipher.

Public-key handlers (`Adobe.PubSec`) are not implemented.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
