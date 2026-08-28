//! A minimal JSON value model with a stable-ordered writer and a parser,
//! sized to this crate's own closed schemas (the scoreboard).
//!
//! Rolled by hand rather than taken as a dependency for the same reason SSIM
//! is (DEPS.md): the scoreboard is the project's fitness function, and its
//! byte-level shape — key order, float formatting — must never shift under a
//! dependency update. Object keys keep insertion order, so a scoreboard
//! written twice from equal data is byte-identical and diffs cleanly.

use std::fmt::Write as _;

/// A JSON value. `Object` preserves insertion order, which is what makes
/// scoreboard output diff-friendly.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// All numbers are carried as `f64`; integral values print without a
    /// fractional part so counts read as counts.
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// What went wrong while reading JSON text.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum JsonError {
    #[error("unexpected end of input at byte {at}")]
    Eof { at: usize },
    #[error("unexpected byte {byte:?} at {at}")]
    Unexpected { byte: char, at: usize },
    #[error("invalid number at {at}")]
    BadNumber { at: usize },
    #[error("invalid escape at {at}")]
    BadEscape { at: usize },
    #[error("trailing data at {at}")]
    Trailing { at: usize },
}

impl Json {
    /// Builds a string value.
    pub fn str(s: impl Into<String>) -> Self {
        Json::Str(s.into())
    }

    /// Builds a number from a count.
    ///
    /// Counts in this crate are file and page tallies, far below the 2^53
    /// where `f64` stops being exact; the conversion is lossless in practice
    /// and saturates rather than wrapping if that ever stops being true.
    pub fn int(n: u64) -> Self {
        Json::Num(f64::from(u32::try_from(n).unwrap_or(u32::MAX)))
    }

    /// Looks a key up in an object, returning `None` for any other variant.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The string payload, if this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The numeric payload, if this is a number.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// The element list, if this is an array.
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }

    /// The boolean payload, if this is a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Renders the value as indented JSON text ending in a newline.
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        write_value(&mut out, self, 0);
        out.push('\n');
        out
    }

    /// Parses JSON text into a value.
    pub fn parse(text: &str) -> Result<Json, JsonError> {
        let bytes = text.as_bytes();
        let mut at = 0usize;
        let value = parse_value(bytes, &mut at)?;
        skip_ws(bytes, &mut at);
        if at != bytes.len() {
            return Err(JsonError::Trailing { at });
        }
        Ok(value)
    }
}

/// Narrows a JSON number to a `u32`, rejecting anything that would not
/// survive the trip (negative, fractional, or out of range).
///
/// Page counts and tallies come back through this rather than an `as` cast so
/// a corrupt scoreboard yields `None`, not a silently wrapped number.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the guard proves the value is a non-negative whole number in range"
)]
pub fn as_u32(value: f64) -> Option<u32> {
    (value.is_finite() && value >= 0.0 && value <= f64::from(u32::MAX) && value.fract() == 0.0)
        .then(|| value.trunc() as u32)
}

/// Narrows a JSON number to a `u8`, on the same terms as `as_u32`.
pub fn as_u8(value: f64) -> Option<u8> {
    as_u32(value).and_then(|n| u8::try_from(n).ok())
}

fn write_value(out: &mut String, value: &Json, depth: usize) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Num(n) => out.push_str(&format_number(*n)),
        Json::Str(s) => write_string(out, s),
        Json::Arr(items) if items.is_empty() => out.push_str("[]"),
        Json::Arr(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                indent(out, depth + 1);
                write_value(out, item, depth + 1);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            indent(out, depth);
            out.push(']');
        }
        Json::Obj(fields) if fields.is_empty() => out.push_str("{}"),
        Json::Obj(fields) => {
            out.push_str("{\n");
            for (i, (key, val)) in fields.iter().enumerate() {
                indent(out, depth + 1);
                write_string(out, key);
                out.push_str(": ");
                write_value(out, val, depth + 1);
                if i + 1 < fields.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            indent(out, depth);
            out.push('}');
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// Integral values print bare (`3`, not `3.0`); everything else gets a fixed
/// six decimals so an SSIM score is stable text across platforms.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the branch guard bounds `n` to a whole number below 1e15"
)]
fn format_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        let mut s = String::new();
        // Guarded by the `fract`/magnitude check above, so this is exact.
        let _ = write!(s, "{}", n.trunc() as i64);
        s
    } else if n.is_finite() {
        let mut s = String::new();
        let _ = write!(s, "{n:.6}");
        s
    } else {
        "null".to_owned()
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn skip_ws(bytes: &[u8], at: &mut usize) {
    while let Some(b) = bytes.get(*at) {
        if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
            *at += 1;
        } else {
            break;
        }
    }
}

