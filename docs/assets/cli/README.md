# CLI showcase

The README embed is a screen capture of a real Kitty window: Kitty's
graphics protocol for `preview` / `view`, not half-blocks. MP4 is the
recording; the GIF is a 720px, 12 fps derivative for GitHub.

```bash
./scripts/record-cli-macos.nu   # a Mac with a display
./scripts/record-cli.nu         # a headless Linux host
```

Both build `pdfrum-cli` in release, stage fixtures under friendly names in
`/tmp/pdfrum-cli-demo`, and drive Kitty over remote control. They differ
only in how the window is captured.

`record-cli-macos.nu` records a real Kitty window with
`screencapture -l<window-id>`, which grabs that window alone: nothing else
on screen can enter the frame and there is no crop geometry to keep in
sync. The window is captured at 2x (1920x1480) and downscaled to 960x740,
so the glyphs are supersampled. Prefer it when a display is available.

`record-cli.nu` drives Xvfb with software GL and grabs the framebuffer
with ffmpeg's `x11grab`. That is Linux-only — macOS ffmpeg has no
`x11grab`, and a Cocoa Kitty does not draw into an X display.

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
| window | 960×740, captured at 2x on macOS |
| capture | `screencapture -l<id>` (macOS), Xvfb `:93` + x11grab (Linux) |
| ffmpeg | H.264, then a 720px GIF |
| font | DejaVu Sans Mono, 13px (`kitty.conf`) — `brew install --cask font-dejavu` |
| outputs | `pdfrum-cli.mp4` (canonical), `pdfrum-cli.gif` (README) |

Theme tokens are set in `kitty.conf` and match the banner SVGs in
`docs/assets/`. Cerise (`#f06a9b`) is the cursor and magenta; errors stay
`#e01b24`, not cerise.

## Beats

`preview`, `stamp … DRAFT`, `preview stamped.pdf`, `view` (page 1, `j`/`k`
between pages, `+` to zoom, `q`), `doctor`, `doctor --json`, `search ISO`,
`extract markdown`. The screen is cleared between the text commands — Ctrl+L
under X11, a verified shell `clear` on macOS, where Ctrl+L only scrolls the
viewport and would stack one beat under the next. Stamp and its preview stay
before `view` in both: after the pager, a later Kitty image does not show up
in the X11 grab at all, and on macOS only once the placement is dropped.

The holds are reading time, not work. Nothing here is slow: a page renders in
about 40ms, `view` emits its first placement in ~5ms, and a page turn lands in
a single frame because neighbours render and transmit while the pager waits
for a key. The one hold that has to stay tight is the wait between `view` and
the `0` that forces page 1 to draw — until that key lands the status bar is up
with no page under it, which is exactly what a slow open would look like.

`preview` / `view` are typed with no flags. Inside Kitty they pick the
graphics protocol. `gradients.pdf` is TCPDF example 030 (two pages of
shadings) so the pager has somewhere to go.

Software GL (`LIBGL_ALWAYS_SOFTWARE=1`, llvmpipe) is used on the headless
host. The beat sequence is the same in both scripts.

Two things the macOS script has to verify rather than time, because
`screencapture` records the window as it finds it. Clears are confirmed
through `kitty @ get-text` before moving on: a `clear` sent while the
previous command still owns the terminal is read as that command's input
and lost, which stacks the next beat under this one. And `view`'s page
stays placed after the pager quits, so the alt-screen is given time to
unwind and the image is dropped before the next command prints.

`serve`, password prompts, and compile/install stay out.

## Budget

Keep `pdfrum-cli.gif` under 8 MB and `pdfrum-cli.mp4` under 15 MB. The
recorder warns above those. The GIF is scaled from the MP4 for GitHub;
do not hand-edit either file.

Not part of `scripts/ci.nu`. Re-run when the CLI's human output changes,
the same way `./scripts/api-snapshot.nu update` is a deliberate commit.
