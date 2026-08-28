//! The corpus `/Encrypt` dictionaries, as literals.
//!
//! The C++ has no unit tests for the revision algorithms — it exercises them
//! only by opening whole documents. The dictionaries are small and
//! self-contained, so transcribing them turns end-to-end fixtures into
//! known-answer tests: every value below is the byte-for-byte content of the
//! named file in the oracle's `testing/resources/`, and the passwords are the
//! ones its embedder test uses.

use pdfrum_object::{Dict, Name, Object, PdfString, names};

use crate::standard::{EncryptParams, parse_encrypt_dict};

/// Decode a hexadecimal literal; a malformed digit contributes nothing.
pub(crate) fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks(2)
        .filter_map(|pair| std::str::from_utf8(pair).ok())
        .filter_map(|pair| u8::from_str_radix(pair, 16).ok())
        .collect()
}

/// A `/Standard` dictionary carrying the given entries.
fn standard(entries: impl IntoIterator<Item = (Name, Object)>) -> Dict {
    let mut dict =
        Dict::from_pairs([(names::FILTER.clone(), Object::Name(names::STANDARD.clone()))]);
    for (key, value) in entries {
        dict.push(key, value);
    }
    dict
}

/// A hex-spelled string entry.
fn hex_entry(key: &Name, text: &str) -> (Name, Object) {
    (key.clone(), Object::Str(PdfString::literal(unhex(text))))
}

/// `encrypted_hello_world_r2.pdf` — `/V 1 /R 2`, owner `âge`, user `hôtel`.
pub(crate) fn r2_dict() -> Dict {
    standard([
        (names::V.clone(), Object::Int(1)),
        (names::R.clone(), Object::Int(2)),
        (names::P.clone(), Object::Int(-64)),
        hex_entry(
            names::O,
            "65b4d14434c8434aeb2e2ddd3922e3233f4fdf4a527f179a3a5cca0563d6249e",
        ),
        hex_entry(
            names::U,
            "4219bd5bea1f046782e698112d6b80b2295e4b19e58f8690486800550c59e63e",
        ),
    ])
}

/// The parsed form of [`r2_dict`].
pub(crate) fn r2() -> EncryptParams {
    parse_encrypt_dict(&r2_dict(), &pdfrum_object::NoResolve).expect("the r2 fixture parses")
}

/// `encrypted_hello_world_r3.pdf` — `/V 2 /R 3 /Length 128`.
pub(crate) fn r3_dict() -> Dict {
    standard([
        (names::V.clone(), Object::Int(2)),
        (names::R.clone(), Object::Int(3)),
        (names::LENGTH.clone(), Object::Int(128)),
        (names::P.clone(), Object::Int(-3904)),
        hex_entry(
            names::O,
            "894b1d3a9003e3bc172d8ff9277bc931a520f52c2d1f206e49d3ee74a901e408",
        ),
        hex_entry(
            names::U,
            "a923680e625d8922366aced0a070775e00000000000000000000000000000000",
        ),
    ])
}

/// The parsed form of [`r3_dict`].
pub(crate) fn r3() -> EncryptParams {
    parse_encrypt_dict(&r3_dict(), &pdfrum_object::NoResolve).expect("the r3 fixture parses")
}

/// [`r3_dict`] with `/U` replaced, for the short-entry damage tests.
pub(crate) fn r3_dict_with_user_entry(entry: &[u8]) -> Dict {
    let mut dict = r3_dict();
    dict.push(names::U.clone(), Object::Str(PdfString::literal(entry)));
    dict
}

/// An RC4 dictionary of the given revision with `/O` replaced, for the
/// `bad_okey` fixtures.
pub(crate) fn rc4_dict_with_owner_entry(revision: i64, entry: &[u8]) -> Dict {
    let mut dict = if revision == 2 { r2_dict() } else { r3_dict() };
    dict.push(names::O.clone(), Object::Str(PdfString::literal(entry)));
    dict
}

/// `encrypted.pdf` — `/V 4 /R 4` AESV2, whose `/CF/StdCF/Length 16` is the
/// crate's highest-value damage-tolerance fixture. User `1234`, owner `5678`.
pub(crate) fn encrypted_pdf_dict() -> Dict {
    let std_cf = Dict::from_pairs([
        (
            names::AUTH_EVENT.clone(),
            Object::Name(Name::from("DocOpen")),
        ),
        (names::CFM.clone(), Object::Name(Name::from("AESV2"))),
        (names::LENGTH.clone(), Object::Int(16)),
    ]);
    standard([
        (
            names::CF.clone(),
            Object::Dict(Dict::from_pairs([(
                Name::from("StdCF"),
                Object::Dict(std_cf),
            )])),
        ),
        (names::LENGTH.clone(), Object::Int(128)),
        (names::V.clone(), Object::Int(4)),
        (names::R.clone(), Object::Int(4)),
        (names::P.clone(), Object::Int(-3392)),
        (names::STM_F.clone(), Object::Name(Name::from("StdCF"))),
        (names::STR_F.clone(), Object::Name(Name::from("StdCF"))),
        hex_entry(
            names::O,
            "f4dac5619702f34666f7e8e1af03e4660072a021cdf7ec908bdb3c78f9c9633c",
        ),
        hex_entry(
            names::U,
            "3a15f3b2387a77be7080464022951ed100000000000000000000000000000000",
        ),
    ])
}

