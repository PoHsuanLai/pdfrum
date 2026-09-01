//! `util.scand`.

use crate::time::parse_date_with_fallback;

/// `CJS_Util::scand`. Empty `date` yields `now_ms`. `NaN` becomes `None`
/// (the C++ returns `undefined`).
#[must_use]
pub fn util_scand(format: &str, date: &str, now_ms: f64) -> Option<f64> {
    let d = if date.is_empty() {
        now_ms
    } else {
        parse_date_with_fallback(date, format, now_ms).0
    };
    if d.is_nan() { None } else { Some(d) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{ms_from_civil, year_from_time};

    /// Fake "now" matching the `public_methods` embedder fixture: 9 May 2014 21:48:50.
    const NOW: f64 = 1_399_672_130_000.0;

    #[test]
    fn scand_fixture_dates() {
        // testing/resources/javascript/util_scand.in
        let t = util_scand("mm/dd/yyyy", "12/20/2016", NOW).unwrap();
        assert_eq!(year_from_time(t), 2016);
        let expected = ms_from_civil(2016, 12, 20, 21, 48, 50);
        assert!((t - expected).abs() < 1.0, "{t} vs {expected}");

        let t = util_scand("dd/mm/yyyy", "20/12/2016", NOW).unwrap();
        assert!((t - expected).abs() < 1.0);

        let t = util_scand("yyyy/mm/dd", "2016/12/20", NOW).unwrap();
        assert!((t - expected).abs() < 1.0);

        let t = util_scand("dd/mmm/yyyy", "20/Dec/2016", NOW).unwrap();
        assert!((t - expected).abs() < 1.0);

        let t = util_scand("..dd:-:mmm/yyyy", "**20/*/Dec.2016", NOW).unwrap();
        assert!((t - expected).abs() < 1.0);
    }

    #[test]
    fn scand_time_keeps_now_date() {
        let t = util_scand("hh:MM:ss", "11:22:03", NOW).unwrap();
        let expected = ms_from_civil(2014, 5, 9, 11, 22, 3);
        assert!((t - expected).abs() < 1.0, "{t} vs {expected}");
    }
}
