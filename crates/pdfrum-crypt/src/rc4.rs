//! RC4, the stream cipher behind `/V 1..4` (ISO 32000 §7.6.2).
//!
//! Textbook key-scheduling and pseudo-random generation over a 256-byte
//! permutation. The one non-textbook detail is the empty key: PDFium's key
//! schedule substitutes a zero key byte rather than dividing by zero, so an
//! empty key names a real, reproducible permutation instead of an error. A
//! recovered user password can legitimately be empty, so that path is
//! reachable from ordinary files.
//!
//! The cipher is an involution — encryption and decryption are the same
//! operation — which the revision 3+ password checks lean on to undo twenty
//! rounds of encryption by running twenty rounds more.

/// The permutation state of one RC4 keystream.
///
/// A value of this type is a keystream position, not a cipher: consuming it
/// with [`Rc4::apply`] is the whole API.
#[derive(Clone)]
struct Rc4 {
    perm: [u8; 256],
    x: u8,
    y: u8,
}

impl Rc4 {
    /// Schedule the permutation from `key`.
    ///
    /// An empty key contributes zero at every step, matching PDFium rather
    /// than rejecting the input.
    fn new(key: &[u8]) -> Self {
        let mut perm = [0u8; 256];
        for (i, slot) in perm.iter_mut().enumerate() {
            *slot = u8::try_from(i).unwrap_or(0);
        }
        let mut j = 0u8;
        for i in 0..=u8::MAX {
            let k = key
                .get(usize::from(i) % key.len().max(1))
                .copied()
                .unwrap_or(0);
            j = j.wrapping_add(at(&perm, i)).wrapping_add(k);
            perm.swap(usize::from(i), usize::from(j));
        }
        Self { perm, x: 0, y: 0 }
    }

    /// Exclusive-or `data` with the keystream, in place.
    fn apply(&mut self, data: &mut [u8]) {
        for byte in data {
            self.x = self.x.wrapping_add(1);
            self.y = self.y.wrapping_add(at(&self.perm, self.x));
            self.perm.swap(usize::from(self.x), usize::from(self.y));
            let s = at(&self.perm, self.x).wrapping_add(at(&self.perm, self.y));
            *byte ^= at(&self.perm, s);
        }
    }
}

/// Read the permutation at a byte-wide position.
///
/// A `u8` cannot address past the end of a 256-element array, which is the
/// whole reason the permutation is that width; stating the lookup once keeps
/// that argument in one place instead of at every use.
fn at(perm: &[u8; 256], index: u8) -> u8 {
    perm.get(usize::from(index)).copied().unwrap_or(0)
}

/// Crypt `data` under `key`, returning a fresh buffer.
///
/// RC4 is symmetric, so this both encrypts and decrypts.
///
/// ```
/// # use pdfrum_crypt::rc4;
/// let ct = rc4(b"foobar", b"secret");
/// assert_eq!(rc4(b"foobar", &ct), b"secret");
/// ```
#[must_use]
pub fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    rc4_in_place(key, &mut out);
    out
}

/// Crypt `data` under `key` in place.
pub(crate) fn rc4_in_place(key: &[u8], data: &mut [u8]) {
    Rc4::new(key).apply(data);
}

#[cfg(test)]
mod tests {
    use super::rc4;

    /// The plaintext of `fx_crypt_unittest.cpp`'s RC4 vectors, which iterates
    /// a `uint8_t[]` string literal and so includes the NUL terminator.
    fn short_plaintext() -> Vec<u8> {
        let mut v = b"The Quick Fox Jumped Over The Lazy Brown Dog.".to_vec();
        v.push(0);
        v
    }

    fn long_plaintext() -> Vec<u8> {
        let mut v = concat!(
            "The Quick Fox Jumped Over The Lazy Brown Dog.\n",
            "1234567890123456789012345678901234567890123456789012345678901234567890\n",
            "1234567890123456789012345678901234567890123456789012345678901234567890\n",
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ\n",
            "!@#$%^&*()[]{};':\",.<>/?\\|\r\t\n"
        )
        .as_bytes()
        .to_vec();
        v.push(0);
        v
    }

    // From fx_crypt_unittest.cpp:328-341 — the empty key is a defined
    // permutation, not an error.
    #[test]
    fn empty_key_short_data() {
        let expected: [u8; 46] = [
            138, 112, 236, 97, 242, 66, 52, 89, 225, 38, 88, 8, 47, 78, 216, 24, 170, 106, 26, 199,
            208, 131, 157, 242, 55, 11, 25, 90, 66, 182, 19, 255, 210, 181, 85, 69, 31, 240, 206,
            171, 97, 62, 202, 172, 30, 252,
        ];
        assert_eq!(rc4(&[], &short_plaintext()), expected);
    }

