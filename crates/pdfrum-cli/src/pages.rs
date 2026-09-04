//! The `--pages` grammar: 1-based numbers and ranges, `end` for the last page.
//!
//! `3`, `1-5`, `2,7,10-end`, `end`. Order is kept and duplicates are kept,
//! because a caller who asks for `3,1,3` is asking for that.

use anyhow::{Context, Result, bail};

/// The 0-based page indices `spec` names, against a document of `count` pages.
///
/// `None` means every page, in order.
pub fn select(spec: Option<&str>, count: u32) -> Result<Vec<u32>> {
    let Some(spec) = spec else {
        return Ok((0..count).collect());
    };
    let mut pages = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            bail!("empty entry in --pages {spec:?}");
        }
        let (lo, hi) = if let Some((lo, hi)) = part.split_once('-') {
            (number(lo, count)?, number(hi, count)?)
        } else {
            let n = number(part, count)?;
            (n, n)
        };
        if lo > hi {
            bail!("--pages {part:?} runs backwards");
        }
        pages.extend(lo - 1..hi);
    }
    Ok(pages)
}

/// One 1-based page number, or `end`.
fn number(word: &str, count: u32) -> Result<u32> {
    let word = word.trim();
    if word == "end" {
        if count == 0 {
            bail!("the document has no pages");
        }
        return Ok(count);
    }
    let n: u32 = word
        .parse()
        .with_context(|| format!("--pages: {word:?} is not a page number"))?;
    if n == 0 {
        bail!("--pages: pages are numbered from 1");
    }
    if n > count {
        bail!("--pages: page {n} is past the last page ({count})");
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::select;

    #[test]
    fn nothing_means_every_page_in_order() {
        assert_eq!(select(None, 3).unwrap(), vec![0, 1, 2]);
        assert!(select(None, 0).unwrap().is_empty());
    }

    #[test]
    fn numbers_ranges_and_end_are_one_based_and_kept_in_order() {
        assert_eq!(select(Some("3"), 5).unwrap(), vec![2]);
        assert_eq!(select(Some("1-3"), 5).unwrap(), vec![0, 1, 2]);
        assert_eq!(select(Some("2,5,3-end"), 5).unwrap(), vec![1, 4, 2, 3, 4]);
        assert_eq!(select(Some("end"), 5).unwrap(), vec![4]);
        assert_eq!(select(Some(" 1 , 2 "), 5).unwrap(), vec![0, 1]);
    }

    #[test]
    fn the_mistakes_are_named() {
        for (spec, what) in [
            ("0", "numbered from 1"),
            ("6", "past the last page"),
            ("3-1", "runs backwards"),
            ("x", "not a page number"),
            ("1,,2", "empty entry"),
        ] {
            let err = select(Some(spec), 5).unwrap_err().to_string();
            assert!(err.contains(what), "{spec}: {err}");
        }
        assert!(select(Some("end"), 0).is_err());
    }
}
