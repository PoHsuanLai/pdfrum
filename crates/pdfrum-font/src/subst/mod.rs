//! Substitution: choosing a face when the document did not supply one.
//!
//! One decision, in four pieces: a request record, a decision record, a pure
//! function between them, and a database seam. The ladder inside that
//! function stays whole and stays in order, because which rung fires first
//! *is* the result.

// The shape is a decomposition of `CFX_FontMapper::FindSubstFace`, a
// thousand-line class whose only real job is that one decision.
mod charset;
mod db;
// Reading only the tables a directory scan needs, rather than each font file
// whole. Filesystem work, so it exists only where the scan does.
#[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
mod probe;
mod standard;
mod style;
mod substfont;
// The table doc comments name their C++ source files, which read as
// identifiers to clippy; the file is machine-generated, so the fix belongs in
// the extractor, not here.
#[allow(clippy::doc_markdown)]
mod tables;

pub use charset::{Charset, charset_from_unicode};
pub(crate) use charset::{CodePage, PitchFamily};
#[cfg(test)]
pub(crate) use db::FaceInfo;
pub(crate) use db::{CroscoreDb, FaceHandle, FontDb, SystemFontDb, TestFontDb};
#[cfg(test)]
pub(crate) use standard::ALL_STANDARD_FONTS;
pub use standard::StandardFont;
pub use standard::canonical_font_name;
pub(crate) use standard::{standard_font_data, standard_font_index};
pub(crate) use style::{
    NARROW_FAMILY, font_family, is_narrow_font_name, parse_styles, strip_subset_prefix, style_bits,
    style_type, subst_name, tt_normalize,
};
pub use substfont::SubstFont;
pub(crate) use substfont::{GlyphSpacingGate, applies_glyph_spacing};

use crate::FontFlags;
use crate::glyphs::{Face, GlyphSource};
use pdfrum_common::{DiagKind, Diagnostics, Severity};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// What a font wants from substitution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontRequest {
    /// The `/BaseFont` name, before any normalization.
    pub name: Vec<u8>,
    /// Whether the font dictionary said `/TrueType`, which changes how the
    /// name is normalized and which charmaps are preferred.
    pub is_truetype: bool,
    /// The descriptor's `/Flags`.
    pub flags: FontFlags,
    /// The requested weight.
    pub weight: i32,
    /// The requested italic angle.
    pub italic_angle: i32,
    /// The code page a CID collection implies, or `DefAnsi`.
    pub code_page: CodePage,
    /// Whether the font writes vertically.
    pub vertical: bool,
}

impl Default for FontRequest {
    fn default() -> Self {
        Self {
            name: Vec::new(),
            is_truetype: false,
            flags: FontFlags::DEFAULT,
            weight: 400,
            italic_angle: 0,
            code_page: CodePage::DefAnsi,
            vertical: false,
        }
    }
}

