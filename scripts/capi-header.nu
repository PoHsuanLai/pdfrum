#!/usr/bin/env nu
# Gate the committed C header against cbindgen and against libpdfrum.so.
#
#   ./scripts/capi-header.nu            # same as check
#   ./scripts/capi-header.nu check      # CI
#   ./scripts/capi-header.nu update     # regenerate include/pdfrum.h
#
# Requires cbindgen: cargo install cbindgen --locked

const CRATE = 'crates/pdfrum-capi'
const HEADER = 'crates/pdfrum-capi/include/pdfrum.h'
# Declared under `#if defined(PDFRUM_MARKDOWN)`; a default build does not
# export it. Named, not derived.
const FEATURE_GUARDED = [pdfrum_page_markdown]

def require-tool []: nothing -> nothing {
    if (which cbindgen | is-empty) {
        print --stderr "error: cbindgen is not installed."
        print --stderr "       install with: cargo install cbindgen --locked"
        exit 1
    }
}

def generate [out: string]: nothing -> nothing {
    let r = (^cbindgen --config $"($CRATE)/cbindgen.toml" --crate pdfrum-capi --output $out
        | complete)
    if $r.exit_code != 0 {
        print --stderr "error: cbindgen failed:"
        $r.stderr | lines | each {|l| print --stderr $"  ($l)" } | ignore
        exit 1
    }
}

# `name(` so comments that mention a function do not count as declarations.
def declared []: nothing -> list<string> {
    open --raw $HEADER
    | lines
    | where {|l| not ($l | str trim | str starts-with '//') }
    | where {|l| not ($l | str trim | str starts-with '*') }
    | parse --regex '(?<name>\bpdfrum_[a-z0-9_]+)\s*\('
    | get name
    | uniq
    | sort
}

def exported [lib: string]: nothing -> list<string> {
    ^nm -D --defined-only $lib
    | lines
    | parse --regex '^\S+\s+T\s+(?<name>pdfrum_[a-z0-9_]+)$'
    | get name
    | uniq
    | sort
}

# `cargo build` names the cdylib after the lib target (`pdfrum_capi`);
# `cargo cinstall` ships `libpdfrum.so`.
def library []: nothing -> string {
    let target = ($env.CARGO_TARGET_DIR? | default 'target')
    let candidates = [
        ($target | path join release libpdfrum.so)
        ($target | path join release libpdfrum_capi.so)
    ]
    let found = ($candidates | where {|p| $p | path exists })
    if ($found | is-empty) {
        print --stderr "error: no built libpdfrum shared library. Looked for:"
        $candidates | each {|p| print --stderr $"  ($p)" } | ignore
        print --stderr "       build it with: cargo build -p pdfrum-capi --release"
        exit 1
    }
    $found | first
}

def "main update" [] {
    cd ($env.FILE_PWD | path dirname)
    require-tool
    generate $HEADER
    let count = (declared | length)
    print $"Wrote ($HEADER) — ($count) declared functions."
}

def "main check" [] {
    cd ($env.FILE_PWD | path dirname)
    require-tool

    print "==> the committed C header matches the Rust source"
    let fresh = (mktemp --tmpdir --suffix .h pdfrum-header-XXXXXX)
    generate $fresh
    let committed = (open --raw $HEADER)
    let regenerated = (open --raw $fresh)
    if $committed != $regenerated {
        let a = ($committed | lines)
        let b = ($regenerated | lines)
        print --stderr $"error: ($HEADER) differs from a fresh generation."
        for line in ($a | where {|l| $l not-in $b }) { print --stderr $"  - ($line)" }
        for line in ($b | where {|l| $l not-in $a }) { print --stderr $"  + ($line)" }
        print --stderr "       if intended: ./scripts/capi-header.nu update"
        rm --force $fresh
        exit 1
    }
    rm --force $fresh
    let names = (declared)
    print $"ok: the header is what the source generates — ($names | length) functions"

    print "==> every declared function is an exported symbol, and the reverse"
    let lib = (library)
    let symbols = (exported $lib)
    let undefined = ($names
        | where {|n| $n not-in $symbols and $n not-in $FEATURE_GUARDED }
        | sort)
    let undeclared = ($symbols | where {|s| $s not-in $names } | sort)

    if not ($undefined | is-empty) {
        print --stderr "error: declared in the header but not exported by the library:"
        $undefined | each {|n| print --stderr $"  ($n)" } | ignore
    }
    if not ($undeclared | is-empty) {
        print --stderr "error: exported by the library but not declared in the header:"
        $undeclared | each {|n| print --stderr $"  ($n)" } | ignore
    }
    if not (($undefined | is-empty) and ($undeclared | is-empty)) {
        exit 1
    }
    let guarded = ($names | where {|n| $n in $FEATURE_GUARDED and $n not-in $symbols })
    print $"ok: ($symbols | length) exported symbols match the header's declarations"
    if not ($guarded | is-empty) {
        print $"    \(($guarded | str join ', ') declared under a feature guard this build does not set\)"
    }
    print ""
    print "C header: green."
}

def main [] {
    main check
}
