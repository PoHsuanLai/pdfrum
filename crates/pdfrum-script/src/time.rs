//! Date and time arithmetic.
//!
//! The time value is milliseconds from 1970-01-01 00:00:00, the same number
//! ECMAScript `Date` uses. There is no machine timezone: callers who want
//! local time add their offset themselves.

use crate::parse::is_decimal_digit;

/// Abbreviated month names (`fxjs::kMonths`).
pub const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Abbreviated weekday names, indexed from Sunday.
pub const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Full weekday names, indexed from Sunday.
pub const FULL_DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Full month names (`fxjs::kFullMonths`).
pub const FULL_MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const DAYS_MONTH: [u16; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
const LEAP_DAYS_MONTH: [u16; 12] = [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335];
const CUMULATIVE_DAYS_IN_MONTHS: [i32; 11] = [59, 90, 120, 151, 181, 212, 243, 273, 304, 334, 365];

const MS_PER_DAY: f64 = 86_400_000.0;

/// Result of [`parse_date_using_format`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionStatus {
    /// A time value was produced.
    Success,
    /// The format did not consume the value (C++ `kBadFormat`).
    BadFormat,
    /// Components were out of range (C++ `kBadDate`).
    BadDate,
}

fn posix_mod(x: f64, y: f64) -> f64 {
    let r = x % y;
    if r < 0.0 { r + y } else { r }
}

/// Narrow a time-derived double to an integer, discarding its fraction and
/// clamping rather than wrapping at the extremes.
///
/// A time value is unbounded — a malformed date can produce one far outside any
/// calendar — so every narrowing in this module goes through here rather than
/// trusting the value to fit.
fn trunc_i32(x: f64) -> i32 {
    if x.is_nan() {
        return 0;
    }
    // `f64` represents every `i32` exactly, so these bounds are not themselves
    // approximations, and the truncation below cannot overflow once they hold.
    let clamped = x.clamp(f64::from(i32::MIN), f64::from(i32::MAX));
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clamped into i32's range on the line above"
    )]
    {
        clamped as i32
    }
}

/// The Gregorian leap-year rule: every fourth year, except centuries, except
/// every fourth century. 2000 is a leap year; 1900 is not.
///
/// This is also the rule the surrounding day arithmetic already assumes —
/// `day_from_year` counts leap days with the correct `/4 − /100 + /400`
/// cadence — so getting it wrong here would make the two disagree with each
/// other for a century year.
//
// [oracle-bug] fx_date_helpers.cpp:71 writes the test as
// `(year % 4 == 0) && ((year % 100 != 0) || (year % 400 != 0))`. That last
// clause should read `year % 400 == 0`; as written it is true for every year
// *not* divisible by 400, so the century exception never fires and its own
// exception fires backwards: 2000 comes out common and 1900 comes out leap.
// The only leap year the oracle's unit test checks is 1972, which both
// spellings agree on, which is why it has survived. pdf.js delegates to the
// JavaScript `Date` object (util.js `_scand`) and so gets this right.
// docs/status/M15.md lists the golden assertions this puts out of reach.
#[must_use]
pub fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn day_from_year(y: i32) -> i32 {
    let y = f64::from(y);
    trunc_i32(
        365.0 * (y - 1970.0) + ((y - 1969.0) / 4.0).floor() - ((y - 1901.0) / 100.0).floor()
            + ((y - 1601.0) / 400.0).floor(),
    )
}

fn time_from_year(y: i32) -> f64 {
    MS_PER_DAY * f64::from(day_from_year(y))
}

fn time_from_year_month(y: i32, m: i32) -> f64 {
    let table = if is_leap_year(y) {
        &LEAP_DAYS_MONTH
    } else {
        &DAYS_MONTH
    };
    let days_before = usize::try_from(m)
        .ok()
        .and_then(|i| table.get(i))
        .copied()
        .unwrap_or(0);
    time_from_year(y) + f64::from(days_before) * MS_PER_DAY
}

fn day(t: f64) -> i32 {
    trunc_i32((t / MS_PER_DAY).floor())
}

/// `FX_GetYearFromTime`.
#[must_use]
pub fn year_from_time(t: f64) -> i32 {
    let mut y = 1970 + trunc_i32(t / (365.2425 * MS_PER_DAY));
    if time_from_year(y) <= t {
        while time_from_year(y.saturating_add(1)) <= t {
            y = y.saturating_add(1);
        }
    } else {
        while time_from_year(y) > t {
            y = y.saturating_sub(1);
        }
    }
    y
}

fn day_within_year(t: f64) -> i32 {
    day(t) - day_from_year(year_from_time(t))
}