/// How substitution finds faces.
#[derive(Debug, Clone, Default)]
pub struct SubstitutionOptions {
    /// Whether the *font-database* weight rule applies rather than the
    /// enumeration one.
    ///
    /// PDFium guards Branch A's weight reset on an internal flag it sets in
    /// its "version 2" mode. `fontdb` **is** that mode — there is no
    /// enumeration callback, we query directly — so `true` is the
    /// architecturally honest value and bold and light variants survive into
    /// the query.
    ///
    /// The default is **`false`**, because that is the behaviour the
    /// conformance corpus was rendered under — an enumerating font info,
    /// where the weight reset applies. Set it to `true` for the modern
    /// behaviour.
    pub skip_font_enumeration: bool,
    /// Directories to scan instead of the system's, for a hermetic run.
    pub font_dirs: Vec<PathBuf>,
    /// Whether the *family a database lookup asks for* is rewritten to its
    /// Croscore equivalent (`--croscore-font-names`).
    ///
    /// The oracle's `test_fonts` directory holds no Arial, Times or Courier:
    /// it holds the metric-compatible Arimo, Tinos and Cousine, so a hermetic
    /// run needs the rewrite or a `/BaseFont /Helvetica` looks for a face that
    /// is not there and falls through to the built-ins with different metrics.
    ///
    /// The rewrite sits at the database boundary — the family a lookup asks
    /// for is renamed, not the `/BaseFont` the ladder starts from — so the
    /// whole name/style/base-14 analysis runs on the document's own spelling
    /// first.
    pub croscore_font_names: bool,
    /// Whether an empty [`font_dirs`](Self::font_dirs) means *the system's own
    /// font directories* rather than *no directories at all*.
    ///
    /// A Linux reference run always enumerates the system's fonts —
    /// `/usr/share/fonts`, `/usr/share/X11/fonts/Type1`,
    /// `/usr/share/X11/fonts/TTF` and `/usr/local/share/fonts` — so a
    /// `--font-dir` *replaces* that search path rather than *enabling* it.
    ///
    /// The default is **`false`**, because a library whose output depends on
    /// which fonts happen to be installed is not testable and every unit test
    /// in the tree is written against the built-in faces. A host that wants
    /// the oracle's behaviour — `pdfrum-tool` invoked without `--font-dir`
    /// does — sets it to `true`.
    ///
    /// It has no effect when `font_dirs` is non-empty: those directories are
    /// then the whole search path either way, which is why every conformance
    /// invocation (which always passes `--font-dir`) is unaffected by it.
    // Where the four directories come from: `pdfium_test` leaves
    // `config.m_pUserFontPaths` null unless `--font-dir` was given
    // (`testing/pdfium_test/pdfium_test.cc:2107-2112`), and a null path list
    // makes `CFX_LinuxFontInfo` add exactly those four
    // (`core/fxge/linux/fx_linux_impl.cpp:173-176`).
    pub system_fonts: bool,
}

/// Rewrite a family name to its Croscore equivalent.
///
/// Three families map, by substring and in this order; everything else is
/// returned unchanged, which is deliberate — some fixtures want the built-in
/// fallback and reaching it depends on *not* matching here.
///
/// The style suffixes are appended from the *same* string, so a family the
/// ladder resolved to `Helvetica-Bold` reaches the database as `Arimo Bold`
/// and a bold face is what comes back.
#[must_use]
pub fn croscore_name(face: &str) -> String {
    let has = |needle: &str| face.contains(needle);
    let base = if has("Arial") || has("Calibri") || has("Helvetica") {
        "Arimo"
    } else if face.is_empty() || has("Times") {
        "Tinos"
    } else if has("Courier") {
        "Cousine"
    } else {
        return face.to_owned();
    };

    let mut out = base.to_owned();
    // Both suffixes can apply, and in this order.
    if has("Bold") {
        out.push_str(" Bold");
    }
    if has("Italic") || has("Oblique") {
        out.push_str(" Italic");
    }
    out
}

/// What substitution decided.
pub struct Substitution {
    /// The face to draw with. Always `Some` unless even the built-in fallback
    /// failed to parse.
    pub glyphs: GlyphSource,
    /// The synthetic adjustments that follow from the choice.
    pub subst: SubstFont,
    /// The standard font this resolved to, when it resolved to one.
    #[cfg(test)]
    pub standard: Option<StandardFont>,
}

/// Resolve a font request to a face (`FindSubstFace`).
///
/// The ladder has five rungs and always ends in something drawable: the last
/// rung is one of the two built-in Multiple-Master faces, which are compiled
/// in and cannot be missing. `glyphs` is `GlyphSource::None` only if even that
/// failed to parse, which would mean a corrupted build.
#[must_use]
pub fn resolve(
    req: &FontRequest,
    db: &impl FontDb,
    opts: &SubstitutionOptions,
    diags: &mut Diagnostics,
) -> Substitution {
    if opts.croscore_font_names {
        return resolve_inner(req, &CroscoreDb::new(db), opts, diags, false);
    }
    resolve_inner(req, db, opts, diags, false)
}

