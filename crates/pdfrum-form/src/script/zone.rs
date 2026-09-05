//! The timezone the engine's `Date` sees.
//!
//! **`Date` and `util.printd` do not share a zone**, and the difference is
//! structural rather than a bug in either — see [`super::GOLDEN_PRINTD_OFFSET_SECS`].
//! `util.printd` goes through `FX_LocalTime`, whose daylight term is
//! `GetDaylightSavingTA` reading `tm_isdst` from `FXSYS_localtime`
//! (`fxjs/fx_date_helpers.cpp:54-67`) — and `pdfium_test` replaces that hook
//! with `gmtime` (`testing/pdfium_test/pdfium_test.cc:2134`), so the term is
//! always zero and printd's shift is a flat standard offset. The engine's
//! `Date` is **not** hooked: V8 resolves `TZ=America/Los_Angeles` through the
//! real zone database, per instant, daylight saving included.
//!
//! That is why this module exists. A flat offset here is right for July and
//! an hour wrong for December, and the error is visible in the goldens: five
//! `util_printd_expected.txt` lines are winter dates, and one of them
//! (`new Date(2525, 11, 31)`) is an hour before midnight, so the hour error
//! prints as `12/30/2525` for an expected `12/31/2525`.

/// How a zone's daylight-saving term is decided.
///
/// A rule rather than a number because the answer depends on the instant
/// being converted, which is the whole distinction this module draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Daylight {
    /// No daylight saving, ever. An embedder's fixed-offset zone, and what a
    /// caller that only has one number can honestly say. The default.
    #[default]
    Never,
    /// The United States federal rule, as the zone database records it for
    /// `America/Los_Angeles`: the pre-1883 solar offset, no daylight saving
    /// before 1918, the April-to-October rule to 2006, and the
    /// March-to-November rule after it.
    UnitedStates,
}

/// The eras of the United States federal daylight-saving rule that this
/// implementation distinguishes.
///
/// **The middle of the twentieth century is deliberately not modelled.**
/// Between the 1918 introduction and the 1987 federal settlement the observed
/// dates were a patchwork of national wartime and local choices that no
/// closed-form rule reproduces; a table of them is a zone database, which is
/// not a dependency this crate takes for a handful of fixture dates. The eras
/// below are the ones the corpus reaches — `new Date(1900, ...)`, the
/// 2013-2015 dates, and `new Date(2525, 11, 31)` — and the unmodelled span
/// answers with the pre-2007 rule, which is the closest single rule to it.
/// (The pre-1883 solar era is handled before these, by
/// [`LOS_ANGELES_LOCAL_MEAN_TIME_SECS`].)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsFederal {
    /// Before 1918: the United States kept no daylight saving at all, and
    /// `America/Los_Angeles` is standard time year round. `new Date(1900, 06,
    /// 04, ...)` is a July date that is nonetheless **not** shifted, which is
    /// what `util_printd_expected.txt:44` records as `07/04/1900 15:59:58`.
    NoDaylight,
    /// 1918 through 2006: the pre-Energy-Policy-Act rule, daylight saving
    /// from the first Sunday in April to the last Sunday in October.
    FirstSundayInAprilToLastSundayInOctober,
    /// 2007 onward: the Energy Policy Act of 2005 rule, daylight saving from
    /// the second Sunday in March to the first Sunday in November. The rule
    /// still in force, so it is the one a far-future date such as
    /// `new Date(2525, 11, 31)` gets.
    SecondSundayInMarchToFirstSundayInNovember,
}

impl UsFederal {
    /// The rule in force in `year`.
    fn in_force(year: i64) -> UsFederal {
        if year < 1918 {
            UsFederal::NoDaylight
        } else if year < 2007 {
            UsFederal::FirstSundayInAprilToLastSundayInOctober
        } else {
            UsFederal::SecondSundayInMarchToFirstSundayInNovember
        }
    }
}

