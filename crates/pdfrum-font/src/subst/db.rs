//! The font-database seam, and the matching policy over it.
//!
//! `fontdb` is used purely as a face **enumerator** — the role
//! `SystemFontInfoIface` played — while the ranking is our port of PDFium's
//! own, so a substitution is predictable rather than dependent on someone
//! else's heuristics. That resolves OQ-6(b) in the direction the brief
//! recommended: `fontdb`'s query ranking is a different function, and Tier-B
//! results would drift with its version.

#[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
use super::charset::charset_for_code_page_bit;
use super::charset::{Charset, PitchFamily};
#[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
use super::probe::{FaceProbe, ProbeError};
use super::style::{style_bits, tt_normalize};
use read_fonts::TableProvider;
#[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
use skrifa::MetadataProvider;
use std::path::PathBuf;
use std::sync::Arc;

/// A handle to one face in a database.
///
/// An index into that database's face table, not a value a caller constructs.
/// The field is private so the index cannot be forged from outside the crate;
/// the type itself never leaves this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FaceHandle(usize);

impl FaceHandle {
    pub(crate) const fn from_index(index: usize) -> Self {
        Self(index)
    }

    pub(crate) const fn index(self) -> usize {
        self.0
    }
}

/// What a database knows about one installed face.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceInfo {
    /// The display name, joined as `GetFontNameFromFace` joins it: family,
    /// then a space and the style unless the style is empty or `Regular`.
    pub name: String,
    /// The style bits read from the face's own tables.
    pub styles: u32,
    /// The charsets the face claims, from its `OS/2` code-page ranges. ANSI is
    /// always included, matching the folder enumerator.
    pub charsets: Vec<Charset>,
}

impl FaceInfo {
    /// The similarity score for a request, out of 68.
    ///
    /// Each term rewards *agreement*, not presence: a non-bold face scores for
    /// a non-bold request exactly as a bold one scores for a bold request. The
    /// weights are 16/16/16/8/8 plus a 4-point bonus when the request's name
    /// is the same length as the face's, which is PDFium's cheap stand-in for
    /// "an exact name match".
    #[must_use]
    pub fn similarity_score(
        &self,
        weight: i32,
        italic: bool,
        pitch: PitchFamily,
        exact_match_bonus: bool,
    ) -> i32 {
        let mut score = 0;
        if (self.styles & style_bits::FORCE_BOLD != 0) == (weight > 400) {
            score += 16;
        }
        if (self.styles & style_bits::ITALIC != 0) == italic {
            score += 16;
        }
        if (self.styles & STYLE_SERIF != 0) == pitch.has(PitchFamily::ROMAN) {
            score += 16;
        }
        if (self.styles & STYLE_SCRIPT != 0) == pitch.has(PitchFamily::SCRIPT) {
            score += 8;
        }
        if (self.styles & STYLE_FIXED_PITCH != 0) == pitch.has(PitchFamily::FIXED) {
            score += 8;
        }
        if exact_match_bonus {
            score += 4;
        }
        score
    }

    /// Whether this face agrees with the request on **every** term.
    ///
    /// The early-exit the substitution ladder takes: a face scoring the
    /// maximum cannot be beaten, so the search stops. This is the predicate
    /// `SIMILARITY_SCORE_MAX` used to be public for — the caller wants the
    /// question, not the number.
    #[must_use]
    pub fn is_exact_match(
        &self,
        weight: i32,
        italic: bool,
        pitch: PitchFamily,
        exact_match_bonus: bool,
    ) -> bool {
        self.similarity_score(weight, italic, pitch, exact_match_bonus) == SIMILARITY_SCORE_MAX
    }

    /// Can this face answer a request for `charset` at all?
    ///
    /// A `Default` charset makes every face eligible, which is what lets the
    /// last rung of the ladder pick anything at all.
    #[must_use]
    pub fn is_eligible(&self, charset: Charset) -> bool {
        charset == Charset::Default || self.charsets.contains(&charset)
    }
}

/// The highest score [`FaceInfo::similarity_score`] can return.
///
/// **Private on purpose.** It was public solely so a caller could
/// equality-test a score against it,
/// which is asking the number to answer a question;
/// [`FaceInfo::is_exact_match`] is that question, and it is what the one
/// caller in this crate now asks.
const SIMILARITY_SCORE_MAX: i32 = 68;

/// The face-style bits read from a face's own tables, which are a different
/// set from the request's `/Flags`.
const STYLE_SERIF: u32 = 1 << 1;
const STYLE_SCRIPT: u32 = 1 << 3;
const STYLE_FIXED_PITCH: u32 = 1 << 0;

/// Does an installed face's name contain a family name **as a whole word**?
///
/// The rule that makes `"Univers"` match `"Univers Bold"` but not
/// `"Universal"`: the byte after the match must not be a lowercase ASCII
/// letter. Uppercase and punctuation are fine, which is why `"Oxygen"` matches
/// `"Oxygen-Sans"`.
#[must_use]
pub fn find_family_name_match(family: &str, installed: &str) -> bool {
    let Some(at) = installed.find(family) else {
        return false;
    };
    let next = at + family.len();
    !installed
        .as_bytes()
        .get(next)
        .is_some_and(u8::is_ascii_lowercase)
}