/// Run the ladder against whichever database `opts` selects.
///
/// Three databases are reachable and the choice is entirely
/// [`SubstitutionOptions`]'s:
///
/// - **named directories** ([`font_dirs`](SubstitutionOptions::font_dirs) is
///   non-empty) — those directories and nothing else, which is what
///   `--font-dir` asks for and what every conformance run uses;
/// - **the system's directories** (`font_dirs` empty and
///   [`system_fonts`](SubstitutionOptions::system_fonts) set) — the oracle's
///   own default on Linux, where `--font-dir` merely *replaces* a search path
///   that is otherwise `/usr/share/fonts` and friends;
/// - **the built-in faces alone** (both unset) — the hermetic default, so a
///   unit test's answer does not depend on what is installed on the machine.
///
/// The host scan is cached for the process; a `--font-dir` scan is not. A
/// document whose fonts are all embedded never reaches here at all.
#[must_use]
pub fn resolve_with_options(
    req: &FontRequest,
    opts: &SubstitutionOptions,
    diags: &mut Diagnostics,
) -> Substitution {
    if opts.font_dirs.is_empty() && !opts.system_fonts {
        return resolve(req, &TestFontDb::new(), opts, diags);
    }
    let db = scanned(ScanKey::of(&opts.font_dirs));
    resolve(req, db.as_ref(), opts, diags)
}

/// Which directories a scan covers — the whole identity of its result.
///
/// A scan is a pure function of this key: `fontdb` enumerates exactly these
/// directories (or, for [`ScanKey::System`], the host's own four), and
/// nothing else about the process changes what it finds. That is what makes
/// [`scanned`] safe to memoize on it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ScanKey {
    /// The host's own font directories — an empty `font_dirs`.
    System,
    /// Exactly these directories, in the order given, and nothing else.
    Dirs(Vec<PathBuf>),
}

impl ScanKey {
    fn of(dirs: &[PathBuf]) -> Self {
        if dirs.is_empty() {
            Self::System
        } else {
            Self::Dirs(dirs.to_vec())
        }
    }

    fn dirs(&self) -> &[PathBuf] {
        match self {
            Self::System => &[],
            Self::Dirs(dirs) => dirs,
        }
    }
}

/// The database for `key`, scanned at most once per process.
///
/// Font directories do not change under a running process, and a scan reads
/// every face they hold to describe it — 0.2 s on a host with 1 183 faces,
/// and on the oracle's hermetic `test_fonts` 33 MB of reads costing ~12 ms of
/// kernel time.
///
/// The measured corpus does not exercise the second scan — all 22 of the 44
/// benchmark files that substitute at all substitute exactly once — so this
/// bounds a per-font cost to per-process. Memoizing on [`ScanKey`] cannot
/// change an answer, because the scan is a pure function of the key.
fn scanned(key: ScanKey) -> Arc<SystemFontDb> {
    static SCANS: OnceLock<Mutex<HashMap<ScanKey, Arc<SystemFontDb>>>> = OnceLock::new();
    let scans = SCANS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = scans.lock()
        && let Some(db) = cache.get(&key)
    {
        return Arc::clone(db);
    }
    // Scanned outside the lock, so one slow scan does not block a thread
    // asking for a different directory set. Two threads racing the same key
    // both scan and then agree on whichever result landed first, which is
    // sound because the scan is a pure function of the key.
    let db = Arc::new(SystemFontDb::scan(key.dirs()));
    match scans.lock() {
        Ok(mut cache) => Arc::clone(cache.entry(key).or_insert(db)),
        Err(_) => db,
    }
}