/// `FX_GetMonthFromTime` — zero-based, or -1 if the day-of-year is negative.
#[must_use]
pub fn month_from_time(t: f64) -> i32 {
    let mut d = day_within_year(t);
    if d < 0 {
        return -1;
    }
    if d < 31 {
        return 0;
    }
    if is_leap_year(year_from_time(t)) {
        d -= 1;
    }
    for (i, bound) in CUMULATIVE_DAYS_IN_MONTHS.iter().enumerate() {
        if d < *bound {
            return i32::try_from(i).unwrap_or(0).saturating_add(1);
        }
    }
    -1
}

/// `FX_GetDayFromTime` — day of month, 1-based.
#[must_use]
pub fn day_from_time(t: f64) -> i32 {
    let d = day_within_year(t);
    let year = year_from_time(t);
    let leap = i32::from(is_leap_year(year));
    match month_from_time(t) {
        0 => d + 1,
        1 => d - 30,
        2 => d - 58 - leap,
        3 => d - 89 - leap,
        4 => d - 119 - leap,
        5 => d - 150 - leap,
        6 => d - 180 - leap,
        7 => d - 211 - leap,
        8 => d - 242 - leap,
        9 => d - 272 - leap,
        10 => d - 303 - leap,
        11 => d - 333 - leap,
        _ => 0,
    }
}

/// The day of the week, `0` for Sunday through `6` for Saturday.
///
/// The epoch fell on a Thursday, which is where the offset of four comes from.
//
// [oracle-bug] cjs_util.cpp:267 builds a `struct tm time = {}` and fills in
// year, month, day, hour, minute and second — but never `tm_wday`, which the
// zero-initialisation leaves at 0. `wcsftime`'s `%A` and `%a` read that field
// directly rather than deriving it, so every date `util.printd` formats with
// `dddd` or `ddd` comes out "Sunday" or "Sun" whatever day it actually fell
// on. pdf.js reads `oDate.getDay()` (util.js `printd`) and prints the real
// weekday. docs/status/M15.md lists the affected goldens.
#[must_use]
pub fn weekday_from_time(dt: f64) -> i32 {
    /// 1970-01-01 was a Thursday, four days after Sunday.
    const EPOCH_WEEKDAY: i32 = 4;
    (day(dt).rem_euclid(7) + EPOCH_WEEKDAY).rem_euclid(7)
}

/// `FX_GetHourFromTime`.
#[must_use]
pub fn hour_from_time(dt: f64) -> i32 {
    trunc_i32(posix_mod((dt / 3_600_000.0).floor(), 24.0))
}

/// `FX_GetMinFromTime`.
#[must_use]
pub fn min_from_time(dt: f64) -> i32 {
    trunc_i32(posix_mod((dt / 60_000.0).floor(), 60.0))
}

/// `FX_GetSecFromTime`.
#[must_use]
pub fn sec_from_time(dt: f64) -> i32 {
    trunc_i32(posix_mod((dt / 1000.0).floor(), 60.0))
}

/// `FX_IsValidMonth` — 1..=12.
#[must_use]
pub fn is_valid_month(m: i32) -> bool {
    (1..=12).contains(&m)
}

/// `FX_IsValidDay` — 1..=31, **without** consulting the month.
#[must_use]
pub fn is_valid_day(d: i32) -> bool {
    (1..=31).contains(&d)
}

/// `FX_IsValid24Hour` — 0..=24 (24 is allowed).
#[must_use]
pub fn is_valid_24_hour(h: i32) -> bool {
    (0..=24).contains(&h)
}

/// `FX_IsValidMinute` — 0..=60 (60 is allowed).
#[must_use]
pub fn is_valid_minute(m: i32) -> bool {
    (0..=60).contains(&m)
}

/// `FX_IsValidSecond` — 0..=60 (60 is allowed).
#[must_use]
pub fn is_valid_second(s: i32) -> bool {
    (0..=60).contains(&s)
}

/// `FX_MakeDay`. `n_month` is zero-based and may overflow into neighbouring years.
#[must_use]
#[allow(clippy::float_cmp)] // C++ compares the reconstructed year/month exactly.
pub fn make_day(n_year: i32, n_month: i32, n_date: i32) -> f64 {
    let y = f64::from(n_year);
    let m = f64::from(n_month);
    let dt = f64::from(n_date);
    let ym = y + (m / 12.0).floor();
    let mn = posix_mod(m, 12.0);
    let t = time_from_year_month(trunc_i32(ym), trunc_i32(mn));
    if f64::from(year_from_time(t)) != ym
        || f64::from(month_from_time(t)) != mn
        || day_from_time(t) != 1
    {
        return f64::NAN;
    }
    f64::from(day(t)) + dt - 1.0
}

/// `FX_MakeTime`.
#[must_use]
pub fn make_time(n_hour: i32, n_min: i32, n_sec: i32, n_ms: i32) -> f64 {
    f64::from(n_hour) * 3_600_000.0
        + f64::from(n_min) * 60_000.0
        + f64::from(n_sec) * 1000.0
        + f64::from(n_ms)
}

