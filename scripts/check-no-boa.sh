#!/usr/bin/env bash
# M15's isolation check: a default build of pdfrum resolves with zero `boa`.
#
# `pdfrum-form`'s `script` feature brings a JavaScript engine and 116 crates
# with it. The feature is default-off, and this script is the mechanical proof
# of that — separate from scripts/ci.sh, on the same reasoning as
# check-no-wgpu.sh, so a reader looking for the blast radius of the engine
# finds one place to look.
#
# A cargo feature is the weaker of the two isolation mechanisms this project
# uses. The GPU backend is isolated by being a *crate nothing depends on*; the
# engine is isolated by a *flag any crate in a workspace can turn on*, and
# feature unification means one member enabling it enables it for the build.
# That is a real weakness, and it is exactly why the check below is not
# optional and why the fourth assertion — that the feature does still bring
# boa — exists: without it, every other assertion here would pass vacuously
# the day someone deleted the dependency.
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
#
# Run standalone, or via scripts/ci.sh which calls it.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0

# Every crate the engine brings that is named for it. A transitive crate with
# some other name arriving is not a policy violation — the policy is about the
# engine, and the engine is what these names are.
boa_crates='^(boa_engine|boa_ast|boa_parser|boa_gc|boa_interner|boa_string|boa_macros)$'

check_clean() {
    local crate="$1" label="$2"
    local found
    # `-e normal` excludes dev- and build-dependencies: a dev-dependency does
    # not ship in a consumer's tree, and the claim here is about what an
    # embedder resolves.
    found=$(cargo tree -e normal -p "$crate" --prefix none 2>/dev/null \
        | awk '{print $1}' | sort -u \
        | grep -E "$boa_crates" \
        || true)
    if [ -n "$found" ]; then
        echo "error: $label reaches the JavaScript engine:" >&2
        printf '  %s\n' $found >&2
        echo "       M15's isolation rule: no crate's DEFAULT features may" >&2
        echo "       depend on boa. See PLAN.md §M15 and DEPS.md." >&2
        fail=1
    else
        echo "ok: $label resolves without boa"
    fi
}

echo "==> M15 isolation: the default tree resolves without boa"
check_clean pdfrum "the pdfrum facade (default features)"
check_clean pdfrum-tool "pdfrum-tool (default features)"

# Every workspace member. `cargo metadata` would be the tidy way to enumerate
# them; the directory listing is used instead so this script needs no JSON
# parser and stays readable — and it is what check-no-wgpu.sh does, so the two
# read the same.
#
# `crates/*/` is not the whole workspace. `conformance/` and `benches/` are
# members too, and they are the two most likely places for the accident this
# check exists to catch. They are also the members whose directory name is
# *not* their crate name (`conformance` and `pdfrum-bench`), so the name is
# read from each manifest rather than guessed from the path.
echo "==> M15 isolation: no workspace crate's default features reach boa"
for manifest in crates/*/Cargo.toml conformance/Cargo.toml \
                benches/Cargo.toml benches/corpus-list/Cargo.toml; do
    [ -f "$manifest" ] || continue
    # The first `name =` under `[package]`, which is the package's own; a
    # dependency's `name =` would be under a `[dependencies.…]` table further
    # down, and every manifest here declares `[package]` first.
    crate=$(sed -n 's/^name[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' "$manifest" | head -1)
    if [ -z "$crate" ]; then
        echo "error: could not read a package name from $manifest" >&2
        fail=1
        continue
    fi
    check_clean "$crate" "$crate"
done

# And the converse, so the check cannot pass by the feature having quietly
# stopped bringing an engine — which would make every assertion above vacuous.
# This is the assertion that caught a leaked edge in M12c and it is worth
# repeating.
echo "==> M15 isolation: the script feature does still bring boa"
if cargo tree -e normal -p pdfrum-form --features script --prefix none 2>/dev/null \
    | awk '{print $1}' | grep -qx boa_engine; then
    echo "ok: pdfrum-form --features script depends on boa_engine, so the"
    echo "    checks above are not vacuous"
else
    echo "error: pdfrum-form --features script no longer reaches boa_engine —" >&2
    echo "       every check above is trivially true and this script is" >&2
    echo "       measuring nothing." >&2
    fail=1
fi

if [ "$fail" -ne 0 ]; then
    exit 1
fi
echo
echo "M15 isolation: green."