// The ladder is one ordered sequence: every step reads state the steps above
// it left behind, and which rung fires first *is* the result. Splitting it into
// helpers would hide that order behind call sites, so it stays whole.
#[allow(clippy::too_many_lines)]
fn resolve_inner(
    req: &FontRequest,
    db: &impl FontDb,
    opts: &SubstitutionOptions,
    diags: &mut Diagnostics,
    retried: bool,
) -> Substitution {
    let mut subst = SubstFont::default();

    // Step 0 — normalize. Without `USE_EXTERN_ATTR` the caller's weight and
    // slant are *discarded entirely*, which is why that flag's five-term
    // conjunction in §1.2 matters so much.
    let mut weight = if req.weight == 0 { 400 } else { req.weight };
    let mut italic_angle = req.italic_angle;
    if !req.flags.uses_extern_attr() {
        weight = 400;
        italic_angle = 0;
    }

    // Step 1 — the name.
    let name = subst_name(&req.name, req.is_truetype);

    // Step 2 — the two symbolic short-circuits. Note `ZapfDingbats` has no
    // TrueType condition while `Symbol` does.
    if name == b"Symbol" && !req.is_truetype {
        "Chrome Symbol".clone_into(&mut subst.family);
        subst.charset = Charset::Symbol;
        return terminal(
            Some(StandardFont::Symbol),
            weight,
            italic_angle,
            PitchFamily::default(),
            subst,
            diags,
        );
    }
    if name == b"ZapfDingbats" {
        "Chrome Dingbats".clone_into(&mut subst.family);
        subst.charset = Charset::Symbol;
        return terminal(
            Some(StandardFont::Dingbats),
            weight,
            italic_angle,
            PitchFamily::default(),
            subst,
            diags,
        );
    }

    // Step 3 — split at the first comma.
    let (mut family, style_str, has_comma) = style::split_style(&name);
    let std_font = if has_comma {
        standard_font_index(&family)
    } else {
        standard_font_index(&name)
    };

    // Step 4 — derive the style. A base-14 font skips name parsing entirely
    // and reads both style and pitch off its index.
    let mut has_hyphen = false;
    let (mut n_style, pitch_family, mut base_font) =
        if let Some(sf) = std_font.filter(|f| f.index() < 12) {
            (
                style_from_standard_font(sf),
                PitchFamily::from_standard_font(sf),
                Some(sf),
            )
        } else {
            let mut style_out = style_bits::NORMAL;
            let mut style_str = style_str.clone();
            if !has_comma {
                // The *last* hyphen, not the first.
                if let Some(p) = family.iter().rposition(|c| *c == b'-') {
                    style_str = family.get(p + 1..).unwrap_or_default().to_vec();
                    family.truncate(p);
                    has_hyphen = true;
                }
            }
            if !has_hyphen
                && let Some(sr) = std::str::from_utf8(&family)
                    .ok()
                    .and_then(|f| style_type(f, true))
            {
                family.truncate(family.len().saturating_sub(sr.name.len()));
                style_out |= sr.style;
            }
            let _ = style_str;
            (style_out, PitchFamily::from_flags(req.flags), None)
        };

    // Step 5 — bold inference. `old_weight` is the *pre-inference* value, and
    // which of the two every downstream call passes is behavior: the internal
    // rungs take the old one, the external rungs the new one.
    let old_weight = weight;
    if n_style & style_bits::FORCE_BOLD != 0 {
        weight = 700;
    }

    // Step 6 — the style suffix, which may abort the whole parse.
    let style_source = if has_comma {
        style_str
    } else {
        suffix_after_hyphen(&name, has_hyphen)
    };
    let parsed = parse_styles(&style_source, weight, n_style);
    let mut is_style_available = parsed.is_style_available;
    if parsed.abort {
        family.clone_from(&name);
        base_font = None;
    } else {
        weight = parsed.weight;
        n_style = parsed.style;
    }

    // Step 7 — with no database at all, go straight to the built-ins.
    if db.faces().is_empty() {
        return terminal(
            base_font,
            old_weight,
            italic_angle,
            pitch_family,
            subst,
            diags,
        );
    }

    // Step 8 — charset and family rewriting.
    let charset = request_charset(req.code_page, base_font, req.flags);
    let is_cjk = charset.is_cjk();
    let mut is_italic = n_style & style_bits::ITALIC != 0;
    let family_str = String::from_utf8_lossy(&family).into_owned();
    let mut family_str = match font_family(n_style, &family_str) {
        Some(f) => f.to_owned(),
        None => family_str,
    };

    // Step 9 — installed-name matching, with a second, looser attempt.
    let name_str = String::from_utf8_lossy(&name).into_owned();
    let mut matched = db.match_installed(&tt_normalize(&family_str));
    if matched.is_none()
        && family_str != name_str
        && !has_comma
        && (!has_hyphen || !is_style_available)
    {
        matched = db.match_installed(&tt_normalize(&name_str));
    }

    // Step 10 — the two branches.
    let mut pitch_family = pitch_family;
    if matched.is_none() && base_font.is_none() {
        if is_cjk {
            subst.subst_cjk = true;
            if n_style != 0 {
                subst.weight_cjk = Some(weight);
            }
            if n_style & style_bits::ITALIC != 0 {
                subst.italic_cjk = true;
            }
        } else {
            if style::is_third_party_font(&family_str) {
                pitch_family = PitchFamily(pitch_family.0 & !PitchFamily::ROMAN);
            } else {
                // The italic decision is *overridden* by the angle here.
                is_italic = italic_angle != 0;
                if !opts.skip_font_enumeration {
                    weight = old_weight;
                }
            }
            if is_narrow_font_name(&name_str) {
                NARROW_FAMILY.clone_into(&mut family_str);
            }
        }
        // The PDF's own italic flag can still force it on.
        if req.flags.is_italic() {
            is_italic = true;
        }
    } else {
        italic_angle = 0;
        // `[oracle-bug]` `cfx_fontmapper.cpp:644` asks `nStyle ==
        // kFontStyleNormal`, which conflates "has no style" with "has no
        // *bold*": an italic standard face such as `Helvetica-Oblique` skips
        // the reset and keeps the requested 700, where Annex D makes it a
        // regular-weight face. The test the reset needs is "not force-bold";
        // pdf.js reaches 400 structurally, weight being a property of the
        // resolved face (`font_substitutions.js:32-35` `ITALIC = { style:
        // "italic", weight: "normal" }`, bound at `:129-133`).
        if n_style & style_bits::FORCE_BOLD == 0 {
            weight = 400;
        }
        if let Some(m) = &matched {
            family_str.clone_from(m);
        }
        if let Some(bf) = base_font {
            let adjusted = adjust_base_font_for_style(bf, n_style);
            base_font = Some(adjusted);
            canonical_font_name(adjusted).clone_into(&mut family_str);
        }
    }
    let _ = &mut is_style_available;

    // Step 11 — rung 1: ask the database.
    if let Some(h) = db.find_font(weight, is_italic, charset, pitch_family, &family_str, true)
        && let Some(s) = external(db, h, weight, is_italic, italic_angle, charset, &mut subst)
    {
        return Substitution {
            glyphs: s,
            subst,
            #[cfg(test)]
            standard: base_font,
        };
    }

    // Step 12 — rung 2: the exact installed name.
    if is_cjk {
        is_italic = italic_angle != 0;
        weight = old_weight;
    }
    if let Some(m) = &matched {
        return match db.font_by_name(m) {
            None => terminal(
                base_font,
                old_weight,
                italic_angle,
                pitch_family,
                subst,
                diags,
            ),
            Some(h) => {
                match external(db, h, weight, is_italic, italic_angle, charset, &mut subst) {
                    Some(s) => Substitution {
                        glyphs: s,
                        subst,
                        #[cfg(test)]
                        standard: base_font,
                    },
                    None => terminal(
                        base_font,
                        old_weight,
                        italic_angle,
                        pitch_family,
                        subst,
                        diags,
                    ),
                }
            }
        };
    }

    // Step 13 — rung 3: retry a symbolic request as a plain one, **once**.
    if charset == Charset::Symbol {
        if name == b"Symbol" {
            "Chrome Symbol".clone_into(&mut subst.family);
            subst.charset = Charset::Symbol;
            return terminal(
                Some(StandardFont::Symbol),
                old_weight,
                italic_angle,
                pitch_family,
                subst,
                diags,
            );
        }
        if !retried {
            // Dropping the symbolic bit makes `request_charset` return ANSI,
            // so this rung cannot be re-entered — but the flag makes that a
            // fact rather than an argument.
            let retry = FontRequest {
                name: family.clone(),
                flags: req.flags.without(FontFlags::SYMBOLIC),
                weight,
                italic_angle,
                code_page: CodePage::DefAnsi,
                ..req.clone()
            };
            return resolve_inner(&retry, db, opts, diags, true);
        }
    }

    // Step 14 — rung 4: an ANSI request that found nothing takes the built-ins.
    if charset == Charset::Ansi {
        return terminal(
            base_font,
            old_weight,
            italic_angle,
            pitch_family,
            subst,
            diags,
        );
    }

    // Step 15 — rung 5: any installed face claiming this charset, in
    // insertion order.
    let by_charset = db
        .faces()
        .iter()
        .position(|f| f.charsets.contains(&charset))
        .map(FaceHandle::from_index);
    match by_charset {
        None => terminal(
            base_font,
            old_weight,
            italic_angle,
            pitch_family,
            subst,
            diags,
        ),
        Some(h) => {
            if let Some(s) = external(db, h, weight, is_italic, italic_angle, charset, &mut subst) {
                Substitution {
                    glyphs: s,
                    subst,
                    #[cfg(test)]
                    standard: base_font,
                }
            } else {
                // The one place PDFium returns nothing at all.
                diags.record(Severity::Suspicious, DiagKind::FontSubstitutionFailed, None);
                Substitution {
                    glyphs: GlyphSource::None,
                    subst,
                    #[cfg(test)]
                    standard: base_font,
                }
            }
        }
    }
}