/// A source of installed faces.
///
/// A genuine seam: there are two real implementations —
/// [`SystemFontDb`] over the operating system's fonts, and [`TestFontDb`] over
/// an explicit list, which the substitution tests and the hermetic
/// `--font-dir` runs both need. Threaded as `&impl FontDb`, never `dyn`.
pub trait FontDb {
    /// Every installed face, in a stable order.
    fn faces(&self) -> &[FaceInfo];

    /// The bytes and collection index of one face.
    fn face_bytes(&self, h: FaceHandle) -> Option<(Arc<[u8]>, u32)>;

    /// Find the best face for a request.
    ///
    /// The default implementation is the scoring scan below; an implementor
    /// overrides it only to delegate somewhere else entirely.
    ///
    /// Two passes, and the first exists purely to seed the score: a face whose
    /// name is exactly the requested family establishes a baseline the general
    /// scan must beat, and returns immediately on a perfect 68.
    fn find_font(
        &self,
        weight: i32,
        italic: bool,
        charset: Charset,
        pitch: PitchFamily,
        family: &str,
        must_match_name: bool,
    ) -> Option<FaceHandle> {
        let faces = self.faces();
        let mut best: Option<usize> = None;
        let mut best_score = 0;

        if must_match_name
            && let Some((i, face)) = faces
                .iter()
                .enumerate()
                .find(|(_, f)| f.name == family && f.is_eligible(charset))
        {
            best_score = face.similarity_score(weight, italic, pitch, true);
            best = Some(i);
            if face.is_exact_match(weight, italic, pitch, true) {
                return Some(FaceHandle::from_index(i));
            }
        }

        for (i, face) in faces.iter().enumerate() {
            if !face.is_eligible(charset) {
                continue;
            }
            // The "exact match" bonus is granted on a *length* comparison, not
            // a content one — PDFium's own shortcut, and cheaper than it looks
            // because a differing length rules out equality anyway.
            let bonus = must_match_name && family.len() == face.name.len();
            let score = face.similarity_score(weight, italic, pitch, bonus);
            if score <= best_score {
                continue;
            }
            if must_match_name && !find_family_name_match(family, &face.name) {
                continue;
            }
            best_score = score;
            best = Some(i);
        }

        if let Some(i) = best {
            return Some(FaceHandle::from_index(i));
        }
        // The one hard-coded consolation prize: a fixed-pitch ANSI request
        // that matched nothing gets Courier New if it exists.
        if charset == Charset::Ansi && pitch.has(PitchFamily::FIXED) {
            return self.font_by_name("Courier New");
        }
        None
    }

    /// An exact-name lookup, which two rungs of the ladder use directly.
    fn font_by_name(&self, name: &str) -> Option<FaceHandle> {
        self.faces()
            .iter()
            .position(|f| f.name == name)
            .map(FaceHandle::from_index)
    }

    /// Find an installed face whose *normalized* name equals `normalized`,
    /// scanning in **reverse** insertion order (`MatchInstalledFonts`).
    ///
    /// The reverse scan means a later-registered face shadows an earlier one
    /// with the same normalized name.
    fn match_installed(&self, normalized: &str) -> Option<String> {
        self.faces()
            .iter()
            .rev()
            .find(|f| tt_normalize(&f.name) == normalized)
            .map(|f| f.name.clone())
    }
}

/// A database whose *by-name* lookups see Croscore family names instead of the
/// ones the ladder computed.
///
/// The `test_fonts` directory carries no Arial, Times or Courier — it carries
/// the metric-compatible Arimo, Tinos and Cousine — so a hermetic run rewrites
/// the family a lookup asks for. The rewrite belongs **here**, on the two
/// by-name queries, and not on the `/BaseFont` name the ladder starts from: the
/// ladder's base-14 recognition, style parsing and charset decision all run on
/// the document's own spelling, and only the family that finally reaches the
/// database is renamed. Rewriting earlier would hide `Arial,Bold` from the
/// base-14 table and lose the bold with it.
///
/// [`faces`](FontDb::faces) and [`face_bytes`](FontDb::face_bytes) pass
/// straight through: the *installed* names are what the enumeration produced,
/// and the name-normalizing rung matches against those unrenamed.
#[derive(Debug, Clone, Copy)]
pub struct CroscoreDb<'a, D> {
    inner: &'a D,
}

impl<'a, D: FontDb> CroscoreDb<'a, D> {
    /// Wrap a database so its by-name lookups take Croscore family names.
    #[must_use]
    pub fn new(inner: &'a D) -> Self {
        Self { inner }
    }
}

