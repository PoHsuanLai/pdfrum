#![doc = include_str!("../README.md")]
// Revisions 2 to 4 derive an RC4 or AES-128 key by an MD5 ladder over the
// padded password; revisions 5 and 6 verify a SHA-2 hash and unwrap a 32-byte
// AES-256 key that the file stores directly.
//
// The AES-CBC initialisation vector is an argument rather than something this
// crate mints, because a decrypting caller reads it off the ciphertext and
// only an encrypting one has to produce it. `pdfrum-edit` draws vectors from
// the operating system for every save.
//
// Building an `/Encrypt` dictionary is out of scope: `/O`, `/U`, `/OE`, `/UE`
// and `/Perms` are written by whoever chose the passwords, and a
// password-preserving save copies the dictionary the file already had.
//
// Two crypto-driven behaviors also live outside this crate, because both need
// to walk the object graph, which this crate deliberately cannot:
//
// - The signature exemption. A `/Contents` value whose parent dictionary has a
//   `/Type` or `/FT` key is deferred during the decrypt walk; once the parent
//   has been decrypted its type can finally be read, and a parent that turns
//   out to be a signature dictionary (`/Type /Sig`, or `/FT /Sig` when `/Type`
//   is absent) keeps its contents undecrypted. The test cannot be made earlier
//   because those names are themselves encrypted strings until the parent is
//   done. `is_signature_dict` is what the walker calls.
// - The metadata exemption. When `SecurityHandler::encrypt_metadata` is false
//   the object `/Root/Metadata` points at is not decrypted.
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
// Every byte reaching this crate came from an untrusted file or a password:
// index with `get()`.
#![warn(clippy::indexing_slicing)]

mod create;
mod handler;
mod key;
mod object;
mod permissions;
mod primitives;
mod rc4;
mod saslprep;
mod standard;

#[cfg(test)]
mod test_fixtures;

pub use create::{ENTROPY_LEN, KeyMaterial, standard_r6};
pub use handler::{Error, SecurityHandler, is_signature_dict};
pub use key::SmallKey;
pub use object::{CryptClass, Iv};
pub use permissions::Permissions;
pub use primitives::{md5, sha1};
pub use rc4::rc4;
pub use standard::{Cipher, EncryptParams, PAD, PasswordEncoding, parse_encrypt_dict};

#[cfg(test)]
mod tests {
    use super::{CryptClass, Error, Iv, Permissions, SecurityHandler, is_signature_dict};
    use crate::standard::Cipher;
    use crate::test_fixtures::{self, unhex};
    use pdfrum_object::{Dict, Name, NoResolve, ObjRef, Object, PdfString, names};

    /// The parse-only view a key-length test wants.
    fn cipher_of(dict: &Dict) -> Result<(Cipher, usize), Error> {
        super::parse_encrypt_dict(dict, &NoResolve).map(|p| (p.cipher, p.key_len))
    }

    // ---- T7: the AESV2 fixture, and the /Length promotion it depends on ----

    #[test]
    fn aes_v2_fixture_promotes_a_byte_length_to_bits() {
        let dict = test_fixtures::encrypted_pdf_dict();
        assert_eq!(cipher_of(&dict), Ok((Cipher::Aes, 16)));
    }

    #[test]
    fn aes_v2_fixture_opens_with_either_password() {
        let dict = test_fixtures::encrypted_pdf_dict();
        let id = unhex("1B0FD0F5E29AD84DBF67775E9E3B009F");

        let user = SecurityHandler::from_encrypt_dict(&dict, &id, b"1234", &NoResolve)
            .expect("the user password");
        assert!(matches!(user, SecurityHandler::AesV4 { .. }));
        assert!(!user.owner_unlocked());
        assert_eq!(user.permission_word(false), 0xFFFF_F2C0);
        assert_eq!(user.permission_word(true), 0xFFFF_F2C0);
        assert_eq!(user.revision(), 4);

        let owner = SecurityHandler::from_encrypt_dict(&dict, &id, b"5678", &NoResolve)
            .expect("the owner password");
        assert!(owner.owner_unlocked());
        assert_eq!(owner.permission_word(true), 0xFFFF_FFFC);
        assert_eq!(owner.permission_word(false), 0xFFFF_F2C0);
    }

    #[test]
    fn aes_v2_fixture_rejects_the_wrong_password() {
        let dict = test_fixtures::encrypted_pdf_dict();
        let id = unhex("1B0FD0F5E29AD84DBF67775E9E3B009F");
        for password in [&b""[..], b"tiger"] {
            assert_eq!(
                SecurityHandler::from_encrypt_dict(&dict, &id, password, &NoResolve).unwrap_err(),
                Error::WrongPassword,
                "password {password:?}"
            );
        }
    }

    // ---- T8 / T9 / T10: the AES-256 revisions ----

    #[test]
    fn revision_5_fixture_opens_with_all_four_spellings() {
        let dict = test_fixtures::r5_dict();
        let id = unhex("7ca64129d20fc9745f1bfc0e4166590a");

        let owner_keys: Vec<_> = [&b"\xe2ge"[..], b"\xc3\xa2ge"]
            .iter()
            .map(|password| {
                let handler = SecurityHandler::from_encrypt_dict(&dict, &id, password, &NoResolve)
                    .unwrap_or_else(|_| panic!("owner {password:?}"));
                assert!(handler.owner_unlocked());
                assert_eq!(handler.revision(), 5);
                match &handler {
                    SecurityHandler::AesV5 { key, .. } => **key,
                    _ => panic!("expected AesV5"),
                }
            })
            .collect();
        // Both spellings of the same role arrive at the same file key.
        assert_eq!(owner_keys.first(), owner_keys.last());

        for password in [&b"h\xf4tel"[..], b"h\xc3\xb4tel"] {
            let handler = SecurityHandler::from_encrypt_dict(&dict, &id, password, &NoResolve)
                .unwrap_or_else(|_| panic!("user {password:?}"));
            assert!(!handler.owner_unlocked());
        }
    }

