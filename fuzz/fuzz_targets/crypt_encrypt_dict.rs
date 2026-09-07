//! `SecurityHandler::from_encrypt_dict` over a parsed `/Encrypt` dictionary.
//!
//! Property: construction never panics; a handler that comes back can decrypt
//! without panicking, and plaintext never grows.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_crypt::{CryptClass, SecurityHandler};
use pdfrum_object::{NoResolve, ObjRef, Object};
use pdfrum_parser::{Strictness, parse_object};

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let file_id = split.take();
    let password = split.take();
    let body = split.rest();

    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();
    let mut lexer = pdfrum_parser::Lexer::new(body);
    let Ok(Object::Dict(dict)) = parse_object(
        &mut lexer,
        &limits,
        &mut diags,
        Strictness::Loose,
        &NoResolve,
    ) else {
        return;
    };

    let Ok(handler) = SecurityHandler::from_encrypt_dict(&dict, file_id, password, &NoResolve)
    else {
        return;
    };

    // A handler that constructed must be usable. Round the payload through
    // every class; the plaintext length may only shrink (AES strips the IV
    // and the padding, RC4 is length-preserving).
    let obj = ObjRef {
        num: 1,
        generation: 0,
    };
    for class in [CryptClass::Stream, CryptClass::String, CryptClass::Embedded] {
        let out = handler.decrypt(obj, class, body);
        assert!(out.len() <= body.len());
    }
});
