# pdfrum-crypt

Standard security handler, revisions 2–6 (RC4 / AES). Decrypts any of them.
Re-encrypts under an existing handler. New `/Encrypt` dictionaries: revision
6 only (`standard_r6`). Graph walks (`/Contents` of a signature,
`/EncryptMetadata`) stay in the caller.

```rust
use pdfrum_crypt::{CryptClass, SecurityHandler};
use pdfrum_object::ObjRef;

let handler = SecurityHandler::Identity;
assert_eq!(handler.decrypt(ObjRef::new(1, 0), CryptClass::Stream, b"raw"), b"raw");
```

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
