//! `util.printd`.

use crate::error::Error;
use crate::time::{Civil, DAYS, FULL_DAYS, FULL_MONTHS, MONTHS, civil_from_ms};

/// Numeric styles `util.printd(0|1|2, date)` understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintdStyle {
    /// `D:YYYYMMDDHHmmss`
    Pdf,
    /// `YYYY.MM.DD HH:MM:SS`
    Dots,
    /// `YYYY/MM/DD HH:MM:SS`
    Slashes,
}

/// `util.printd` with one of the three numeric styles rather than a pattern.
///
/// # Errors
///
/// [`Error::InvalidDate`] if the time value is not a real instant, and
/// [`Error::Value`] for a year before the common era or a style outside
/// `0..=2`.
pub fn util_printd_style(style: i32, date_ms: f64) -> Result<String, Error> {
    if !date_ms.is_finite() {
        return Err(Error::InvalidDate);
    }
    let c = civil_from_ms(date_ms);
    if c.year < 0 {
        return Err(Error::Value);
    }
    match style {
        0 => Ok(format!(
            "D:{:04}{:02}{:02}{:02}{:02}{:02}",
            c.year, c.month, c.day, c.hour, c.minute, c.second
        )),
        1 => Ok(format!(
            "{:04}.{:02}.{:02} {:02}:{:02}:{:02}",
            c.year, c.month, c.day, c.hour, c.minute, c.second
        )),
        2 => Ok(format!(
            "{:04}/{:02}/{:02} {:02}:{:02}:{:02}",
            c.year, c.month, c.day, c.hour, c.minute, c.second
        )),
        _ => Err(Error::Value),
    }
}

/// `util.printd` with a PDF-style pattern.
///
/// The pattern's own tokens are mapped onto the C library's date directives,
/// then whatever single `m`, `d`, `H`, `h`, `M` or `s` characters are left over
/// are substituted with their values — which is why a literal `d` in a pattern
/// silently becomes a day number.
///
/// A `%` already in the pattern is stripped before any of that, so a caller
/// cannot smuggle a directive of its own through.
///
/// # Errors
///
/// [`Error::InvalidDate`] if the time value is not a real instant, and
/// [`Error::Value`] for a year before the common era.
pub fn util_printd(pattern: &str, date_ms: f64) -> Result<String, Error> {
    if !date_ms.is_finite() {
        return Err(Error::InvalidDate);
    }
    let c = civil_from_ms(date_ms);
    if c.year < 0 {
        return Err(Error::Value);
    }
    Ok(printd_pattern(pattern, c))
}

const PRINTD_TOKENS: &[(&str, &str)] = &[
    ("mmmm", "\u{0001}B"), // stand-in for %B (sentinel so leftover m isn't eaten)
    ("mmm", "\u{0001}b"),
    ("mm", "\u{0001}m"),
    ("dddd", "\u{0001}A"),
    ("ddd", "\u{0001}a"),
    ("dd", "\u{0001}d"),
    ("yyyy", "\u{0001}Y"),
    ("yy", "\u{0001}y"),
    ("HH", "\u{0001}H"),
    ("hh", "\u{0001}I"),
    ("MM", "\u{0001}M"),
    ("ss", "\u{0001}S"),
    ("TT", "\u{0001}p"),
    ("tt", "\u{0001}P"),
    ("h", "\u{0001}l"),
];

fn printd_pattern(pattern: &str, c: Civil) -> String {
    // Strip pre-existing '%' like the C++.
    let stripped: String = pattern.chars().filter(|ch| *ch != '%').collect();
    let mut s = stripped;
    for (js, cpp) in PRINTD_TOKENS {
        s = s.replace(js, cpp);
    }

    // Leftover m/d/H/h/M/s not preceded by the sentinel, matching table_additional.
    let hour_12 = if c.hour > 12 { c.hour - 12 } else { c.hour };
    let additional: &[(char, i32)] = &[
        ('m', c.month),
        ('d', c.day),
        ('H', c.hour),
        ('h', hour_12),
        ('M', c.minute),
        ('s', c.second),
    ];
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let Some(ch) = chars.get(i).copied() else {
            break;
        };
        if ch != '\u{0001}' {
            let mut replaced = false;
            for &(js, val) in additional {
                if ch == js {
                    out.push_str(&val.to_string());
                    replaced = true;
                    break;
                }
            }
            if !replaced {
                out.push(ch);
            }
            i = i.saturating_add(1);
            continue;
        }
        let next = chars.get(i.saturating_add(1)).copied();
        i = i.saturating_add(2);
        out.push_str(&expand_strftime(next, c));
    }
    out
}

