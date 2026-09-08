#!/usr/bin/env bash
# Rebuild fuzz/corpus/<target>/ from seeds/ plus, when present, the oracle's
# testing/resources and testing/corpus PDFs.
#
# Usage: fuzz/seed-corpus.sh [path-to-pdfium-c++-checkout]
# Default: $PDFRUM_ORACLE_CHECKOUT, else <repo>/../pdfium-c++.
set -euo pipefail
cd "$(dirname "$0")"

repo_root="$(cd .. && pwd)"
ORACLE="${1:-${PDFRUM_ORACLE_CHECKOUT:-$repo_root/../pdfium-c++}}"

targets=(
    object_decode_text object_name_decode
    crypt_encrypt_dict crypt_decrypt
    filters_flate filters_lzw filters_a85 filters_ahx filters_rle
    filters_predictor filters_chain filters_ccitt
    cmap_embedded cmap_predefined
    parser_lexer parser_object parser_xref parser_load parser_load_password
    page_parse_content page_inline_image page_colorspace page_psengine
    page_mesh_stream page_decode_image page_jbig2 page_jpx
    edit_save_roundtrip edit_subset edit_import
)
for t in "${targets[@]}"; do
    mkdir -p "corpus/$t"
    if [ -d "seeds/$t" ]; then
        cp -n "seeds/$t"/* "corpus/$t/" 2>/dev/null || true
    fi
done

# Whole-file targets take real PDFs. Named by content hash so re-running
# never duplicates.
pdf_targets=(
    parser_load parser_load_password parser_xref parser_lexer
    edit_save_roundtrip edit_import
)
if [ -d "$ORACLE/testing" ]; then
    count=0
    while IFS= read -r pdf; do
        for t in "${pdf_targets[@]}"; do
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