impl<D: FontDb> FontDb for CroscoreDb<'_, D> {
    fn faces(&self) -> &[FaceInfo] {
        self.inner.faces()
    }

    fn face_bytes(&self, h: FaceHandle) -> Option<(Arc<[u8]>, u32)> {
        self.inner.face_bytes(h)
    }

    fn find_font(
        &self,
        weight: i32,
        italic: bool,
        charset: Charset,
        pitch: PitchFamily,
        family: &str,
        must_match_name: bool,
    ) -> Option<FaceHandle> {
        self.inner.find_font(
            weight,
            italic,
            charset,
            pitch,
            &super::croscore_name(family),
            must_match_name,
        )
    }

    fn font_by_name(&self, name: &str) -> Option<FaceHandle> {
        self.inner.font_by_name(&super::croscore_name(name))
    }
}

/// A database backed by an explicit list of faces — the test seam, and what a
/// hermetic `--font-dir` run uses.
#[derive(Debug, Clone, Default)]
pub struct TestFontDb {
    faces: Vec<FaceInfo>,
    bytes: Vec<Option<(Arc<[u8]>, u32)>>,
}

impl TestFontDb {
    /// An empty database, in which every lookup fails and the ladder falls all
    /// the way through to the built-in faces.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a face with no bytes, for testing the ladder's decisions
    /// without needing a real font program.
    #[cfg(test)]
    pub fn push(&mut self, name: &str, styles: u32, charsets: Vec<Charset>) {
        self.faces.push(FaceInfo {
            name: name.to_owned(),
            styles,
            charsets,
        });
        self.bytes.push(None);
    }

    /// Register a face with real bytes.
    #[cfg(test)]
    pub fn push_with_bytes(
        &mut self,
        name: &str,
        styles: u32,
        charsets: Vec<Charset>,
        bytes: Arc<[u8]>,
    ) {
        self.faces.push(FaceInfo {
            name: name.to_owned(),
            styles,
            charsets,
        });
        self.bytes.push(Some((bytes, 0)));
    }
}

impl FontDb for TestFontDb {
    fn faces(&self) -> &[FaceInfo] {
        &self.faces
    }

    fn face_bytes(&self, h: FaceHandle) -> Option<(Arc<[u8]>, u32)> {
        self.bytes.get(h.0).cloned().flatten()
    }
}

/// A database over the operating system's installed fonts, via `fontdb`.
///
/// Scanning is eager and can be slow on a machine with thousands of fonts, so
/// a caller builds one per document at most — which is why it lives behind
/// [`SubstitutionOptions`](super::SubstitutionOptions) rather than being
/// created implicitly.
#[derive(Debug)]
pub struct SystemFontDb {
    faces: Vec<FaceInfo>,
    #[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
    sources: Vec<FaceSource>,
}

/// Where a scanned face's bytes come from — read only when the face is
/// chosen. The scan used to copy every installed face into memory and keep
/// all of them; on a host with 490 MB of fonts that was 1.1 GB resident per
/// substituted font, for the one face the ladder picks.
#[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
#[derive(Debug, Clone)]
enum FaceSource {
    /// A file on disk and the collection index within it.
    File(PathBuf, u32),
    /// Bytes `fontdb` already held in memory, and the index.
    Bytes(Arc<[u8]>, u32),
}

impl SystemFontDb {
    /// Scan the system font directories, plus any extra directories given.
    ///
    /// An empty `extra_dirs` list means "the system's own directories"; a
    /// non-empty one means **only** those, which is how a hermetic run pins
    /// the font set.
    /// The host's installed fonts, or the faces under `extra_dirs` when any
    /// are named — behind the `system-fonts` feature and off the WebAssembly
    /// target, where there is no host to scan: there the database is empty
    /// and the bundled faces alone answer.
    #[cfg(not(all(feature = "system-fonts", not(target_arch = "wasm32"))))]
    #[must_use]
    pub fn scan(extra_dirs: &[PathBuf]) -> Self {
        let _ = extra_dirs;
        Self { faces: Vec::new() }
    }