fn expand_strftime(dir: Option<char>, c: Civil) -> String {
    match dir {
        Some('B') => month_name(&FULL_MONTHS, c.month),
        Some('b') => month_name(&MONTHS, c.month),
        Some('m') => format!("{:02}", c.month),
        Some('A') => FULL_DAYS
            .get(usize::try_from(c.weekday).unwrap_or(0))
            .unwrap_or(&"")
            .to_string(),
        Some('a') => DAYS
            .get(usize::try_from(c.weekday).unwrap_or(0))
            .unwrap_or(&"")
            .to_string(),
        Some('d') => format!("{:02}", c.day),
        Some('Y') => format!("{:04}", c.year),
        Some('y') => format!("{:02}", c.year.rem_euclid(100)),
        Some('H') => format!("{:02}", c.hour),
        Some('I') => format!("{:02}", hour_on_twelve_hour_clock(c.hour)),
        Some('l') => format!("{:2}", hour_on_twelve_hour_clock(c.hour)),
        Some('M') => format!("{:02}", c.minute),
        Some('S') => format!("{:02}", c.second),
        Some('p') => {
            if c.hour >= 12 {
                "PM".to_string()
            } else {
                "AM".to_string()
            }
        }
        Some('P') => {
            if c.hour >= 12 {
                "pm".to_string()
            } else {
                "am".to_string()
            }
        }
        Some(other) => {
            let mut s = String::from("%");
            s.push(other);
            s
        }
        None => "%".to_string(),
    }
}

/// The name of a one-based month, or nothing when the month is out of range —
/// which a time value with no civil date produces.
fn month_name(table: &[&str; 12], month: i32) -> String {
    usize::try_from(month - 1)
        .ok()
        .and_then(|i| table.get(i))
        .unwrap_or(&"")
        .to_string()
}

/// The hour on a twelve-hour clock: midnight and noon are both `12`.
fn hour_on_twelve_hour_clock(hour: i32) -> i32 {
    1 + (hour + 11).rem_euclid(12)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::ms_from_civil;

    #[test]
    fn printd_numeric_styles() {
        let t = ms_from_civil(2014, 7, 4, 15, 59, 58);
        assert_eq!(util_printd_style(0, t).unwrap(), "D:20140704155958");
        assert_eq!(util_printd_style(1, t).unwrap(), "2014.07.04 15:59:58");
        assert_eq!(util_printd_style(2, t).unwrap(), "2014/07/04 15:59:58");
        assert!(util_printd_style(3, t).is_err());
    }

    /// The tokens a pattern can carry, over 4 July 2014 — a Friday.
    #[test]
    fn printd_tokens() {
        let t = ms_from_civil(2014, 7, 4, 15, 59, 58);
        assert_eq!(
            util_printd("mm/dd/yyyy HH:MM:ss", t).unwrap(),
            "07/04/2014 15:59:58"
        );
        assert_eq!(util_printd("mmmm", t).unwrap(), "July");
        assert_eq!(util_printd("mmm", t).unwrap(), "Jul");
        assert_eq!(util_printd("yyyy", t).unwrap(), "2014");
        assert_eq!(util_printd("yy", t).unwrap(), "14");
        assert_eq!(util_printd("hh", t).unwrap(), "03");
        assert_eq!(util_printd("tt", t).unwrap(), "pm");
        // A lone `t` is not a token, so it prints as itself.
        assert_eq!(util_printd("t", t).unwrap(), "t");
    }

    /// The weekday follows the date. The oracle leaves its `tm_wday` at zero
    /// and so prints Sunday for every date; see the note on
    /// [`weekday_from_time`](crate::time::weekday_from_time). pdf.js reads
    /// `oDate.getDay()`.
    #[test]
    fn the_weekday_follows_the_date() {
        // 4 July 2014 was a Friday.
        let friday = ms_from_civil(2014, 7, 4, 15, 59, 58);
        assert_eq!(util_printd("dddd", friday).unwrap(), "Friday");
        assert_eq!(util_printd("ddd", friday).unwrap(), "Fri");

        // Seven consecutive days name seven different weekdays.
        let names: Vec<String> = (0..7)
            .map(|offset| {
                util_printd("dddd", ms_from_civil(2014, 7, 4 + offset, 0, 0, 0)).unwrap_or_default()
            })
            .collect();
        assert_eq!(
            names,
            [
                "Friday",
                "Saturday",
                "Sunday",
                "Monday",
                "Tuesday",
                "Wednesday",
                "Thursday"
            ]
        );
    }

    /// Midnight and noon both read `12` on a twelve-hour clock, and noon is pm.
    #[test]
    fn printd_twelve_hour_clock() {
        let at = |hour| ms_from_civil(2014, 7, 4, hour, 0, 0);
        assert_eq!(util_printd("hh tt", at(0)).unwrap(), "12 am");
        assert_eq!(util_printd("hh tt", at(11)).unwrap(), "11 am");
        assert_eq!(util_printd("hh tt", at(12)).unwrap(), "12 pm");
        assert_eq!(util_printd("hh tt", at(13)).unwrap(), "01 pm");
        assert_eq!(util_printd("hh tt", at(23)).unwrap(), "11 pm");
    }

    #[test]
    fn printd_rejects_nan_and_negative_year() {
        assert_eq!(util_printd("mm", f64::NAN).unwrap_err(), Error::InvalidDate);
        let t = ms_from_civil(-1, 7, 4, 15, 59, 58);
        assert_eq!(util_printd("mm", t).unwrap_err(), Error::Value);
    }
}