    // At revision 5 the /ID plays no part: the same passwords work without it.
    #[test]
    fn revision_5_ignores_the_file_id() {
        let dict = test_fixtures::r5_dict();
        assert!(SecurityHandler::from_encrypt_dict(&dict, &[], b"h\xf4tel", &NoResolve).is_ok());
        assert!(SecurityHandler::from_encrypt_dict(&dict, &[], b"\xe2ge", &NoResolve).is_ok());
    }

    // T8 — the /Perms block is genuinely checked, not just decrypted.
    #[test]
    fn a_tampered_perms_block_rejects_the_password() {
        let mut dict = test_fixtures::r5_dict();
        let mut perms = unhex("c954c264d796dfd131ddb784f5a8b1bf");
        if let Some(byte) = perms.get_mut(9) {
            *byte ^= 0xFF;
        }
        dict.push(
            names::PERMS.clone(),
            Object::Str(PdfString::literal(&perms)),
        );
        assert_eq!(
            SecurityHandler::from_encrypt_dict(&dict, &[], b"h\xf4tel", &NoResolve).unwrap_err(),
            Error::WrongPassword
        );
    }

    // T9 — the only test that drives the hardened hash's 64-round loop, the
    // SHA-384/512 branches and the mod-3 selector.
    #[test]
    fn revision_6_fixture_opens_with_all_four_spellings() {
        let dict = test_fixtures::r6_dict();
        for password in [&b"\xe2ge"[..], b"\xc3\xa2ge"] {
            let handler = SecurityHandler::from_encrypt_dict(&dict, &[], password, &NoResolve)
                .unwrap_or_else(|_| panic!("owner {password:?}"));
            assert!(handler.owner_unlocked());
            assert_eq!(handler.revision(), 6);
        }
        for password in [&b"h\xf4tel"[..], b"h\xc3\xb4tel"] {
            let handler = SecurityHandler::from_encrypt_dict(&dict, &[], password, &NoResolve)
                .unwrap_or_else(|_| panic!("user {password:?}"));
            assert!(!handler.owner_unlocked());
        }
        assert_eq!(
            SecurityHandler::from_encrypt_dict(&dict, &[], b"tiger", &NoResolve).unwrap_err(),
            Error::WrongPassword
        );
    }

    // T10 — bug_644.pdf: ASCII passwords, so the encoding fallback must not
    // fire, and a /P of 4092 that masks to the same word for both roles.
    //
    // The roles are the reverse of what the C++ test *names* suggest: `b` is
    // the owner password and `a` the user one. The embedder test cannot tell,
    // because both roles report the same permissions here — which is exactly
    // why `/P 4092` was picked for that fixture.
    #[test]
    fn revision_5_alternate_fixture() {
        let dict = test_fixtures::bug_644_dict();
        let owner = SecurityHandler::from_encrypt_dict(&dict, &[], b"b", &NoResolve)
            .expect("the owner password");
        assert!(owner.owner_unlocked());
        assert_eq!(owner.permission_word(true), 0xFFFF_FFFC);
        assert_eq!(owner.permission_word(false), 0xFFFF_FFFC);

        let user = SecurityHandler::from_encrypt_dict(&dict, &[], b"a", &NoResolve)
            .expect("the user password");
        assert!(!user.owner_unlocked());
        assert_eq!(user.permission_word(false), 0xFFFF_FFFC);
        // Both roles reach the same file key, since /OE and /UE wrap it.
        assert_eq!(
            format!("{:?}", (owner.revision(), user.revision())),
            "(5, 5)"
        );

        for password in [&b""[..], b"tiger"] {
            assert_eq!(
                SecurityHandler::from_encrypt_dict(&dict, &[], password, &NoResolve).unwrap_err(),
                Error::WrongPassword,
                "password {password:?}"
            );
        }
        assert_eq!(
            owner.password_encoding(),
            crate::PasswordEncoding::AsGiven,
            "an ASCII password never converts"
        );
    }

    // ---- T14: the key-length resolution table ----

    /// One row: `/V`, `/Length`, the crypt filter's own `/Length`, `/CFM`,
    /// and what the pair should resolve to.
    type KeyLengthRow = (
        i64,
        Option<i64>,
        Option<i64>,
        Option<&'static str>,
        Result<(Cipher, usize), Error>,
    );

    #[test]
    fn key_length_resolution_table() {
        use test_fixtures::encrypt_dict;
        let cases: [KeyLengthRow; 14] = [
            (1, None, None, None, Ok((Cipher::Rc4, 5))),
            // /V 1 is 40-bit by definition; its /Length is ignored outright.
            (1, Some(128), None, None, Ok((Cipher::Rc4, 5))),
            (2, None, None, None, Ok((Cipher::Rc4, 5))),
            (2, Some(40), None, None, Ok((Cipher::Rc4, 5))),
            (2, Some(128), None, None, Ok((Cipher::Rc4, 16))),
            (
                2,
                Some(256),
                None,
                None,
                Err(Error::CipherKeyLength {
                    cipher: "RC4",
                    len: 32,
                }),
            ),
            // The `< 40 ⇒ × 8` promotion lives only in the /V >= 4 branch, so
            // /Length 8 here is a bare divide to a one-byte key.
            (
                2,
                Some(8),
                None,
                None,
                Err(Error::CipherKeyLength {
                    cipher: "RC4",
                    len: 1,
                }),
            ),
            (4, Some(128), None, Some("V2"), Ok((Cipher::Rc4, 16))),
            (4, Some(128), Some(16), Some("AESV2"), Ok((Cipher::Aes, 16))),
            (
                4,
                Some(128),
                Some(128),
                Some("AESV2"),
                Ok((Cipher::Aes, 16)),
            ),
            (4, None, None, Some("AESV2"), Ok((Cipher::Aes, 16))),
            (
                4,
                Some(128),
                Some(40),
                Some("AESV2"),
                Err(Error::CipherKeyLength {
                    cipher: "AES",
                    len: 5,
                }),
            ),
            (5, Some(256), Some(32), Some("AESV3"), Ok((Cipher::Aes, 32))),
            (5, None, None, Some("AESV3"), Ok((Cipher::Aes, 32))),
        ];
        for (version, length, filter_length, method, expected) in cases {
            let dict = encrypt_dict(version, length, filter_length, method);
            assert_eq!(
                cipher_of(&dict),
                expected,
                "/V {version} /Length {length:?} /CF Length {filter_length:?} /CFM {method:?}"
            );
        }
    }