fn parse_value(bytes: &[u8], at: &mut usize) -> Result<Json, JsonError> {
    skip_ws(bytes, at);
    let Some(&byte) = bytes.get(*at) else {
        return Err(JsonError::Eof { at: *at });
    };
    match byte {
        b'{' => parse_object(bytes, at),
        b'[' => parse_array(bytes, at),
        b'"' => Ok(Json::Str(parse_string(bytes, at)?)),
        b't' => parse_literal(bytes, at, b"true", Json::Bool(true)),
        b'f' => parse_literal(bytes, at, b"false", Json::Bool(false)),
        b'n' => parse_literal(bytes, at, b"null", Json::Null),
        b'-' | b'0'..=b'9' => parse_number(bytes, at),
        other => Err(JsonError::Unexpected {
            byte: other as char,
            at: *at,
        }),
    }
}

fn parse_literal(
    bytes: &[u8],
    at: &mut usize,
    word: &[u8],
    value: Json,
) -> Result<Json, JsonError> {
    if bytes.get(*at..*at + word.len()) == Some(word) {
        *at += word.len();
        Ok(value)
    } else {
        Err(JsonError::Unexpected {
            byte: bytes.get(*at).copied().unwrap_or(b'?') as char,
            at: *at,
        })
    }
}

fn parse_object(bytes: &[u8], at: &mut usize) -> Result<Json, JsonError> {
    *at += 1; // '{'
    let mut fields = Vec::new();
    skip_ws(bytes, at);
    if bytes.get(*at) == Some(&b'}') {
        *at += 1;
        return Ok(Json::Obj(fields));
    }
    loop {
        skip_ws(bytes, at);
        let key = parse_string(bytes, at)?;
        skip_ws(bytes, at);
        if bytes.get(*at) != Some(&b':') {
            return Err(JsonError::Unexpected {
                byte: bytes.get(*at).copied().unwrap_or(b'?') as char,
                at: *at,
            });
        }
        *at += 1;
        fields.push((key, parse_value(bytes, at)?));
        skip_ws(bytes, at);
        match bytes.get(*at) {
            Some(b',') => *at += 1,
            Some(b'}') => {
                *at += 1;
                return Ok(Json::Obj(fields));
            }
            Some(&other) => {
                return Err(JsonError::Unexpected {
                    byte: other as char,
                    at: *at,
                });
            }
            None => return Err(JsonError::Eof { at: *at }),
        }
    }
}

fn parse_array(bytes: &[u8], at: &mut usize) -> Result<Json, JsonError> {
    *at += 1; // '['
    let mut items = Vec::new();
    skip_ws(bytes, at);
    if bytes.get(*at) == Some(&b']') {
        *at += 1;
        return Ok(Json::Arr(items));
    }
    loop {
        items.push(parse_value(bytes, at)?);
        skip_ws(bytes, at);
        match bytes.get(*at) {
            Some(b',') => *at += 1,
            Some(b']') => {
                *at += 1;
                return Ok(Json::Arr(items));
            }
            Some(&other) => {
                return Err(JsonError::Unexpected {
                    byte: other as char,
                    at: *at,
                });
            }
            None => return Err(JsonError::Eof { at: *at }),
        }
    }
}

fn parse_string(bytes: &[u8], at: &mut usize) -> Result<String, JsonError> {
    if bytes.get(*at) != Some(&b'"') {
        return Err(JsonError::Unexpected {
            byte: bytes.get(*at).copied().unwrap_or(b'?') as char,
            at: *at,
        });
    }
    *at += 1;
    let mut out = String::new();
    loop {
        let Some(&byte) = bytes.get(*at) else {
            return Err(JsonError::Eof { at: *at });
        };
        match byte {
            b'"' => {
                *at += 1;
                return Ok(out);
            }
            b'\\' => {
                *at += 1;
                let Some(&esc) = bytes.get(*at) else {
                    return Err(JsonError::Eof { at: *at });
                };
                *at += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => out.push(parse_unicode_escape(bytes, at)?),
                    _ => return Err(JsonError::BadEscape { at: *at - 1 }),
                }
            }
            _ => {
                // Copy one whole UTF-8 sequence.
                let start = *at;
                let len = utf8_len(byte);
                let Some(chunk) = bytes.get(start..start + len) else {
                    return Err(JsonError::Eof { at: start });
                };
                let text = std::str::from_utf8(chunk).map_err(|_| JsonError::Unexpected {
                    byte: byte as char,
                    at: start,
                })?;
                out.push_str(text);
                *at += len;
            }
        }
    }
}

