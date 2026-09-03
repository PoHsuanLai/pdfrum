//! Every public enum variant reachable from `pdfrum::*` is constructible
//! with only `pdfrum::` paths.
//!
//! This is the gate WP13 adds on top of [`reexports`]: WP7 proved a type in a
//! signature can be *named*; this proves a variant can be *written*, payload
//! and all. `ColorMode::Forced` was the first known hole — `ColorScheme` was
//! re-exported, `Argb` was not, and there was no expression a
//! `cargo add pdfrum` caller could write that produced the variant.
//!
//! The list is derived from `docs/status/api-baseline/pdfrum.txt` and
//! `pdfrum+javascript.txt`, not by hand:
//!
//! 1. Collect every `pub use pdfrum::Foo` and `pub enum pdfrum::Foo`.
//! 2. Look those names up as `pub enum` in the member-crate snapshots
//!    (renames from WP7's error payloads: `OpenError` is `LoadError`,
//!    `PageRotation` is `pdfrum_page::Rotation`, and so on).
//! 3. Take every `pub Enum::Variant` / `pub Enum::Variant(payload)` line
//!    (struct-variant fields are `Enum::Variant::field` and are skipped).
//!
//! `snapshot_variant_count` re-parses those files at
//! runtime and asserts the construction count. A new variant updates the
//! snapshot (the other WP13 gate) and then fails this test until a
//! construction line is added.
//!
//! Script-only enums (`ScriptStop`, `TranscriptLine`) are the `pub use`
//! names in `pdfrum+javascript.txt` that `pdfrum.txt` does not carry; they are
//! gated on `feature = "javascript"`.

#![allow(clippy::too_many_lines)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::cognitive_complexity)]
#![allow(clippy::items_after_statements)]

use pdfrum::*;

/// Snapshot path, rustdoc enum path, facade name. The derivation index.
const SNAPSHOT_ENUMS: &[(&str, &str, &str)] = &[
    ("pdfrum.txt", "pdfrum::Error", "Error"),
    ("pdfrum.txt", "pdfrum::Rotation", "Rotation"),
    ("pdfrum.txt", "pdfrum::Update", "Update"),
    ("pdfrum.txt", "pdfrum::FlattenMode", "FlattenMode"),
    ("pdfrum.txt", "pdfrum::Flattened", "Flattened"),
    ("pdfrum-common.txt", "pdfrum_common::DiagKind", "DiagKind"),
    ("pdfrum-common.txt", "pdfrum_common::Severity", "Severity"),
    ("pdfrum-doc.txt", "pdfrum_doc::Subtype", "Subtype"),
    ("pdfrum-doc.txt", "pdfrum_doc::FocusBox", "FocusBox"),
    ("pdfrum-doc.txt", "pdfrum_doc::form::FieldKind", "FieldKind"),
    ("pdfrum-doc.txt", "pdfrum_doc::ActionKind", "ActionKind"),
    ("pdfrum-doc.txt", "pdfrum_doc::Error", "DocError"),
    ("pdfrum-edit.txt", "pdfrum_edit::Error", "SaveError"),
    (
        "pdfrum-edit.txt",
        "pdfrum_edit::FontEncoding",
        "FontEncoding",
    ),
    ("pdfrum-edit.txt", "pdfrum_edit::PixelFormat", "PixelFormat"),
    ("pdfrum-form.txt", "pdfrum_form::Button", "Button"),
    ("pdfrum-form.txt", "pdfrum_form::Event", "Event"),
    ("pdfrum-form.txt", "pdfrum_form::Key", "Key"),
    (
        "pdfrum-form.txt",
        "pdfrum_form::KeystrokeOutcome",
        "KeystrokeOutcome",
    ),
    ("pdfrum-form.txt", "pdfrum_form::Placement", "Placement"),
    ("pdfrum-form.txt", "pdfrum_form::UpdateKind", "UpdateKind"),
    ("pdfrum-object.txt", "pdfrum_object::Error", "ObjectError"),
    ("pdfrum-object.txt", "pdfrum_object::Object", "Object"),
    ("pdfrum-page.txt", "pdfrum_page::PageObject", "PageObject"),
    ("pdfrum-page.txt", "pdfrum_page::Rotation", "PageRotation"),
    (
        "pdfrum-page.txt",
        "pdfrum_page::TextRenderMode",
        "TextRenderMode",
    ),
    ("pdfrum-parser.txt", "pdfrum_parser::Error", "ReadError"),
    ("pdfrum-parser.txt", "pdfrum_parser::LoadError", "OpenError"),
    ("pdfrum-render.txt", "pdfrum_render::ColorMode", "ColorMode"),
    ("pdfrum-render.txt", "pdfrum_render::Error", "RenderError"),
    ("pdfrum-render.txt", "pdfrum_render::TextAa", "TextAa"),
    ("pdfrum-text.txt", "pdfrum_text::Error", "TextError"),
];

