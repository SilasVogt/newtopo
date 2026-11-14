<#
.SYNOPSIS
  Downloads Open CASCADE 8.0.1 (GitHub tag V8.0.1) and builds/installs it with
  MSVC into a prefix, using the configuration newtopo uses everywhere
  (SPEC §3.4): Release, shared libraries, no Draw, no Visualization module,
  no Tcl/Tk, FreeType, OpenGL, GLES2 or Xlib.

.EXAMPLE
  # From a "Developer PowerShell for VS" (needed for the Ninja generator):
  scripts\build-occt.ps1 -Prefix C:\occt
  # Or with the Visual Studio generator (no dev shell needed):
  scripts\build-occt.ps1 -Prefix C:\occt -Generator "Visual Studio 17 2022"

  Then: $env:OCCT_DIR = "C:\occt"; $env:PATH = "C:\occt\win64\vc14\bin;$env:PATH"; cargo build
#>
param(
  [Parameter(Mandatory = $true)][string]$Prefix,
  [string]$WorkDir = "",
  # "Ninja" (default when ninja.exe and cl.exe are on PATH) or a Visual Studio generator.
  [string]$Generator = "",
  [string]$Archive = "",
  [switch]$ConfigureOnly,
  [switch]$Force
)
$ErrorActionPreference = "Stop"

$Version = "8.0.1"
$Tag = "V$Version"
$Url = "https://github.com/Open-Cascade-SAS/OCCT/archive/refs/tags/$Tag.tar.gz"

New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
$Prefix = (Resolve-Path $Prefix).Path
if ($WorkDir -eq "") { $WorkDir = Join-Path (Split-Path $Prefix -Parent) "occt-build-$Version" }
New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
$WorkDir = (Resolve-Path $WorkDir).Path

# Already installed? (header layout differs between generators/installs)
if (-not $Force) {
  foreach ($inc in @("inc", "include\opencascade")) {
    $hdr = Join-Path $Prefix "$inc\Standard_Version.hxx"
    if ((Test-Path $hdr) -and (Select-String -Path $hdr -Pattern "OCC_VERSION_COMPLETE `"$Version`"" -Quiet)) {
      Write-Host "OCCT $Version already installed in $Prefix (use -Force to rebuild)"
      exit 0
    }
  }
}

foreach ($tool in @("cmake", "tar")) {
  if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "$tool is required" }
}

if ($Archive -eq "") { $Archive = Join-Path $WorkDir "occt-$Version.tar.gz" }
if (-not (Test-Path $Archive)) {
  Write-Host "downloading $Url"
  $ProgressPreference = "SilentlyContinue"
  Invoke-WebRequest -Uri $Url -OutFile "$Archive.part" -UseBasicParsing
  Move-Item -Force "$Archive.part" $Archive
}

$Src = Join-Path $WorkDir "OCCT-$Version"
if (-not (Test-Path (Join-Path $Src "CMakeLists.txt"))) {
  if (Test-Path $Src) { Remove-Item -Recurse -Force $Src }
  tar -xzf $Archive -C $WorkDir
  if ($LASTEXITCODE -ne 0) { throw "extracting $Archive failed" }
}

if ($Generator -eq "") {
  if ((Get-Command ninja -ErrorAction SilentlyContinue) -and (Get-Command cl -ErrorAction SilentlyContinue)) {
    $Generator = "Ninja"
  } else {
    $Generator = "Visual Studio 17 2022"
  }
}
$GenArgs = @("-G", $Generator)
if ($Generator -like "Visual Studio*") { $GenArgs += @("-A", "x64") }

$Build = Join-Path $WorkDir "build"
cmake -S $Src -B $Build @GenArgs `
  -DCMAKE_BUILD_TYPE=Release `
  "-DINSTALL_DIR=$Prefix" `
  "-DCMAKE_INSTALL_PREFIX=$Prefix" `
  -DBUILD_LIBRARY_TYPE=Shared `
  -DBUILD_MODULE_Draw=OFF `
  -DBUILD_MODULE_Visualization=OFF `
  -DBUILD_MODULE_DETools=OFF `
  -DBUILD_DOC_Overview=OFF `
  -DBUILD_SAMPLES_QT=OFF `
  -DBUILD_GTEST=OFF `
  -DUSE_TCL=OFF `
  -DUSE_TK=OFF `
  -DUSE_FREETYPE=OFF `
  -DUSE_OPENGL=OFF `
  -DUSE_GLES2=OFF `
  -DUSE_XLIB=OFF
if ($LASTEXITCODE -ne 0) { throw "cmake configure failed" }

if ($ConfigureOnly) {
  Write-Host "configured in $Build (-ConfigureOnly; not building)"
  exit 0
}

cmake --build $Build --config Release --parallel
if ($LASTEXITCODE -ne 0) { throw "build failed" }
cmake --install $Build --config Release
if ($LASTEXITCODE -ne 0) { throw "install failed" }

Write-Host "OCCT $Version installed in $Prefix"
Write-Host "build newtopo with: `$env:OCCT_DIR = '$Prefix'; cargo build"
Write-Host "and put the OCCT bin directory (win64\vc14\bin or bin) on PATH to run it"
