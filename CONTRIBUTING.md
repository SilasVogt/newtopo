# Contributing to newtopo

Bug reports, fixes and features are welcome. This page says how to build,
what a pull request has to pass, and the few rules that are not negotiable.

Coding agents: read [`AGENTS.md`](AGENTS.md) too. It is the short version
with the exact commands.

## Reporting a bug

In the app, *Help > Report a problem…* builds a zip with the error, the log
and the steps you took, and opens a pre-filled GitHub issue. The app uploads
nothing itself: attach the zip to the issue yourself. The model is only
included if you tick it.

Without the app, open an issue with the newtopo version, your system, what
you did and what happened. A CAD file that shows the problem helps most.

## Building

You need Rust 1.88 or newer, CMake 3.16+ and a C++17 compiler. Open CASCADE
8.0.1 is built once by a script:

```sh
scripts/build-occt.sh "$HOME/.cache/newtopo-occt/install"
export OCCT_DIR="$HOME/.cache/newtopo-occt/install"
cargo build --release
./target/release/newtopo
```

Windows uses `scripts\build-occt.ps1`. System packages and platform notes are
in [`docs/INSTALL.md`](docs/INSTALL.md).

The app and the renderer also build without Open CASCADE (no CAD import, an
analytic mock kernel instead), which is enough for most UI work:

```sh
cargo run -p newtopo --no-default-features -- --sample
```

## Before you open a pull request

CI runs these three on Windows, macOS and Linux. Run them locally first:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

- **Tests.** A bug fix comes with a test that fails without the fix. A new
  tool or op comes with tests of its own. The mesh tools and the document
  engine have property tests; run more cases locally with
  `MESHEDIT_CASES=600 cargo test -p newtopo-core --test meshedit` and
  `NT_PROPTEST_CASES=<n> cargo test -p newtopo-core --test property`.
- **The spec.** [`SPEC.md`](SPEC.md) describes how the app behaves. A change
  in behaviour changes the spec in the same pull request. If the spec does
  not answer a question, decide it, and write the decision in
  [`docs/QUESTIONS.md`](docs/QUESTIONS.md) or the notes file of that area
  under [`docs/notes/`](docs/notes/).
- **UI changes.** Add screenshots to the pull request.
  `cargo run -p newtopo --no-default-features -- --shot <dir>` renders the UI
  to PNG files without opening a window. For anything larger than a small
  fix, show a mockup in an issue first.
- **One topic per pull request**, with a description of what changed, how it
  was tested, and what was not tested.

## Rules that are not negotiable

### Clean room and licences

newtopo is MIT-licensed and was written clean-room
([`SPEC.md` §0](SPEC.md#0-clean-room-rules-read-first)).

- Do not copy or port code from GPL or proprietary software into the crates.
  That includes Blender, Blender add-ons and any commercial retopology or CAD
  product. Reading such code to "see how they do it" and then writing it here
  is not acceptable either.
- Do not use a commercial CAD-retopology product, its documentation, videos
  or forums as a source.
- Allowed sources: public papers, textbooks, the documentation of the
  open-source libraries in use, and your own design.
- Every non-trivial algorithm gets a line in
  [`docs/PROVENANCE.md`](docs/PROVENANCE.md) saying where it came from.
- New dependencies must have a licence compatible with MIT distribution
  (MIT, Apache-2.0, BSD, MPL-2.0, Zlib and similar). No GPL or AGPL crates.
- The Blender add-on in `addons/blender` is GPL-3.0-or-later because Blender
  add-ons must be. It talks to the app over a local socket and shares no
  code with the crates. Keep it that way.

By opening a pull request you confirm that the contribution is your own work
and that you license it under the project's licence.

### Product rules

- **The app never changes the mesh on its own.** Tools show a preview and
  apply on confirmation. No automatic topology changes, no silent clean-up.
- **Either the output validates or the op is refused with a reason.** No
  op may leave an invalid mesh behind.
- **No telemetry, no uploads.** The only network request the app makes is
  the opt-in update check ([`SPEC.md` §9.4](SPEC.md)), which carries no user
  data. A problem report is a file the user saves and attaches themselves.
- **C++ stays in `crates/occt-shim`.** Everything else is Rust.
- **No test, script or tool may read or write the user's real config
  folder.** Set `NEWTOPO_CONFIG_DIR=<dir>`.

## Where things are

The repository layout is in the [README](README.md#repository-layout).
Current state and known gaps are in [`docs/STATUS.md`](docs/STATUS.md).
Engineering notes per area are in [`docs/notes/`](docs/notes/).
