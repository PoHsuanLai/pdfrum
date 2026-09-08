//! Making a document encrypted: the `/Encrypt` dictionary and the handler
//! for a fresh AES-256 (revision 6) file.
//!
//! ISO 32000-2 §7.6.4.4.7–8, algorithms 8, 9 and 10 — the inverse of what
//! [`crate::standard`] checks when such a file is opened. Only revision 6:
//! RC4 and the 128-bit AES of revisions 2–4 are deprecated by the same
//! standard, and a file this crate writes should be one its reader would
//! choose. Opening covers every revision regardless.
//!
//! The file key and the four salts are 64 bytes of operating-system
//! randomness, held by [`KeyMaterial`]. That type is the only way to reach
//! [`standard_r6`], and its only constructor draws from the OS, so no derived
//! or reproducible byte sequence can stand in for a key.

use pdfrum_object::{Dict, Name, NoResolve, Object, PdfString};

use zeroize::Zeroize;

use crate::Error;
use crate::SecurityHandler;
use crate::permissions::Permissions;
use crate::primitives::aes_cbc_encrypt;
use crate::standard::{r6_prepared, revision6_hash};

/// The bytes of a new file's secrets: the 32-byte file key, then the
/// user validation salt, user key salt, owner validation salt and owner key
/// salt, 8 bytes each, then four random bytes for `/Perms` (ISO 32000-2
/// Algorithm 10).
pub const ENTROPY_LEN: usize = 68;

/// The secret bytes behind one encrypted file: the AES-256 file key, the
/// four revision-6 salts (ISO 32000-2 §7.6.4.4.7, algorithms 8 and 9), and
/// four random bytes for `/Perms` (Algorithm 10).
///
/// The bytes come from the operating system's cryptographic generator and
/// from nowhere else. There is no constructor taking a seed, a slice or a
/// byte array, so a caller cannot substitute a derived sequence: an
/// unguessable file key is a property of the type, not of the call site.
/// Neither `Clone` nor `Debug`, so the bytes are neither duplicated across
/// two files nor printed. `Drop` wipes them.
pub struct KeyMaterial([u8; ENTROPY_LEN]);

impl KeyMaterial {
    /// Sixty-eight fresh bytes from the operating system.
    ///
    /// # Errors
    ///
    /// [`Error::NoEntropy`] when the platform's generator is unavailable —
    /// the only outcome besides success, since a partial read is not one the
    /// underlying interface reports.
    pub fn from_os() -> Result<Self, Error> {
        let mut bytes = [0u8; ENTROPY_LEN];
        getrandom::fill(&mut bytes).map_err(|_| Error::NoEntropy)?;
        Ok(Self(bytes))
    }
}