    // From fx_crypt_unittest.cpp:363-391 — over 256 bytes, so the
    // permutation wraps.
    #[test]
    fn empty_key_long_data() {
        let expected: [u8; 271] = [
            138, 112, 236, 97, 242, 66, 52, 89, 225, 38, 88, 8, 47, 78, 216, 24, 170, 106, 26, 199,
            208, 131, 157, 242, 55, 11, 25, 90, 66, 182, 19, 255, 210, 181, 85, 69, 31, 240, 206,
            171, 97, 62, 202, 172, 30, 246, 19, 43, 184, 0, 173, 27, 140, 90, 167, 240, 122, 125,
            184, 49, 149, 71, 63, 104, 171, 144, 242, 106, 121, 124, 209, 149, 61, 1, 66, 186, 252,
            47, 51, 170, 253, 75, 95, 41, 203, 28, 197, 174, 144, 209, 166, 98, 142, 125, 44, 5,
            147, 42, 73, 178, 119, 90, 253, 69, 103, 178, 15, 136, 51, 112, 39, 81, 37, 111, 129,
            232, 106, 159, 126, 142, 120, 124, 48, 140, 253, 12, 223, 208, 106, 76, 60, 238, 5,
            162, 100, 226, 251, 156, 169, 35, 193, 10, 242, 210, 20, 96, 37, 84, 99, 183, 179, 203,
            62, 122, 54, 6, 51, 239, 142, 250, 238, 41, 223, 58, 48, 101, 29, 187, 43, 235, 3, 5,
            176, 33, 14, 171, 36, 26, 234, 207, 105, 79, 69, 126, 82, 183, 105, 228, 31, 173, 8,
            240, 99, 5, 147, 206, 215, 140, 48, 190, 165, 50, 41, 232, 29, 105, 156, 64, 229, 165,
            12, 64, 163, 255, 146, 108, 212, 125, 142, 101, 13, 99, 174, 10, 160, 68, 196, 120,
            110, 201, 254, 158, 97, 215, 0, 207, 90, 23, 208, 161, 105, 226, 164, 114, 80, 137, 58,
            107, 109, 42, 110, 100, 202, 170, 224, 89, 28, 5, 138, 19, 253, 105, 220, 105, 24, 187,
            109, 89, 205, 89, 202,
        ];
        assert_eq!(rc4(&[], &long_plaintext()), expected);
    }

    // From fx_crypt_unittest.cpp:421-426.
    #[test]
    fn foobar_key_short_data() {
        let expected: [u8; 46] = [
            59, 193, 117, 206, 167, 54, 218, 7, 229, 214, 188, 55, 90, 205, 196, 25, 36, 114, 199,
            218, 161, 107, 122, 119, 106, 167, 44, 175, 240, 123, 192, 102, 174, 167, 105, 187,
            202, 70, 121, 81, 17, 30, 5, 138, 116, 166,
        ];
        assert_eq!(rc4(b"foobar", &short_plaintext()), expected);
    }

    // From fx_crypt_unittest.cpp:447-477.
    #[test]
    fn foobar_key_long_data() {
        let expected: [u8; 271] = [
            59, 193, 117, 206, 167, 54, 218, 7, 229, 214, 188, 55, 90, 205, 196, 25, 36, 114, 199,
            218, 161, 107, 122, 119, 106, 167, 44, 175, 240, 123, 192, 102, 174, 167, 105, 187,
            202, 70, 121, 81, 17, 30, 5, 138, 116, 172, 169, 50, 160, 116, 237, 117, 108, 241, 127,
            61, 83, 45, 77, 176, 0, 106, 191, 221, 132, 143, 219, 94, 2, 235, 204, 166, 201, 139,
            140, 163, 104, 115, 48, 37, 18, 114, 168, 49, 235, 163, 179, 131, 182, 218, 120, 200,
            9, 90, 60, 47, 55, 235, 135, 37, 21, 170, 48, 112, 185, 169, 43, 233, 88, 134, 117,
            126, 248, 40, 176, 248, 30, 131, 108, 43, 139, 68, 232, 219, 7, 39, 223, 45, 199, 243,
            54, 171, 31, 37, 161, 24, 38, 251, 13, 144, 106, 215, 179, 203, 5, 253, 25, 32, 25,
            146, 109, 193, 143, 141, 177, 226, 134, 222, 95, 79, 156, 202, 240, 34, 153, 145, 169,
            150, 231, 63, 113, 242, 156, 39, 136, 249, 108, 50, 181, 22, 22, 180, 57, 76, 69, 62,
            254, 47, 141, 249, 235, 90, 25, 34, 40, 194, 66, 86, 110, 192, 235, 191, 205, 133, 91,
            32, 104, 65, 43, 36, 140, 36, 228, 156, 105, 251, 169, 168, 203, 189, 238, 221, 64,
            200, 68, 137, 153, 9, 183, 84, 153, 140, 239, 0, 15, 50, 126, 145, 22, 110, 43, 56, 94,
            127, 48, 96, 47, 172, 3, 31, 130, 249, 243, 73, 206, 89, 9, 93, 156, 167, 205, 166, 75,
            227, 36, 34, 81, 124, 195, 246, 152,
        ];
        assert_eq!(rc4(b"foobar", &long_plaintext()), expected);
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert!(rc4(b"key", &[]).is_empty());
        assert!(rc4(&[], &[]).is_empty());
    }

    #[test]
    fn crypting_twice_restores_the_input() {
        let data: Vec<u8> = (0..300u32).map(|i| (i * 7 % 251) as u8).collect();
        for key in [&b""[..], b"k", b"a longer key than one block would need"] {
            assert_eq!(rc4(key, &rc4(key, &data)), data, "key {key:?}");
        }
    }
}
