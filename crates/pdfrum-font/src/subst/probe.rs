//! Reading just enough of a font file to describe the face.
//!
//! The directory scan (`subst::db::SystemFontDb::scan`) asks
//! [`super::db::describe`] for a face's display name and its `OS/2` code-page
//! ranges, and nothing else. Reading the file whole to answer that is what a
//! scan of a real font directory cannot afford: the oracle's hermetic
//! `test_fonts` set is 33.8 MB across 31 files, 26 MB of it one CJK face and
//! one colour-emoji face the substitution ladder rarely picks, and faulting all
//! of it in cost 20.5 ms of `read` per cold render.
//!
//! So this module reads the sfnt header and table directory — a few hundred
//! bytes — and then only the byte ranges of the two tables `describe` consults,
//! reassembling them into a minimal single-face sfnt. The reassembled font goes
//! through the *same* `skrifa` parse the whole file went through, so the parsed
//! `FaceInfo` is identical by construction rather than by a second
//! implementation of the two tables; `a_probe_describes_a_face_exactly_as_the_whole_file_does` in
//! `db`'s tests asserts that over every font file on the machine.
//!
//! A file that is not an sfnt — a bare CFF, a Type 1 `.pfb` — has no table
//! directory to read, so [`FaceProbe::read`] reports
//! [`ProbeError::NotSfnt`] and the caller falls back to the whole-file read it
//! did before. `describe` rejects those files anyway (`skrifa::FontRef` parses
//! only sfnt-wrapped faces), so the fallback costs a read of a file that was
//! never going to yield a face; both kinds are rare in a system font directory.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The four tables' worth of header an sfnt starts with: the version tag, the
/// table count, and three fields the directory does not need but the format
/// reserves. A table record is four `u32`s after its four-byte tag.
const SFNT_HEADER_LEN: usize = 12;
const TABLE_RECORD_LEN: usize = 16;

/// The `ttcf` header: the tag, a version, the face count, then one `u32`
/// offset per face.
const TTC_HEADER_LEN: usize = 12;

/// The tables [`super::db::describe`] reads, and the only ones a probe fetches.
///
/// `name` carries the family and subfamily strings; `OS/2` carries the
/// code-page ranges the charset bits come from. Keeping the set as a constant
/// rather than a parameter is deliberate: it is the definition of what a probe
/// is for, and a caller that needed a third table would be asking for a
/// different type.
const WANTED: [Tag; 2] = [Tag(*b"name"), Tag(*b"OS/2")];

/// A four-byte sfnt table tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Tag([u8; 4]);

/// Why a file could not be probed.
///
/// [`ProbeError::NotSfnt`] is the one a caller acts on — it means "this file
/// has no table directory, read it whole if you still want it". The others say
/// the file is unreadable or malformed, and a scan skips such a face either
/// way.
#[derive(Debug)]
pub(super) enum ProbeError {
    /// The file does not begin with a recognised sfnt or `ttcf` signature.
    NotSfnt,
    /// The signature was right but the directory was truncated or absurd.
    Malformed,
    /// The file could not be opened or read.
    Io,
}

impl From<std::io::Error> for ProbeError {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}

/// One face's table directory: where each table lives in the file.
///
/// Parsed from the header alone, before any table body is read. The entries
/// keep the tag, offset, length and checksum verbatim, because a reassembled
/// sfnt has to carry the same records for the tables it keeps.
#[derive(Debug)]
struct TableDirectory {
    /// The sfnt version tag of the face this directory belongs to —
    /// `0x00010000` for TrueType outlines, `OTTO` for CFF ones. Carried so the
    /// reassembled font declares what the original declared.
    sfnt_version: [u8; 4],
    entries: Vec<TableRecord>,
}

/// One row of a table directory.
#[derive(Debug, Clone, Copy)]
struct TableRecord {
    tag: Tag,
    checksum: u32,
    offset: u32,
    length: u32,
}