impl Drop for KeyMaterial {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// The `/P` bits this crate names; every other bit is reserved and written
/// as 1, which is what ISO 32000-2 Table 22 asks for.
const NAMED_PERMISSION_BITS: u32 = 0x0F3C;

/// The `/Encrypt` dictionary and the handler that enciphers under it, for a
/// document protected by `user` (opens with reading rights) and `owner`
/// (opens with every right). An empty user password means anyone can open
/// the file; an empty owner password is replaced by the user password, as
/// Acrobat does, so there is always a way to unlock it.
///
/// # Errors
///
/// [`Error::WrongPassword`] only in the impossible case that the handler
/// built from the dictionary does not accept the password it was built for
/// — which would be a defect in this function, not in the caller's input;
/// a password that is not valid UTF-8 or does not survive `SASLprep`.
pub fn standard_r6(
    user: &[u8],
    owner: &[u8],
    permissions: Permissions,
    encrypt_metadata: bool,
    key_material: &KeyMaterial,
) -> Result<(Dict, SecurityHandler), Error> {
    let entropy = &key_material.0;
    let owner = if owner.is_empty() { user } else { owner };
    let user_prepared = r6_prepared(6, user).ok_or(Error::WrongPassword)?;
    let owner_prepared = r6_prepared(6, owner).ok_or(Error::WrongPassword)?;

    let mut file_key: [u8; 32] = slice(entropy, 0)?;
    let user_validation: [u8; 8] = slice(entropy, 32)?;
    let user_key_salt: [u8; 8] = slice(entropy, 40)?;
    let owner_validation: [u8; 8] = slice(entropy, 48)?;
    let owner_key_salt: [u8; 8] = slice(entropy, 56)?;
    let perms_random: [u8; 4] = slice(entropy, 64)?;

    // Algorithm 8: /U is the hash of the user password and its validation
    // salt, then the two salts; /UE is the file key enciphered under the hash
    // of the password and its key salt.
    let mut u = [0u8; 48];
    u[..32].copy_from_slice(&revision6_hash(&user_prepared, user_validation, None));
    u[32..40].copy_from_slice(&user_validation);
    u[40..48].copy_from_slice(&user_key_salt);
    let ue = wrap(
        &revision6_hash(&user_prepared, user_key_salt, None),
        &file_key,
    )?;

    // Algorithm 9: the same for the owner, with /U folded into both hashes.
    let mut o = [0u8; 48];
    o[..32].copy_from_slice(&revision6_hash(&owner_prepared, owner_validation, Some(&u)));
    o[32..40].copy_from_slice(&owner_validation);
    o[40..48].copy_from_slice(&owner_key_salt);
    let oe = wrap(
        &revision6_hash(&owner_prepared, owner_key_salt, Some(&u)),
        &file_key,
    )?;

    // Algorithm 10: /Perms is the permissions word, four 0xFF bytes, T or F
    // for metadata, "adb", and four random bytes, under the file key.
    // The four bytes are their own slice of entropy, not a prefix of the
    // file key: ISO 32000-2 asks for random bytes, and putting key material
    // in the plaintext (then encrypting it under that key with a zero IV)
    // would hand four key bytes to anyone who recovered the block.
    let p = permissions.bits() | !NAMED_PERMISSION_BITS;
    let mut perms = [0u8; 16];
    perms[..4].copy_from_slice(&p.to_le_bytes());
    perms[4..8].copy_from_slice(&[0xFF; 4]);
    perms[8] = if encrypt_metadata { b'T' } else { b'F' };
    perms[9..12].copy_from_slice(b"adb");
    perms[12..16].copy_from_slice(&perms_random);
    aes_cbc_encrypt(&file_key, &[0u8; 16], &mut perms).map_err(|_| Error::WrongPassword)?;
    file_key.zeroize();

    let name = |s: &str| Object::Name(Name::from(s));
    let bytes = |b: &[u8]| Object::Str(PdfString::hex(b));
    let std_cf = Dict::from_pairs([
        (Name::from("CFM"), name("AESV3")),
        (Name::from("AuthEvent"), name("DocOpen")),
        (Name::from("Length"), Object::Int(32)),
    ]);
    let cf = Dict::from_pairs([(Name::from("StdCF"), Object::Dict(std_cf))]);
    #[expect(
        clippy::cast_possible_wrap,
        reason = "/P is the same 32 bits read as a signed integer, per the standard"
    )]
    let p_signed = i64::from(p as i32);
    let dict = Dict::from_pairs([
        (Name::from("Filter"), name("Standard")),
        (Name::from("V"), Object::Int(5)),
        (Name::from("R"), Object::Int(6)),
        (Name::from("Length"), Object::Int(256)),
        (Name::from("P"), Object::Int(p_signed)),
        (Name::from("O"), bytes(&o)),
        (Name::from("U"), bytes(&u)),
        (Name::from("OE"), bytes(&oe)),
        (Name::from("UE"), bytes(&ue)),
        (Name::from("Perms"), bytes(&perms)),
        (Name::from("CF"), Object::Dict(cf)),
        (Name::from("StmF"), name("StdCF")),
        (Name::from("StrF"), name("StdCF")),
        (
            Name::from("EncryptMetadata"),
            Object::Bool(encrypt_metadata),
        ),
    ]);

    // The handler is built the way an opened file's is, from the dictionary
    // and the owner password, so what this function wrote is what the
    // reader will check.
    let handler = SecurityHandler::from_encrypt_dict(&dict, &[], owner, &NoResolve)?;
    Ok((dict, handler))
}