    #[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
    #[must_use]
    pub fn scan(extra_dirs: &[PathBuf]) -> Self {
        let mut db = fontdb::Database::new();
        if extra_dirs.is_empty() {
            db.load_system_fonts();
        } else {
            for dir in extra_dirs {
                db.load_fonts_dir(dir);
            }
        }

        let mut faces = Vec::new();
        let mut sources = Vec::new();
        for face in db.faces() {
            // Describe the face from the bytes `fontdb` holds — a mapping for
            // a file it enumerated — without copying them; remember only
            // where to read the face from if the ladder chooses it.
            let (bytes, source): (std::borrow::Cow<'_, [u8]>, FaceSource) = match &face.source {
                fontdb::Source::Binary(data) => {
                    let bytes: Arc<[u8]> = Arc::from(data.as_ref().as_ref());
                    (
                        std::borrow::Cow::Owned(bytes.to_vec()),
                        FaceSource::Bytes(bytes, face.index),
                    )
                }
                fontdb::Source::SharedFile(path, data) => (
                    std::borrow::Cow::Borrowed(data.as_ref().as_ref()),
                    FaceSource::File(path.clone(), face.index),
                ),
                fontdb::Source::File(path) => {
                    // The scan's hot path, and the reason this is not a plain
                    // `read`: `describe` wants two small tables, and reading
                    // every enumerated file whole cost 20.5 ms of `read` per
                    // cold render on the oracle's 33.8 MB font set
                    // (`docs/status/cold-start.md`). A probe fetches the table
                    // directory and those two tables and nothing else; a file
                    // with no directory at all — a bare CFF, a Type 1 `.pfb` —
                    // falls back to the whole-file read it had before.
                    match FaceProbe::read(path, face.index) {
                        Ok(probe) => {
                            // The probe reassembles one face, so its own index
                            // is 0 whatever the face's index in the file was.
                            let Some(info) = describe(0, probe.as_font_bytes()) else {
                                continue;
                            };
                            faces.push(info);
                            sources.push(FaceSource::File(path.clone(), face.index));
                            continue;
                        }
                        Err(ProbeError::NotSfnt) => {
                            let Ok(read) = std::fs::read(path) else {
                                continue;
                            };
                            (
                                std::borrow::Cow::Owned(read),
                                FaceSource::File(path.clone(), face.index),
                            )
                        }
                        Err(_) => continue,
                    }
                }
            };
            let Some(info) = describe(face.index, &bytes) else {
                continue;
            };
            faces.push(info);
            sources.push(source);
        }

        // Sorted by face name, because the C++'s font list is a
        // `std::map<ByteString, ...>` (`cfx_folderfontinfo.h:98`) and two of
        // the ladder's decisions read it *in order*: a face can only displace
        // the incumbent by scoring strictly higher, so a tie goes to whichever
        // came first, and rung 5 takes the first face claiming the charset
        // outright. Leaving these in `fontdb`'s enumeration order — which is
        // directory order, so it varies by filesystem — would make both
        // answers depend on something the oracle's does not.
        let mut order: Vec<usize> = (0..faces.len()).collect();
        order.sort_by(|&l, &r| match (faces.get(l), faces.get(r)) {
            (Some(l), Some(r)) => l.name.cmp(&r.name),
            _ => std::cmp::Ordering::Equal,
        });
        let sorted_faces = order
            .iter()
            .filter_map(|&i| faces.get(i).cloned())
            .collect();
        let sorted_sources = order
            .iter()
            .filter_map(|&i| sources.get(i).cloned())
            .collect();

        Self {
            faces: sorted_faces,
            sources: sorted_sources,
        }
    }
}

impl FontDb for SystemFontDb {
    fn faces(&self) -> &[FaceInfo] {
        &self.faces
    }

    #[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
    fn face_bytes(&self, h: FaceHandle) -> Option<(Arc<[u8]>, u32)> {
        match self.sources.get(h.0)? {
            FaceSource::File(path, index) => std::fs::read(path)
                .ok()
                .map(|bytes| (Arc::from(bytes), *index)),
            FaceSource::Bytes(bytes, index) => Some((bytes.clone(), *index)),
        }
    }

    #[cfg(not(all(feature = "system-fonts", not(target_arch = "wasm32"))))]
    fn face_bytes(&self, h: FaceHandle) -> Option<(Arc<[u8]>, u32)> {
        let _ = h;
        None
    }
}

#[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
/// The three style bits an enumerated face carries, from its two name strings.
///
/// `name` is the joined face name — family, then the style unless the style is
/// `Regular` — and `style` is the subfamily on its own. The asymmetry is
/// deliberate, not a simplification: bold and italic read the **style**,
/// serif reads the **whole face name**. So a family literally called
/// `PT Serif` carries the bit through every one of its faces, and no other
/// family carries it at all.
///
/// All three are case-sensitive substring tests. A `SERIF` in caps or a
/// `serif` in lower case does not match, and that is reproduced rather than
/// corrected — the scoring must agree with the reference implementation's,
/// including where that one is crude.
// The rule and its crudeness are `CFX_FolderFontInfo::ReportFace`,
// `cfx_folderfontinfo.cpp:349-358`; the case sensitivity is
// `ByteString::Contains`.
fn face_styles(name: &str, style: &str) -> u32 {
    let mut styles = style_bits::NORMAL;
    if style.contains("Bold") {
        styles |= style_bits::FORCE_BOLD;
    }
    if style.contains("Italic") || style.contains("Oblique") {
        styles |= style_bits::ITALIC;
    }
    if name.contains("Serif") {
        styles |= STYLE_SERIF;
    }
    styles
}