/// The style bits a base-14 index implies (`GetStyleFromBaseFont`).
///
/// Reads `index % 4` against the family layout Regular / Bold / BoldOblique /
/// Oblique — bold at positions 1 and 2, italic at 2 and 3.
#[must_use]
pub fn style_from_standard_font(f: StandardFont) -> u32 {
    let pos = f.index() % 4;
    let mut style = style_bits::NORMAL;
    if pos == 1 || pos == 2 {
        style |= style_bits::FORCE_BOLD;
    }
    if pos / 2 != 0 {
        style |= style_bits::ITALIC;
    }
    style
}

/// Apply a style to a base-14 index by arithmetic (`AdjustBaseFontForStyle`).
///
/// Only the three family heads can be styled; anything else is already a
/// styled member and is returned unchanged.
#[must_use]
pub fn adjust_base_font_for_style(base: StandardFont, style: u32) -> StandardFont {
    if style == style_bits::NORMAL || !base.is_stylable() {
        return base;
    }
    let bold = style & style_bits::FORCE_BOLD != 0;
    let italic = style & style_bits::ITALIC != 0;
    let offset = match (bold, italic) {
        (true, true) => 2,
        (true, false) => 1,
        (false, true) => 3,
        (false, false) => 0,
    };
    StandardFont::from_index(base.index() + offset).unwrap_or(base)
}