    #[test]
    fn a_negative_filter_length_is_malformed() {
        let dict = test_fixtures::encrypt_dict(4, Some(128), Some(-8), Some("AESV2"));
        assert!(matches!(
            cipher_of(&dict),
            Err(Error::MalformedEncryptDict(_))
        ));
    }

    // The /Identity crypt filter is a handler, not a failure.
    #[test]
    fn an_identity_crypt_filter_yields_the_identity_handler() {
        let dict = test_fixtures::identity_dict();
        assert_eq!(cipher_of(&dict), Ok((Cipher::None, 0)));
        let handler = SecurityHandler::from_encrypt_dict(&dict, &[], b"", &NoResolve)
            .expect("identity needs no password");
        assert!(matches!(handler, SecurityHandler::Identity));
        assert_eq!(
            handler.decrypt(ObjRef::new(3, 0), CryptClass::Stream, b"plain"),
            b"plain"
        );
    }

    // ---- T15: the crypt-filter class rules ----

    /// §7.6.5 table 20 makes `/StmF` and `/StrF` two independent entries, so
    /// differing names are conformant — the stream filter supplies the
    /// cipher this record models.
    // [oracle-bug] cpdf_security_handler.cpp:305 and :325 return false on a
    // raw name inequality.
    #[test]
    fn differing_stream_and_string_filters_open_rather_than_refusing() {
        let mut dict = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        dict.push(names::STR_F.clone(), Object::Name(Name::from("Other")));
        assert_eq!(cipher_of(&dict), Ok((Cipher::Aes, 16)));
    }

    /// An absent `/StmF` against an explicit `/StrF`. PDFium compares the
    /// looked-up bytes *before* applying any default and absent reads as
    /// empty. §7.6.5's default is `/Identity`, so the streams pass through
    /// while the strings are enciphered by `/StrF`'s filter.
    #[test]
    fn an_absent_stream_filter_defaults_to_identity_beside_a_named_string_filter() {
        let mut dict = test_fixtures::bare_v4_dict();
        dict.push(names::STR_F.clone(), Object::Name(Name::from("StdCF")));
        let params =
            super::parse_encrypt_dict(&dict, &NoResolve).expect("a defaulted /StmF is conformant");
        assert_eq!(params.cipher, Cipher::None, "/StmF defaults to /Identity");
        assert_eq!(params.string_cipher, Cipher::Aes, "/StrF names StdCF");
    }

    /// Both class filters absent. §7.6.5 defaults **both** to `/Identity`, so
    /// `/CF` is never consulted and nothing is enciphered.
    #[test]
    fn both_class_filters_absent_default_to_identity() {
        let dict = test_fixtures::bare_v4_dict();
        assert_eq!(cipher_of(&dict), Ok((Cipher::None, 0)));
        let handler = SecurityHandler::from_encrypt_dict(&dict, &[], b"", &NoResolve)
            .expect("a document with neither class filter opens");
        assert!(matches!(handler, SecurityHandler::Identity));
    }

    /// A V4 dictionary with no `/CF`. With both classes defaulting to
    /// `/Identity`, `/CF` is never consulted, so the dictionary resolves to no
    /// cipher rather than to a malformation.
    #[test]
    fn a_missing_crypt_filter_dictionary_defaults_to_identity() {
        let dict = Dict::from_pairs([
            (names::FILTER.clone(), Object::Name(names::STANDARD.clone())),
            (names::V.clone(), Object::Int(4)),
            (names::R.clone(), Object::Int(4)),
        ]);
        assert_eq!(cipher_of(&dict), Ok((Cipher::None, 0)));
    }

    /// `/StrF /Identity` beside an enciphering `/StmF`. PDFium refuses the
    /// document; §7.6.5 says its strings are plaintext while its streams are
    /// enciphered.
    #[test]
    fn an_identity_string_filter_leaves_strings_plaintext_beside_an_enciphering_stream() {
        let mut dict = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        dict.push(names::STR_F.clone(), Object::Name(names::IDENTITY.clone()));
        let params = super::parse_encrypt_dict(&dict, &NoResolve).expect("conformant per §7.6.5");
        assert_eq!(params.cipher, Cipher::Aes);
        assert_eq!(params.string_cipher, Cipher::None);
    }

    // ---- Handler-level facts ----

    #[test]
    fn a_non_standard_filter_is_unsupported() {
        for spelling in ["Adobe.PubSec", "Nonesuch"] {
            let dict =
                Dict::from_pairs([(names::FILTER.clone(), Object::Name(Name::from(spelling)))]);
            assert_eq!(
                SecurityHandler::from_encrypt_dict(&dict, &[], b"", &NoResolve).unwrap_err(),
                Error::UnsupportedHandler(spelling.as_bytes().into())
            );
        }
    }

    // The /Filter check is name-typed, so a string-valued one is not the
    // standard handler even though it spells "Standard".
    #[test]
    fn a_string_valued_filter_is_not_the_standard_handler() {
        let dict = Dict::from_pairs([(
            names::FILTER.clone(),
            Object::Str(PdfString::literal(b"Standard")),
        )]);
        assert_eq!(
            SecurityHandler::from_encrypt_dict(&dict, &[], b"", &NoResolve).unwrap_err(),
            Error::UnsupportedHandler(Box::default())
        );
    }

