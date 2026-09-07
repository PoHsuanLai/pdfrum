# CLI showcase

The README embed is a screen capture of a real Kitty window: Kitty's
graphics protocol for `preview` / `view`, not half-blocks. MP4 is the
recording; the GIF is a 720px, 12 fps derivative for GitHub.

```bash
./scripts/record-cli.nu
```

That builds `pdfrum-cli` in release, stages fixtures under friendly names
in `/tmp/pdfrum-cli-demo`, drives Kitty over remote control on Xvfb, and
captures the framebuffer with ffmpeg.

| Typed as | Source |
|---|---|
| `report.pdf` | `crates/pdfrum-cli/tests/fixtures/bookmarks.pdf` |
| `damaged.pdf` | `crates/pdfrum-cli/tests/fixtures/parser_rebuildxref_correct.pdf` |
| `paper.pdf` | `benches/fixtures/foxittext.pdf` |
| `gradients.pdf` | `benches/corpus/shading_tcpdf_030.pdf` |

Do not commit copies. Provenance stays with those fixtures.

## Tools

Pinned by what produced the committed files:

| | |
|---|---|
| kitty | 0.48.x (graphics protocol) |
| Xvfb | framebuffer `:93`, 960×1280 |
| ffmpeg | x11grab → H.264, then a 720px GIF |
| font | DejaVu Sans Mono, 13px (`kitty.conf`) |
| outputs | `pdfrum-cli.mp4` (canonical), `pdfrum-cli.gif` (README) |

Theme tokens match `branding/final/README.md`. Cerise (`#f06a9b`) is the
cursor and magenta; errors stay `#e01b24`, not cerise.

## Beats

`preview`, `stamp … DRAFT`, `preview stamped.pdf`, `view` (page 1, `j` to
page 2, `q`), `doctor`, `doctor --json`, `search ISO`, `extract markdown`.
Ctrl+L between the text commands. Stamp and its preview stay before `view`:
after the pager, a later Kitty image does not show up in the X11 grab.

`preview` / `view` are typed with no flags. Inside Kitty they pick the
graphics protocol. `gradients.pdf` is TCPDF example 030 (two pages of
shadings) so the pager has somewhere to go.

Software GL (`LIBGL_ALWAYS_SOFTWARE=1`, llvmpipe) is used on a headless
host. A machine with a real Kitty window can run the same script with
`DISPLAY` already set by dropping Xvfb — the send-key sequence is the
script.

`serve`, password prompts, and compile/install stay out.

## Budget

Keep `pdfrum-cli.gif` under 8 MB and `pdfrum-cli.mp4` under 15 MB. The
recorder warns above those. The GIF is scaled from the MP4 for GitHub;
do not hand-edit either file.

Not part of `scripts/ci.nu`. Re-run when the CLI's human output changes,
the same way `./scripts/api-snapshot.nu update` is a deliberate commit.