/// `FX_MakeDate`.
#[must_use]
pub fn make_date(day: f64, time: f64) -> f64 {
    if !day.is_finite() || !time.is_finite() {
        return f64::NAN;
    }
    day * MS_PER_DAY + time
}

/// Civil date assembled from a time value. Month is 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    /// Year (may be < 1970).
    pub year: i32,
    /// Month 1..=12 (0 when the time value is not a valid civil date).
    pub month: i32,
    /// Day of month.
    pub day: i32,
    /// Hour 0..=23.
    pub hour: i32,
    /// Minute 0..=59.
    pub minute: i32,
    /// Second 0..=59.
    pub second: i32,
    /// Day of the week, `0` for Sunday through `6` for Saturday.
    pub weekday: i32,
}

/// Split a time value into civil components using the C++ extractors.
#[must_use]
pub fn civil_from_ms(dt: f64) -> Civil {
    Civil {
        year: year_from_time(dt),
        month: month_from_time(dt) + 1,
        day: day_from_time(dt),
        hour: hour_from_time(dt),
        minute: min_from_time(dt),
        second: sec_from_time(dt),
        weekday: weekday_from_time(dt),
    }
}

/// Build a time value from 1-based month civil components.
#[must_use]
pub fn ms_from_civil(year: i32, month: i32, day: i32, hour: i32, min: i32, sec: i32) -> f64 {
    make_date(
        make_day(year, month.saturating_sub(1), day),
        make_time(hour, min, sec, 0),
    )
}

/// `FX_ParseStringInteger`: up to `max_step` digits starting at `start` (char index).
///
/// Returns `(value, digits_consumed)`. Overflow wraps like two's-complement `int`.
#[must_use]
pub fn parse_string_integer(s: &str, start: usize, max_step: usize) -> (i32, usize) {
    let chars: Vec<char> = s.chars().collect();
    let mut n_ret: i32 = 0;
    let mut n_skip: usize = 0;
    let mut i = start;
    while i < chars.len() {
        if i.saturating_sub(start) > 10 {
            break;
        }
        let Some(c) = chars.get(i).copied() else {
            break;
        };
        if !is_decimal_digit(c) {
            break;
        }
        let digit = i32::from((c as u8).saturating_sub(b'0'));
        n_ret = n_ret.wrapping_mul(10).wrapping_add(digit);
        n_skip = n_skip.saturating_add(1);
        if n_skip >= max_step {
            break;
        }
        i = i.saturating_add(1);
    }
    (n_ret, n_skip)
}

fn find_subword_length(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() {
        match chars.get(i).copied() {
            Some(c) if c.is_alphanumeric() => i = i.saturating_add(1),
            _ => break,
        }
    }
    i.saturating_sub(start)
}