    // /EncryptMetadata is read boolean-typed before resolving, so an Int(0)
    // there does not turn metadata encryption off.
    #[test]
    fn encrypt_metadata_reads_only_a_boolean() {
        let mut dict = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        dict.push(names::ENCRYPT_METADATA.clone(), Object::Int(0));
        let params = super::parse_encrypt_dict(&dict, &NoResolve).expect("parses");
        assert!(params.encrypt_metadata, "an integer is not a boolean");

        let mut dict = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        dict.push(names::ENCRYPT_METADATA.clone(), Object::Bool(false));
        let params = super::parse_encrypt_dict(&dict, &NoResolve).expect("parses");
        assert!(!params.encrypt_metadata);
    }

    #[test]
    fn identity_reports_no_restrictions_and_no_revision() {
        let handler = SecurityHandler::Identity;
        assert_eq!(handler.permission_word(false), 0xFFFF_FFFF);
        assert_eq!(handler.permission_word(true), 0xFFFF_FFFF);
        assert_eq!(handler.revision(), 0);
        assert!(handler.encrypt_metadata());
        assert!(!handler.owner_unlocked());
    }

    #[test]
    fn the_permission_mask_clears_reserved_bits_and_forces_the_high_ones() {
        let dict = test_fixtures::encrypted_pdf_dict();
        let id = unhex("1B0FD0F5E29AD84DBF67775E9E3B009F");
        let handler = SecurityHandler::from_encrypt_dict(&dict, &id, b"1234", &NoResolve)
            .expect("the user password");
        let reported = handler.permission_word(false);
        assert_eq!(reported & 0b11, 0, "the two reserved bits are cleared");
        assert_eq!(
            reported & 0xFFFF_F0C0,
            0xFFFF_F0C0,
            "the forced bits are set"
        );
    }

    // The two public methods are the private word, decoded. The AESV2
    // fixture's `/P` reports `0xFFFF_F2C0`, which grants neither form filling
    // (bit 9) nor annotation modification (bit 6) — and the owner's view of
    // the same document grants everything.
    #[test]
    fn the_public_methods_decode_the_word_the_private_one_reports() {
        let dict = test_fixtures::encrypted_pdf_dict();
        let id = unhex("1B0FD0F5E29AD84DBF67775E9E3B009F");

        let user = SecurityHandler::from_encrypt_dict(&dict, &id, b"1234", &NoResolve)
            .expect("the user password");
        let granted = user.permissions();
        assert_eq!(granted, Permissions::from_bits(user.permission_word(false)));
        // `0xFFFF_F2C0` sets exactly one of table 22's eight named bits — 10,
        // accessibility extraction. Neither of the two the form session asks
        // about is granted, and neither is printing.
        assert_eq!(
            granted,
            Permissions {
                extract: true,
                ..Permissions::NONE
            }
        );
        // The user's own owner view is still the user's, since the user
        // password opened it, not the owner's.
        assert_eq!(user.owner_permissions(), granted);

        let owner = SecurityHandler::from_encrypt_dict(&dict, &id, b"5678", &NoResolve)
            .expect("the owner password");
        assert_eq!(owner.owner_permissions(), Permissions::ALL);
        assert_eq!(owner.permissions(), granted);
    }

    // ---- T12 / T13: damage tolerance ----

    #[test]
    fn a_short_user_entry_rejects_rather_than_reading_out_of_bounds() {
        for len in 0..16usize {
            let dict = test_fixtures::r3_dict_with_user_entry(&vec![0xCD; len]);
            let id = unhex("9b744068bb5efbe920baaba6da63c2bf");
            assert_eq!(
                SecurityHandler::from_encrypt_dict(&dict, &id, b"h\xf4tel", &NoResolve)
                    .unwrap_err(),
                Error::WrongPassword,
                "/U of {len} bytes"
            );
        }
    }

    // A /U of 16 to 31 bytes is zero-padded into the working buffer rather
    // than rejected, so the comparison still runs over its first 16 bytes.
    #[test]
    fn a_partial_user_entry_is_zero_padded_and_still_compared() {
        for len in 16..32usize {
            let dict = test_fixtures::r3_dict_with_user_entry(&vec![0xCD; len]);
            let id = unhex("9b744068bb5efbe920baaba6da63c2bf");
            // No panic; the wrong bytes simply do not match.
            assert!(
                SecurityHandler::from_encrypt_dict(&dict, &id, b"h\xf4tel", &NoResolve).is_err(),
                "/U of {len} bytes"
            );
        }
    }

    // T13 — /O and /U must each be at least 48 bytes whichever role is
    // checked, because the owner check hashes the whole of /U alongside the
    // password; /UE must be 32 for a user open and /OE for an owner one.
    #[test]
    fn short_version_five_entries_reject_rather_than_panicking() {
        for len in [0usize, 1, 31, 47] {
            let short = vec![0xEFu8; len];
            // Both password entries gate both roles.
            for key in [names::O, names::U, names::PERMS] {
                let mut dict = test_fixtures::r5_dict();
                dict.push(key.clone(), Object::Str(PdfString::literal(&short)));
                for password in [&b"h\xf4tel"[..], b"\xe2ge"] {
                    assert!(
                        SecurityHandler::from_encrypt_dict(&dict, &[], password, &NoResolve)
                            .is_err(),
                        "{key:?} of {len} bytes with {password:?}"
                    );
                }
            }
            // The wrapped-key entries gate only the role that unwraps them.
            for (key, password) in [(names::UE, &b"h\xf4tel"[..]), (names::OE, b"\xe2ge")] {
                let mut dict = test_fixtures::r5_dict();
                dict.push(key.clone(), Object::Str(PdfString::literal(&short)));
                assert!(
                    SecurityHandler::from_encrypt_dict(&dict, &[], password, &NoResolve).is_err(),
                    "{key:?} of {len} bytes"
                );
            }
        }
    }