/// The charset a request is for (`GetCharset`).
#[must_use]
fn request_charset(cp: CodePage, base: Option<StandardFont>, flags: FontFlags) -> Charset {
    if cp != CodePage::DefAnsi {
        return Charset::from_code_page(cp);
    }
    // Symbolic *and* not a standard font: only then is it a symbol request.
    if flags.is_symbolic() && base.is_none() {
        return Charset::Symbol;
    }
    Charset::Ansi
}

/// The style suffix a hyphen split produced, recomputed rather than threaded
/// because the split is local to step 4.
fn suffix_after_hyphen(name: &[u8], has_hyphen: bool) -> Vec<u8> {
    if !has_hyphen {
        return Vec::new();
    }
    name.iter()
        .rposition(|c| *c == b'-')
        .and_then(|p| name.get(p + 1..))
        .unwrap_or_default()
        .to_vec()
}

/// Build a face from a database handle (`external_subst`).
fn external(
    db: &impl FontDb,
    h: FaceHandle,
    weight: i32,
    is_italic: bool,
    italic_angle: i32,
    charset: Charset,
    subst: &mut SubstFont,
) -> Option<GlyphSource> {
    let (bytes, index) = db.face_bytes(h)?;
    let face = Face::new(bytes, index)?;
    let info = db.faces().get(h.index())?;
    // A database that cannot name its own face falls back to the face's, which
    // is the `SetSubstFontNameWhenGetFaceNameFails` behavior.
    let name = if info.name.is_empty() {
        face.display_name().unwrap_or_default()
    } else {
        info.name.clone()
    };
    subst.configure_external(
        name,
        charset,
        weight,
        is_italic,
        italic_angle,
        info.styles & style_bits::FORCE_BOLD != 0,
        info.styles & style_bits::ITALIC != 0,
    );
    Some(GlyphSource::Fontations(face))
}