/// A face's `name` and `OS/2` tables, wrapped as a standalone sfnt.
///
/// The point of the type is that it owns *only* what was fetched: a few
/// kilobytes for a face whose file may be sixteen megabytes. `as_font_bytes`
/// hands out a byte slice that parses as a single-face font, so the describing
/// code does not know it is looking at a probe rather than a file.
#[derive(Debug)]
pub(super) struct FaceProbe {
    sfnt: Vec<u8>,
}

impl FaceProbe {
    /// Read the tables `describe` needs for face `index` of the file at `path`.
    ///
    /// Three reads for a simple font — the header, the directory, then one per
    /// wanted table — against one read of the whole file.
    pub(super) fn read(path: &Path, index: u32) -> Result<Self, ProbeError> {
        let mut file = File::open(path)?;
        let directory = TableDirectory::read(&mut file, index)?;
        let mut kept = Vec::new();
        for tag in WANTED {
            let Some(record) = directory.find(tag) else {
                // A face with no `OS/2` is normal and describes fine — it just
                // claims no code pages. A face with no `name` fails later, in
                // the same place a whole-file read would have failed.
                continue;
            };
            let body = read_at(&mut file, record.offset, record.length)?;
            kept.push((*record, body));
        }
        Ok(Self {
            sfnt: assemble(directory.sfnt_version, &kept),
        })
    }

    /// The probe as a parseable single-face sfnt.
    pub(super) fn as_font_bytes(&self) -> &[u8] {
        &self.sfnt
    }
}

impl TableDirectory {
    /// Parse the directory for face `index`, following a `ttcf` header when the
    /// file is a collection.
    fn read(file: &mut File, index: u32) -> Result<Self, ProbeError> {
        let mut header = [0u8; SFNT_HEADER_LEN];
        file.rewind()?;
        read_exact_or_malformed(file, &mut header)?;
        let tag = subarray(&header, 0)?;

        let directory_start = if &tag == b"ttcf" {
            // A collection: the face's own offset table is elsewhere in the
            // file, and `index` selects which.
            let count = be_u32(&header, 8)?;
            if index >= count {
                return Err(ProbeError::Malformed);
            }
            let mut offsets = vec![0u8; count as usize * 4];
            file.seek(SeekFrom::Start(TTC_HEADER_LEN as u64))?;
            read_exact_or_malformed(file, &mut offsets)?;
            let at = index
                .checked_mul(4)
                .ok_or(ProbeError::Malformed)?
                .try_into()
                .map_err(|_| ProbeError::Malformed)?;
            be_u32(&offsets, at)?
        } else if is_sfnt_version(tag) {
            // A single-face file: only index 0 exists, which is what `fontdb`
            // reports for one.
            if index != 0 {
                return Err(ProbeError::Malformed);
            }
            0
        } else {
            return Err(ProbeError::NotSfnt);
        };

        let offset_table = read_at(file, directory_start, SFNT_HEADER_LEN as u32)?;
        let sfnt_version: [u8; 4] = subarray(&offset_table, 0)?;
        if !is_sfnt_version(sfnt_version) {
            return Err(ProbeError::Malformed);
        }
        let num_tables = be_u16(&offset_table, 4)?;
        let records_at = directory_start
            .checked_add(SFNT_HEADER_LEN as u32)
            .ok_or(ProbeError::Malformed)?;
        let records_len = u32::from(num_tables)
            .checked_mul(TABLE_RECORD_LEN as u32)
            .ok_or(ProbeError::Malformed)?;
        let records = read_at(file, records_at, records_len)?;

        let mut entries = Vec::with_capacity(num_tables as usize);
        for i in 0..num_tables as usize {
            let at = i
                .checked_mul(TABLE_RECORD_LEN)
                .ok_or(ProbeError::Malformed)?;
            entries.push(TableRecord {
                tag: Tag(subarray(&records, at)?),
                checksum: be_u32(&records, at + 4)?,
                offset: be_u32(&records, at + 8)?,
                length: be_u32(&records, at + 12)?,
            });
        }
        Ok(Self {
            sfnt_version,
            entries,
        })
    }

    fn find(&self, tag: Tag) -> Option<&TableRecord> {
        self.entries.iter().find(|record| record.tag == tag)
    }
}

