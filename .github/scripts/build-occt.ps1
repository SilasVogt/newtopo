<#
  Build Open CASCADE Technology from source and install it to a prefix.
  CI copy of the OCCT recipe (Windows, MSVC). Usage:
    .github/scripts/build-occt.ps1 -Prefix <install-prefix>
  Env: OCCT_VERSION (default 8.0.1)
  Uses CMake's default Visual Studio generator, so no developer shell is
  needed. Changing this file changes the CI cache key, so OCCT is rebuilt.
#>
param([Parameter(Mandatory)][string]$Prefix)
$ErrorActionPreference = 'Stop'

$version = if ($env:OCCT_VERSION) { $env:OCCT_VERSION } else { '8.0.1' }
# Short paths: MSBuild still trips over 260-character paths.
$tmp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }
$work = Join-Path $tmp "o$($version -replace '\.', '')"
New-Item -ItemType Directory -Force $work, $Prefix | Out-Null
$Prefix = (Resolve-Path $Prefix).Path

if (-not (Test-Path "$work\s")) {
  Invoke-WebRequest -UseBasicParsing -OutFile "$work\occt.tar.gz" `
    -Uri "https://github.com/Open-Cascade-SAS/OCCT/archive/refs/tags/V$version.tar.gz"
  New-Item -ItemType Directory -Force "$work\s" | Out-Null
  tar -xzf "$work\occt.tar.gz" -C "$work\s" --strip-components=1
  if ($LASTEXITCODE) { throw "extracting OCCT failed" }
}

cmake -S "$work\s" -B "$work\b" -A x64 `
  -DCMAKE_BUILD_TYPE=Release `
  -DBUILD_LIBRARY_TYPE=Shared `
  "-DINSTALL_DIR=$Prefix" `
  -DINSTALL_DIR_LAYOUT=Unix `
  -DBUILD_MODULE_Draw=OFF `
  -DBUILD_MODULE_Visualization=OFF `
  -DUSE_TCL=OFF -DUSE_TK=OFF `
  -DUSE_FREETYPE=OFF `
  -DUSE_OPENGL=OFF -DUSE_GLES2=OFF `
  -DUSE_XLIB=OFF `
  -DBUILD_DOC_Overview=OFF `
  -DBUILD_GTEST=OFF
if ($LASTEXITCODE) { throw "cmake configure failed" }
cmake --build "$work\b" --config Release --parallel
if ($LASTEXITCODE) { throw "cmake build failed" }
cmake --install "$work\b" --config Release
if ($LASTEXITCODE) { throw "cmake install failed" }
Write-Host "OCCT $version installed to $Prefix"