/// The terminal rung (`internal_subst`), itself two-level.
///
/// A resolved base-14 index takes the exact Foxit blob and **leaves the
/// substitution record untouched** — weight, angle and pitch are all ignored,
/// because the blob is already the right face. Anything else takes one of the
/// two Multiple-Master generics, which *do* record the weight, because their
/// design space is how the weight gets applied at all.
fn terminal(
    base_font: Option<StandardFont>,
    weight: i32,
    italic_angle: i32,
    pitch_family: PitchFamily,
    mut subst: SubstFont,
    diags: &mut Diagnostics,
) -> Substitution {
    if let Some(f) = base_font {
        let glyphs = builtin_standard(f);
        if !glyphs.is_some() {
            diags.record(Severity::Suspicious, DiagKind::FontSubstitutionFailed, None);
        }
        return Substitution {
            glyphs,
            subst,
            #[cfg(test)]
            standard: Some(f),
        };
    }

    subst.is_builtin_generic = true;
    subst.italic_angle = italic_angle;
    if weight != 0 {
        subst.weight = Some(weight);
    }
    let serif = pitch_family.has(PitchFamily::ROMAN);
    let (glyphs, family) = builtin_generic(serif);
    if serif {
        subst.use_chrome_serif();
    } else {
        family.clone_into(&mut subst.family);
    }
    if !glyphs.is_some() {
        diags.record(Severity::Suspicious, DiagKind::FontSubstitutionFailed, None);
    }
    Substitution {
        glyphs,
        subst,
        #[cfg(test)]
        standard: None,
    }
}

/// One of the fourteen standard faces, parsed once per process.
///
/// The same memoization [`builtin_generic`] gets and for the same reason: the
/// blob is an `include_bytes!` constant, so the parse is a pure function of the
/// `StandardFont` index and there is no key to get wrong. A dense array rather
/// than a map because the index is already `0..14` and dense — `StandardFont`'s
/// discriminants are load-bearing arithmetic (see its own docs), not an
/// arbitrary tag.
///
/// `Face` holds its bytes behind an `Arc`, so the clone is a refcount bump and
/// the 66-113 KB of CFF is stored once rather than once per `Helv` in a form's
/// resource dictionary.
fn builtin_standard(f: StandardFont) -> GlyphSource {
    /// One cell per base-14 index.
    static FACES: OnceLock<[GlyphSource; 14]> = OnceLock::new();

    let faces = FACES.get_or_init(|| {
        std::array::from_fn(|i| {
            let Some(f) = StandardFont::from_index(i) else {
                return GlyphSource::None;
            };
            let bytes: Arc<[u8]> = Arc::from(standard_font_data(f));
            Face::new(bytes, 0).map_or(GlyphSource::None, GlyphSource::Fontations)
        })
    });
    faces.get(f.index()).cloned().unwrap_or_default()
}

