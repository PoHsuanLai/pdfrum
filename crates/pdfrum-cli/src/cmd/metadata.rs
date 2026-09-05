//! `pdfrum metadata set`: the `/Info` keys, changed and saved.
//!
//! `info` reads them; this writes them. A key not named is kept as it
//! was, so setting the title leaves the author alone; `--clear` removes
//! keys by the names `info` prints. The save stamps `/ModDate` with its
//! own time unless `--deterministic`, which keeps the date as it was.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Result, bail};
use pdfrum::{Document, Metadata};

use crate::cmd::pages::save_options;
use crate::out;
use crate::term::Term;

/// The keys `--clear` names: the words `info` prints for them.
const KEYS: [&str; 8] = [
    "title", "author", "subject", "keywords", "creator", "producer", "created", "modified",
];

/// What to set and what to clear.
#[derive(Debug, Default)]
pub struct Changes {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    /// Keys to remove, by the names in [`KEYS`].
    pub clear: Vec<String>,
}

/// The `--clear` list: comma-separated key names, checked by [`apply`].
pub fn parse_clear(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `metadata` with `changes` applied, and how many keys were set and
/// cleared. An unknown key in `clear`, or nothing to change, is an error
/// before anything is touched.
pub fn apply(mut metadata: Metadata, changes: &Changes) -> Result<(Metadata, usize, usize)> {
    let mut set = 0;
    for (field, value) in [
        (&mut metadata.title, &changes.title),
        (&mut metadata.author, &changes.author),
        (&mut metadata.subject, &changes.subject),
        (&mut metadata.keywords, &changes.keywords),
        (&mut metadata.creator, &changes.creator),
    ] {
        if let Some(value) = value {
            *field = Some(value.clone());
            set += 1;
        }
    }
    for key in &changes.clear {
        let field = match key.as_str() {
            "title" => &mut metadata.title,
            "author" => &mut metadata.author,
            "subject" => &mut metadata.subject,
            "keywords" => &mut metadata.keywords,
            "creator" => &mut metadata.creator,
            "producer" => &mut metadata.producer,
            "created" => &mut metadata.creation_date,
            "modified" => &mut metadata.modification_date,
            other => bail!(
                "--clear: {other:?} is not a metadata key; the keys are {}",
                KEYS.join(", ")
            ),
        };
        *field = None;
    }
    if set == 0 && changes.clear.is_empty() {
        bail!("nothing to change: give a key to set (--title, --author, ...) or --clear");
    }
    Ok((metadata, set, changes.clear.len()))
}

/// `doc` with its metadata changed, as the bytes of a saved file, and how
/// many keys were set and cleared.
pub fn set_bytes(
    doc: &Document,
    changes: &Changes,
    deterministic: bool,
) -> Result<(Vec<u8>, usize, usize)> {
    let (metadata, set, cleared) = apply(doc.metadata(), changes)?;
    let mut edit = doc.edit();
    edit.set_metadata(&metadata);
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, doc.bytes()))?;
    Ok((bytes, set, cleared))
}

/// What `metadata set` was asked for.
pub struct Set<'a> {
    pub file: &'a Path,
    pub password: Option<&'a str>,
    pub changes: &'a Changes,
    pub output: &'a Path,
    pub deterministic: bool,
}

/// `metadata set`: the keys named set, the keys cleared removed, the rest
/// kept, into a new file.
pub fn set(req: &Set<'_>, term: Term) -> Result<ExitCode> {
    let sink = out::Sink::new(req.output, "PDF")?;
    let doc = out::open(req.file, req.password)?;
    let (bytes, set, cleared) = set_bytes(&doc, req.changes, req.deterministic)?;
    let mut detail = Vec::new();
    if set > 0 {
        detail.push(format!("{set} key{}", if set == 1 { "" } else { "s" }));
    }
    if cleared > 0 {
        detail.push(format!("{cleared} cleared"));
    }
    sink.finish(term, &bytes, "metadata set", Some(&detail.join(", ")))?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use pdfrum::Metadata;

    use super::{Changes, apply, parse_clear};

    #[test]
    fn the_keys_named_change_and_the_rest_stay() {
        let mut before = Metadata::default();
        before.title = Some("Old".into());
        before.author = Some("Ann".into());
        before.producer = Some("pdfrum".into());
        let changes = Changes {
            title: Some("New".into()),
            subject: Some("S".into()),
            clear: parse_clear("producer, author"),
            ..Changes::default()
        };
        let (after, set, cleared) = apply(before, &changes).unwrap();
        assert_eq!((set, cleared), (2, 2));
        assert_eq!(after.title.as_deref(), Some("New"));
        assert_eq!(after.subject.as_deref(), Some("S"));
        assert!(after.author.is_none() && after.producer.is_none());
    }

    #[test]
    fn an_unknown_key_and_no_change_at_all_are_refused() {
        let unknown = Changes {
            clear: vec!["date".into()],
            ..Changes::default()
        };
        let err = apply(Metadata::default(), &unknown)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a metadata key"), "{err}");
        let err = apply(Metadata::default(), &Changes::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("nothing to change"), "{err}");
        assert!(parse_clear(" , ").is_empty());
    }
}