    #[test]
    fn an_empty_perms_entry_rejects() {
        let mut dict = test_fixtures::r5_dict();
        dict.push(names::PERMS.clone(), Object::Str(PdfString::literal(b"")));
        assert_eq!(
            SecurityHandler::from_encrypt_dict(&dict, &[], b"h\xf4tel", &NoResolve).unwrap_err(),
            Error::WrongPassword
        );
    }

    // T11 — the bad-okey fixtures: a truncated /O must fail the open, not
    // read past its end (crbug.com/42270437).
    #[test]
    fn a_truncated_owner_entry_fails_the_open() {
        for revision in [2i64, 3] {
            for len in [0usize, 1, 31] {
                let dict = test_fixtures::rc4_dict_with_owner_entry(revision, &vec![0x11; len]);
                assert_eq!(
                    SecurityHandler::from_encrypt_dict(&dict, &[], b"a", &NoResolve).unwrap_err(),
                    Error::WrongPassword,
                    "/R {revision} with /O of {len} bytes"
                );
            }
        }
    }

    // ---- The signature-dictionary predicate ----

    #[test]
    fn signature_dictionaries_are_recognised_by_type_then_field_type() {
        let sig_type = Dict::from_pairs([(names::TYPE.clone(), Object::Name(names::SIG.clone()))]);
        assert!(is_signature_dict(&sig_type));

        let sig_field = Dict::from_pairs([(names::FT.clone(), Object::Name(names::SIG.clone()))]);
        assert!(is_signature_dict(&sig_field));

        // /Type present and not Sig shuts /FT out.
        let annot = Dict::from_pairs([
            (names::TYPE.clone(), Object::Name(Name::from("Annot"))),
            (names::FT.clone(), Object::Name(names::SIG.clone())),
        ]);
        assert!(!is_signature_dict(&annot));

        // Neither key at all.
        assert!(!is_signature_dict(&Dict::new()));
    }

    // The value is read through an accessor that spells a name and a string
    // alike, so a string-valued /Type counts.
    #[test]
    fn a_string_valued_type_still_names_a_signature() {
        let dict =
            Dict::from_pairs([(names::TYPE.clone(), Object::Str(PdfString::literal(b"Sig")))]);
        assert!(is_signature_dict(&dict));
    }

    // ---- The public decrypt entry point ----

    /// The three real fixtures, as opened handlers, for the payload tests.
    fn opened_handlers() -> Vec<(&'static str, SecurityHandler)> {
        let aes_v2_id = unhex("1B0FD0F5E29AD84DBF67775E9E3B009F");
        let rc4_id = unhex("9b744068bb5efbe920baaba6da63c2bf");
        vec![
            (
                "RC4 (/R 3)",
                SecurityHandler::from_encrypt_dict(
                    &test_fixtures::r3_dict(),
                    &rc4_id,
                    b"h\xf4tel",
                    &NoResolve,
                )
                .expect("the r3 user password"),
            ),
            (
                "AESV2 (/R 4)",
                SecurityHandler::from_encrypt_dict(
                    &test_fixtures::encrypted_pdf_dict(),
                    &aes_v2_id,
                    b"1234",
                    &NoResolve,
                )
                .expect("the encrypted.pdf user password"),
            ),
            (
                "AESV3 (/R 6)",
                SecurityHandler::from_encrypt_dict(
                    &test_fixtures::r6_dict(),
                    &[],
                    b"h\xf4tel",
                    &NoResolve,
                )
                .expect("the r6 user password"),
            ),
        ]
    }

    // RC4 is symmetric, so decrypting twice restores the payload; AES is not,
    // so only its length behavior is asserted here.
    #[test]
    fn rc4_decrypt_round_trips_through_the_public_api() {
        let rc4_id = unhex("9b744068bb5efbe920baaba6da63c2bf");
        let handler = SecurityHandler::from_encrypt_dict(
            &test_fixtures::r3_dict(),
            &rc4_id,
            b"h\xf4tel",
            &NoResolve,
        )
        .expect("the r3 user password");
        let obj = ObjRef::new(12, 3);
        let payload = b"Hello, encrypted world.".to_vec();
        for class in [CryptClass::Stream, CryptClass::String, CryptClass::Embedded] {
            let once = handler.decrypt(obj, class, &payload);
            assert_ne!(once, payload, "{class:?} actually enciphered");
            assert_eq!(handler.decrypt(obj, class, &once), payload, "{class:?}");
        }
    }

    // D1 — all three classes resolve to the same cipher and key, so the same
    // bytes decrypt identically whichever class they are labelled with.
    #[test]
    fn every_crypt_class_decrypts_the_same_way() {
        for (name, handler) in opened_handlers() {
            let obj = ObjRef::new(7, 0);
            let payload: Vec<u8> = (0..64u8).collect();
            let stream = handler.decrypt(obj, CryptClass::Stream, &payload);
            assert_eq!(
                handler.decrypt(obj, CryptClass::String, &payload),
                stream,
                "{name}"
            );
            assert_eq!(
                handler.decrypt(obj, CryptClass::Embedded, &payload),
                stream,
                "{name}"
            );
        }
    }

    // -----------------------------------------------------------------
    // `/EFF`. The oracle reads the key nowhere
    // (`grep '"EFF"' core/ fpdfsdk/` is empty), so an embedded file stream
    // decrypts with the stream filter whatever `/EFF` says. The corpus has
    // no file with an `/EFF` at all, let alone one differing from `/StmF`,
    // so these fixtures are constructed rather than taken from it.
    // -----------------------------------------------------------------