/// Reads the four hex digits of a `\uXXXX` escape, joining a surrogate pair
/// when one follows. Lone surrogates become U+FFFD rather than an error —
/// scoreboard notes come from PDF corpora and may hold anything.
fn parse_unicode_escape(bytes: &[u8], at: &mut usize) -> Result<char, JsonError> {
    let high = read_hex4(bytes, at)?;
    if (0xD800..0xDC00).contains(&high)
        && bytes.get(*at) == Some(&b'\\')
        && bytes.get(*at + 1) == Some(&b'u')
    {
        let mut probe = *at + 2;
        let low = read_hex4(bytes, &mut probe)?;
        if (0xDC00..0xE000).contains(&low) {
            *at = probe;
            let scalar = 0x1_0000 + ((high - 0xD800) << 10) + (low - 0xDC00);
            return Ok(char::from_u32(scalar).unwrap_or(char::REPLACEMENT_CHARACTER));
        }
    }
    Ok(char::from_u32(high).unwrap_or(char::REPLACEMENT_CHARACTER))
}

fn read_hex4(bytes: &[u8], at: &mut usize) -> Result<u32, JsonError> {
    let Some(digits) = bytes.get(*at..*at + 4) else {
        return Err(JsonError::Eof { at: *at });
    };
    let mut value = 0u32;
    for &digit in digits {
        let nibble = match digit {
            b'0'..=b'9' => u32::from(digit - b'0'),
            b'a'..=b'f' => u32::from(digit - b'a') + 10,
            b'A'..=b'F' => u32::from(digit - b'A') + 10,
            _ => return Err(JsonError::BadEscape { at: *at }),
        };
        value = value * 16 + nibble;
    }
    *at += 4;
    Ok(value)
}

fn utf8_len(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

fn parse_number(bytes: &[u8], at: &mut usize) -> Result<Json, JsonError> {
    let start = *at;
    if bytes.get(*at) == Some(&b'-') {
        *at += 1;
    }
    while matches!(
        bytes.get(*at),
        Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
    ) {
        *at += 1;
    }
    let Some(chunk) = bytes.get(start..*at) else {
        return Err(JsonError::BadNumber { at: start });
    };
    std::str::from_utf8(chunk)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .map(Json::Num)
        .ok_or(JsonError::BadNumber { at: start })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integral_numbers_print_without_a_fraction() {
        assert_eq!(Json::int(42).to_pretty().trim(), "42");
        assert_eq!(Json::Num(0.99).to_pretty().trim(), "0.990000");
    }

    #[test]
    fn object_key_order_is_insertion_order() {
        let value = Json::Obj(vec![
            ("z".to_owned(), Json::int(1)),
            ("a".to_owned(), Json::int(2)),
        ]);
        assert_eq!(value.to_pretty(), "{\n  \"z\": 1,\n  \"a\": 2\n}\n");
    }

    #[test]
    fn round_trips_nested_values() {
        let value = Json::Obj(vec![
            (
                "totals".to_owned(),
                Json::Obj(vec![
                    ("pass".to_owned(), Json::int(3)),
                    ("fail".to_owned(), Json::int(4)),
                ]),
            ),
            (
                "tags".to_owned(),
                Json::Arr(vec![Json::str("unsupported-tool"), Json::str("pixel-fail")]),
            ),
            ("flag".to_owned(), Json::Bool(true)),
            ("nothing".to_owned(), Json::Null),
        ]);
        let text = value.to_pretty();
        assert_eq!(Json::parse(&text).unwrap(), value);
    }

    #[test]
    fn escapes_survive_a_round_trip() {
        let value = Json::str("quote \" back \\ newline \n tab \t \u{1}");
        assert_eq!(Json::parse(&value.to_pretty()).unwrap(), value);
    }

    #[test]
    fn non_ascii_and_surrogate_pairs_parse() {
        assert_eq!(Json::parse(r#""héllo""#).unwrap(), Json::str("héllo"));
        assert_eq!(Json::parse(r#""😀""#).unwrap(), Json::str("😀"));
        assert_eq!(Json::parse(r#""\ud800""#).unwrap(), Json::str("\u{fffd}"));
    }

    #[test]
    fn numeric_narrowing_rejects_what_will_not_fit() {
        assert_eq!(as_u32(3.0), Some(3));
        assert_eq!(as_u32(0.0), Some(0));
        assert_eq!(as_u32(-1.0), None);
        assert_eq!(as_u32(2.5), None);
        assert_eq!(as_u32(f64::from(u32::MAX) + 1.0), None);
        assert_eq!(as_u32(f64::NAN), None);
        assert_eq!(as_u8(255.0), Some(255));
        assert_eq!(as_u8(256.0), None);
    }

    #[test]
    fn empty_containers_stay_compact() {
        assert_eq!(Json::Arr(vec![]).to_pretty().trim(), "[]");
        assert_eq!(Json::Obj(vec![]).to_pretty().trim(), "{}");
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(Json::parse("{").is_err());
        assert!(Json::parse("[1, 2").is_err());
        assert!(Json::parse("tru").is_err());
        assert!(Json::parse("{} extra").is_err());
        assert!(Json::parse("").is_err());
    }
}
