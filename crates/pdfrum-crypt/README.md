# pdfrum-crypt

Standard security handler, revisions 2–6 (RC4 / AES). Decrypts; does not
encrypt. Graph walks (`/Contents` of a signature, `/EncryptMetadata`) stay
in the caller.

```rust
use pdfrum_crypt::{CryptClass, SecurityHandler};
use pdfrum_object::ObjRef;

let handler = SecurityHandler::Identity;
assert_eq!(handler.decrypt(ObjRef::new(1, 0), CryptClass::Stream, b"raw"), b"raw");
```

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