    /// The `encrypted.pdf` dictionary — whose `/StmF` is AESV2 and whose
    /// `/O`/`/U` are real, so it opens with `1234` — extended with a second
    /// `/CF` entry that `/EFF` names.
    fn eff_dict(embedded_method: &str) -> Dict {
        use pdfrum_object::Name;
        let mut dict = test_fixtures::encrypted_pdf_dict();
        let std_cf = Dict::from_pairs([
            (names::CFM.clone(), Object::Name(Name::from("AESV2"))),
            (names::LENGTH.clone(), Object::Int(16)),
        ]);
        let emb_cf = Dict::from_pairs([
            (
                names::CFM.clone(),
                Object::Name(Name::from(embedded_method)),
            ),
            (names::LENGTH.clone(), Object::Int(16)),
        ]);
        dict.push(
            names::CF.clone(),
            Object::Dict(Dict::from_pairs([
                (Name::from("StdCF"), Object::Dict(std_cf)),
                (Name::from("EmbCF"), Object::Dict(emb_cf)),
            ])),
        );
        dict.push(names::EFF.clone(), Object::Name(Name::from("EmbCF")));
        dict
    }

    /// The handler `eff_dict` opens to, under `encrypted.pdf`'s password.
    fn eff_handler(dict: &Dict) -> SecurityHandler {
        SecurityHandler::from_encrypt_dict(
            dict,
            &unhex("1B0FD0F5E29AD84DBF67775E9E3B009F"),
            b"1234",
            &NoResolve,
        )
        .expect("the encrypted.pdf user password")
    }

    /// An `/EFF` naming a filter with a different `/CFM` gives the embedded
    /// class its own cipher — pdf.js `crypto.js:1120`, `:1336`; ISO 32000-1
    /// §7.6.5 table 20. The embedded class is not folded onto `Stream`.
    #[test]
    fn an_eff_naming_another_filter_decrypts_embedded_files_with_it() {
        let dict = eff_dict("V2");
        let params = super::parse_encrypt_dict(&dict, &NoResolve).unwrap();
        assert_eq!(params.cipher, Cipher::Aes);
        assert_eq!(params.embedded_cipher, Some(Cipher::Rc4));

        let handler = eff_handler(&dict);
        assert_eq!(handler.embedded_cipher(), Some(Cipher::Rc4));

        let obj = ObjRef::new(9, 0);
        let payload: Vec<u8> = (0..48u8).collect();
        // The two classes now genuinely differ: AES reads the first sixteen
        // bytes as an initialisation vector, RC4 preserves length.
        let as_stream = handler.decrypt(obj, CryptClass::Stream, &payload);
        let as_embedded = handler.decrypt(obj, CryptClass::Embedded, &payload);
        assert_eq!(as_embedded.len(), payload.len());
        assert_ne!(as_embedded, as_stream);
        // And the string class follows the stream, as `/StrF` names `StdCF`.
        assert_eq!(
            handler.decrypt(obj, CryptClass::String, &payload),
            as_stream
        );
    }

    /// The embedded class round-trips through its own cipher.
    #[test]
    fn the_embedded_class_round_trips_under_its_own_cipher() {
        let dict = eff_dict("V2");
        let handler = eff_handler(&dict);
        let obj = ObjRef::new(9, 0);
        let payload = b"an attachment".to_vec();
        let sealed = handler.encrypt(obj, CryptClass::Embedded, Iv([3; 16]), &payload);
        assert_eq!(handler.decrypt(obj, CryptClass::Embedded, &sealed), payload);
        // Sealed under RC4, so it is not what the stream cipher would make.
        assert_ne!(
            sealed,
            handler.encrypt(obj, CryptClass::Stream, Iv([3; 16]), &payload)
        );
    }

    /// Table 20's default for an absent `/EFF` is `/StmF`, so a document
    /// without the key — every file in the corpus — is unchanged.
    #[test]
    fn an_absent_eff_leaves_every_class_on_the_stream_cipher() {
        for (name, handler) in opened_handlers() {
            assert_eq!(handler.embedded_cipher(), None, "{name}");
        }
        let dict = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        let params = super::parse_encrypt_dict(&dict, &NoResolve).unwrap();
        assert_eq!(params.embedded_cipher, None);
    }

    /// Naming `/StmF`'s own filter is the default written out, and an `/EFF`
    /// whose `/CFM` resolves to the same cipher needs no override either.
    #[test]
    fn an_eff_that_agrees_with_the_stream_filter_is_no_override() {
        use pdfrum_object::Name;
        let mut same = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        same.push(names::EFF.clone(), Object::Name(Name::from("StdCF")));
        assert_eq!(
            super::parse_encrypt_dict(&same, &NoResolve)
                .unwrap()
                .embedded_cipher,
            None
        );
        // Different filter name, same cipher — AESV3 and AESV2 are both
        // `Cipher::Aes`, which is the level `/EFF` can actually change.
        let agreeing = eff_dict("AESV3");
        assert_eq!(
            super::parse_encrypt_dict(&agreeing, &NoResolve)
                .unwrap()
                .embedded_cipher,
            None
        );
    }

    /// `/EFF /Identity` leaves embedded files in the clear while the streams
    /// stay enciphered — the case that makes `/EFF` worth reading at all.
    #[test]
    fn an_identity_eff_leaves_embedded_files_unenciphered() {
        let mut dict = test_fixtures::encrypted_pdf_dict();
        dict.push(names::EFF.clone(), Object::Name(names::IDENTITY.clone()));
        let handler = eff_handler(&dict);
        assert_eq!(handler.embedded_cipher(), Some(Cipher::None));
        let obj = ObjRef::new(9, 0);
        let payload: Vec<u8> = (0..48u8).collect();
        assert_eq!(
            handler.decrypt(obj, CryptClass::Embedded, &payload),
            payload
        );
        assert_ne!(handler.decrypt(obj, CryptClass::Stream, &payload), payload);
    }

    /// An `/EFF` naming a filter `/CF` does not have is damage, not a reason
    /// to refuse the document: the streams still decrypt, and the embedded
    /// class falls back to the stream cipher — the absent-key default.
    #[test]
    fn an_eff_naming_a_missing_filter_falls_back_rather_than_failing() {
        use pdfrum_object::Name;
        let mut dict = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        dict.push(names::EFF.clone(), Object::Name(Name::from("NoSuchCF")));
        let params = super::parse_encrypt_dict(&dict, &NoResolve).expect("still opens");
        assert_eq!(params.embedded_cipher, None);
    }

