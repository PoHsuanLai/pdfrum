#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(clippy::indexing_slicing)]
#![warn(missing_docs)]
// The picture-driven date and printf parsers each walk one long table-shaped
// match. Splitting them would scatter a single decision across several
// functions without making any of them clearer, so the length is allowed and
// the argument counts that come with reproducing a fixed external signature go
// with it.
#![allow(
    clippy::fn_params_excessive_bools,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
// Tests compare exact values against a transcript, and reach past the API's
// guard rails to build the inputs the transcript describes.
#![cfg_attr(test, allow(clippy::float_cmp, clippy::unwrap_used))]

pub mod af;
pub mod error;
mod parse;
mod printd;
mod printf;
mod printx;
mod scand;
mod time;

pub use af::{
    AfFormat, AfOutcome, AfValue, DATE_FORMATS, Keystroke, KeystrokeOutcome, KeystrokeResult,
    SimpleOp, TIME_FORMATS, Thrown, af_date_format, af_date_format_ex, af_date_keystroke,
    af_date_keystroke_ex, af_extract_nums, af_make_number, af_merge_change, af_number_format,
    af_number_keystroke, af_parse_date_ex, af_percent_format, af_percent_keystroke,
    af_range_validate, af_simple, af_simple_calculate, af_simple_calculate_texts,
    af_special_format, af_special_keystroke, af_special_keystroke_ex, af_split_field_list,
    af_time_format, af_time_format_ex, af_time_keystroke, af_time_keystroke_ex,
    is_reserved_mask_char, mask_satisfied, merge_change,
};
pub use error::{
    ALERT_BUTTON_OK, ALERT_ICON_STATUS, AfAlert, AfColor, AfColorSpace, AfEffects, AlertMessage,
    Error,
};
pub use parse::{c_atof, is_number, js_to_number, string_to_double, trim_spaces};
pub use printd::{PrintdStyle, util_printd, util_printd_style};
pub use printf::{PrintfArg, PrintfDataType, parse_data_type, util_printf};
pub use printx::util_printx;
pub use scand::util_scand;
pub use time::{
    Civil, ConversionStatus, FULL_MONTHS, MONTHS, civil_from_ms, day_from_time, hour_from_time,
    is_leap_year, is_valid_24_hour, is_valid_day, is_valid_minute, is_valid_month, is_valid_second,
    make_date, make_day, make_time, min_from_time, month_from_time, ms_from_civil,
    parse_date_using_format, parse_date_with_fallback, print_date_using_format, sec_from_time,
    year_from_time,
};

/// Acrobat's name for this error type.
pub type AfError = Error;