/// `key` enciphered under `intermediate` with AES-256, no IV, no padding —
/// the /UE and /OE wrapping.
fn wrap(intermediate: &[u8; 32], key: &[u8; 32]) -> Result<[u8; 32], Error> {
    let mut wrapped = *key;
    aes_cbc_encrypt(intermediate, &[0u8; 16], &mut wrapped).map_err(|_| Error::WrongPassword)?;
    Ok(wrapped)
}

fn slice<const N: usize>(entropy: &[u8; ENTROPY_LEN], at: usize) -> Result<[u8; N], Error> {
    entropy
        .get(at..at + N)
        .and_then(|s| <[u8; N]>::try_from(s).ok())
        .ok_or(Error::WrongPassword)
}

#[cfg(test)]
mod tests {
    use super::{KeyMaterial, standard_r6};
    use crate::permissions::Permissions;
    use crate::{CryptClass, SecurityHandler};
    use pdfrum_object::{NoResolve, ObjRef};

    fn entropy() -> KeyMaterial {
        KeyMaterial::from_os().unwrap()
    }

    #[test]
    fn the_file_opens_with_either_password_and_not_with_a_wrong_one() {
        let perms = Permissions {
            print: true,
            ..Permissions::NONE
        };
        let (dict, handler) = standard_r6(b"user", b"owner", perms, true, &entropy()).unwrap();
        let as_user = SecurityHandler::from_encrypt_dict(&dict, &[], b"user", &NoResolve).unwrap();
        assert!(!as_user.owner_unlocked());
        assert!(as_user.permissions().print && !as_user.permissions().copy);
        let as_owner =
            SecurityHandler::from_encrypt_dict(&dict, &[], b"owner", &NoResolve).unwrap();
        assert!(as_owner.owner_unlocked());
        assert!(SecurityHandler::from_encrypt_dict(&dict, &[], b"nope", &NoResolve).is_err());
        assert!(
            handler.owner_unlocked(),
            "the handler this function hands back is the owner's"
        );
    }

    #[test]
    fn what_the_handler_enciphers_the_opened_one_deciphers() {
        let (dict, handler) =
            standard_r6(b"", b"secret", Permissions::ALL, true, &entropy()).unwrap();
        let obj = ObjRef::new(7, 0);
        let iv = crate::Iv([3u8; 16]);
        let enciphered = handler.encrypt(obj, CryptClass::Stream, iv, b"hello, cipher");
        assert_ne!(enciphered, b"hello, cipher");
        let reader = SecurityHandler::from_encrypt_dict(&dict, &[], b"", &NoResolve).unwrap();
        assert_eq!(
            reader.decrypt(obj, CryptClass::Stream, &enciphered),
            b"hello, cipher"
        );
    }

    #[test]
    fn an_empty_owner_password_falls_back_to_the_user_password() {
        let (dict, _) = standard_r6(b"pw", b"", Permissions::ALL, false, &entropy()).unwrap();
        let opened = SecurityHandler::from_encrypt_dict(&dict, &[], b"pw", &NoResolve).unwrap();
        assert!(opened.owner_unlocked());
        assert!(!opened.encrypt_metadata());
    }

    #[test]
    fn perms_random_bytes_are_not_the_file_key_prefix() {
        let mut bytes = [0u8; super::ENTROPY_LEN];
        bytes[..32].fill(0xAA);
        bytes[32..64].fill(0x11);
        bytes[64..68].fill(0xBB);
        let material = super::KeyMaterial(bytes);
        let (dict, _) = standard_r6(b"user", b"owner", Permissions::ALL, true, &material).unwrap();
        let perms = dict
            .string(&pdfrum_object::Name::from("Perms"))
            .expect("standard_r6 writes /Perms");
        let mut block = [0u8; 16];
        block.copy_from_slice(perms.bytes.as_ref());
        crate::primitives::aes_cbc_decrypt(&[0xAA; 32], &[0u8; 16], &mut block)
            .expect("the file key decrypts /Perms");
        assert_eq!(&block[12..16], &[0xBB; 4], "Algorithm 10's four bytes");
        assert_ne!(
            &block[12..16],
            &bytes[..4],
            "must not reuse the file-key prefix"
        );
    }
}