/// `encrypted_hello_world_r5.pdf` — `/V 5 /R 5` AESV3.
pub(crate) fn r5_dict() -> Dict {
    aes256_dict(
        5,
        -4,
        "a770489a67f9076d8edbd86032fc3f926295c2d04707d2a1b007d44a2b64f6f7b10bb0c97ec06e945e33ad56d5e8cca4",
        "e111339fe969ac0851e4f9542f4e3e3600be8f4b2fee27f572e92edc59378640",
        "3e5d54b881ae698f01104dfe4a36fccd5a94d913c575ebd4d44f43e6366269067bdef258bb4b37deb90db87904f84917",
        "8608d184b6f3cd03ebf896946cd9e9420b361fa380d5de9c94c2a819aa5a0638",
        "c954c264d796dfd131ddb784f5a8b1bf",
    )
}

/// `encrypted_hello_world_r6.pdf` — `/V 5 /R 6`, the hardened-hash fixture.
pub(crate) fn r6_dict() -> Dict {
    aes256_dict(
        6,
        -4,
        "d80e6106fd39478c8860c9145e896f126c3fa0c9125ad2096d242c075fa621ac992241608cc3d397135a2c0aec96db3b",
        "9046a22c32d33286559594eaef09b9cf49228b58d02bf5ddc3383df9263282e2",
        "0798ab4b1c93d360f96b8ba41d1add5b7eaf4b110f014de88a57615fdd6f677c1a5e059d15ed6eed88d94c0349583f86",
        "9e304c9fff647b71536db3684c72914cae3882885eb8cf9cfbf3026dae2e1f35",
        "030e31486569c2d3c03f01e57ece54fc",
    )
}

/// `bug_644.pdf` — `/V 5 /R 5` with `/P 4092` and the ASCII passwords
/// `a` (owner) and `b` (user).
pub(crate) fn bug_644_dict() -> Dict {
    aes256_dict(
        5,
        4092,
        "b6c711683d98f878929688ef497a0bb928e1f0013a0b5c357be701e42dc4a6a9e124b0c505ddda91562c5ea791e2b7ac",
        "26b337b3b635c18262b4915289f1d353eb432d7e7ff6be5450c82d690202a093",
        "69f20e0450e8b2a8aca6af1de1284db11ec4e38f6e7cb2b9ae9a1cff6f95ba6cd83783c4ed8b31d933482cbb7a791290",
        "5104e81c113d43246a264580fe82d2890b7b8ceef4a3d667b81a32eed62d8c54",
        "3d62c200cdb31a603ef202e12993ae13",
    )
}

/// The shared shape of the three `/V 5` fixtures.
fn aes256_dict(
    revision: i64,
    permissions: i64,
    o: &str,
    oe: &str,
    u: &str,
    ue: &str,
    perms: &str,
) -> Dict {
    let std_cf = Dict::from_pairs([
        (names::CFM.clone(), Object::Name(Name::from("AESV3"))),
        (names::LENGTH.clone(), Object::Int(32)),
    ]);
    standard([
        (names::V.clone(), Object::Int(5)),
        (names::R.clone(), Object::Int(revision)),
        (names::LENGTH.clone(), Object::Int(256)),
        (names::P.clone(), Object::Int(permissions)),
        (
            names::CF.clone(),
            Object::Dict(Dict::from_pairs([(
                Name::from("StdCF"),
                Object::Dict(std_cf),
            )])),
        ),
        (names::STM_F.clone(), Object::Name(Name::from("StdCF"))),
        (names::STR_F.clone(), Object::Name(Name::from("StdCF"))),
        hex_entry(names::O, o),
        hex_entry(names::OE, oe),
        hex_entry(names::U, u),
        hex_entry(names::UE, ue),
        hex_entry(names::PERMS, perms),
    ])
}

/// A synthetic dictionary for the key-length resolution table.
pub(crate) fn encrypt_dict(
    version: i64,
    length: Option<i64>,
    filter_length: Option<i64>,
    method: Option<&str>,
) -> Dict {
    let mut dict = standard([
        (names::V.clone(), Object::Int(version)),
        (names::R.clone(), Object::Int(version.clamp(2, 4))),
    ]);
    if let Some(bits) = length {
        dict.push(names::LENGTH.clone(), Object::Int(bits));
    }
    if version >= 4 {
        let mut std_cf = Dict::new();
        if let Some(name) = method {
            std_cf.push(names::CFM.clone(), Object::Name(Name::from(name)));
        }
        if let Some(bits) = filter_length {
            std_cf.push(names::LENGTH.clone(), Object::Int(bits));
        }
        dict.push(
            names::CF.clone(),
            Object::Dict(Dict::from_pairs([(
                Name::from("StdCF"),
                Object::Dict(std_cf),
            )])),
        );
        dict.push(names::STM_F.clone(), Object::Name(Name::from("StdCF")));
        dict.push(names::STR_F.clone(), Object::Name(Name::from("StdCF")));
    }
    dict
}

/// A `/V 4` dictionary with a `/CF` but neither class filter named.
pub(crate) fn bare_v4_dict() -> Dict {
    standard([
        (names::V.clone(), Object::Int(4)),
        (names::R.clone(), Object::Int(4)),
        (names::LENGTH.clone(), Object::Int(128)),
        (
            names::CF.clone(),
            Object::Dict(Dict::from_pairs([(
                Name::from("StdCF"),
                Object::Dict(Dict::from_pairs([(
                    names::CFM.clone(),
                    Object::Name(Name::from("AESV2")),
                )])),
            )])),
        ),
    ])
}

/// A `/V 4` dictionary naming `/Identity` as its crypt filter.
pub(crate) fn identity_dict() -> Dict {
    let mut dict = bare_v4_dict();
    dict.push(names::STM_F.clone(), Object::Name(names::IDENTITY.clone()));
    dict.push(names::STR_F.clone(), Object::Name(names::IDENTITY.clone()));
    dict
}
