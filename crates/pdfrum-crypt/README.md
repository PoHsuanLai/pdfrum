# pdfrum-crypt

**PDF standard security handler: RC4/AES, revisions 2-6.**

Opening an encrypted PDF and decrypting its strings and streams, per ISO 32000
§7.6. A document's `/Encrypt` dictionary plus a password produce a
`SecurityHandler`; every string and stream a reader pulls out of the file then
passes through `SecurityHandler::decrypt`, keyed by the indirect object it
belongs to.

All of the standard handler's revisions are implemented. Revisions 2 to 4
derive an RC4 or AES-128 file key by an MD5 ladder over the padded password;
revisions 5 and 6 verify a SHA-2 hash and unwrap the 32-byte AES-256 key the
file stores directly. `SecurityHandler` is a closed enum rather than a trait,
because the set of standard handlers is closed — adding one must break every
`match`.

```rust
use pdfrum_crypt::{CryptClass, SecurityHandler};
use pdfrum_object::ObjRef;

// A document with no /Encrypt needs no handler: payloads pass through.
let handler = SecurityHandler::Identity;
assert_eq!(handler.decrypt(ObjRef::new(1, 0), CryptClass::Stream, b"raw"), b"raw");
assert_eq!(handler.permissions(false), 0xFFFF_FFFF);
```

It decrypts; it does not encrypt. Two exemptions that the specification
requires — a signature dictionary's `/Contents` staying undecrypted, and
`/Root/Metadata` when `/EncryptMetadata` is false — need to walk the object
graph, which this crate deliberately cannot, so the predicates live here
(`is_signature_dict`) and the walk lives in the caller.

## Part of pdfrum

`pdfrum-crypt` is the security layer of
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine. For a
batteries-included API that wires the handler up for you — including password
prompting at load — use the `pdfrum` facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
