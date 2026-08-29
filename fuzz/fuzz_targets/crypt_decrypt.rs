//! `SecurityHandler::decrypt` over arbitrary ciphertext.
//!
//! Where `crypt_encrypt_dict` fuzzes handler *construction* and decrypts one
//! payload as an afterthought, this target fixes a small set of real handlers
//! and pours arbitrary bytes through them at every length — which is where
//! the AES block/IV/padding arithmetic lives. The handlers are built from
//! synthesised `/Encrypt` dictionaries rather than reached through a
//! document, so no input is wasted getting past the key derivation.
//!
//! Property: no panic at any payload length, and no payload grows.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_crypt::{CryptClass, SecurityHandler};
use pdfrum_object::{Dict, Name, NoResolve, ObjRef, Object, PdfString};

/// An `/Encrypt` dictionary for revision `r`, version `v`, cipher `cfm`.
///
/// `/O` and `/U` are the padding string itself, which is what an empty
/// user password over a zero file id produces closely enough for the
/// handler to construct — and when it does not, the handler simply reports
/// a wrong password and this input is skipped.
fn encrypt_dict(v: i64, r: i64, length: i64, cfm: &str) -> Dict {
    let mut cf = Dict::new();
    let mut stdcf = Dict::new();
    stdcf.push(Name::new("CFM"), Object::Name(Name::new(cfm)));
    stdcf.push(Name::new("Length"), Object::Int(length / 8));
    cf.push(Name::new("StdCF"), Object::Dict(stdcf));

    let mut dict = Dict::new();
    dict.push(Name::new("Filter"), Object::Name(Name::new("Standard")));
    dict.push(Name::new("V"), Object::Int(v));
    dict.push(Name::new("R"), Object::Int(r));
    dict.push(Name::new("Length"), Object::Int(length));
    dict.push(Name::new("P"), Object::Int(-1));
    dict.push(
        Name::new("O"),
        Object::Str(PdfString::literal(pdfrum_crypt::PAD)),
    );
    dict.push(
        Name::new("U"),
        Object::Str(PdfString::literal(pdfrum_crypt::PAD)),
    );
    if v >= 4 {
        dict.push(Name::new("CF"), Object::Dict(cf));
        dict.push(Name::new("StmF"), Object::Name(Name::new("StdCF")));
        dict.push(Name::new("StrF"), Object::Name(Name::new("StdCF")));
    }
    dict
}

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let selector = split.byte();
    let num = u32::from(split.byte());
    let generation = u16::from(split.byte());
    let payload = split.rest();

    // Five handler shapes: RC4 at two key lengths, AES-128, AES-256, and the
    // identity handler. Between them they cover every branch `decrypt` has.
    let dict = match selector % 5 {
        0 => encrypt_dict(1, 2, 40, "V2"),
        1 => encrypt_dict(2, 3, 128, "V2"),
        2 => encrypt_dict(4, 4, 128, "AESV2"),
        3 => encrypt_dict(5, 6, 256, "AESV3"),
        _ => encrypt_dict(4, 4, 128, "None"),
    };

    let Ok(handler) = SecurityHandler::from_encrypt_dict(&dict, b"", b"", &NoResolve) else {
        return;
    };

    let obj = ObjRef { num, generation };
    for class in [CryptClass::Stream, CryptClass::String, CryptClass::Embedded] {
        let out = handler.decrypt(obj, class, payload);
        assert!(out.len() <= payload.len());
    }
});
