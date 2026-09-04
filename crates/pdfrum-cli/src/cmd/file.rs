//! Whole-file commands: `repair`, `optimize`, `security decrypt`.
//!
//! All three are one full rewrite with different options, and say so in
//! their help: opening is the recovery, and a rewrite from the trailer drops
//! every object nothing points at and re-encodes every stream.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use pdfrum::{Encryption, Permissions, SaveOptions, Update};

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

/// What `security encrypt` was asked for.
pub struct EncryptRequest<'a> {
    pub file: &'a Path,
    pub password: Option<&'a str>,
    pub user_password: &'a str,
    pub owner_password: &'a str,
    /// The `--allow` list, or `None` for everything.
    pub allow: Option<&'a str>,
    pub encrypt_metadata: bool,
    pub output: &'a Path,
    pub deterministic: bool,
}

/// The `--allow` words: `print`, `modify`, `copy`, `annotate`, `fill-forms`,
/// `extract`, `assemble`, `print-hq`, or `all` / `none`.
pub fn parse_allow(text: &str) -> Result<Permissions> {
    let mut p = Permissions::NONE;
    for word in text.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        match word {
            "all" => p = Permissions::ALL,
            "none" => p = Permissions::NONE,
            "print" => p.print = true,
            "modify" => p.modify = true,
            "copy" => p.copy = true,
            "annotate" => p.annotate = true,
            "fill-forms" | "fill_form" | "forms" => p.fill_form = true,
            "extract" => p.extract = true,
            "assemble" => p.assemble = true,
            "print-hq" | "print_high_quality" => p.print_high_quality = true,
            other => bail!(
                "--allow: {other:?} is not one of print, modify, copy, annotate, fill-forms, extract, assemble, print-hq, all, none"
            ),
        }
    }
    Ok(p)
}

/// `security encrypt`: AES-256 (revision 6) under two passwords. An
/// encrypted input is refused: decrypt it first, then encrypt the result.
pub fn encrypt(req: &EncryptRequest<'_>) -> Result<ExitCode> {
    let doc = out::open(req.file, req.password)?;
    if doc.is_encrypted() {
        bail!(
            "{} is already encrypted; `security decrypt` it first, then encrypt the result",
            req.file.display()
        );
    }
    if req.user_password.is_empty() && req.owner_password.is_empty() {
        bail!("give at least one of --user-password and --owner-password");
    }
    let permissions = match req.allow {
        Some(list) => parse_allow(list)?,
        None => Permissions::ALL,
    };
    let options = SaveOptions {
        update: Update::Rewrite,
        encrypt: Some(Encryption {
            user_password: req.user_password.as_bytes().to_vec(),
            owner_password: req.owner_password.as_bytes().to_vec(),
            permissions,
            encrypt_metadata: req.encrypt_metadata,
        }),
        ..save_options(req.deterministic, doc.bytes())
    };
    doc.save_with(req.output, &options)
        .with_context(|| format!("cannot write {}", req.output.display()))?;
    outln!(
        "{}: encrypted (AES-256){}",
        req.output.display(),
        if req.user_password.is_empty() {
            "; opens without a password, the owner password unlocks it"
        } else {
            ""
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