#[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
/// Read a face's display name, style bits and charsets from its own tables.
///
/// The charsets come from `OS/2`'s code-page ranges, which `fontdb` does not
/// expose — the reason this crate reads the face itself rather than trusting
/// the database's metadata. ANSI is added unconditionally, matching the folder
/// enumerator, so a face with no code-page claims at all is still usable for
/// Latin text.
///
/// The three **style bits are read off the two name strings**, not off the
/// face's own tables — `face_styles` below is the whole of the rule.
///
/// Three bits, three substring tests, and **nothing else is ever set** — the
/// script and fixed-pitch bits stay zero for every enumerated face, so the two
/// terms of [`FaceInfo::similarity_score`] that read them score for exactly
/// the requests whose pitch family is neither script nor fixed.
///
/// This is coarser than the face's own tables and deliberately so. Reading the
/// serif bit from `OS/2`'s PANOSE instead would split a single family three
/// ways on the hermetic font set: it gives `Tinos`, `Cousine`, `GardinerMod`
/// and three of the four `Gelasio` faces a serif bit the name rule gives none
/// of them, and it gives `Gelasio Bold` no serif bit while giving that face's
/// own Regular and Bold Italic siblings one. A rule that splits one family
/// three ways is scoring on something other than the family, and the
/// 16-point serif term is the largest in the score.
// The name rule is `CFX_FolderFontInfo::ReportFace`,
// `cfx_folderfontinfo.cpp:349-358`:
//
//   pInfo->styles_ = 0;
//   if (style.Contains("Bold")) { pInfo->styles_ |= kFontStyleForceBold; }
//   if (style.Contains("Italic") || style.Contains("Oblique")) {
//     pInfo->styles_ |= kFontStyleItalic;
//   }
//   if (facename.Contains("Serif")) { pInfo->styles_ |= kFontStyleSerif; }
//
// The PANOSE rule is the other one PDFium has, `CFX_Face::GetFontStyle`
// (`cfx_face.cpp:1608-1633`), and is reachable only from the XFA and Android
// font managers — never from the folder enumerator a `--font-dir` run goes
// through, which is why this side takes the name rule.
fn describe(index: u32, bytes: &[u8]) -> Option<FaceInfo> {
    let font = skrifa::FontRef::from_index(bytes, index).ok()?;

    // Name IDs **1 and 2**, which is what the enumerator asks for
    // (`GetNameFromTT(names, 1)` / `(names, 2)`), and not the typographic
    // family of name ID 16. `fontdb` prefers ID 16 where a face has one, which
    // is the better answer for a font picker and the wrong one here: the
    // oracle's `Noto Sans CJK JP Regular` would arrive as `Noto Sans CJK JP`,
    // and the name is what `find_family_name_match`, the exact-name lookup,
    // `match_installed` and the score's 4-point length bonus all read.
    let family: String = font
        .localized_strings(skrifa::string::StringId::FAMILY_NAME)
        .english_or_first()
        .map(|s| s.chars().collect())?;
    let style: String = font
        .localized_strings(skrifa::string::StringId::SUBFAMILY_NAME)
        .english_or_first()
        .map(|s| s.chars().collect())
        .unwrap_or_default();
    let name = if style.is_empty() || style == "Regular" {
        family
    } else {
        format!("{family} {style}")
    };

    let styles = face_styles(&name, &style);

    let mut charsets = vec![Charset::Ansi];
    if let Ok(os2) = font.os2() {
        let ranges = u64::from(os2.ul_code_page_range_1().unwrap_or(0))
            | (u64::from(os2.ul_code_page_range_2().unwrap_or(0)) << 32);
        for bit in 0..64u32 {
            if ranges & (1u64 << bit) == 0 {
                continue;
            }
            if let Some(c) = charset_for_code_page_bit(bit)
                && !charsets.contains(&c)
            {
                charsets.push(c);
            }
        }
    }

    Some(FaceInfo {
        name,
        styles,
        charsets,
    })
}

#[cfg(all(test, feature = "system-fonts", not(target_arch = "wasm32")))]
mod tests {
    // Test fixtures are fixed-size arrays with known contents.
    #![allow(clippy::indexing_slicing)]
    use super::*;