/// Build a single-face sfnt carrying exactly the tables handed in.
///
/// The offsets are rewritten to point at the bodies' new positions; every table
/// is padded to a four-byte boundary as the format requires. The
/// `searchRange`/`entrySelector`/`rangeShift` triple is written as the spec
/// derives it, so a strict parser sees a well-formed file — `skrifa` does not
/// check them, but a font that is only nearly valid is a trap for whoever reads
/// this next.
fn assemble(sfnt_version: [u8; 4], tables: &[(TableRecord, Vec<u8>)]) -> Vec<u8> {
    let count = u16::try_from(tables.len()).unwrap_or(u16::MAX);
    let entry_selector = if count == 0 { 0 } else { count.ilog2() };
    // `search_range` is defined as the largest power of two not exceeding
    // `count`, times sixteen — so it never exceeds `count * 16` and the shift
    // below cannot go negative. A face carrying neither wanted table assembles
    // an empty directory, where all three fields are zero.
    let search_range = if count == 0 {
        0
    } else {
        (1u32 << entry_selector) * 16
    };
    let range_shift = u32::from(count) * 16 - search_range;

    let mut out = Vec::new();
    out.extend_from_slice(&sfnt_version);
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&(search_range as u16).to_be_bytes());
    out.extend_from_slice(&(entry_selector as u16).to_be_bytes());
    out.extend_from_slice(&(range_shift as u16).to_be_bytes());

    let mut body_at = SFNT_HEADER_LEN + tables.len() * TABLE_RECORD_LEN;
    for (record, body) in tables {
        out.extend_from_slice(&record.tag.0);
        out.extend_from_slice(&record.checksum.to_be_bytes());
        out.extend_from_slice(&(body_at as u32).to_be_bytes());
        out.extend_from_slice(&record.length.to_be_bytes());
        body_at += (body.len() + 3) & !3;
    }
    for (_, body) in tables {
        out.extend_from_slice(body);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}

/// The sfnt version tags a face can start with.
///
/// `0x00010000` and `true` are TrueType outlines, `OTTO` is CFF ones. `typ1`
/// is the old Type 1 wrapper, which `skrifa` does not parse — it is excluded so
/// such a file takes the whole-file path and fails there, exactly as before.
fn is_sfnt_version(tag: [u8; 4]) -> bool {
    matches!(&tag, [0x00, 0x01, 0x00, 0x00] | b"true" | b"OTTO")
}

/// Read `length` bytes at `offset`, refusing a length no font table has.
fn read_at(file: &mut File, offset: u32, length: u32) -> Result<Vec<u8>, ProbeError> {
    // A `name` or `OS/2` table is kilobytes; the cap only stops a corrupt
    // directory from asking for a gigabyte of allocation.
    const MAX_TABLE_LEN: u32 = 64 * 1024 * 1024;
    if length > MAX_TABLE_LEN {
        return Err(ProbeError::Malformed);
    }
    let mut buf = vec![0u8; length as usize];
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    read_exact_or_malformed(file, &mut buf)?;
    Ok(buf)
}

/// `read_exact`, with a short file reported as malformed rather than as I/O —
/// a truncated directory is a bad font, not a bad disk.
fn read_exact_or_malformed(file: &mut File, buf: &mut [u8]) -> Result<(), ProbeError> {
    file.read_exact(buf).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            ProbeError::Malformed
        } else {
            ProbeError::Io
        }
    })
}

fn subarray(bytes: &[u8], at: usize) -> Result<[u8; 4], ProbeError> {
    bytes
        .get(at..at + 4)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .ok_or(ProbeError::Malformed)
}

fn be_u32(bytes: &[u8], at: usize) -> Result<u32, ProbeError> {
    subarray(bytes, at).map(u32::from_be_bytes)
}

fn be_u16(bytes: &[u8], at: usize) -> Result<u16, ProbeError> {
    bytes
        .get(at..at + 2)
        .and_then(|s| <[u8; 2]>::try_from(s).ok())
        .map(u16::from_be_bytes)
        .ok_or(ProbeError::Malformed)
}