/// `America/Los_Angeles`'s local mean time, in seconds east of UTC:
/// −7:52:58, the city's solar offset.
///
/// **Before standard time there were no time zones**, and the zone database
/// records the city's own solar offset for every instant before the railroads
/// adopted the meridian hours. V8 reads that record, so
/// `new Date(1850, 0, 1)` is 07:52:58 UTC and not 08:00:00 — which
/// `util.printd`, shifting a flat eight hours back, prints as
/// `12/31/1849` rather than `01/01/1850`
/// (`util_printd_expected.txt:32`). Two minutes of arc across the
/// midnight boundary, and the golden records it.
const LOS_ANGELES_LOCAL_MEAN_TIME_SECS: i32 = -(7 * 3600 + 52 * 60 + 58);

/// The instant `America/Los_Angeles` left local mean time for `GMT-0800`:
/// 1883-11-18 at noon standard time, the Day of Two Noons, when the North
/// American railroads adopted the meridian zones.
const LOS_ANGELES_STANDARD_TIME_ADOPTED: i64 = -2_717_640_000;

/// A local zone: a standard offset plus a rule for the daylight term.
///
/// Not a bare `i32`, because the two halves answer different questions and
/// only one of them depends on the instant.
///
/// [`Default`] is [`Zone::UTC`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Zone {
    /// Seconds east of UTC in standard time. `-28800` is `GMT-0800`, which is
    /// `America/Los_Angeles`'s.
    pub standard_offset_secs: i32,
    /// Whether, and how, an hour is added in summer.
    pub daylight: Daylight,
}

impl Zone {
    /// A zone that is the same offset all year — an embedder's, and the
    /// honest answer when all a caller has is one number.
    pub const fn fixed(offset_secs: i32) -> Zone {
        Zone {
            standard_offset_secs: offset_secs,
            daylight: Daylight::Never,
        }
    }

    /// `America/Los_Angeles`: `GMT-0800` standard, United States rule.
    ///
    /// The zone `pdfium_test` runs under (`TZ=America/Los_Angeles`, set by
    /// `testing/tools/common.py`), and therefore the one the goldens record.
    pub const LOS_ANGELES: Zone = Zone {
        standard_offset_secs: -8 * 3600,
        daylight: Daylight::UnitedStates,
    };

    /// UTC: no offset and no daylight saving. [`Default`]'s answer, and the
    /// one a session with no configured zone gets.
    pub const UTC: Zone = Zone::fixed(0);

    /// The offset in seconds east of UTC that applies at `unix_time_seconds`.
    pub fn offset_secs_at(self, unix_time_seconds: i64) -> i32 {
        match self.daylight {
            Daylight::Never => self.standard_offset_secs,
            Daylight::UnitedStates => {
                if unix_time_seconds < LOS_ANGELES_STANDARD_TIME_ADOPTED {
                    LOS_ANGELES_LOCAL_MEAN_TIME_SECS
                } else if self.is_daylight(unix_time_seconds) {
                    self.standard_offset_secs + 3600
                } else {
                    self.standard_offset_secs
                }
            }
        }
    }

    /// Whether daylight saving is in force at `unix_time_seconds`.
    ///
    /// The transitions are 02:00 **local standard** time in spring and 02:00
    /// local daylight time in autumn; both are evaluated against the instant
    /// shifted by the standard offset, which puts the autumn boundary an hour
    /// early. That hour is the ambiguous repeated hour, and no golden line
    /// lands in it.
    fn is_daylight(self, unix_time_seconds: i64) -> bool {
        /// 02:00 local standard time, when both transitions happen.
        const TWO_AM: i64 = 2 * 3600;

        let local = unix_time_seconds + i64::from(self.standard_offset_secs);
        let (year, month, day, seconds_into_day) = civil_from_unix(local);
        let rule = UsFederal::in_force(year);
        let (start, end) = match rule {
            UsFederal::NoDaylight => return false,
            UsFederal::FirstSundayInAprilToLastSundayInOctober => {
                ((4, nth_sunday(year, 4, 1)), (10, last_sunday(year, 10)))
            }
            UsFederal::SecondSundayInMarchToFirstSundayInNovember => {
                ((3, nth_sunday(year, 3, 2)), (11, nth_sunday(year, 11, 1)))
            }
        };
        let after_start = (month, day, seconds_into_day) >= (start.0, start.1, TWO_AM);
        let before_end = (month, day, seconds_into_day) < (end.0, end.1, TWO_AM);
        after_start && before_end
    }
}

