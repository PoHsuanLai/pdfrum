//! `util.printx`: formatting a string through a mask.

use crate::parse::{is_ascii_alnum, is_ascii_alpha, is_decimal_digit};

#[derive(Clone, Copy)]
enum CaseMode {
    Preserve,
    Upper,
    Lower,
}

fn translate_case(input: char, mode: CaseMode) -> char {
    // cjs_util.cpp TranslateCase: ASCII only, via `| 0x20` / `& ~0x20`.
    match mode {
        CaseMode::Lower if input.is_ascii_uppercase() => {
            char::from(u8::try_from(input as u32).unwrap_or(0) | 0x20)
        }
        CaseMode::Upper if input.is_ascii_lowercase() => {
            char::from(u8::try_from(input as u32).unwrap_or(0) & !0x20)
        }
        _ => input,
    }
}

/// Format `source` through `format`'s mask, as `util.printx` does.
///
/// This is the mask language the special-format field actions use.
///
/// Directives: `?` any, `X` alnum, `A` alpha, `9` digit, `*` rest of source,
/// `\` escape, `>`/`<`/`=` case.
#[must_use]
pub fn util_printx(format: &str, source: &str) -> String {
    let fmt: Vec<char> = format.chars().collect();
    let src: Vec<char> = source.chars().collect();
    let mut result = String::new();
    let mut i_source = 0;
    let mut i_format = 0;
    let mut case_mode = CaseMode::Preserve;
    let mut escaped = false;
    while i_format < fmt.len() {
        if escaped {
            escaped = false;
            if let Some(c) = fmt.get(i_format).copied() {
                result.push(c);
            }
            i_format = i_format.saturating_add(1);
            continue;
        }
        let Some(ch) = fmt.get(i_format).copied() else {
            break;
        };
        match ch {
            '\\' => {
                escaped = true;
                i_format = i_format.saturating_add(1);
            }
            '<' => {
                case_mode = CaseMode::Lower;
                i_format = i_format.saturating_add(1);
            }
            '>' => {
                case_mode = CaseMode::Upper;
                i_format = i_format.saturating_add(1);
            }
            '=' => {
                case_mode = CaseMode::Preserve;
                i_format = i_format.saturating_add(1);
            }
            '?' => {
                if i_source < src.len()
                    && let Some(s) = src.get(i_source).copied()
                {
                    result.push(translate_case(s, case_mode));
                    i_source = i_source.saturating_add(1);
                }
                i_format = i_format.saturating_add(1);
            }
            'X' => {
                if i_source < src.len() {
                    if let Some(s) = src.get(i_source).copied()
                        && is_ascii_alnum(s)
                    {
                        result.push(translate_case(s, case_mode));
                        i_format = i_format.saturating_add(1);
                    }
                    i_source = i_source.saturating_add(1);
                } else {
                    i_format = i_format.saturating_add(1);
                }
            }
            'A' => {
                if i_source < src.len() {
                    if let Some(s) = src.get(i_source).copied()
                        && is_ascii_alpha(s)
                    {
                        result.push(translate_case(s, case_mode));
                        i_format = i_format.saturating_add(1);
                    }
                    i_source = i_source.saturating_add(1);
                } else {
                    i_format = i_format.saturating_add(1);
                }
            }
            '9' => {
                if i_source < src.len() {
                    if let Some(s) = src.get(i_source).copied()
                        && is_decimal_digit(s)
                    {
                        result.push(s);
                        i_format = i_format.saturating_add(1);
                    }
                    i_source = i_source.saturating_add(1);
                } else {
                    i_format = i_format.saturating_add(1);
                }
            }
            '*' => {
                if i_source < src.len() {
                    if let Some(s) = src.get(i_source).copied() {
                        result.push(translate_case(s, case_mode));
                    }
                    i_source = i_source.saturating_add(1);
                } else {
                    i_format = i_format.saturating_add(1);
                }
            }
            _ => {
                result.push(ch);
                i_format = i_format.saturating_add(1);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printx_matches_util_printx_fixture() {
        // testing/resources/javascript/util_printx.in + util_printx_expected.txt
        let src = "-1Afp3.d33F$";
        let cases: &[(&str, &str, &str)] = &[
            ("", "", ""),
            ("", "123", ""),
            ("??", "", ""),
            ("??", "f2", "f2"),
            ("??", "f27", "f2"),
            ("XXX", "", ""),
            ("XXX", "1afp3.", "1af"),
            ("XXX", src, "1Af"),
            ("AAA", "", ""),
            ("AAA", "-1Afp3.", "Afp"),
            ("AAA", src, "Afp"),
            ("999", "", ""),
            ("999", "-1Afp3.", "13"),
            ("999", src, "133"),
            ("9*9", "", ""),
            ("9*9", "-1Afp3.", "1Afp3."),
            ("[*]X", "-1Afp3.", "[-1Afp3.]"),
            ("<*", src, "-1afp3.d33f$"),
            (">*", src, "-1AFP3.D33F$"),
            ("<[AAAAAAAAAAA]", src, "[afpdf]"),
            (">[AAAAAAAAAAA]", src, "[AFPDF]"),
            ("<[XXXXXXXXXXX]", src, "[1afp3d33f]"),
            (">[XXXXXXXXXXX]", src, "[1AFP3D33F]"),
            (">[???????????]", src, "[-1AFP3.D33F]"),
            ("<[???????????]", src, "[-1afp3.d33f]"),
            ("\\>[\\**]", src, ">[*-1Afp3.d33F$]"),
            ("\\>[\\\\**]", src, ">[\\-1Afp3.d33F$]"),
            ("=*", src, "-1Afp3.d33F$"),
            ("<??????=*", src, "-1afp3.d33F$"),
            (">??????=*", src, "-1AFP3.d33F$"),
            (">??????<*", src, "-1AFP3.d33f$"),
            ("clams", src, "clams"),
            ("cl9ms", src, "cl1ms"),
            ("cl\\9ms", src, "cl9ms"),
        ];
        for &(fmt, source, expected) in cases {
            assert_eq!(
                util_printx(fmt, source),
                expected,
                "printx({fmt:?}, {source:?})"
            );
        }
    }
}
