# CLI showcase

The GIF in the READMEs is a VHS recording of `pdfrum`, not a live capture.

```bash
./scripts/record-cli.nu
```

That builds `pdfrum-cli` in release, stages three in-tree fixtures under
friendly names in `/tmp/pdfrum-cli-demo`, and runs `docs/assets/cli/pdfrum-cli.tape`.

| Typed as | Source |
|---|---|
| `report.pdf` | `crates/pdfrum-cli/tests/fixtures/bookmarks.pdf` |
| `damaged.pdf` | `crates/pdfrum-cli/tests/fixtures/parser_rebuildxref_correct.pdf` |
| `paper.pdf` | `benches/fixtures/foxittext.pdf` |

Do not commit copies. Provenance stays with those fixtures.

## Tools

Pinned by what produced the committed GIF:

| | |
|---|---|
| vhs | 0.11.0 |
| ttyd | 1.7.x |
| ffmpeg | 4.x or later |
| font | DejaVu Sans Mono, 16px |
| PTY picture | 960 × 540, 20px padding, 12 fps |
| host | `xvfb-run -a` when `DISPLAY` is unset |

Theme tokens match `branding/final/README.md`. Cerise (`#f06a9b`) is the
cursor and magenta; errors stay `#e01b24`, not cerise.

## Beats

`info`, `doctor`, `extract toc`, `search ISO`. The last frame holds the
whole tour.

`preview --graphics halfblock` was tried and dropped: at a README size a
letter-size page is a noisy postage stamp. Mention `preview` / `view` in
the caption instead. Kitty and iTerm2 picture protocols will not survive
a GIF.

`view`, `serve`, password prompts, and compile/install stay out.

## Budget

Keep `pdfrum-cli.gif` under 2 MB. The recorder warns above that. Tighten
`Sleep`, `Framerate`, or drop a beat — do not re-encode with a third tool.

Not part of `scripts/ci.nu`. Re-run when the CLI's human output changes,
the same way `./scripts/api-snapshot.nu update` is a deliberate commit.
