#!/usr/bin/env bash
# Populate fuzz/corpus/<target>/ from the committed seeds plus, when the
# read-only oracle checkout is present, its full testing/resources and
# testing/corpus PDF sets.
#
# `corpus/` is gitignored working state — this script rebuilds it. What is
# committed is `seeds/`, the small curated set (see fuzz/README.md
# "Seed corpora" for provenance).
#
# Usage: fuzz/seed-corpus.sh [path-to-pdfium-c++-checkout]
set -euo pipefail
cd "$(dirname "$0")"

ORACLE="${1:-/mnt/data2/pdfium/pdfium-c++}"

targets=(
    object_decode_text object_name_decode
    crypt_encrypt_dict crypt_decrypt
    filters_flate filters_lzw filters_a85 filters_ahx filters_rle
    filters_predictor filters_chain
    cmap_embedded cmap_predefined
    parser_lexer parser_object parser_xref parser_load parser_load_password
    edit_save_roundtrip edit_subset edit_import
)
for t in "${targets[@]}"; do
    mkdir -p "corpus/$t"
    if [ -d "seeds/$t" ]; then
        cp -n "seeds/$t"/* "corpus/$t/" 2>/dev/null || true
    fi
done

# The whole-file targets take real PDFs. Everything the oracle ships is fair
# game: `testing/resources` is the unit-test fixture set (small, and rich in
# deliberately broken files), `testing/corpus` is the rendering corpus.
# The writer's targets want the same real PDFs: `edit_save_roundtrip` opens
# one and writes it back out, and `edit_import` splits the input into two
# documents, so a real file gives it two halves of real structure.
pdf_targets=(
    parser_load parser_load_password parser_xref parser_lexer
    edit_save_roundtrip edit_import
)
if [ -d "$ORACLE/testing" ]; then
    count=0
    while IFS= read -r pdf; do
        for t in "${pdf_targets[@]}"; do
            # Name by content hash so re-running never duplicates and the
            # two source trees cannot collide on a basename.
            h=$(sha1sum "$pdf" | cut -c1-16)
            cp -n "$pdf" "corpus/$t/$h.pdf" 2>/dev/null || true
        done
        count=$((count + 1))
    done < <(find "$ORACLE/testing/resources" "$ORACLE/testing/corpus" \
        -name .git -prune -o -name '*.pdf' -print 2>/dev/null)
    echo "seeded $count oracle PDFs into: ${pdf_targets[*]}"
else
    echo "note: no oracle checkout at $ORACLE; seeded from fuzz/seeds/ only" >&2
fi

for t in "${targets[@]}"; do
    printf '  %-24s %s files\n' "$t" "$(find "corpus/$t" -type f | wc -l)"
done
