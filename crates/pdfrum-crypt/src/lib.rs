//! PDF standard security (ISO 32000 §7.6): RC4 and AES-CBC decryption with
//! MD5/SHA-1/SHA-2 key derivation via RustCrypto, covering standard security
//! handler revisions 2–6 with distinct wrong-password reporting and per-class
//! crypt filters (`/StmF`, `/StrF`) (SPEC.md §3).

#![forbid(unsafe_code)]