fn argb() -> Argb {
    Argb {
        a: 0,
        r: 0,
        g: 0,
        b: 0,
    }
}

fn generated_ap() -> GeneratedAp {
    GeneratedAp {
        stream: Vec::new(),
        bbox: Rect::new(0.0, 0.0, 1.0, 1.0),
        matrix: Affine::IDENTITY,
        resources: Dict::new(),
        rect_override: None,
        as_override: None,
    }
}

fn keystroke() -> Keystroke {
    Keystroke {
        change: String::new(),
        value: String::new(),
        selection_start: 0,
        selection_end: 0,
    }
}

fn origin() -> Point {
    Point::new(0.0, 0.0)
}

/// Construct every default-feature variant. Returns how many were written.
fn construct_default_feature_variants() -> usize {
    let mut n = 0;

    // pdfrum::Error — 8
    let _ = Error::WrongPassword;
    let _ = Error::Open(OpenError::NotPdf);
    let _ = Error::Read(ReadError::NoCatalog);
    let _ = Error::Render(RenderError::TargetEmpty {
        width: 0,
        height: 0,
    });
    let _ = Error::Doc(DocError::NoCatalog);
    let _ = Error::Save(SaveError::BadPageRange);
    let _ = Error::Text(TextError::CharIndexOutOfRange {
        index: CharIndex::new(0),
        len: 0,
    });
    let _ = Error::Io(std::io::Error::other("construct"));
    n += 8;

    // pdfrum::Rotation — 4
    let _ = Rotation::None;
    let _ = Rotation::Quarter;
    let _ = Rotation::Half;
    let _ = Rotation::ThreeQuarter;
    n += 4;

    // pdfrum::Update — 2
    let _ = Update::Incremental;
    let _ = Update::Rewrite;
    n += 2;

    // pdfrum::FlattenMode — 2, pdfrum::Flattened — 2
    let _ = FlattenMode::Display;
    let _ = FlattenMode::Print;
    let _ = Flattened::Done;
    let _ = Flattened::NothingToDo;
    n += 4;

    // DiagKind — 102. Two variants carry a `u32` count.
    let diag: &[DiagKind] = &[
        DiagKind::AnnotSubtypeUnknown,
        DiagKind::AppearanceGenerated,
        DiagKind::BadStartXref,
        DiagKind::BadTextRenderMode,
        DiagKind::CMapCodespaceDropped,
        DiagKind::CMapNameUnknown,
        DiagKind::CMapOperandOverflow,
        DiagKind::CMapRangeLimit,
        DiagKind::CMapReversedRange,
        DiagKind::CMapTableMissing,
        DiagKind::CMapTruncatedCodespace,
        DiagKind::CMapUsecmapDepth,
        DiagKind::CMapUsecmapUnknown,
        DiagKind::CMapWideMappingsDropped,
        DiagKind::CidToGidStreamShort,
        DiagKind::ColorKeyArrayShort,
        DiagKind::ColorSpaceUnsupported,
        DiagKind::DashElementClamped,
        DiagKind::DashPatternDropped,
        DiagKind::DefaultAppearanceMalformed,
        DiagKind::DestPageUnresolved,
        DiagKind::FieldSkippedNoName,
        DiagKind::FieldSkippedNoType,
        DiagKind::FontProgramUnreadable,
        DiagKind::FontSubstitutionFailed,
        DiagKind::FontWidthsTruncated,
        DiagKind::FormRecursionRefused,
        DiagKind::FormResourcesInvalid,
        DiagKind::FunctionUnsupported,
        DiagKind::GsubUnreadable,
        DiagKind::HeaderOffset,
        DiagKind::IccAlternateMismatch,
        DiagKind::IccAlternateUsed,
        DiagKind::IccStockFallback,
        DiagKind::ImageBadBitDepth,
        DiagKind::ImageBadDimensions,
        DiagKind::ImageDecodeFailed,
        DiagKind::ImageDimensionsFromCodec,
        DiagKind::ImageStreamTruncated,
        DiagKind::IndexedHivalClamped,
        DiagKind::InkPathDropped,
        DiagKind::InlineImageAbandoned,
        DiagKind::InlineImageResync,
        DiagKind::InlineImageUnsupported,
        DiagKind::JpxColorSpaceOverride,
        DiagKind::KeywordResync,
        DiagKind::LegacyNamedDest,
        DiagKind::LengthMismatch,
        DiagKind::MalformedArray,
        DiagKind::MalformedDict,
        DiagKind::MaskDropped,
        DiagKind::MediaBoxDefaulted,
        DiagKind::MeshDecodeMalformed,
        DiagKind::MeshTruncated,
        DiagKind::NameTreeLimitsRepaired,
        DiagKind::NameTreeMalformed,
        DiagKind::NavigationCycle,
        DiagKind::ObjNumMismatch,
        DiagKind::ObjStmEntryDropped,
        DiagKind::OperandCountMismatch,
        DiagKind::OperandsDropped,
        DiagKind::OptionalContentPolicyUnknown,
        DiagKind::PageLabelStyleUnknown,
        DiagKind::PageTreeDepthExceeded,
        DiagKind::PageTreeRepaired,
        DiagKind::PasswordReencoded,
        DiagKind::PostScriptMalformedProc,
        DiagKind::PostScriptStackAbuse,
        DiagKind::QuadPointsTruncated,
        DiagKind::RootRecovered,
        DiagKind::ScriptFailed,
        DiagKind::ScriptLimitReached,
        DiagKind::ShadingUnsupported,
        DiagKind::StreamInCompositeDropped,
        DiagKind::StructElementDropped,
        DiagKind::TextActualTextCharDropped,
        DiagKind::TextActualTextUnprintable,
        DiagKind::TextCharcodeZero,
        DiagKind::TextCharcodesUnmapped(0),
        DiagKind::TextCharsDeduplicated(0),
        DiagKind::TextHyphenNoPrevChar,
        DiagKind::TextObjectDegenerate,
        DiagKind::TextObjectDropped,
        DiagKind::TextObjectDuplicate,
        DiagKind::TilingRangeOverflow,
        DiagKind::TilingStepInvalid,
        DiagKind::TintTransformDropped,
        DiagKind::ToUnicodeBlockRejected,
        DiagKind::TreeDepthExceeded,
        DiagKind::Type1BlendInconsistent,
        DiagKind::Type1CharstringAborted,
        DiagKind::Type1EncodingGlyphMissing,
        DiagKind::Type1HexTruncated,
        DiagKind::Type1PfbTruncated,
        DiagKind::UnbalancedMarkedContent,
        DiagKind::UnbalancedRestore,
        DiagKind::UndecodableStream,
        DiagKind::UnknownOperator,
        DiagKind::XrefEntriesShifted,
        DiagKind::XrefPrevLoop,
        DiagKind::XrefRebuilt,
        DiagKind::XrefStreamEntryDropped,
    ];
    n += diag.len();

    let _ = Severity::Recovered;
    let _ = Severity::Suspicious;
    n += 2;

    let subtypes: &[Subtype] = &[
        Subtype::Caret,
        Subtype::Circle,
        Subtype::FileAttachment,
        Subtype::FreeText,
        Subtype::Highlight,
        Subtype::Ink,
        Subtype::Line,
        Subtype::Link,
        Subtype::Movie,
        Subtype::PolyLine,
        Subtype::Polygon,
        Subtype::Popup,
        Subtype::PrinterMark,
        Subtype::Redact,
        Subtype::RichMedia,
        Subtype::Screen,
        Subtype::Sound,
        Subtype::Square,
        Subtype::Squiggly,
        Subtype::Stamp,
        Subtype::StrikeOut,
        Subtype::Text,
        Subtype::ThreeD,
        Subtype::TrapNet,
        Subtype::Underline,
        Subtype::Unknown,
        Subtype::Watermark,
        Subtype::Widget,
        Subtype::XfaWidget,
    ];
    n += subtypes.len();

    let _ = FocusBox::None;
    let _ = FocusBox::Inflated;
    let _ = FocusBox::Rect(Rect::new(0.0, 0.0, 1.0, 1.0));
    n += 3;

    let _ = FieldKind::Button;
    let _ = FieldKind::Check;
    let _ = FieldKind::Combo;
    let _ = FieldKind::List;
    let _ = FieldKind::Radio;
    let _ = FieldKind::Signature;
    let _ = FieldKind::Text;
    n += 7;

    let kinds: &[ActionKind] = &[
        ActionKind::GoTo,
        ActionKind::GoTo3DView,
        ActionKind::GoToE,
        ActionKind::GoToR,
        ActionKind::Hide,
        ActionKind::ImportData,
        ActionKind::JavaScript,
        ActionKind::Launch,
        ActionKind::Movie,
        ActionKind::Named,
        ActionKind::Rendition,
        ActionKind::ResetForm,
        ActionKind::SetOcgState,
        ActionKind::Sound,
        ActionKind::SubmitForm,
        ActionKind::Thread,
        ActionKind::Trans,
        ActionKind::Unknown,
        ActionKind::Uri,
    ];
    n += kinds.len();

    let _ = DocError::NoCatalog;
    let _ = DocError::Unresolved(ObjRef::new(1, 0));
    n += 2;

    let _ = SaveError::BadNupParams;
    let _ = SaveError::BadPageRange;
    let _ = SaveError::EncryptedSaveUnsupported;
    let _ = SaveError::Io(std::io::Error::other("construct"));
    let _ = SaveError::NoDestinationCatalog;
    let _ = SaveError::Object(ObjectError::UnresolvedRef(ObjRef::new(1, 0)));
    let _ = SaveError::PageIndexOutOfRange(PageIndex::from(0u32));
    let _ = SaveError::Subset(String::new());
    let _ = SaveError::UnrecognisedFontProgram;
    let _ = SaveError::EmptyFontProgram;
    let _ = SaveError::EmptyToUnicodeCMap;
    let _ = SaveError::BadCidToGidMap(3);
    let _ = SaveError::UnrecognisedImageData;
    let _ = SaveError::EmptyImage;
    let _ = SaveError::ImageDataLength {
        expected: 0,
        found: 0,
    };
    n += 15;

    let _ = FontEncoding::Simple;
    let _ = FontEncoding::Composite;
    n += 2;

    let _ = PixelFormat::Gray8;
    let _ = PixelFormat::Rgb8;
    let _ = PixelFormat::Cmyk8;
    let _ = PixelFormat::Rgba8;
    let _ = PixelFormat::Mask1;
    n += 5;

    let _ = Button::Left;
    let _ = Button::Right;
    n += 2;

    let none = Modifiers::NONE;
    let at = origin();
    let _ = Event::MouseMove {
        at,
        modifiers: none,
    };
    let _ = Event::MouseDown {
        button: Button::Left,
        at,
        modifiers: none,
    };
    let _ = Event::MouseUp {
        button: Button::Left,
        at,
        modifiers: none,
    };
    let _ = Event::DoubleClick {
        at,
        modifiers: none,
    };
    let _ = Event::MouseWheel {
        at,
        delta: (0, 0),
        modifiers: none,
    };
    let _ = Event::Focus {
        at,
        modifiers: none,
    };
    let _ = Event::KeyDown {
        key: Key::Tab,
        modifiers: none,
    };
    let _ = Event::Char {
        ch: 'a',
        modifiers: none,
    };
    n += 8;

    let keys: &[Key] = &[
        Key::A,
        Key::Backspace,
        Key::Control,
        Key::Delete,
        Key::Down,
        Key::End,
        Key::Escape,
        Key::Home,
        Key::Insert,
        Key::Left,
        Key::Newline,
        Key::Other(0),
        Key::PageDown,
        Key::PageUp,
        Key::Return,
        Key::Right,
        Key::Shift,
        Key::Space,
        Key::Tab,
        Key::Unknown,
        Key::Up,
        Key::Y,
        Key::Z,
    ];
    n += keys.len();

    let _ = KeystrokeOutcome::Accept(keystroke());
    let _ = KeystrokeOutcome::Reject;
    n += 2;

    let _ = Placement::Above;
    let _ = Placement::Below;
    n += 2;

    let _ = UpdateKind::ActionRequested {
        action: Box::new(Action::new(Dict::new())),
        modifiers: none,
    };
    let _ = UpdateKind::FocusChanged {
        from: None,
        to: None,
    };
    let _ = UpdateKind::LiveEdit(Box::new(generated_ap()));
    let _ = UpdateKind::Regenerated(Box::new(generated_ap()));
    let _ = UpdateKind::RevertedToFileAppearance;
    n += 5;

    let _ = ObjectError::RefLoop(ObjRef::new(1, 0));
    let _ = ObjectError::SpanOutOfBounds {
        start: 0,
        end: 0,
        len: 0,
    };
    let _ = ObjectError::UnresolvedRef(ObjRef::new(1, 0));
    n += 3;

    // Object — 10 variants. Four payloads (`Name`, `PdfString`, `Array`,
    // `Stream`) are pdfrum-object types this crate does not re-export; they
    // are the object-store escape hatch (WP7). The six nameable variants are
    // constructed; an exhaustive match names every variant so a new one
    // fails to compile.
    let objects: &[Object] = &[
        Object::Null,
        Object::Bool(false),
        Object::Int(0),
        Object::Real(0.0),
        Object::Dict(Dict::new()),
        Object::Ref(ObjRef::new(1, 0)),
    ];
    fn every_object_variant(obj: &Object) {
        match obj {
            Object::Array(_)
            | Object::Bool(_)
            | Object::Dict(_)
            | Object::Int(_)
            | Object::Name(_)
            | Object::Null
            | Object::Real(_)
            | Object::Ref(_)
            | Object::Str(_)
            | Object::Stream(_) => {}
        }
    }
    for o in objects {
        every_object_variant(o);
    }
    n += 10;

    // PageObject — 5 variants. Path/Text/Image are what the facade builders
    // produce; Form and Shading have no facade constructor (they are parsed,
    // not created). An exhaustive match names every variant.
    let path = PathBuilder::rect(Rect::new(0.0, 0.0, 1.0, 1.0)).build();
    let text = TextBuilder::new(b"a".as_slice(), ObjRef::new(1, 0), 12.0).build();
    let image = ImageBuilder::at(ObjRef::new(1, 0), Rect::new(0.0, 0.0, 1.0, 1.0)).build();
    fn every_page_object_variant(obj: &PageObject) {
        match obj {
            PageObject::Form(_)
            | PageObject::Image(_)
            | PageObject::Path(_)
            | PageObject::Shading(_)
            | PageObject::Text(_) => {}
        }
    }
    every_page_object_variant(&path);
    every_page_object_variant(&text);
    every_page_object_variant(&image);
    n += 5;

    let _ = PageRotation::None;
    let _ = PageRotation::Quarter;
    let _ = PageRotation::Half;
    let _ = PageRotation::ThreeQuarter;
    n += 4;

    let modes: &[TextRenderMode] = &[
        TextRenderMode::Fill,
        TextRenderMode::Stroke,
        TextRenderMode::FillStroke,
        TextRenderMode::Invisible,
        TextRenderMode::FillClip,
        TextRenderMode::StrokeClip,
        TextRenderMode::FillStrokeClip,
        TextRenderMode::Clip,
    ];
    n += modes.len();

    let _ = ReadError::Cycle(ObjRef::new(1, 0));
    let _ = ReadError::NoCatalog;
    let _ = ReadError::NoObject(0);
    let _ = ReadError::NoPage(PageIndex::from(0u32));
    let _ = ReadError::TooDeep(0);
    let _ = ReadError::Unresolved(ObjRef::new(1, 0));
    let _ = ReadError::XrefBroken;
    n += 7;

    let _ = OpenError::Broken(String::new());
    let _ = OpenError::NotPdf;
    let _ = OpenError::UnsupportedEncryption(String::new());
    let _ = OpenError::WrongPassword;
    n += 4;

    let _ = ColorMode::Alpha;
    let _ = ColorMode::Forced(ColorScheme {
        path_fill: argb(),
        path_stroke: argb(),
        text_fill: argb(),
        text_stroke: argb(),
    });
    let _ = ColorMode::Gray;
    let _ = ColorMode::Normal;
    n += 4;

    let _ = RenderError::TargetEmpty {
        width: 0,
        height: 0,
    };
    let _ = RenderError::TargetTooLarge {
        width: 0,
        height: 0,
        limit: 0,
    };
    n += 2;

    let _ = TextAa::Grayscale;
    let _ = TextAa::LcdSubpixel;
    let _ = TextAa::None;
    n += 3;

    let _ = TextError::CharIndexOutOfRange {
        index: CharIndex::new(0),
        len: 0,
    };
    n += 1;

    n
}

