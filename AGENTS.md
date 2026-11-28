# Instructions for coding agents

newtopo is a Rust desktop app (egui, wgpu) that turns CAD models into quad
meshes. Humans: see [`CONTRIBUTING.md`](CONTRIBUTING.md); it has the
reasons behind the rules below.

## Commands

```sh
# Once: build Open CASCADE 8.0.1 (takes a while). Windows: scripts\build-occt.ps1
scripts/build-occt.sh "$HOME/.cache/newtopo-occt/install"
export OCCT_DIR="$HOME/.cache/newtopo-occt/install"

# The three checks CI runs. All must pass before you open a pull request.
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

- macOS: also `export DYLD_LIBRARY_PATH="$OCCT_DIR/lib"` for tests.
- Without Open CASCADE, the app, renderer and most of the core still build
  and test: `cargo test -p newtopo -p newtopo-render --no-default-features`.
- Always set `NEWTOPO_CONFIG_DIR=<a temp dir>` so nothing touches the
  user's real preferences, autosaves or logs.

## Do not open a window

Never run the `newtopo` binary plainly: it opens a window on the user's
desktop. Check the UI without one:

```sh
cargo run -p newtopo --no-default-features -- --shot <dir>   # UI screenshots as PNG
cargo test -p newtopo --no-default-features                   # simulated-input UI tests
```

Headless work on real models goes through `newtopo-cli` (`info`, `run`,
`validate`, `export`, `render`; see the README).

## Reproducing a problem report

A report zip from *Help > Report a problem…* holds `error.txt`, `log.txt`
and, if the user included them, `steps.json`, `system.txt` and the model.

1. Check out the commit or version named in `error.txt`.
2. `newtopo-cli run steps.json --input <model> -o out.ntopo` replays the
   ops. It also replays ops the user later undid or discarded, so the
   result can differ from what they saw; the `session` list in `steps.json`
   is the full timeline.
3. Turn the reproduction into a failing test, then fix it.

If the model is missing and the bug needs it, ask the reporter. Do not guess.

## Hard rules

- **Clean room.** Do not read, copy or port code from Blender, Blender
  add-ons, or any GPL or proprietary software, and do not consult any
  commercial CAD-retopology product, its docs or videos. Sources are public
  papers, textbooks, the docs of the libraries in use, and your own design.
  Add a line to [`docs/PROVENANCE.md`](docs/PROVENANCE.md) for every
  non-trivial algorithm.
- **Dependencies** must be MIT-compatible (MIT, Apache-2.0, BSD, MPL-2.0,
  Zlib). No GPL or AGPL.
- **[`SPEC.md`](SPEC.md) is the source of truth.** A behaviour change
  updates the spec in the same pull request. Where the spec is silent,
  decide and record it in [`docs/QUESTIONS.md`](docs/QUESTIONS.md) or the
  area's file in [`docs/notes/`](docs/notes/).
- **The app never changes the mesh on its own.** Preview, then apply. No
  automatic topology changes.
- **An op either leaves a valid mesh or is refused with a reason.**
- **No telemetry and no uploads.** Do not add network requests.
- **C++ only in `crates/occt-shim`.**
- **Never commit** user models from a report, research material, build
  output or secrets.

## Pull requests

- One topic. Small diffs. Match the style of the code around your change.
- A bug fix includes a test that fails without it.
- UI changes include screenshots from `--shot`.
- Say what you tested and what you did not. Do not claim a check you did
  not run.
- State in the description that an AI agent wrote the change, and which
  one.

## Where things are

| Path | What |
| --- | --- |
| `crates/core` | Document, ops, evaluator, mesh editing, export. No UI, no GPU. |
| `crates/app` | The desktop app; `src/uitest.rs` has the UI tests |
| `crates/kernel`, `crates/occt-shim` | CAD kernel trait, mock kernel, the C++ bridge |
| `crates/cli` | `newtopo-cli` |
| `docs/STATUS.md` | Current state and known gaps |
| `docs/notes/` | Engineering notes and decisions per area |