    // The object number keys the payload for RC4 and AESV2 but not for
    // AESV3, whose key is the file key itself.
    #[test]
    fn only_the_pre_version_five_handlers_key_by_object() {
        let payload: Vec<u8> = (0..48u8).map(|i| i.wrapping_mul(5)).collect();
        for (name, handler) in opened_handlers() {
            let first = handler.decrypt(ObjRef::new(1, 0), CryptClass::Stream, &payload);
            let second = handler.decrypt(ObjRef::new(2, 0), CryptClass::Stream, &payload);
            if handler.revision() >= 5 {
                assert_eq!(second, first, "{name} uses the file key verbatim");
            } else {
                assert_ne!(second, first, "{name} salts by object number");
            }
        }
    }

    // No input length may panic, and nothing may produce more bytes than it
    // was given.
    #[test]
    fn decrypt_never_panics_and_never_grows_a_payload() {
        for (name, handler) in opened_handlers() {
            for len in 0..80usize {
                let payload = vec![0xA5u8; len];
                let out = handler.decrypt(ObjRef::new(9, 1), CryptClass::Stream, &payload);
                assert!(out.len() <= len, "{name} at {len} bytes");
            }
        }
        // The identity handler is the one that returns exactly its input.
        for len in 0..40usize {
            let payload = vec![0x5Au8; len];
            assert_eq!(
                SecurityHandler::Identity.decrypt(ObjRef::new(0, 0), CryptClass::String, &payload),
                payload
            );
        }
    }

    // ---- encrypt-then-decrypt, per revision ----

    /// Every revision the corpus exercises, as an opened handler.
    ///
    /// `opened_handlers` covers three; this adds R2 and R5 so the round-trip
    /// matrix spans /R 2 through /R 6, which is what byte-identity per
    /// revision requires.
    fn every_revision() -> Vec<(&'static str, SecurityHandler)> {
        let r2_id = unhex("2b778de1bcef1733b35e680882812409");
        let mut all = vec![(
            "RC4 (/R 2)",
            SecurityHandler::from_encrypt_dict(
                &test_fixtures::r2_dict(),
                &r2_id,
                b"h\xf4tel",
                &NoResolve,
            )
            .expect("the r2 user password"),
        )];
        all.extend(opened_handlers());
        all.push((
            "AESV3 (/R 5)",
            SecurityHandler::from_encrypt_dict(
                &test_fixtures::r5_dict(),
                &[],
                b"h\xf4tel",
                &NoResolve,
            )
            .expect("the r5 user password"),
        ));
        all
    }

    // The KAT: whatever we encipher, our own decipher returns byte for byte,
    // at every revision, every class and every length that straddles a block
    // boundary.
    #[test]
    fn every_revision_round_trips_encrypt_then_decrypt() {
        for (name, handler) in every_revision() {
            let obj = ObjRef::new(11, 0);
            for len in [0usize, 1, 15, 16, 17, 31, 32, 33, 64, 127] {
                let payload: Vec<u8> = (0..len)
                    .map(|i| u8::try_from(i % 253).unwrap_or(0))
                    .collect();
                for class in [CryptClass::Stream, CryptClass::String, CryptClass::Embedded] {
                    let iv = Iv([u8::try_from(len % 256).unwrap_or(0); 16]);
                    let sealed = handler.encrypt(obj, class, iv, &payload);
                    assert_eq!(
                        handler.decrypt(obj, class, &sealed),
                        payload,
                        "{name} {class:?} at {len} bytes"
                    );
                }
            }
        }
    }

    // A payload really is enciphered — a handler that returned its input
    // would pass the round-trip test above and write a plaintext file.
    #[test]
    fn an_encrypted_payload_is_not_its_own_plaintext() {
        let payload = b"Hello, encrypted world.".to_vec();
        for (name, handler) in every_revision() {
            let sealed = handler.encrypt(
                ObjRef::new(4, 0),
                CryptClass::Stream,
                Iv([0x5A; 16]),
                &payload,
            );
            assert_ne!(sealed, payload, "{name}");
        }
        // Identity is the one handler that passes bytes through untouched.
        assert_eq!(
            SecurityHandler::Identity.encrypt(
                ObjRef::new(4, 0),
                CryptClass::Stream,
                Iv([0; 16]),
                &payload
            ),
            payload
        );
    }

    // The object reference keys the payload for RC4 and AESV2 and does not
    // for AESV3 — the same split the decrypt side has, since it is the same
    // derivation.
    #[test]
    fn only_the_pre_version_five_handlers_key_an_encryption_by_object() {
        let payload = b"payload".to_vec();
        for (name, handler) in every_revision() {
            let iv = Iv([1; 16]);
            let first = handler.encrypt(ObjRef::new(1, 0), CryptClass::Stream, iv, &payload);
            let second = handler.encrypt(ObjRef::new(2, 0), CryptClass::Stream, iv, &payload);
            if handler.revision() >= 5 {
                assert_eq!(second, first, "{name} uses the file key verbatim");
            } else {
                assert_ne!(second, first, "{name} salts by object number");
            }
        }
    }

    // Determinism is a parameter here too: the same vector gives the same
    // bytes, which is what lets `pdfrum-edit` snapshot a whole encrypted file.
    #[test]
    fn the_same_vector_produces_the_same_ciphertext() {
        for (name, handler) in every_revision() {
            let obj = ObjRef::new(6, 0);
            let once = handler.encrypt(obj, CryptClass::Stream, Iv([2; 16]), b"stable");
            let again = handler.encrypt(obj, CryptClass::Stream, Iv([2; 16]), b"stable");
            assert_eq!(once, again, "{name}");
            // And a different vector does not, for the ciphers that read it.
            let other = handler.encrypt(obj, CryptClass::Stream, Iv([3; 16]), b"stable");
            if handler.revision() >= 4 {
                assert_ne!(other, once, "{name} mixes the vector in");
            }
        }
    }

