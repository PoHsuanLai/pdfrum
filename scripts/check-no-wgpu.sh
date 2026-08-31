#!/usr/bin/env bash
# M12c's isolation check: a headless build of pdfrum resolves with zero `wgpu`.
#
# PLAN.md §M12c grants the GPU backend an exemption from DEPS.md's pure-Rust
# guarantee, and bounds it with two rules. This script is the mechanical half
# of the first one — "prove this with a committed check, not an assertion" —
# and it is deliberately a separate file from scripts/ci.sh so that a reader
# looking for the blast radius of that exemption finds one place to look.
#
# What it asserts, in the order a violation would most likely arrive:
#
#   1. The `pdfrum` facade's default feature set contains no `wgpu` and no
#      `vello` at all. This is the claim an embedder cares about: `cargo add
#      pdfrum` must not put a graphics driver in their tree.
#   2. `pdfrum-tool`'s default features likewise, because the CLI is what a
#      headless or CI build actually runs.
#   3. No crate in the workspace *except* pdfrum-raster-vello-gpu depends on
#      `vello` or `wgpu`. This is the one that catches the accident the other
#      two would eventually catch anyway — a new crate reaching for the GPU
#      backend "just for a test" — at the point where it is one line to undo.
#
# Run standalone, or via scripts/ci.sh which calls it.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0

# The crate the exemption is scoped to. Everything else must be clean.
gpu_crate="pdfrum-raster-vello-gpu"

check_clean() {
    local crate="$1" label="$2"
    local found
    # `-e normal` excludes dev- and build-dependencies: a dev-dependency does
    # not ship in a consumer's tree, and the claim here is about what an
    # embedder resolves.
    found=$(cargo tree -e normal -p "$crate" --prefix none 2>/dev/null \
        | awk '{print $1}' | sort -u \
        | grep -E '^(wgpu|wgpu-core|wgpu-hal|wgpu-types|vello|vello_encoding|vello_shaders)$' \
        || true)
    if [ -n "$found" ]; then
        echo "error: $label reaches the GPU stack:" >&2
        printf '  %s\n' $found >&2
        echo "       M12c's isolation rule: nothing in the core ring may depend" >&2
        echo "       on $gpu_crate. See PLAN.md §M12c and DEPS.md." >&2
        fail=1
    else
        echo "ok: $label resolves without wgpu"
    fi
}

echo "==> M12c isolation: the core ring resolves without wgpu"
check_clean pdfrum "the pdfrum facade (default features)"
check_clean pdfrum-tool "pdfrum-tool (default features)"

# Every workspace member but the GPU backend itself. `cargo metadata` would be
# the tidy way to enumerate them; the directory listing is used instead so this
# script needs no JSON parser and stays readable.
echo "==> M12c isolation: no other workspace crate reaches the GPU stack"
for dir in crates/*/; do
    crate=$(basename "$dir")
    [ "$crate" = "$gpu_crate" ] && continue
    check_clean "$crate" "$crate"
done

# And the converse, so the check cannot pass by the backend having quietly
# stopped depending on vello — which would make every assertion above vacuous.
echo "==> M12c isolation: the GPU backend does still reach vello"
if cargo tree -e normal -p "$gpu_crate" --prefix none 2>/dev/null \
    | awk '{print $1}' | grep -qx vello; then
    echo "ok: $gpu_crate depends on vello, so the checks above are not vacuous"
else
    echo "error: $gpu_crate no longer depends on vello — every check above is" >&2
    echo "       trivially true and this script is measuring nothing." >&2
    fail=1
fi

if [ "$fail" -ne 0 ]; then
    exit 1
fi
echo
echo "M12c isolation: green."