    /// Every font file the machine has to offer, for the probe equivalence
    /// test: the bundled base-14 CFFs (which are *not* sfnts, so they exercise
    /// the fallback), the benchmark fixtures, and — when this is the box that
    /// has it — the oracle's hermetic `test_fonts` set, which is where the
    /// interesting faces are: a 16 MB CJK OTF with CFF outlines, a 9 MB colour
    /// emoji TTF, and 29 others.
    fn font_files() -> Vec<PathBuf> {
        let mut roots = vec![
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fontdata"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benches/fixtures"),
        ];
        // A machine path, not a repository one: skipped silently elsewhere.
        let oracle = PathBuf::from("/mnt/data2/pdfium/pdfium-c++/third_party/test_fonts");
        if oracle.is_dir() {
            roots.push(oracle);
        }
        let mut out = Vec::new();
        while let Some(root) = roots.pop() {
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    roots.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    /// The probe must describe a face exactly as reading the whole file does.
    ///
    /// This is the whole safety argument for the bounded reader. The scan's
    /// output feeds the substitution ladder, whose choice of face decides what
    /// 1759 conformance files render, so "reads less" is only acceptable
    /// alongside "parses the same". The comparison is over real files rather
    /// than a synthesised one because the failure mode being guarded against is
    /// a real font's layout — a `ttcf` collection, an `OTTO` face, a table the
    /// directory lists out of order — not a hand-written edge case.
    #[test]
    fn a_probe_describes_a_face_exactly_as_the_whole_file_does() {
        let files = font_files();
        assert!(!files.is_empty(), "no font files found to compare");
        let mut sfnts = 0;
        let mut fallbacks = 0;
        for path in &files {
            let Ok(whole) = std::fs::read(path) else {
                continue;
            };
            match FaceProbe::read(path, 0) {
                Ok(probe) => {
                    sfnts += 1;
                    assert_eq!(
                        describe(0, probe.as_font_bytes()),
                        describe(0, &whole),
                        "probe and whole-file describe disagree for {}",
                        path.display()
                    );
                }
                Err(ProbeError::NotSfnt) => {
                    fallbacks += 1;
                    // The fallback path: whatever `describe` said before, it
                    // still says, because the caller still reads the file whole.
                }
                Err(_) => {}
            }
        }
        assert!(sfnts > 0, "no sfnt files among {} candidates", files.len());
        // The bundled base-14 are bare CFF, so the fallback is always exercised.
        assert!(fallbacks > 0, "the non-sfnt fallback was never taken");
    }

    /// A probe reads kilobytes where the file holds megabytes.
    ///
    /// The point of the change, asserted rather than only measured: on the
    /// oracle's set the two largest faces are 16.4 MB and 9.4 MB, and their
    /// `name` and `OS/2` tables together are a few kilobytes.
    #[test]
    fn a_probe_is_far_smaller_than_the_file_it_came_from() {
        for path in font_files() {
            let Ok(probe) = FaceProbe::read(&path, 0) else {
                continue;
            };
            let Ok(len) = std::fs::metadata(&path).map(|m| m.len()) else {
                continue;
            };
            if len < 1024 * 1024 {
                continue;
            }
            assert!(
                (probe.as_font_bytes().len() as u64) < len / 100,
                "probe of {} is {} bytes against a {} byte file",
                path.display(),
                probe.as_font_bytes().len(),
                len
            );
        }
    }

    fn db() -> TestFontDb {
        let mut db = TestFontDb::new();
        db.push("Arial", 0, vec![Charset::Ansi]);
        db.push("Arial Bold", style_bits::FORCE_BOLD, vec![Charset::Ansi]);
        db.push("Arial Italic", style_bits::ITALIC, vec![Charset::Ansi]);
        db.push("Times New Roman", STYLE_SERIF, vec![Charset::Ansi]);
        db.push("Courier New", STYLE_FIXED_PITCH, vec![Charset::Ansi]);
        db.push("MS Gothic", 0, vec![Charset::ShiftJis]);
        db
    }

    /// `cfx_folderfontinfo_unittest.cpp`'s `TestFindFont` name rule, which is
    /// the most consequential row of that test.
    #[test]
    fn a_family_name_must_match_at_a_word_boundary() {
        // The C++'s own worked example.
        assert!(find_family_name_match("Univers", "Univers Bold"));
        assert!(!find_family_name_match("Univers", "Universal"));
        // The oracle test's other rows.
        assert!(!find_family_name_match("Book", "Bookshelf Symbol 7"));
        assert!(find_family_name_match("Bookshelf", "Bookshelf Symbol 7"));
        assert!(find_family_name_match("Tofu", "Tofu"));
        assert!(find_family_name_match("Lato", "Lato"));
        assert!(find_family_name_match("Oxygen", "Oxygen"));
        // A hyphen is not a lowercase letter, so this matches.
        assert!(find_family_name_match("Oxygen", "Oxygen-Sans"));
        // An absent name matches nothing.
        assert!(!find_family_name_match("Helvetica", "Arial"));
    }

    #[test]
    fn an_uppercase_next_character_still_matches() {
        assert!(find_family_name_match("Foo", "FooBar"));
        assert!(!find_family_name_match("Foo", "Foobar"));
    }

    #[test]
    fn the_similarity_score_rewards_agreement_in_both_directions() {
        let plain = FaceInfo {
            name: "Plain".to_owned(),
            styles: 0,
            charsets: vec![Charset::Ansi],
        };
        let bold = FaceInfo {
            name: "Bold".to_owned(),
            styles: style_bits::FORCE_BOLD,
            charsets: vec![Charset::Ansi],
        };
        let p = PitchFamily::default();
        // A plain face scores for a plain request...
        assert!(
            plain.similarity_score(400, false, p, false)
                > bold.similarity_score(400, false, p, false)
        );
        // ...and a bold face for a bold one.
        assert!(
            bold.similarity_score(700, false, p, false)
                > plain.similarity_score(700, false, p, false)
        );
    }

    #[test]
    fn a_perfect_score_is_sixty_eight() {
        let face = FaceInfo {
            name: "F".to_owned(),
            styles: style_bits::FORCE_BOLD | style_bits::ITALIC,
            charsets: vec![Charset::Ansi],
        };
        assert_eq!(
            face.similarity_score(700, true, PitchFamily::default(), true),
            SIMILARITY_SCORE_MAX
        );
    }

    #[test]
    fn the_exact_match_bonus_is_worth_four() {
        let face = FaceInfo {
            name: "F".to_owned(),
            styles: 0,
            charsets: vec![Charset::Ansi],
        };
        let p = PitchFamily::default();
        assert_eq!(
            face.similarity_score(400, false, p, true)
                - face.similarity_score(400, false, p, false),
            4
        );
    }

    #[test]
    fn eligibility_gates_on_charset_unless_the_request_is_default() {
        let face = FaceInfo {
            name: "MS Gothic".to_owned(),
            styles: 0,
            charsets: vec![Charset::ShiftJis],
        };
        assert!(face.is_eligible(Charset::ShiftJis));
        assert!(!face.is_eligible(Charset::Ansi));
        // A Default request makes everything eligible.
        assert!(face.is_eligible(Charset::Default));
    }

    #[test]
    fn find_font_prefers_the_matching_style() {
        let db = db();
        let bold = db
            .find_font(
                700,
                false,
                Charset::Ansi,
                PitchFamily::default(),
                "Arial",
                true,
            )
            .expect("a face is found");
        assert_eq!(db.faces()[bold.0].name, "Arial Bold");

        let italic = db
            .find_font(
                400,
                true,
                Charset::Ansi,
                PitchFamily::default(),
                "Arial",
                true,
            )
            .expect("a face is found");
        assert_eq!(db.faces()[italic.0].name, "Arial Italic");
    }

    #[test]
    fn find_font_respects_the_charset_gate() {
        let db = db();
        let jp = db
            .find_font(
                400,
                false,
                Charset::ShiftJis,
                PitchFamily::default(),
                "MS Gothic",
                true,
            )
            .expect("the Japanese face is found");
        assert_eq!(db.faces()[jp.0].name, "MS Gothic");
        // A charset nothing claims finds nothing.
        assert!(
            db.find_font(
                400,
                false,
                Charset::Hebrew,
                PitchFamily::default(),
                "Arial",
                true
            )
            .is_none()
        );
    }

    #[test]
    fn find_font_falls_back_to_courier_new_for_a_fixed_ansi_request() {
        let mut db = TestFontDb::new();
        // Nothing whose name matches, so the scan finds nothing...
        db.push("Courier New", STYLE_FIXED_PITCH, vec![Charset::Ansi]);
        let h = db.find_font(
            400,
            false,
            Charset::Ansi,
            PitchFamily(PitchFamily::FIXED),
            "NoSuchFamily",
            true,
        );
        // ...and the hard-coded consolation prize applies.
        assert_eq!(
            h.map(|h| db.faces()[h.0].name.as_str()),
            Some("Courier New")
        );
    }

    #[test]
    fn the_name_gate_is_lifted_when_must_match_name_is_false() {
        let db = db();
        // With the gate on, a family nothing carries finds nothing...
        assert!(
            db.find_font(
                400,
                false,
                Charset::Ansi,
                PitchFamily::default(),
                "Zzz",
                true
            )
            .is_none()
        );
        // ...and with it off, the best-scoring eligible face wins.
        assert!(
            db.find_font(
                400,
                false,
                Charset::Ansi,
                PitchFamily::default(),
                "Zzz",
                false
            )
            .is_some()
        );
    }

    #[test]
    fn match_installed_scans_in_reverse_so_a_later_face_shadows_an_earlier() {
        let mut db = TestFontDb::new();
        db.push("Foo Bar", 0, vec![Charset::Ansi]);
        db.push("FooBar", 0, vec![Charset::Ansi]);
        // Both normalize to "foobar"; the later one wins.
        assert_eq!(db.match_installed("foobar").as_deref(), Some("FooBar"));
        assert_eq!(db.match_installed("nothing"), None);
    }

    #[test]
    fn an_empty_database_answers_nothing() {
        let db = TestFontDb::new();
        assert!(db.faces().is_empty());
        assert!(
            db.find_font(
                400,
                false,
                Charset::Ansi,
                PitchFamily::default(),
                "Arial",
                true
            )
            .is_none()
        );
        assert!(db.font_by_name("Arial").is_none());
        assert!(db.face_bytes(FaceHandle(0)).is_none());
    }
}

/// What an *enumerated* face's style bits are read from.
///
/// The bits decide 48 of [`FaceInfo::similarity_score`]'s 68 points, so the
/// rule that sets them is the rule that picks the face. These tests pin it to
/// the two name strings and to nothing else — in particular not to the face's
/// own `OS/2` PANOSE.
// The rule pinned here is `CFX_FolderFontInfo::ReportFace`; the PANOSE rule
// these tests exclude is `CFX_Face::GetFontStyle`, which PDFium reaches only
// from XFA and Android.
#[cfg(all(test, feature = "system-fonts", not(target_arch = "wasm32")))]
mod face_style_bits {
    use super::*;

