# newtopo marketing site

A single static page: `index.html`, `styles.css`, `main.js` and `assets/`.
There is no build step and no framework. It is deployed to GitHub Pages by
`.github/workflows/pages.yml` on every push to `main` that touches `site/`
(or by running the workflow by hand). The page lives under a project subpath
(`https://silasvogt.github.io/newtopo/`), so keep every URL relative.

One-time repo setup: Settings > Pages > Build and deployment > Source:
**GitHub Actions**.

## Preview locally

```sh
python3 -m http.server --bind 127.0.0.1 8765 -d site
# open http://127.0.0.1:8765/
```

Any static server works. Check both colour schemes (the page and the art follow
`prefers-color-scheme`) and reduced motion (DevTools > Rendering).

## Layout of the page

| Section | Anchor | What it shows |
| --- | --- | --- |
| Hero | `#top` | One-line pitch, Download and How it works |
| Before / after | `#why` | Drag slider: tessellated export vs newtopo output |
| Core loop | `#loop` | Import, add lines, auto-fill, preview and export (sticky stage on desktop) |
| Rules | `#rules` | Interactive torus: Connect A and B counts regenerate the lines |
| Tools | `#tools` | Bento of Connect, Surface Grid, Slice, Iso, Cut, clean-up, Link and Symmetry |
| Export | `#export` | FBX, OBJ, glTF and the Blender bridge |
| CLI | `#cli` | `newtopo-cli` commands |
| Download | `#download` | One card per OS (all link to the latest release), source link, system requirements |

`main.js` is progressive enhancement only: slider, the sticky loop stage, OS
detection for the download cards, the torus demo and the bridge animation.
Without JavaScript every section still reads correctly.

## Swapping in real screenshots

Every illustration sits in a screenshot slot: an element with
`data-shot="<slot>"` and an HTML comment `<!-- SCREENSHOT: <slot>. ... -->`
that says what the capture should show and its size. To replace one:

1. Save the capture as `site/assets/screens/<slot>.png` (or `.webp`), at 2x the
   displayed size given in the comment. Keep the same aspect ratio as the
   current art so nothing shifts.
2. In `index.html`, change that slot's `<img src>` to `assets/screens/<slot>.png`,
   update `width`/`height` to the real pixel size, and rewrite the `alt` text to
   describe the screenshot.
3. Screenshots are a single image for both colour schemes. If you want a light
   variant, wrap the `<img>` in a `<picture>` with
   `<source media="(prefers-color-scheme: light)" srcset="assets/screens/<slot>-light.png">`.

| Slot | Where | Current art |
| --- | --- | --- |
| `hero-viewport` | Hero | `assets/art/hero-part.svg` |
| `compare-before` | Before / after, left side | `assets/art/compare-soup.svg` |
| `compare-after` | Before / after, right side (same camera as before) | `assets/art/compare-quads.svg` |
| `loop-import` | Core loop step 1 | `assets/art/loop-import.svg` |
| `loop-lines` | Core loop step 2 | `assets/art/loop-lines.svg` |
| `loop-fill` | Core loop step 3 | `assets/art/loop-fill.svg` |
| `loop-preview` | Core loop step 4 | `assets/art/loop-preview.svg` |
| `tool-connect` | Tools: Connect | `assets/art/tool-connect.svg` |
| `tool-grid` | Tools: Surface Grid | `assets/art/tool-grid.svg` |
| `tool-slice` | Tools: Slice | `assets/art/tool-slice.svg` |
| `tool-iso` | Tools: Iso | `assets/art/tool-iso.svg` |
| `tool-cut` | Tools: Cut | `assets/art/tool-cut.svg` |
| `tool-cleanup` | Tools: remove, collapse, weld | `assets/art/tool-cleanup.svg` |
| `tool-link` | Tools: Link and Symmetry | `assets/art/tool-link.svg` |
| `bridge-blender` | Export: Blender bridge | HTML diagram inside the figure; replace the `.bridge__diagram` div with an `<img>` |

The core-loop images are copied into the sticky desktop stage by `main.js`, so
swapping the `<img>` inside each step is enough.

## Regenerating the art

The SVGs in `assets/art/` are generated, not hand-drawn: a small orthographic
renderer projects the exact parametric surfaces of an invented demo part.

```sh
python3 tools/site-art/gen_art.py      # writes site/assets/art/*.svg
```

Each SVG carries its own light and dark palette. The Open Graph card
(`assets/og.png`, 1200x630) is rendered from `tools/site-art/og.html` with
headless Chromium in dark mode, for example:

```sh
chromium --headless --force-dark-mode --window-size=1200,630 \
  --screenshot=site/assets/og.png tools/site-art/og.html
```

## Licences

newtopo is MIT-licensed. The footer also carries the Open CASCADE Technology
notice (LGPL 2.1 with the Open CASCADE exception); keep it while the app ships
OCCT.
