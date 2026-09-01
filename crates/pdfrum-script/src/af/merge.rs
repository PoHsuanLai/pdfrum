//! `AFMergeChange` and `AFExtractNums`.

use super::Keystroke;
use crate::parse::is_decimal_digit;

/// `AFMergeChange` — what the field would hold once this keystroke lands.
///
/// On commit there is no pending insertion, so the value comes back as it is.
/// Otherwise the change is spliced in over the selection.
#[must_use]
pub fn af_merge_change(event: &Keystroke) -> String {
    event.merged()
}

/// `AFExtractNums` — every run of digits in the string, in order.
///
/// A value starting with a decimal mark gets a `0` planted in front of it
/// first, which is meant to make `.5` read as a number — but the mark itself
/// still breaks the run, so the answer is the two pieces `0` and `5` rather
/// than the one number `0.5`. The nicety does not achieve what it looks like it
/// achieves; it is reproduced because the split is what callers see.
///
/// `None` where the oracle answers `undefined`: a string with no digits at all.
#[must_use]
pub fn af_extract_nums(s: &str) -> Option<Vec<String>> {
    let mut runs: Vec<String> = Vec::new();
    let mut run = String::new();

    // The planted zero is a run of its own, since the mark that follows it
    // immediately ends it.
    if matches!(s.chars().next(), Some('.' | ',')) {
        runs.push("0".to_string());
    }

    for ch in s.chars() {
        if is_decimal_digit(ch) {
            run.push(ch);
        } else if !run.is_empty() {
            runs.push(std::mem::take(&mut run));
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    (!runs.is_empty()).then_some(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both `AFMergeChange` lines the transcript records — the same call under
    /// the two event kinds, answering differently.
    #[test]
    fn merge_change_transcript_lines() {
        assert_eq!(af_merge_change(&Keystroke::commit("one")), "one");
        assert_eq!(af_merge_change(&Keystroke::insert("one", "A", 0)), "Aone");
    }

    /// A selection is replaced rather than pushed aside.
    #[test]
    fn a_selection_is_replaced() {
        assert_eq!(
            af_merge_change(&Keystroke::replace("abcd", "X", 1, 3)),
            "aXd"
        );
        assert_eq!(af_merge_change(&Keystroke::replace("abcd", "", 0, 4)), "");
    }

    /// A negative selection start keeps the whole value and appends, because
    /// the prefix is taken by an unsigned count that a negative one saturates.
    #[test]
    fn a_negative_selection_start_keeps_everything() {
        let mut event = Keystroke::insert("abc", "X", 0);
        event.sel_start = -1;
        event.sel_end = -1;
        assert_eq!(af_merge_change(&event), "abcX");
    }

    /// A selection end past the value's length leaves nothing behind the
    /// insertion.
    #[test]
    fn a_stale_selection_end_truncates() {
        assert_eq!(
            af_merge_change(&Keystroke::replace("abc", "X", 1, 99)),
            "aX"
        );
    }

    /// The transcript's `AFExtractNums('100 200')` line.
    #[test]
    fn extract_nums_transcript_line() {
        assert_eq!(af_extract_nums("100 200").unwrap(), ["100", "200"]);
    }

    /// A leading decimal mark yields two runs, not one number — the planted
    /// zero is separated from the digits by the very mark it was meant to fix.
    #[test]
    fn a_leading_mark_yields_two_runs_not_one_number() {
        assert_eq!(af_extract_nums(".5").unwrap(), ["0", "5"]);
        assert_eq!(af_extract_nums(",5").unwrap(), ["0", "5"]);
        // An interior mark gets no such treatment and simply splits.
        assert_eq!(af_extract_nums("1.5").unwrap(), ["1", "5"]);
    }

    /// No digits and no leading mark is the one case with no answer — a
    /// leading mark plants its zero even when nothing follows.
    #[test]
    fn a_string_without_digits_has_no_answer() {
        for value in ["", "abc", " ", "-"] {
            assert!(af_extract_nums(value).is_none(), "{value:?}");
        }
        assert_eq!(af_extract_nums("...").unwrap(), ["0"]);
    }

    #[test]
    fn runs_keep_their_leading_zeros_and_their_order() {
        assert_eq!(af_extract_nums("a01b002c3").unwrap(), ["01", "002", "3"]);
    }
}
