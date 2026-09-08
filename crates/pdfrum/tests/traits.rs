//! The standard-trait surface: what `Display`, `FromStr` and `TryFrom` say
//! about the facade's value types, and that the pairs are inverses.
//!
//! Every type here that has both `Display` and `FromStr` must round-trip
//! through them for every variant, because that is the property a caller
//! relies on when a value goes out to a config file, a CLI flag or a log line
//! and comes back.

// A round trip that does not round-trip is the failure this file exists to
// catch, and `expect` is how it says so.
#![allow(clippy::expect_used)]

use std::str::FromStr;

use pdfrum::{ErrorCode, FontFileKind, ImageEncoding, Rotation};

#[cfg(feature = "edit")]
use pdfrum::{FlattenMode, Flattened, StampPosition, Update};

/// Every variant's `Display` parses back to itself.
fn round_trips<T>(all: &[T])
where
    T: FromStr + std::fmt::Display + std::fmt::Debug + PartialEq,
    <T as FromStr>::Err: std::fmt::Debug,
{
    for value in all {
        let text = value.to_string();
        let back = T::from_str(&text).expect("Display must parse back");
        assert_eq!(&back, value, "round trip through {text:?}");
    }
}

#[test]
fn rotation_round_trips_and_reads_as_degrees() {
    round_trips(&[
        Rotation::None,
        Rotation::Quarter,
        Rotation::Half,
        Rotation::ThreeQuarter,
    ]);
    assert_eq!(Rotation::Quarter.to_string(), "90");
    assert_eq!(Rotation::ThreeQuarter.to_string(), "270");

    // Out-of-range degrees are the file reader's to normalize, not the
    // parser's — a round trip that silently accepted them would be lossy.
    assert!("450".parse::<Rotation>().is_err());
    assert!("-90".parse::<Rotation>().is_err());
    assert!("".parse::<Rotation>().is_err());
}

#[test]
fn image_encoding_round_trips_on_its_name_not_its_extension() {
    round_trips(&[
        ImageEncoding::Jpeg,
        ImageEncoding::Jpeg2000,
        ImageEncoding::Jbig2,
        ImageEncoding::CcittFax,
    ]);
    assert_eq!(ImageEncoding::Jpeg.to_string(), "jpeg");
    // The extension is a different question and is not what parses.
    assert_eq!(ImageEncoding::Jpeg.extension(), "jpg");
    assert!("jpg".parse::<ImageEncoding>().is_err());
}

#[test]
fn font_file_kind_round_trips() {
    round_trips(&[
        FontFileKind::Type1,
        FontFileKind::TrueType,
        FontFileKind::Cff,
        FontFileKind::OpenType,
    ]);
    assert_eq!(FontFileKind::OpenType.to_string(), "opentype");
    assert!("pfb".parse::<FontFileKind>().is_err());
}

#[test]
fn error_code_displays_its_domain_and_converts_both_ways() {
    let all = [
        ErrorCode::Io,
        ErrorCode::Open,
        ErrorCode::WrongPassword,
        ErrorCode::Read,
        ErrorCode::Render,
        ErrorCode::Doc,
        ErrorCode::Save,
        ErrorCode::Text,
        ErrorCode::Limit,
        ErrorCode::Svg,
    ];
    for code in all {
        assert_eq!(ErrorCode::try_from(u32::from(code)), Ok(code));
        // Display is the domain word, never a repeat of Debug.
        assert_ne!(code.to_string(), format!("{code:?}"));
    }
    assert_eq!(ErrorCode::WrongPassword.to_string(), "wrong-password");

    // The numbering has no 0 and stops at 10; TryFrom is fallible because the
    // enum is non_exhaustive and most u32s name nothing.
    assert!(ErrorCode::try_from(0).is_err());
    assert!(ErrorCode::try_from(11).is_err());
    assert!(ErrorCode::try_from(u32::MAX).is_err());
}

#[cfg(feature = "edit")]
#[test]
fn edit_mode_enums_round_trip() {
    round_trips(&[FlattenMode::Display, FlattenMode::Print]);
    round_trips(&[Update::Rewrite, Update::Incremental]);
    round_trips(&[
        StampPosition::Center,
        StampPosition::TopLeft,
        StampPosition::TopRight,
        StampPosition::BottomLeft,
        StampPosition::BottomRight,
    ]);
    assert_eq!(StampPosition::TopLeft.to_string(), "top-left");
    assert_eq!(Update::Incremental.to_string(), "incremental");
}

/// `Flattened` is an outcome a call reports, so it has `Display` and
/// deliberately no `FromStr` — there is no caller who writes one down.
#[cfg(feature = "edit")]
#[test]
fn flattened_displays_but_does_not_parse() {
    assert_eq!(Flattened::Done.to_string(), "flattened");
    assert_eq!(Flattened::NothingToDo.to_string(), "nothing to do");
}

/// The builders are sugar: each must produce exactly what the struct-update
/// path produces, and the struct-update path must still compile.
#[test]
fn builders_agree_with_struct_update() {
    use pdfrum::{ColorMode, RenderOptions};

    let built = RenderOptions::builder()
        .scale(2.0)
        .grayscale()
        .annotations(false)
        .smooth_paths(false)
        .build();
    let written = {
        let mut __o = RenderOptions::default();
        __o.transform = kurbo::Affine::scale(2.0);
        __o.color_mode = ColorMode::Gray;
        __o.annotations = false;
        __o.smooth_paths = false;
        __o
    };
    assert_eq!(built, written);

    assert_eq!(RenderOptions::builder().build(), RenderOptions::default());
    assert_eq!(
        RenderOptions::builder().scale(1.5).build(),
        RenderOptions::scaled(1.5)
    );
}

#[cfg(feature = "edit")]
#[test]
fn edit_builders_agree_with_struct_update() {
    use pdfrum::{AttachmentOptions, SaveOptions, StampOptions, Update};

    assert_eq!(SaveOptions::builder().incremental().build(), {
        let mut __o = SaveOptions::default();
        __o.update = Update::Incremental;
        __o
    });
    assert_eq!(StampOptions::builder().angle(45.0).margin(18.0).build(), {
        let mut __o = StampOptions::default();
        __o.angle = 45.0;
        __o.margin = 18.0;
        __o
    });
    assert_eq!(
        AttachmentOptions::builder().mime_type("text/csv").build(),
        {
            let mut __o = AttachmentOptions::default();
            __o.mime_type = Some("text/csv".into());
            __o
        }
    );
}

#[test]
fn open_options_builder_takes_str_and_bytes_alike() {
    use pdfrum::OpenOptions;

    let from_str = OpenOptions::builder().password("secret").build();
    let from_bytes = OpenOptions::builder()
        .password(b"secret".as_slice())
        .build();
    assert_eq!(from_str.password, from_bytes.password);
    assert_eq!(from_str.password.as_deref(), Some(&b"secret"[..]));

    // A password need not be UTF-8, and the builder does not require it to be.
    let raw = OpenOptions::builder()
        .password(b"\xff\xfe".as_slice())
        .build();
    assert_eq!(raw.password.as_deref(), Some(&b"\xff\xfe"[..]));
}