fn eq_ascii_nocase(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn chars_slice_to_string(chars: &[char], start: usize, len: usize) -> String {
    chars.iter().skip(start).take(len).collect()
}

/// `FX_ParseDateUsingFormat`. Empty value or format yields `now_ms` as success.
#[must_use]
pub fn parse_date_using_format(value: &str, format: &str, now_ms: f64) -> (ConversionStatus, f64) {
    if format.is_empty() || value.is_empty() {
        return (ConversionStatus::Success, now_ms);
    }

    let fmt: Vec<char> = format.chars().collect();
    let val: Vec<char> = value.chars().collect();
    let value_str = value;

    let now = civil_from_ms(now_ms);
    let mut n_year = now.year;
    let mut n_month = now.month;
    let mut n_day = now.day;
    let mut n_hour = now.hour;
    let mut n_min = now.minute;
    let mut n_sec = now.second;
    let mut b_pm = false;
    let mut b_exit = false;
    let mut b_bad_format = false;
    let mut i: usize = 0;
    let mut j: usize = 0;

    while i < fmt.len() {
        if b_exit {
            break;
        }
        let Some(c) = fmt.get(i).copied() else {
            break;
        };
        match c {
            ':' | '.' | '-' | '\\' | '/' => {
                i = i.saturating_add(1);
                j = j.saturating_add(1);
            }
            'y' | 'm' | 'd' | 'H' | 'h' | 'M' | 's' | 't' => {
                let old_j = j;
                let remaining = fmt.len().saturating_sub(i).saturating_sub(1);
                let next1 = fmt.get(i.saturating_add(1)).copied();
                let next2 = fmt.get(i.saturating_add(2)).copied();
                let next3 = fmt.get(i.saturating_add(3)).copied();
                let next4 = fmt.get(i.saturating_add(4)).copied();

                if remaining == 0 || next1 != Some(c) {
                    match c {
                        'y' => {
                            i = i.saturating_add(1);
                            j = j.saturating_add(1);
                        }
                        'm' => {
                            let (n, skip) = parse_string_integer(value_str, j, 2);
                            n_month = n;
                            i = i.saturating_add(1);
                            j = j.saturating_add(skip);
                        }
                        'd' => {
                            let (n, skip) = parse_string_integer(value_str, j, 2);
                            n_day = n;
                            i = i.saturating_add(1);
                            j = j.saturating_add(skip);
                        }
                        'H' | 'h' => {
                            let (n, skip) = parse_string_integer(value_str, j, 2);
                            n_hour = n;
                            i = i.saturating_add(1);
                            j = j.saturating_add(skip);
                        }
                        'M' => {
                            let (n, skip) = parse_string_integer(value_str, j, 2);
                            n_min = n;
                            i = i.saturating_add(1);
                            j = j.saturating_add(skip);
                        }
                        's' => {
                            let (n, skip) = parse_string_integer(value_str, j, 2);
                            n_sec = n;
                            i = i.saturating_add(1);
                            j = j.saturating_add(skip);
                        }
                        't' => {
                            b_pm = j < val.len() && val.get(j).copied() == Some('p');
                            i = i.saturating_add(1);
                            j = j.saturating_add(1);
                        }
                        _ => {}
                    }
                } else if remaining == 1 || next2 != Some(c) {
                    match c {
                        'y' => {
                            let (n, skip) = parse_string_integer(value_str, j, 2);
                            n_year = n;
                            i = i.saturating_add(2);
                            j = j.saturating_add(skip);
                        }
                        'm' => {
                            let (n, skip) = parse_string_integer(value_str, j, 2);
                            n_month = n;
                            i = i.saturating_add(2);
                            j = j.saturating_add(skip);
                        }
                        'd' => {
                            let (n, skip) = parse_string_integer(value_str, j, 2);
                            n_day = n;
                            i = i.saturating_add(2);
                            j = j.saturating_add(skip);
                        }
                        'H' | 'h' => {
                            let (n, skip) = parse_string_integer(value_str, j, 2);
                            n_hour = n;
                            i = i.saturating_add(2);
                            j = j.saturating_add(skip);
                        }
                        'M' => {
                            let (n, skip) = parse_string_integer(value_str, j, 2);
                            n_min = n;
                            i = i.saturating_add(2);
                            j = j.saturating_add(skip);
                        }
                        's' => {
                            let (n, skip) = parse_string_integer(value_str, j, 2);
                            n_sec = n;
                            i = i.saturating_add(2);
                            j = j.saturating_add(skip);
                        }
                        't' => {
                            b_pm = j + 1 < val.len()
                                && val.get(j).copied() == Some('p')
                                && val.get(j + 1).copied() == Some('m');
                            i = i.saturating_add(2);
                            j = j.saturating_add(2);
                        }
                        _ => {}
                    }
                } else if remaining == 2 || next3 != Some(c) {
                    match c {
                        'm' => {
                            let mut found = false;
                            let skip = find_subword_length(&val, j);
                            if skip == 3 {
                                let s_month = chars_slice_to_string(&val, j, 3);
                                for (m, name) in MONTHS.iter().enumerate() {
                                    if eq_ascii_nocase(&s_month, name) {
                                        n_month = i32::try_from(m).unwrap_or(0) + 1;
                                        i = i.saturating_add(3);
                                        j = j.saturating_add(skip);
                                        found = true;
                                        break;
                                    }
                                }
                            }
                            if !found {
                                let (n, skip) = parse_string_integer(value_str, j, 3);
                                n_month = n;
                                i = i.saturating_add(3);
                                j = j.saturating_add(skip);
                            }
                        }
                        'y' => {}
                        _ => {
                            i = i.saturating_add(3);
                            j = j.saturating_add(3);
                        }
                    }
                } else if remaining == 3 || next4 != Some(c) {
                    match c {
                        'y' => {
                            let (n, skip) = parse_string_integer(value_str, j, 4);
                            n_year = n;
                            j = j.saturating_add(skip);
                            i = i.saturating_add(4);
                        }
                        'm' => {
                            let mut found = false;
                            let skip = find_subword_length(&val, j);
                            if skip <= 9 {
                                let s_month = chars_slice_to_string(&val, j, skip).to_lowercase();
                                for (m, name) in FULL_MONTHS.iter().enumerate() {
                                    if name.to_lowercase().contains(&s_month) {
                                        n_month = i32::try_from(m).unwrap_or(0) + 1;
                                        i = i.saturating_add(4);
                                        j = j.saturating_add(skip);
                                        found = true;
                                        break;
                                    }
                                }
                            }
                            if !found {
                                let (n, skip) = parse_string_integer(value_str, j, 4);
                                n_month = n;
                                i = i.saturating_add(4);
                                j = j.saturating_add(skip);
                            }
                        }
                        _ => {
                            i = i.saturating_add(4);
                            j = j.saturating_add(4);
                        }
                    }
                } else {
                    if j >= val.len() || fmt.get(i).copied() != val.get(j).copied() {
                        b_bad_format = true;
                        b_exit = true;
                    }
                    i = i.saturating_add(1);
                    j = j.saturating_add(1);
                }

                if old_j == j {
                    b_bad_format = true;
                    b_exit = true;
                }
            }
            _ => {
                if val.len() <= j {
                    b_exit = true;
                } else if fmt.get(i).copied() != val.get(j).copied() {
                    b_bad_format = true;
                    b_exit = true;
                }
                i = i.saturating_add(1);
                j = j.saturating_add(1);
            }
        }
    }

    if b_bad_format {
        return (ConversionStatus::BadFormat, f64::NAN);
    }
    if b_pm {
        n_hour += 12;
    }
    n_year = expand_two_digit_year(n_year);
    if !is_valid_month(n_month)
        || !is_valid_day(n_day)
        || !is_valid_24_hour(n_hour)
        || !is_valid_minute(n_min)
        || !is_valid_second(n_sec)
    {
        return (ConversionStatus::BadDate, f64::NAN);
    }
    let dt = ms_from_civil(n_year, n_month, n_day, n_hour, n_min, n_sec);
    if dt.is_nan() {
        return (ConversionStatus::BadDate, f64::NAN);
    }
    (ConversionStatus::Success, dt)
}

/// A two-digit year, expanded around the pivot Acrobat documents: below fifty
/// is this century, fifty and above is the last one. So `05` is 2005 and `85`
/// is 1985.
///
/// A year already written in full is left alone.
//
// [oracle-bug] fx_date_helpers.cpp:330 declares `int nYearSub = 99;` with the
// comment `// nYear - 2000;` — the intended formula, left unwritten — and
// :556 then maps every year in 0..=99 to 2000..=2099 unconditionally. A date
// typed `31/12/85` therefore lands in 2085 rather than 1985, which no user
// entering a birth date expects. pdf.js applies the documented pivot at
// util.js:372-376 (`n < 50 → +2000`, else `n < 100 → +1900`), matching
// Acrobat. docs/status/M15.md lists what this puts out of reach.
#[must_use]
pub fn expand_two_digit_year(year: i32) -> i32 {
    /// Years below this belong to the current century.
    const PIVOT: i32 = 50;
    match year {
        0..PIVOT => year + 2000,
        PIVOT..100 => year + 1900,
        _ => year,
    }
}

/// The hour on a twelve-hour clock: midnight and noon are both `12`, and every
/// other hour is its remainder.
///
// [oracle-bug] cjs_publicmethods.cpp:519 and :528 compute this as
// `nHour > 12 ? nHour - 12 : nHour`, which leaves midnight printing as `0` and
// noon printing as `12` — the latter correct only by accident, since :526 and
// :535 then decide the meridiem with the same `nHour > 12` and so label noon
// `am`. pdf.js uses `1 + ((hours + 11) % 12)` with `hours < 12 ? "am" : "pm"`
// (util.js:240-247), which is the ordinary convention and what Acrobat
// documents. docs/status/M15.md lists the affected goldens.
fn hour_on_twelve_hour_clock(hour: i32) -> i32 {
    1 + (hour + 11).rem_euclid(12)
}

/// The name of a one-based month, or nothing when the month is out of range.
fn month_name(table: &[&str; 12], month: i32) -> String {
    usize::try_from(month - 1)
        .ok()
        .and_then(|i| table.get(i))
        .unwrap_or(&"")
        .to_string()
}

/// Whether `hour` falls in the afternoon half of the day. Noon is `pm`.
fn is_afternoon(hour: i32) -> bool {
    hour >= 12
}

/// `CJS_PublicMethods::PrintDateUsingFormat`.
#[must_use]
pub fn print_date_using_format(date_ms: f64, format: &str) -> String {
    let c = civil_from_ms(date_ms);
    let fmt: Vec<char> = format.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < fmt.len() {
        let Some(ch) = fmt.get(i).copied() else {
            break;
        };
        let remaining = fmt.len().saturating_sub(i).saturating_sub(1);
        let next1 = fmt.get(i.saturating_add(1)).copied();
        let next2 = fmt.get(i.saturating_add(2)).copied();
        let next3 = fmt.get(i.saturating_add(3)).copied();
        let next4 = fmt.get(i.saturating_add(4)).copied();
        let part = match ch {
            'y' | 'm' | 'd' | 'H' | 'h' | 'M' | 's' | 't' => {
                if remaining == 0 || next1 != Some(ch) {
                    i = i.saturating_add(1);
                    match ch {
                        'y' => ch.to_string(),
                        'm' => format!("{}", c.month),
                        'd' => format!("{}", c.day),
                        'H' => format!("{}", c.hour),
                        'h' => format!("{}", hour_on_twelve_hour_clock(c.hour)),
                        'M' => format!("{}", c.minute),
                        's' => format!("{}", c.second),
                        't' => if is_afternoon(c.hour) { "p" } else { "a" }.to_string(),
                        _ => String::new(),
                    }
                } else if remaining == 1 || next2 != Some(ch) {
                    i = i.saturating_add(2);
                    match ch {
                        'y' => format!("{:02}", c.year - (c.year / 100) * 100),
                        'm' => format!("{:02}", c.month),
                        'd' => format!("{:02}", c.day),
                        'H' => format!("{:02}", c.hour),
                        'h' => format!("{:02}", hour_on_twelve_hour_clock(c.hour)),
                        'M' => format!("{:02}", c.minute),
                        's' => format!("{:02}", c.second),
                        't' => if is_afternoon(c.hour) { "pm" } else { "am" }.to_string(),
                        _ => String::new(),
                    }
                } else if remaining == 2 || next3 != Some(ch) {
                    i = i.saturating_add(3);
                    match ch {
                        'm' if is_valid_month(c.month) => month_name(&MONTHS, c.month),
                        'm' => String::new(),
                        _ => ch.to_string().repeat(3),
                    }
                } else if remaining == 3 || next4 != Some(ch) {
                    i = i.saturating_add(4);
                    match ch {
                        'y' => format!("{:04}", c.year),
                        'm' if is_valid_month(c.month) => month_name(&FULL_MONTHS, c.month),
                        'm' => String::new(),
                        _ => ch.to_string().repeat(4),
                    }
                } else {
                    i = i.saturating_add(1);
                    ch.to_string()
                }
            }
            _ => {
                i = i.saturating_add(1);
                ch.to_string()
            }
        };
        out.push_str(&part);
    }
    out
}

/// The heuristic date parser Acrobat applies, which is **not** ECMAScript
/// `Date.parse`.
///
/// Missing pieces stay at `now_ms`. Two-digit groups are month/day (or
/// day/month if that is the only valid assignment). Three groups try
/// year/month/day, month/day/year, day/month/year.
#[must_use]
pub fn parse_date_heuristic(value: &str, now_ms: f64) -> (f64, bool) {
    let now = civil_from_ms(now_ms);
    let mut n_year = now.year;
    let mut n_month = now.month;
    let mut n_day = now.day;
    let n_hour = now.hour;
    let n_min = now.minute;
    let n_sec = now.second;

    let mut number = [0i32; 3];
    let mut n_index = 0;
    let chars: Vec<char> = value.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if n_index > 2 {
            break;
        }
        let Some(c) = chars.get(i).copied() else {
            break;
        };
        if is_decimal_digit(c) {
            let (n, skip) = parse_string_integer(value, i, 4);
            if let Some(slot) = number.get_mut(n_index) {
                *slot = n;
            }
            n_index = n_index.saturating_add(1);
            i = i.saturating_add(skip);
        } else {
            i = i.saturating_add(1);
        }
    }

    let wrong_format;
    if n_index == 2 {
        let a = number[0];
        let b = number[1];
        if is_valid_month(a) && is_valid_day(b) {
            n_month = a;
            n_day = b;
        } else if is_valid_day(a) && is_valid_month(b) {
            n_day = a;
            n_month = b;
        }
        wrong_format = false;
    } else if n_index == 3 {
        let a = number[0];
        let b = number[1];
        let c = number[2];
        if a > 12 && is_valid_month(b) && is_valid_day(c) {
            n_year = a;
            n_month = b;
            n_day = c;
        } else if is_valid_month(a) && is_valid_day(b) && c > 31 {
            n_month = a;
            n_day = b;
            n_year = c;
        } else if is_valid_day(a) && is_valid_month(b) && c > 31 {
            n_day = a;
            n_month = b;
            n_year = c;
        }
        wrong_format = false;
    } else {
        return (now_ms, true);
    }

    (
        ms_from_civil(n_year, n_month, n_day, n_hour, n_min, n_sec),
        wrong_format,
    )
}