/// One of the two built-in Multiple-Master generic faces, and its family name.
///
/// These are the reason `pdfrum-type1` exists: they are PFB Type 1 Multiple
/// Master, and instantiating them at an arbitrary weight and width is what
/// draws every font neither the document nor the system supplied.
///
/// # Parsed once per process, then shared
///
/// The two PFB blobs are `include_bytes!` constants, so parsing one is a pure
/// function of a `bool` — the same 66 KB (sans) or 113 KB of container split,
/// `eexec` decryption, charstring extraction and glyph-name indexing, producing
/// the same face, every time. Unmemoized it ran on **every call**, and the call
/// is the last rung of the substitution ladder: it fires for every non-embedded
/// font whose name is not one of the base fourteen and which no system database
/// supplied. `mixed_formfield.pdf` has sixteen such fonts in its AcroForm
/// `/DR /Font` — fifteen of them byte-identical SimSun descriptors under
/// different resource names — and the form-field appearance pass reloads all of
/// them on every render, so a single render of a single page paid **fifteen
/// full Multiple-Master parses**. That was 55 ms of the document's 87 ms.
///
/// A `OnceLock` per variant fixes it at the only layer where the memoization is
/// unconditionally sound: the input is a compile-time constant, so there is no
/// key to get wrong, no lifetime to scope, and no document whose cache this
/// could leak across. `GlyphSource::Type1` holds an `Arc`, so the clone handed
/// to each caller is a refcount bump. `Face` is likewise `Arc<[u8]>`-backed.
///
/// This is deliberately *not* the general font cache `FontCache`'s doc comment
/// promises and its single `AtomicU64` field does not deliver. That remains
/// outstanding, and it is the fix for a document that loads the same *embedded*
/// font sixteen times. What is fixed here is the built-in fallback path, which
/// is the one the corpus actually exercises.
#[must_use]
pub fn builtin_generic(serif: bool) -> (GlyphSource, &'static str) {
    /// The parsed sans face, or `None` if the blob failed to parse.
    static SANS: OnceLock<GlyphSource> = OnceLock::new();
    /// The parsed serif face.
    static SERIF: OnceLock<GlyphSource> = OnceLock::new();

    let (cell, bytes, family) = if serif {
        (
            &SERIF,
            &include_bytes!("../../fontdata/FoxitSerifMM.pfb")[..],
            "Chrome Serif",
        )
    } else {
        (
            &SANS,
            &include_bytes!("../../fontdata/FoxitSansMM.pfb")[..],
            "Chrome Sans",
        )
    };
    // Diagnostics are discarded here exactly as they were before: the limit is
    // zero, the input is a constant this crate ships, and a caller has no way
    // to act on damage in a blob they did not supply. `is_some()` is how the
    // one failure that matters reaches `terminal`.
    let source = cell.get_or_init(|| {
        let font = pdfrum_type1::Type1Font::parse(
            bytes,
            &pdfrum_common::Limits::default(),
            &mut Diagnostics::with_limit(0),
        );
        match font {
            Ok(f) => GlyphSource::Type1(Arc::new(f)),
            Err(_) => GlyphSource::None,
        }
    });
    (source.clone(), family)
}

#[cfg(test)]
#[path = "subst_tests.rs"]
mod tests;
