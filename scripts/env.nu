# The four paths every script and test outside this repository needs, resolved
# in one place.
#
# Nothing here is machine-specific. Each value is an environment variable with
# a default expressed **relative to this repository**, so a fresh clone — a
# GitHub CI checkout included — resolves every one of them without a single
# path being edited, and a machine whose layout differs overrides the one
# variable that differs rather than patching a dozen files.
#
# | variable                  | default                            |
# |---------------------------|------------------------------------|
# | `PDFRUM_ORACLE_CHECKOUT`  | `<repo>/../pdfium-c++`             |
# | `PDFRUM_ORACLE_BIN`       | `<checkout>/out/Release/pdfium_test` |
# | `PDFRUM_GOLDENS`          | `<repo>/conformance/goldens`       |
# | `PDFRUM_TARGET_ROOT`      | `<repo>/../cargo-target`           |
# | `PDFRUM_WORKTREE_ROOT`    | `<repo>/../worktrees`              |
#
# The oracle checkout's default is `../pdfium-c++` because that is where
# README.md already says it lives — a sibling of this
# repository. `conformance/src/main.rs` resolves the same four with the same
# defaults through clap's `env =`, so the CLI and the scripts cannot drift.
#
# Usage from a script in this directory:
#
#     use env.nu *
#     let oracle = (oracle-bin)
#
# Every function returns an absolute path, resolved but **not** required to
# exist: a caller that needs the path to be real says so itself, with its own
# message about what to do. `require-oracle-bin` is that message, for the
# callers that all want the same one.

# This repository's root — the directory above `scripts/`.
#
# `$env.FILE_PWD` is the *importing* script's directory under nushell's module
# semantics, so this is computed from a constant path relative to this file
# instead, which is the same answer wherever it is used from.
export def repo-root []: nothing -> string {
    ($env.CURRENT_FILE | path dirname | path dirname | path expand)
}

# The read-only C++ PDFium checkout: `testing/corpus`, `testing/resources`,
# `third_party/test_fonts`, and the sources the table generators read.
export def oracle-checkout []: nothing -> string {
    ($env.PDFRUM_ORACLE_CHECKOUT? | default ((repo-root) | path join '..' 'pdfium-c++') | path expand)
}

# The oracle's `pdfium_test` binary, built from the oracle checkout.
export def oracle-bin []: nothing -> string {
    ($env.PDFRUM_ORACLE_BIN?
        | default ((oracle-checkout) | path join 'out' 'Release' 'pdfium_test')
        | path expand)
}

# The golden store. `.gitignore`d, so a fresh checkout has none until
# `conformance generate-goldens` has filled it.
export def goldens []: nothing -> string {
    ($env.PDFRUM_GOLDENS? | default ((repo-root) | path join 'conformance' 'goldens') | path expand)
}

# Where the per-agent `CARGO_TARGET_DIR` trees live, one directory per name.
#
# `CARGO_TARGET_DIR` names *one* tree; this names the directory holding all of
# them, which is why it cannot simply be read off that variable — a caller
# building into `…/cargo-target/paths` would otherwise have `clean-targets.nu`
# sweep its siblings from inside itself.
export def target-root []: nothing -> string {
    ($env.PDFRUM_TARGET_ROOT? | default ((repo-root) | path join '..' 'cargo-target') | path expand)
}

# Where `git worktree add` puts working trees, one directory per branch.
export def worktree-root []: nothing -> string {
    ($env.PDFRUM_WORKTREE_ROOT? | default ((repo-root) | path join '..' 'worktrees') | path expand)
}

# The oracle binary, or an actionable failure. For the callers that cannot
# proceed without it and all want to say the same thing.
export def require-oracle-bin [] {
    let bin = (oracle-bin)
    if ($bin | path exists) { return $bin }
    print --stderr $"error: no pdfium_test at ($bin)"
    print --stderr "       build it, or set PDFRUM_ORACLE_BIN to"
    print --stderr "       an existing binary (PDFRUM_ORACLE_CHECKOUT moves the"
    print --stderr "       whole checkout)."
    exit 1
}