/// The day of `month` in `year` that is the `n`th Sunday of it.
fn nth_sunday(year: i64, month: u32, n: u32) -> u32 {
    let first_weekday = weekday_of(year, month, 1);
    // Days from the 1st to the first Sunday, then whole weeks.
    let first_sunday = 1 + (7 - first_weekday) % 7;
    first_sunday + (n - 1) * 7
}

/// The day of `month` in `year` that is its last Sunday.
fn last_sunday(year: i64, month: u32) -> u32 {
    let last = days_in_month(year, month);
    last - weekday_of(year, month, last)
}

/// The weekday of a civil date, 0 for Sunday.
fn weekday_of(year: i64, month: u32, day: u32) -> u32 {
    let days = days_from_civil(year, month, day);
    // 1970-01-01 was a Thursday, weekday 4.
    u32::try_from((days + 4).rem_euclid(7)).unwrap_or(0)
}

/// Whether `year` is a leap year in the proleptic Gregorian calendar.
fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// The number of days in `month` of `year`.
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(year) => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian civil date.
///
/// Howard Hinnant's `days_from_civil`, which is exact for every year this
/// crate can be handed, negative ones included.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let month = i64::from(month);
    let day = i64::from(day);
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The civil date and second-of-day of a unix instant: `(year, month, day,
/// seconds_into_day)`, the inverse of [`days_from_civil`].
fn civil_from_unix(seconds: i64) -> (i64, u32, u32, i64) {
    let days = seconds.div_euclid(86_400);
    let seconds_into_day = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (year + i64::from(month <= 2), month, day, seconds_into_day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The calendar round-trips, which is what every rule below rests on.
    #[test]
    fn the_civil_calendar_round_trips() {
        for &(year, month, day) in &[
            (1849, 12, 31),
            (1900, 7, 4),
            (1970, 1, 1),
            (2014, 7, 4),
            (2525, 12, 31),
            (0, 3, 1),
        ] {
            let days = days_from_civil(year, month, day);
            assert_eq!(
                civil_from_unix(days * 86_400),
                (year, month, day, 0),
                "{year}-{month}-{day}"
            );
        }
    }

    /// 1970-01-01 was a Thursday, and 2014-07-04 a Friday — the second is the
    /// date every `util_printd` line but five is built from.
    #[test]
    fn weekdays_are_the_calendars_own() {
        assert_eq!(weekday_of(1970, 1, 1), 4);
        assert_eq!(weekday_of(2014, 7, 4), 5);
        // 2525-12-31 is a Monday, far outside any table a lookup could hold.
        assert_eq!(weekday_of(2525, 12, 31), 1);
    }

    /// The two transition-date rules, at the years either side of the 2007
    /// change.
    #[test]
    fn the_transition_days_are_the_federal_rules() {
        // 2006: first Sunday in April was the 2nd, last Sunday in October the 29th.
        assert_eq!(nth_sunday(2006, 4, 1), 2);
        assert_eq!(last_sunday(2006, 10), 29);
        // 2014: second Sunday in March was the 9th, first Sunday in November the 2nd.
        assert_eq!(nth_sunday(2014, 3, 2), 9);
        assert_eq!(nth_sunday(2014, 11, 1), 2);
    }

    /// **The regression this module exists for.** Every `util_printd` date
    /// whose expected line the flat `GMT-0700` offset got wrong, pinned by
    /// its own instant rather than by today's — the bug was invisible for two
    /// days because nothing in the pass depended on the calendar, and then a
    /// board run recorded `12/30/2525` for an expected `12/31/2525`.
    ///
    /// The instants are UTC seconds for the local wall-clock times the
    /// fixture's `new Date(...)` calls name, under `America/Los_Angeles`.
    #[test]
    fn los_angeles_is_standard_time_in_winter_and_in_1900() {
        let winter = [
            // 2525-12-31 00:00:00 PST.
            (
                days_from_civil(2525, 12, 31) * 86_400 + 8 * 3600,
                "2525-12-31",
            ),
            // 1900-07-04 15:59:58 — July, but before the United States had
            // daylight saving at all.
            (
                days_from_civil(1900, 7, 4) * 86_400 + 15 * 3600 + 59 * 60 + 58 + 8 * 3600,
                "1900-07-04",
            ),
            // 2015-12-09, 2014-03-02 and 2013-12-30, the other three.
            (
                days_from_civil(2015, 12, 9) * 86_400 + 8 * 3600,
                "2015-12-09",
            ),
            (
                days_from_civil(2014, 3, 2) * 86_400 + 8 * 3600,
                "2014-03-02",
            ),
            (
                days_from_civil(2013, 12, 30) * 86_400 + 8 * 3600,
                "2013-12-30",
            ),
        ];
        for (instant, label) in winter {
            assert_eq!(
                Zone::LOS_ANGELES.offset_secs_at(instant),
                -8 * 3600,
                "{label} is standard time"
            );
        }
    }

    /// And daylight time in summer, which is the offset the other 52 lines
    /// and the frozen seed itself were recorded under.
    #[test]
    fn los_angeles_is_daylight_time_in_summer() {
        for (instant, label) in [
            (
                days_from_civil(2014, 7, 4) * 86_400 + 7 * 3600,
                "2014-07-04",
            ),
            // The frozen clock, 2014-05-09 — `the_timezone_is_pdfiums_own`
            // asserts the 420-minute answer this produces.
            (super::super::GOLDEN_CLOCK_SECS.cast_signed(), "the seed"),
            (
                days_from_civil(2015, 9, 4) * 86_400 + 7 * 3600,
                "2015-09-04",
            ),
        ] {
            assert_eq!(
                Zone::LOS_ANGELES.offset_secs_at(instant),
                -7 * 3600,
                "{label} is daylight time"
            );
        }
    }

    /// Before 1883 the offset is the city's solar one, which is the other
    /// midnight-boundary golden line: `new Date(1850, 0, 1)` prints
    /// `12/31/1849`, not `01/01/1850`.
    #[test]
    fn los_angeles_kept_local_mean_time_before_the_railroads() {
        // 1850-01-01 00:00:00 local mean time.
        let instant = days_from_civil(1850, 1, 1) * 86_400 + 7 * 3600 + 52 * 60 + 58;
        assert_eq!(
            Zone::LOS_ANGELES.offset_secs_at(instant),
            LOS_ANGELES_LOCAL_MEAN_TIME_SECS
        );
        // And 1900 is already standard time, an era later.
        let nineteen_hundred = days_from_civil(1900, 7, 4) * 86_400 + 8 * 3600;
        assert_eq!(
            Zone::LOS_ANGELES.offset_secs_at(nineteen_hundred),
            -8 * 3600
        );
    }

    /// A fixed zone answers the same for every instant, which is what an
    /// embedder that has one number gets.
    #[test]
    fn a_fixed_zone_never_shifts() {
        let zone = Zone::fixed(3600);
        assert_eq!(zone.offset_secs_at(0), 3600);
        assert_eq!(
            zone.offset_secs_at(days_from_civil(2014, 7, 4) * 86_400),
            3600
        );
    }
}
