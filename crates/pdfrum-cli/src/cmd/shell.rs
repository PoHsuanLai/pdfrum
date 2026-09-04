//! `pdfrum completions` and `pdfrum manpage`: the shell's and the manual's
//! view of the command tree, generated from the same clap definition that
//! parses it, so they cannot drift.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Command, CommandFactory};
use clap_complete::Shell;

use crate::Cli;
use crate::out::outln;

pub fn completions(shell: Shell) -> ExitCode {
    let mut command = Cli::command();
    let name = command.get_name().to_owned();
    let mut buffer = Vec::new();
    clap_complete::generate(shell, &mut command, name, &mut buffer);
    crate::out::write_bytes(&buffer);
    ExitCode::SUCCESS
}

/// One roff page per command, `pdfrum.1`, `pdfrum-extract.1`,
/// `pdfrum-extract-text.1` and so on, into `dir`.
pub fn manpage(dir: &Path) -> Result<ExitCode> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let mut command = Cli::command();
    // Building fills in each subcommand's display name, `pdfrum-extract`,
    // which is what the manual calls the page.
    command.build();
    let mut written = 0;
    write_tree(dir, &command, "pdfrum", &mut written)?;
    outln!("{}: {written} manual pages", dir.display());
    Ok(ExitCode::SUCCESS)
}

fn write_tree(dir: &Path, command: &Command, name: &str, written: &mut usize) -> Result<()> {
    let page = clap_mangen::Man::new(command.clone());
    let mut buffer = Vec::new();
    page.render(&mut buffer)
        .context("cannot render the manual page")?;
    let path = dir.join(format!("{name}.1"));
    std::fs::write(&path, buffer).with_context(|| format!("cannot write {}", path.display()))?;
    *written += 1;
    for sub in command.get_subcommands() {
        // `help` is clap's own and has no page of its own anywhere.
        if sub.is_hide_set() || sub.get_name() == "help" {
            continue;
        }
        let child = format!("{name}-{}", sub.get_name());
        write_tree(dir, sub, &child, written)?;
    }
    Ok(())
}