/// `ParseDateAsGMT`: split on space and colon, require 8 tokens.
#[must_use]
pub fn parse_date_as_gmt(value: &str) -> f64 {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    for c in value.chars() {
        if c == ' ' || c == ':' {
            tokens.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(c);
    }
    tokens.push(cur);
    if tokens.len() != 8 {
        return 0.0;
    }
    let mut n_month = 1;
    if let Some(s_temp) = tokens.get(1) {
        for (i, name) in MONTHS.iter().enumerate() {
            if s_temp == *name {
                n_month = i32::try_from(i).unwrap_or(0) + 1;
                break;
            }
        }
    }
    // Each remaining field is one token read as a number and narrowed.
    let field = |at: usize| {
        tokens
            .get(at)
            .map_or(0, |s| trunc_i32(crate::parse::string_to_double(s)))
    };
    let n_day = field(2);
    let n_hour = field(3);
    let n_min = field(4);
    let n_sec = field(5);
    let n_year = field(7);
    let d = ms_from_civil(n_year, n_month, n_day, n_hour, n_min, n_sec);
    if d.is_nan() { 0.0 } else { d }
}

/// `ParseDateUsingFormat` with the C++ fallbacks (`kBadDate` then heuristic).
///
/// V8 `Date.parse` is not used; `kBadDate` falls through to the heuristic, which
/// is what the AF format/keystroke paths actually need.
#[must_use]
pub fn parse_date_with_fallback(value: &str, format: &str, now_ms: f64) -> (f64, bool) {
    let (status, d) = parse_date_using_format(value, format, now_ms);
    match status {
        ConversionStatus::Success => (d, false),
        ConversionStatus::BadDate | ConversionStatus::BadFormat => {
            parse_date_heuristic(value, now_ms)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS_DAY: f64 = 1000.0 * 60.0 * 60.0 * 24.0;

    #[test]
    fn year_from_time_matches_fx_date_helpers_unittest() {
        // fxjs/fx_date_helpers_unittest.cpp TEST(FXDateHelperTest, GetYearFromTime)
        let tests: &[(f64, i32)] = &[
            (-400.0 * MS_DAY, 1968),
            (-1.0, 1969),
            (0.0, 1970),
            (1.0, 1970),
            (364.9 * MS_DAY, 1970),
            (365.0 * MS_DAY, 1971),
            (365.1 * MS_DAY, 1971),
            (2.0 * 365.0 * MS_DAY, 1972),
            (3.0 * 365.0 * MS_DAY, 1972),
            ((3.0 * 365.0 + 1.0) * MS_DAY, 1973),
        ];
        for &(t, y) in tests {
            assert_eq!(year_from_time(t), y, "year_from_time({t})");
        }
    }

    #[test]
    fn month_from_time_matches_fx_date_helpers_unittest() {
        // fxjs/fx_date_helpers_unittest.cpp TEST(FXDateHelperTest, GetMonthFromTime)
        let tests: &[(f64, i32)] = &[
            (-400.0 * MS_DAY, 10),
            (-1.0, 11),
            (0.0, 0),
            (1.0, 0),
            (364.9 * MS_DAY, 11),
            (365.0 * MS_DAY, 0),
            (30.0 * MS_DAY, 0),
            (31.0 * MS_DAY, 1),
            (58.0 * MS_DAY, 1),
            (59.0 * MS_DAY, 2),
        ];
        for &(t, m) in tests {
            assert_eq!(month_from_time(t), m, "month_from_time({t})");
        }
    }

    /// The Gregorian rule, including both century exceptions. The oracle gets
    /// the second one backwards; see the note on [`is_leap_year`].
    #[test]
    fn leap_years_follow_the_gregorian_rule() {
        // Every fourth year.
        for year in [1972, 1996, 2024, 2028] {
            assert!(is_leap_year(year), "{year}");
        }
        for year in [1970, 1971, 1973, 2023] {
            assert!(!is_leap_year(year), "{year}");
        }
        // Except centuries.
        for year in [1700, 1800, 1900, 2100, 2200, 2300] {
            assert!(!is_leap_year(year), "{year}");
        }
        // Except every fourth century.
        for year in [1600, 2000, 2400] {
            assert!(is_leap_year(year), "{year}");
        }
    }

    /// The leap-year test and the day arithmetic must agree, or a century year
    /// gains or loses a day somewhere between them. The oracle's spelling makes
    /// them disagree for 1900 and 2000.
    #[test]
    fn the_leap_test_agrees_with_the_day_arithmetic() {
        for year in [1896, 1900, 1904, 1996, 2000, 2004, 2096, 2100] {
            let days_in_year = day_from_year(year + 1) - day_from_year(year);
            let expected = if is_leap_year(year) { 366 } else { 365 };
            assert_eq!(days_in_year, expected, "{year}");
        }
    }

    /// 29 February 2000 exists and is the sixtieth day of that year.
    #[test]
    fn the_year_2000_has_a_leap_day() {
        let leap_day = ms_from_civil(2000, 2, 29, 0, 0, 0);
        assert!(!leap_day.is_nan());
        let c = civil_from_ms(leap_day);
        assert_eq!((c.year, c.month, c.day), (2000, 2, 29));
        // 1900 has no such day, so asking for it rolls into March.
        let c = civil_from_ms(ms_from_civil(1900, 2, 29, 0, 0, 0));
        assert_eq!((c.year, c.month, c.day), (1900, 3, 1));
    }

    /// Two-digit years pivot at fifty, which is Acrobat's documented rule and
    /// pdf.js's (util.js:372-376). The oracle maps every one of them into
    /// 2000..=2099; see the note on [`expand_two_digit_year`].
    #[test]
    fn two_digit_years_pivot_at_fifty() {
        for (typed, expected) in [
            (0, 2000),
            (5, 2005),
            (49, 2049),
            (50, 1950),
            (85, 1985),
            (99, 1999),
        ] {
            assert_eq!(expand_two_digit_year(typed), expected, "{typed:02}");
        }
        // Anything already written in full passes through.
        for year in [100, 1985, 2024, 9999] {
            assert_eq!(expand_two_digit_year(year), year, "{year}");
        }

        let (st, t) = parse_date_using_format("31/12/85", "dd/mm/yy", 0.0);
        assert_eq!(st, ConversionStatus::Success);
        assert_eq!(year_from_time(t), 1985);
        let (st, t) = parse_date_using_format("01/02/05", "dd/mm/yy", 0.0);
        assert_eq!(st, ConversionStatus::Success);
        assert_eq!(year_from_time(t), 2005);
    }

    #[test]
    fn print_date_using_format_embeddertest_cases() {
        // fxjs/cjs_publicmethods_embeddertest.cpp PrintDateUsingFormat
        assert_eq!(
            print_date_using_format(-47_952_000_000.0, "ddmmyy"),
            "250668"
        );
        assert_eq!(
            print_date_using_format(-47_952_000_000.0, "yy/mm/dd"),
            "68/06/25"
        );
        assert_eq!(print_date_using_format(-0.0001, "ddmmyy"), "311269");
        assert_eq!(print_date_using_format(-0.0001, "yy!mmdd"), "69!1231");
        assert_eq!(print_date_using_format(0.0, "ddmmyy"), "010170");
        assert_eq!(print_date_using_format(0.0, "mm-yyyy-dd"), "01-1970-01");
        assert_eq!(
            print_date_using_format(504_835_200_000.0, "ddmmyy"),
            "311285"
        );
        assert_eq!(
            print_date_using_format(791_596_800_000.0, "ddmmyy"),
            "010295"
        );
        assert_eq!(
            print_date_using_format(1_107_216_000_000.0, "ddmmyy"),
            "010205"
        );
        assert_eq!(
            print_date_using_format(3_660_595_200_000.0, "ddmmyy"),
            "311285"
        );
        assert_eq!(
            print_date_using_format(3_947_356_800_000.0, "mmddyyyy"),
            "02012095"
        );
    }

    #[test]
    fn parse_date_using_format_empty_returns_now() {
        // fx_date_helpers_unittest.cpp ParseDateUsingFormatWithEmptyParams
        let now = 1_587_654_321_000.0;
        let (st, t) = parse_date_using_format("", "", now);
        assert_eq!(st, ConversionStatus::Success);
        assert_eq!(t, now);
        let (st, t) = parse_date_using_format("value", "", now);
        assert_eq!(st, ConversionStatus::Success);
        assert_eq!(t, now);
    }

    /// The twelve-hour clock runs 12, 1, …, 11 in each half of the day, and
    /// noon is pm. The oracle prints midnight as `0` and calls noon `am`; see
    /// the note on `hour_on_twelve_hour_clock`.
    #[test]
    fn the_twelve_hour_clock_and_its_meridiem() {
        let at = |hour| ms_from_civil(1970, 1, 1, hour, 0, 0);
        let cases: &[(i32, &str, &str, &str)] = &[
            (0, "12", "a", "am"),
            (1, "1", "a", "am"),
            (11, "11", "a", "am"),
            (12, "12", "p", "pm"),
            (13, "1", "p", "pm"),
            (23, "11", "p", "pm"),
        ];
        for &(hour, twelve, short, long) in cases {
            let t = at(hour);
            assert_eq!(print_date_using_format(t, "h"), twelve, "hour {hour}");
            assert_eq!(print_date_using_format(t, "t"), short, "hour {hour}");
            assert_eq!(print_date_using_format(t, "tt"), long, "hour {hour}");
        }
        // The padded form pads the same numbers.
        assert_eq!(print_date_using_format(at(0), "hh"), "12");
        assert_eq!(print_date_using_format(at(9), "hh"), "09");
        // The twenty-four hour form is untouched by any of this.
        assert_eq!(print_date_using_format(at(0), "HH"), "00");
        assert_eq!(print_date_using_format(at(13), "HH"), "13");
    }

    /// The weekday is derived from the date rather than left at zero.
    #[test]
    fn weekdays_are_computed() {
        // The epoch was a Thursday.
        assert_eq!(weekday_from_time(0.0), 4);
        assert_eq!(weekday_from_time(MS_DAY), 5);
        assert_eq!(weekday_from_time(3.0 * MS_DAY), 0);
        assert_eq!(weekday_from_time(-MS_DAY), 3);
        // 4 July 2014 was a Friday.
        assert_eq!(civil_from_ms(ms_from_civil(2014, 7, 4, 0, 0, 0)).weekday, 5);
        // 29 February 2000 was a Tuesday.
        assert_eq!(
            civil_from_ms(ms_from_civil(2000, 2, 29, 0, 0, 0)).weekday,
            2
        );
    }
}