    // No length may panic, and the growth is exactly the documented law.
    #[test]
    fn encrypt_never_panics_and_grows_by_the_documented_amount() {
        for (name, handler) in every_revision() {
            for len in 0..80usize {
                let out = handler.encrypt(
                    ObjRef::new(9, 1),
                    CryptClass::Stream,
                    Iv([0xC3; 16]),
                    &vec![0xA5u8; len],
                );
                let expected = if len == 0 || matches!(handler, SecurityHandler::Rc4V2 { .. }) {
                    len
                } else {
                    32 + (len / 16) * 16
                };
                assert_eq!(out.len(), expected, "{name} at {len} bytes");
            }
        }
    }

    // The one length that skips the cipher entirely. Without it a save would
    // rewrite every `()` in a document as 32 bytes of vector and padding —
    // which round-trips, but is not what the oracle writes.
    #[test]
    fn an_empty_payload_stays_empty_at_every_revision() {
        for (name, handler) in every_revision() {
            for class in [CryptClass::Stream, CryptClass::String, CryptClass::Embedded] {
                assert!(
                    handler
                        .encrypt(ObjRef::new(2, 0), class, Iv([0xFF; 16]), b"")
                        .is_empty(),
                    "{name} {class:?}"
                );
            }
        }
    }

    // ---- The properties a fuzzer would look for ----
    //
    // Every byte of an /Encrypt dictionary comes from the file, so no
    // combination of them may panic. These sweep the shape space
    // deterministically rather than randomly: a failure names the exact input
    // instead of a corpus file.

    /// A cheap deterministic byte sequence — no dependency, and reproducible.
    fn pseudo_random(seed: u64, len: usize) -> Vec<u8> {
        let mut state = seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                u8::try_from(state >> 56).unwrap_or(0)
            })
            .collect()
    }

    #[test]
    fn arbitrary_encrypt_dictionaries_never_panic() {
        for seed in 0..8u64 {
            for version in [-1i64, 0, 1, 2, 3, 4, 5, 6, 99] {
                for revision in [0i64, 2, 3, 4, 5, 6, 7] {
                    // The extremes are `INT_RANGE`'s: a lexer folds anything
                    // wider to zero, so nothing outside it can reach here.
                    for length in [
                        *pdfrum_object::INT_RANGE.start(),
                        -8,
                        0,
                        8,
                        40,
                        128,
                        256,
                        4096,
                        *pdfrum_object::INT_RANGE.end(),
                    ] {
                        let mut dict = test_fixtures::encrypt_dict(
                            version,
                            Some(length),
                            Some(length),
                            Some("AESV2"),
                        );
                        dict.push(names::R.clone(), Object::Int(revision));
                        for key in [names::O, names::U, names::OE, names::UE, names::PERMS] {
                            let entry = pseudo_random(
                                seed.wrapping_add(key.as_bytes().len() as u64),
                                usize::try_from(seed % 60).unwrap_or(0),
                            );
                            dict.push(key.clone(), Object::Str(PdfString::literal(entry)));
                        }
                        let password = pseudo_random(seed, usize::try_from(seed % 9).unwrap_or(0));
                        let file_id =
                            pseudo_random(seed + 1, usize::try_from(seed % 20).unwrap_or(0));
                        // The only requirement is that it returns.
                        let _ = SecurityHandler::from_encrypt_dict(
                            &dict, &file_id, &password, &NoResolve,
                        );
                    }
                }
            }
        }
    }

    // The revision 6 loop is the one unbounded-looking construction; a
    // hostile /U salt cannot make it run away, and a long password only
    // enlarges each round rather than adding rounds.
    #[test]
    fn arbitrary_version_five_entries_terminate() {
        for seed in 0..4u64 {
            let mut dict = test_fixtures::r6_dict();
            for key in [names::O, names::U, names::OE, names::UE, names::PERMS] {
                let entry = pseudo_random(seed, 48);
                dict.push(key.clone(), Object::Str(PdfString::literal(entry)));
            }
            // Long enough that each round moves real data, short enough that
            // the worst case — 287 rounds of 64 repetitions — stays quick.
            let password = pseudo_random(seed, 64);
            let _ = SecurityHandler::from_encrypt_dict(&dict, &[], &password, &NoResolve);
        }
    }

    // T11/T12/T13 as a sweep: any length of any password entry, at any
    // revision, must return rather than panic.
    #[test]
    fn every_password_entry_length_is_survivable() {
        for len in 0..64usize {
            let entry = pseudo_random(len as u64, len);
            // Revision 6 is covered by its own sweep above; running it for
            // every length here would spend minutes on the hardened hash.
            for base in [
                test_fixtures::r2_dict(),
                test_fixtures::r3_dict(),
                test_fixtures::encrypted_pdf_dict(),
                test_fixtures::r5_dict(),
            ] {
                for key in [names::O, names::U, names::OE, names::UE, names::PERMS] {
                    let mut dict = base.clone();
                    dict.push(key.clone(), Object::Str(PdfString::literal(&entry)));
                    let _ = SecurityHandler::from_encrypt_dict(&dict, &[], b"pw", &NoResolve);
                }
            }
        }
    }

    #[test]
    fn handlers_are_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SecurityHandler>();
        assert_send_sync::<Error>();
        assert_send_sync::<crate::EncryptParams>();
    }

    #[test]
    fn a_handler_debug_dump_never_shows_key_material() {
        let dict = test_fixtures::encrypted_pdf_dict();
        let id = unhex("1B0FD0F5E29AD84DBF67775E9E3B009F");
        let handler = SecurityHandler::from_encrypt_dict(&dict, &id, b"1234", &NoResolve)
            .expect("the user password");
        let dump = format!("{handler:?}");
        assert!(dump.contains("redacted"), "{dump}");
    }
}