    /// The bold bit is the style string's, and the style string's alone.
    #[test]
    fn bold_reads_the_style_not_the_face_name() {
        assert_eq!(
            face_styles("Arimo Bold", "Bold") & style_bits::FORCE_BOLD,
            style_bits::FORCE_BOLD
        );
        assert_eq!(
            face_styles("Arimo Bold Italic", "Bold Italic") & style_bits::FORCE_BOLD,
            style_bits::FORCE_BOLD
        );
        assert_eq!(face_styles("Arimo", "Regular") & style_bits::FORCE_BOLD, 0);
        // A family whose *name* says Bold but whose subfamily does not is not
        // bold: the two strings are read separately and only one is asked.
        assert_eq!(
            face_styles("Bold Sans", "Regular") & style_bits::FORCE_BOLD,
            0
        );
    }

    /// Italic takes `Oblique` too — the one place the port reads two words.
    #[test]
    fn italic_takes_oblique_as_well() {
        assert_eq!(
            face_styles("X Italic", "Italic") & style_bits::ITALIC,
            style_bits::ITALIC
        );
        assert_eq!(
            face_styles("X Oblique", "Oblique") & style_bits::ITALIC,
            style_bits::ITALIC
        );
        assert_eq!(face_styles("X", "Regular") & style_bits::ITALIC, 0);
    }