#[cfg(feature = "javascript")]
fn construct_script_feature_variants() -> usize {
    let _ = ScriptStop::LimitReached;
    let _ = ScriptStop::Threw(String::new());
    let lines: &[TranscriptLine] = &[
        TranscriptLine::Alert {
            title: String::new(),
            message: String::new(),
            icon: 0,
            button: 0,
        },
        TranscriptLine::Beep(0),
        TranscriptLine::Response {
            question: String::new(),
            title: String::new(),
            default_value: String::new(),
            label: String::new(),
            password: false,
        },
        TranscriptLine::MailMsg {
            ui: false,
            to: String::new(),
            cc: String::new(),
            bcc: String::new(),
            subject: String::new(),
            body: String::new(),
        },
        TranscriptLine::Print {
            ui: false,
            start: 0,
            end: 0,
            silent: false,
            shrink_to_fit: false,
            print_as_image: false,
            reverse: false,
            annotations: false,
        },
        TranscriptLine::SubmitForm {
            url: String::new(),
            data: Vec::new(),
        },
        TranscriptLine::GotoPage(0),
        TranscriptLine::NamedAction(String::new()),
        TranscriptLine::ConsolePrintln(String::new()),
        TranscriptLine::FunctionAlert {
            caller: String::new(),
            message: String::new(),
        },
    ];
    2 + lines.len()
}

