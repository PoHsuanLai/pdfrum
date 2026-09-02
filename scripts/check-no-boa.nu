#!/usr/bin/env nu
# M15's isolation check: a default build of pdfrum resolves with zero `boa`.
#
# `pdfrum-form`'s `script` feature brings a JavaScript engine and 116 crates
# with it. The feature is default-off, and this script is the mechanical proof
# of that — separate from scripts/ci.nu, on the same reasoning as
# check-no-wgpu.nu, so a reader looking for the blast radius of the engine
# finds one place to look.
#
# A cargo feature is the weaker of the two isolation mechanisms this project
# uses. The GPU backend is isolated by being a *crate nothing depends on*; the
# engine is isolated by a *flag any crate in a workspace can turn on*, and
# feature unification means one member enabling it enables it for the build.
# That is a real weakness, and it is exactly why the check below is not
# optional and why the last two assertions — that the feature does still bring
# boa, at both the engine's crate and the facade that forwards to it — exist:
# without them, every other assertion here would pass vacuously the day
# someone deleted the dependency.
#
# What it asserts, in the order a violation would most likely arrive:
#
#   1. The `pdfrum` facade's default feature set contains no `boa_*` crate at
#      all. This is the claim an embedder cares about: `cargo add pdfrum` must
#      not put a JavaScript engine in their tree.
#   2. `pdfrum-tool`'s default features likewise — the CLI is what a headless
#      or CI build actually runs.
#   3. No workspace crate's default features reach boa, `conformance/` and
#      `benches/` included. This catches the accident the other two would
#      eventually catch anyway — a crate enabling `pdfrum-form/script` "just
#      for a test" — at the point where it is one line to undo.
#   4. The converse: `pdfrum-form --features script` DOES reach `boa_engine`,
#      so none of the above is vacuously true.
#   5. And the facade's own forwarding feature: `pdfrum --features script`
#      DOES reach `boa_engine` too. Added with WP12, which gave `pdfrum` a
#      `script = ["pdfrum-form/script"]` feature so an embedder can turn
#      scripting on without depending on `pdfrum-form` directly. Assertion 1
#      is the claim that feature must not break; this is the claim that it is
#      not merely a name — a forwarding feature that forwarded nothing would
#      pass assertion 1 perfectly and do nothing at all.
#
# Run standalone, or via scripts/ci.nu which calls it.

# Every crate the engine brings that is named for it. A transitive crate with
# some other name arriving is not a policy violation — the policy is about the
# engine, and the engine is what these names are.
const BOA_CRATES = [boa_engine boa_ast boa_parser boa_gc boa_interner boa_string boa_macros]

# The normal-dependency closure of one crate, as a list of package names.
#
# `-e normal` excludes dev- and build-dependencies: a dev-dependency does not
# ship in a consumer's tree, and the claim here is about what an embedder
# resolves.
#
# A `cargo tree` that *fails* is not a clean tree: an unresolvable manifest
# yields no output, which every filter below would read as "depends on
# nothing" and report as ok. The bash predecessor had exactly that hole
# (`2>/dev/null … || true`). Here a non-zero exit is fatal instead, because a
# check that cannot run has not passed.
def deps [crate: string, extra: list<string> = []]: nothing -> list<string> {
    let r = (^cargo tree -e normal -p $crate ...$extra --prefix none | complete)
    if $r.exit_code != 0 {
        print --stderr $"error: `cargo tree -p ($crate)` failed; the tree could not be"
        print --stderr "       resolved, so this check has not run. cargo said:"
        $r.stderr | lines | each {|l| print --stderr $"  ($l)" } | ignore
        exit 1
    }
    $r.stdout | lines | split column ' ' name | get name | uniq
}

def check-clean [crate: string, label: string]: nothing -> bool {
    let found = (deps $crate | where {|d| $d in $BOA_CRATES } | sort)
    if ($found | is-empty) {
        print $"ok: ($label) resolves without boa"
        return true
    }
    print --stderr $"error: ($label) reaches the JavaScript engine:"
    $found | each {|d| print --stderr $"  ($d)" } | ignore
    print --stderr "       M15's isolation rule: no crate's DEFAULT features may"
    print --stderr "       depend on boa. See PLAN.md §M15 and DEPS.md."
    false
}

def main [] {
    cd ($env.FILE_PWD | path dirname)

    mut ok = true

    print "==> M15 isolation: the default tree resolves without boa"
    if not (check-clean pdfrum 'the pdfrum facade (default features)') { $ok = false }
    if not (check-clean pdfrum-tool 'pdfrum-tool (default features)') { $ok = false }

    # Every workspace member.
    #
    # `crates/*/` is not the whole workspace. `conformance/` and `benches/` are
    # members too, and they are the two most likely places for the accident
    # this check exists to catch. They are also the members whose directory
    # name is *not* their crate name (`conformance` and `pdfrum-bench`), so the
    # name is read from each manifest rather than guessed from the path.
    print "==> M15 isolation: no workspace crate's default features reach boa"
    let manifests = ((glob crates/*/Cargo.toml | sort)
        ++ ([conformance/Cargo.toml benches/Cargo.toml benches/corpus-list/Cargo.toml]
            | where {|m| $m | path exists }))
    for manifest in $manifests {
        # `open` parses the manifest as TOML, so this is the package's own
        # `[package] name` rather than the first `name =` in the file — which
        # is what the bash predecessor had to approximate with sed and a
        # comment about table ordering.
        let crate = (do --ignore-errors { open $manifest | get package.name })
        if $crate == null {
            print --stderr $"error: could not read a package name from ($manifest)"
            $ok = false
            continue
        }
        if not (check-clean $crate $crate) { $ok = false }
    }

    # And the converse, so the check cannot pass by the feature having quietly
    # stopped bringing an engine — which would make every assertion above
    # vacuous. This is the assertion that caught a leaked edge in M12c and it
    # is worth repeating.
    print "==> M15 isolation: the script feature does still bring boa"
    for probe in [
        {crate: 'pdfrum-form', why: "the engine's own crate"}
        {crate: 'pdfrum', why: "the facade's forwarding feature (WP12)"}
    ] {
        if 'boa_engine' in (deps $probe.crate [--features script]) {
            print $"ok: ($probe.crate) --features script depends on boa_engine — ($probe.why)"
        } else {
            print --stderr $"error: ($probe.crate) --features script no longer reaches boa_engine —"
            print --stderr "       every check above is trivially true and this script is"
            print --stderr "       measuring nothing."
            $ok = false
        }
    }

    if not $ok { exit 1 }
    print ""
    print "M15 isolation: green."
}