    /// The serif bit is a substring test on the **face name**, which is the
    /// divergence this test exists for: PANOSE would answer differently for
    /// every serif family in the hermetic set.
    #[test]
    fn serif_is_the_face_name_containing_serif() {
        assert_eq!(
            face_styles("PT Serif", "Regular") & STYLE_SERIF,
            STYLE_SERIF
        );
        assert_eq!(
            face_styles("Noto Serif Bold", "Bold") & STYLE_SERIF,
            STYLE_SERIF
        );
        // Case-sensitive, as `ByteString::Contains` is.
        assert_eq!(face_styles("PT SERIF", "Regular") & STYLE_SERIF, 0);
        assert_eq!(face_styles("PT serif", "Regular") & STYLE_SERIF, 0);
    }

    /// All thirty distinct faces of the oracle's own hermetic font directory,
    /// by name, with the bits `ReportFace` gives them — which for the serif,
    /// script and fixed-pitch terms is **none, for every one of them**.
    ///
    /// No face there is serif because the Croscore serif family is spelled
    /// `Tinos` and the other one `Gelasio`, and neither contains the word.
    /// Reading the bit from PANOSE instead gives eleven of these faces a serif
    /// bit and three a fixed-pitch bit, and splits `Gelasio` three ways —
    /// `Gelasio Bold`'s PANOSE disagrees with its own Regular and Bold Italic
    /// siblings'. Serif is the 16-point term, the largest in the score.
    ///
    /// The whole directory is listed rather than only the faces that were
    /// wrong, so the test reads as a comparison against the oracle's
    /// enumeration and not as a list of past failures.
    #[test]
    fn no_face_in_the_hermetic_font_set_is_serif() {
        for (name, style) in [
            ("Ahem", "Regular"),
            ("Arimo", "Regular"),
            ("Arimo Bold", "Bold"),
            ("Arimo Bold Italic", "Bold Italic"),
            ("Arimo Italic", "Italic"),
            ("Cousine", "Regular"),
            ("Cousine Bold", "Bold"),
            ("Cousine Bold Italic", "Bold Italic"),
            ("Cousine Italic", "Italic"),
            ("DejaVu Sans Bold", "Bold"),
            ("DejaVu Sans Book", "Book"),
            ("GardinerMod", "Regular"),
            ("Garuda", "Regular"),
            ("Gelasio", "Regular"),
            ("Gelasio Bold", "Bold"),
            ("Gelasio Bold Italic", "Bold Italic"),
            ("Gelasio Italic", "Italic"),
            ("Lohit Devanagari", "Regular"),
            ("Lohit Gurmukhi", "Regular"),
            ("Lohit Tamil", "Regular"),
            ("Mukti Narrow", "Regular"),
            ("Noto Color Emoji", "Regular"),
            ("Noto Sans CJK JP Regular", "Regular"),
            ("Noto Sans Khmer", "Regular"),
            ("Noto Sans Symbols2", "Regular"),
            ("Noto Sans Tibetan", "Regular"),
            ("Tinos", "Regular"),
            ("Tinos Bold", "Bold"),
            ("Tinos Bold Italic", "Bold Italic"),
            ("Tinos Italic", "Italic"),
        ] {
            let bits = face_styles(name, style);
            assert_eq!(bits & STYLE_SERIF, 0, "{name} took a serif bit");
            assert_eq!(bits & STYLE_SCRIPT, 0, "{name} took a script bit");
            assert_eq!(bits & STYLE_FIXED_PITCH, 0, "{name} took a fixed-pitch bit");
        }
    }

    /// The enumerator sets three bits and no others, so the script and
    /// fixed-pitch terms of the score are decided entirely by the *request*.
    ///
    /// `Cousine` is the case that would otherwise bite: it is the monospaced
    /// Croscore face, its `post` table says so, and a `FIXED` request would
    /// score 8 more for it than for `Arimo` — which the oracle does not do,
    /// because the folder enumerator never asked the `post` table anything.
    #[test]
    fn a_monospaced_face_takes_no_fixed_pitch_bit() {
        let cousine = face_styles("Cousine", "Regular");
        let arimo = face_styles("Arimo", "Regular");
        assert_eq!(cousine, arimo);
        let fixed = PitchFamily(PitchFamily::FIXED);
        let info = |styles| FaceInfo {
            name: String::new(),
            styles,
            charsets: vec![Charset::Ansi],
        };
        assert_eq!(
            info(cousine).similarity_score(400, false, fixed, false),
            info(arimo).similarity_score(400, false, fixed, false),
            "a fixed-pitch request must not separate Cousine from Arimo"
        );
    }
}