#[allow(clippy::expect_used)]
fn snapshot_variant_count() -> usize {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut total = 0;
    for &(file, path, _) in SNAPSHOT_ENUMS {
        let text = std::fs::read_to_string(root.join("docs/status/api-baseline").join(file))
            .expect("api-baseline snapshot is readable");
        let prefix = format!("pub {path}::");
        let mut collecting = false;
        for line in text.lines() {
            if line.contains(&format!("pub enum {path}")) {
                collecting = true;
                continue;
            }
            if !collecting {
                continue;
            }
            if let Some(rest) = line.strip_prefix(&prefix) {
                if !rest.split_once("::").is_some_and(|(head, _)| {
                    head.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                }) {
                    total += 1;
                }
                continue;
            }
            if line.contains("pub enum ")
                || line.starts_with("pub struct ")
                || line.starts_with("impl ")
                || line.starts_with("pub fn ")
                || line.starts_with("pub const ")
                || line.starts_with("pub type ")
                || line.starts_with("pub mod ")
                || line.starts_with("pub use ")
            {
                collecting = false;
            }
        }
    }
    total
}

#[test]
fn every_public_enum_variant_is_constructible_from_the_facade() {
    let constructed = construct_default_feature_variants();
    let derived = snapshot_variant_count();
    assert_eq!(
        constructed, derived,
        "constructed {constructed} default-feature variants, snapshots derive {derived}; \
         SNAPSHOT_ENUMS is the derivation index — add a construction when a variant lands"
    );
    assert_eq!(constructed, 297, "default-feature variant count");
    assert_eq!(SNAPSHOT_ENUMS.len(), 32, "default-feature enum count");
}

#[test]
#[cfg(feature = "javascript")]
fn every_script_enum_variant_is_constructible_from_the_facade() {
    // The two enums `pdfrum+javascript.txt` adds as `pub use` over `pdfrum.txt`.
    // Their variant lines are not in the facade snapshots (cargo-public-api
    // does not expand re-exported enums there); the constructions are the
    // variants of `ScriptStop` and `TranscriptLine`.
    assert_eq!(construct_script_feature_variants(), 12);
}
