# Installing newtopo

Downloads are on the project's [Releases page](https://github.com/SilasVogt/newtopo/releases).
Each release has these files (`<ver>` is the version, for example `0.1.0`):

| System | File | What it is |
| --- | --- | --- |
| Windows 10/11 x64 | `newtopo-<ver>-windows-x64-setup.exe` | Installer |
| | `newtopo-<ver>-windows-x64.zip` | Portable folder, no install |
| macOS 11+ (Apple silicon) | `newtopo-<ver>-macos-arm64.dmg` | Disk image with `newtopo.app` |
| Linux x86-64 | `newtopo-<ver>-linux-x86_64.AppImage` | Single file, runs on most distributions |
| | `newtopo-<ver>-linux-amd64.deb` | Package for Debian, Ubuntu and derivatives |
| | `newtopo-<ver>-linux-x86_64.tar.gz` | Relocatable folder |
| all | `SHA256SUMS.txt` | Checksums of all of the above |

Linux builds need glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36
and later). All builds need a GPU driver with Vulkan, DirectX 12 or Metal.

To check a download: `sha256sum -c SHA256SUMS.txt --ignore-missing` (Linux),
`shasum -a 256 -c SHA256SUMS.txt --ignore-missing` (macOS) or
`Get-FileHash newtopo-<ver>-windows-x64-setup.exe` (PowerShell) and compare.

Every package contains three programs that must stay together in one folder:
`newtopo` (the app), `newtopo-cli` (command line) and `newtopo-import` (the
helper the other two start to read CAD files).

## Windows

**Installer.** Run `newtopo-<ver>-windows-x64-setup.exe`. It asks whether to
install for all users (needs admin rights, goes to `C:\Program Files\newtopo`)
or just for you (goes to `%LOCALAPPDATA%\Programs\newtopo`). It adds a
Start-menu entry, optionally a desktop icon, and opens `.ntopo` scene files
with newtopo (you can untick that).

**Portable.** Unzip `newtopo-<ver>-windows-x64.zip` anywhere and run
`newtopo.exe`. Everything it needs, including the Visual C++ runtime, is in
the folder.

**SmartScreen.** The builds are not code-signed yet, so Windows may show
"Windows protected your PC". Click **More info**, then **Run anyway**. If your
browser blocks the download, choose **Keep**. For the zip you can also
right-click it, choose **Properties**, tick **Unblock** and click OK before
extracting.

**Command line.** `newtopo-cli.exe` is in the install folder. Add that folder
to your `PATH` to call it from any terminal.

**Uninstall.** Settings > Apps > Installed apps > newtopo > Uninstall (or
"Add or remove programs"). The portable version: delete the folder.
Uninstalling keeps your settings and recent-files list (under `%APPDATA%`);
delete newtopo's folder there for a clean removal.

## macOS

Open the `.dmg` and drag **newtopo** onto **Applications**.

**Gatekeeper.** The app is ad-hoc signed but not notarised by Apple yet. The
first time you open it macOS says it "cannot verify that newtopo is free of
malware" (or that the app "is damaged"). Open it anyway with one of:

- In Finder, Control-click (right-click) newtopo in Applications, choose
  **Open**, then **Open** again (macOS 14 and earlier).
- Try to open it once, then go to **System Settings > Privacy & Security**,
  scroll down and click **Open Anyway** (macOS 15 and later).
- In Terminal: `xattr -dr com.apple.quarantine /Applications/newtopo.app`

You only need to do this once per version.

**Command line.** The CLI is inside the app bundle. To call it as
`newtopo-cli`:

```sh
sudo ln -sf /Applications/newtopo.app/Contents/MacOS/newtopo-cli /usr/local/bin/newtopo-cli
```

**Uninstall.** Drag `/Applications/newtopo.app` to the Bin, remove the
`/usr/local/bin/newtopo-cli` link if you made one. Settings stay in
`~/Library/Application Support`; delete newtopo's folder there for a clean
removal.

Intel Macs are not supported by the release builds yet; build from source.

## Linux

**AppImage.** Make it executable and run it:

```sh
chmod +x newtopo-<ver>-linux-x86_64.AppImage
./newtopo-<ver>-linux-x86_64.AppImage
```

Run the CLI with `./newtopo-<ver>-linux-x86_64.AppImage --cli <args>`, or make
a symlink named `newtopo-cli` that points to the AppImage. AppImages need
FUSE 2 (`libfuse2` / `fuse2`); without it, run with `--appimage-extract-and-run`.
Tools such as Gear Lever or AppImageLauncher add it to your app menu.

**Debian / Ubuntu.**

```sh
sudo apt install ./newtopo-<ver>-linux-amd64.deb
```

This installs to `/opt/newtopo`, puts `newtopo` and `newtopo-cli` on your
`PATH`, adds a menu entry and opens `.ntopo` files with newtopo. Uninstall
with `sudo apt remove newtopo`.

**tar.gz.** Extract it anywhere and run `bin/newtopo`. The folder's
`README.txt` shows how to add a menu entry and the `.ntopo` file type for
your user. Uninstall by deleting the folder (and the files that README told
you to copy).

Removing newtopo keeps your settings under `~/.config`; delete newtopo's
folder there for a clean removal.

## Building from source

You need Rust (stable, via [rustup](https://rustup.rs)), CMake 3.16+, a C++17
compiler (GCC 11+, Clang 14+, or Visual Studio 2022 with the C++ workload),
and Git. On Linux also install the windowing headers, for example on
Ubuntu:

```sh
sudo apt install build-essential cmake ninja-build pkg-config \
  libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev \
  libgl1-mesa-dev
```

**1. Build Open CASCADE (OCCT) 8.0.1** once, into a prefix of your choice:

```sh
scripts/build-occt.sh "$HOME/.cache/newtopo-occt/install"         # Linux, macOS
```
```powershell
scripts\build-occt.ps1 -Prefix "$env:LOCALAPPDATA\newtopo-occt"    # Windows
```

If `scripts/` doesn't have these yet, use the identical CI copies in
`.github/scripts/`. The build takes 20 to 60 minutes.

**2. Point the build at it and build:**

```sh
export OCCT_DIR="$HOME/.cache/newtopo-occt/install"
cargo build --release
```
```powershell
$env:OCCT_DIR = "$env:LOCALAPPDATA\newtopo-occt"
cargo build --release
```

The programs land in `target/release/`. To run them from there, the OCCT
libraries must be findable: add `$OCCT_DIR/lib` to `LD_LIBRARY_PATH` (Linux)
or `DYLD_FALLBACK_LIBRARY_PATH` (macOS), or `%OCCT_DIR%\bin` to `PATH`
(Windows).

**3. Optional: build the packages** (they bundle the OCCT libraries, so they
run anywhere):

```sh
packaging/linux/package.sh      # tar.gz, AppImage, .deb in dist/ (needs patchelf, dpkg-deb)
packaging/macos/package.sh      # newtopo.app and .dmg in dist/
```
```powershell
packaging\windows\package.ps1   # setup.exe and .zip in dist\ (needs Inno Setup 6)
```

Each script builds the workspace first unless you pass `--skip-build`
(`-SkipBuild` on Windows). App icons are generated from `assets/icon.svg`
with `packaging/icons/generate.sh`.

## Licences

newtopo is MIT-licensed. It uses Open CASCADE Technology under the LGPL 2.1
with the Open CASCADE exception, linked dynamically; the OCCT libraries ship
as separate files you can replace. Each package includes `LICENSE`,
`THIRD-PARTY-NOTICES.txt`, `THIRD-PARTY-RUST.txt` and the OCCT licence texts.
