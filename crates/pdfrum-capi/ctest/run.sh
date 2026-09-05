#!/bin/sh
# Compile and run the C test against the built libpdfrum.
#
# A C compiler is a *test-time* tool here, not a build dependency: DEPS.md's
# pure-Rust guarantee is about what the library ships, and this program is a
# consumer of the shipped library rather than part of it. Nothing in
# `crates/pdfrum-capi`'s own build touches `cc`.
#
# Usage:
#   crates/pdfrum-capi/ctest/run.sh              # builds release, then runs
#   crates/pdfrum-capi/ctest/run.sh --no-build   # runs against what is there
#   crates/pdfrum-capi/ctest/run.sh --build-only # builds the library, runs nothing
#
# `--build-only` is for scripts/capi-header.nu, whose symbol half reads the
# release cdylib and should not have to know how to build it.
#
# `scripts/ci.nu` calls this after the pure-Rust check and skips it, with a
# printed note, when no C compiler is installed.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
crate=$(dirname "$here")
root=$(dirname "$(dirname "$crate")")

# The same target directory cargo used, so the script finds the library the
# rest of the gate just built rather than compiling a second copy.
target=${CARGO_TARGET_DIR:-$root/target}
lib=$target/release

cc=${CC:-cc}
if ! command -v "$cc" >/dev/null 2>&1; then
    echo "no C compiler ($cc); skipping the C test" >&2
    exit 0
fi

if [ "${1:-}" != "--no-build" ]; then
    echo "==> cargo build -p pdfrum-capi --release"
    (cd "$root" && cargo build -p pdfrum-capi --release)
fi

if [ "${1:-}" = "--build-only" ]; then
    exit 0
fi

# A plain `cargo build` names the artefact after the *lib target*, which is
# `pdfrum_capi` — the Rust crate name has to differ from the facade's, or the
# workspace's docs and doctests collide (see the manifest). The **shipped**
# artefact is `libpdfrum.so`: that name comes from
# `[package.metadata.capi.library]`, is what `cargo cinstall` lays down, and is
# what `-lpdfrum` finds. So link whichever is here, preferring the shipped name.
if [ -f "$lib/libpdfrum.so" ]; then
    linkname=pdfrum
elif [ -f "$lib/libpdfrum_capi.so" ]; then
    linkname=pdfrum_capi
else
    echo "error: neither $lib/libpdfrum.so nor $lib/libpdfrum_capi.so is there." >&2
    echo "       build it with: cargo build -p pdfrum-capi --release" >&2
    exit 1
fi

# Somewhere to put the binary that is not the source tree.
out=${TMPDIR:-/tmp}/pdfrum-ctest-$$
trap 'rm -f "$out"' EXIT

echo "==> $cc -std=c11 -Wall -Werror"
# `-Werror` on the C side is the same bar the Rust side holds: a header that
# makes a caller warn is a header defect. `-lm` and `-lpthread` are what the
# Rust standard library and the test's own threads need.
"$cc" -std=c11 -Wall -Werror \
    -I "$crate/include" \
    "$here/test.c" \
    -L "$lib" "-l$linkname" -lm -lpthread \
    -o "$out"

echo "==> $out"
# The fixtures the test reads are named relative to the repository root, so it
# runs from there. `LD_LIBRARY_PATH` is how the loader finds a library that has
# not been installed; `cargo cinstall` is the path that does not need it.
LD_LIBRARY_PATH=$lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH} \
    sh -c 'cd "$1" && exec "$2"' sh "$root" "$out"
