//! Whole-file commands: `repair`, `optimize`, `security decrypt`.
//!
//! All three are one full rewrite with different options, and say so in
//! their help: opening is the recovery, and a rewrite from the trailer drops
//! every object nothing points at and re-encodes every stream.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use pdfrum::{SaveOptions, Update};

use crate::cmd::pages::save_options;
use crate::out;
use crate::out::outln;

/// `repair` and `optimize` share this: open with recovery, rewrite whole.
///
/// `repair` reports what was recovered on the way; `optimize` reports the
/// size change. The bytes written are the same.
pub fn rewrite(
    file: &Path,
    password: Option<&str>,
    output: &Path,
    deterministic: bool,
    verb: &str,
) -> Result<ExitCode> {
    let doc = out::open_quietly(file, password)?;
    let before = doc.bytes().len();
    let options = SaveOptions {
        update: Update::Rewrite,
        ..save_options(deterministic, doc.bytes())
    };
    doc.save_with(output, &options)
        .with_context(|| format!("cannot write {}", output.display()))?;
    let after = std::fs::metadata(output).map_or(0, |m| m.len());
    let notices = doc.all_diagnostics().len();
    outln!(
        "{}: {verb} {} -> {} bytes{}",
        output.display(),
        before,
        after,
        if notices > 0 {
            format!(
                "; {notices} notice{} recovered on open",
                if notices == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        }
    );
    Ok(ExitCode::SUCCESS)
}

/// `security decrypt`: the same rewrite with `/Encrypt` dropped.
pub fn decrypt(
    file: &Path,
    password: Option<&str>,
    output: &Path,
    deterministic: bool,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    if !doc.is_encrypted() {
        bail!("{} is not encrypted", file.display());
    }
    let options = SaveOptions {
        update: Update::Rewrite,
        remove_security: true,
        ..save_options(deterministic, doc.bytes())
    };
    doc.save_with(output, &options)
        .with_context(|| format!("cannot write {}", output.display()))?;
    outln!("{}: decrypted", output.display());
    Ok(ExitCode::SUCCESS)
}
