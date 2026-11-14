<#
.SYNOPSIS
  Build the Windows x64 packages of newtopo: an Inno Setup installer and a
  portable .zip. Works locally and in CI (.github/workflows/release.yml).

.PARAMETER SkipBuild
  Use binaries already in the cargo target dir.
.PARAMETER BinDir
  Take the executables from this dir (implies -SkipBuild).
.PARAMETER Out
  Output directory (default: <repo>\dist).
.PARAMETER Formats
  Comma list of setup,zip (default: both).

Environment:
  OCCT_DIR                OCCT install prefix. Every DLL the executables need
                          (recursively) found under it is copied next to them.
  NEWTOPO_BUNDLE_LIB_DIRS extra ';'-separated dirs to bundle DLLs from
  NEWTOPO_VERSION         override the version (default: cargo metadata)
  NEWTOPO_VCREDIST_DIR    folder with the MSVC runtime DLLs (default: found
                          through vswhere in the Visual Studio install)
  CARGO_TARGET_DIR        cargo target dir (default: <repo>\target)

Needs: cargo, Visual Studio Build Tools (dumpbin, CRT redist), Inno Setup 6
(installed with choco when missing and running in CI).
#>
param(
  [switch]$SkipBuild,
  [string]$BinDir = "",
  [string]$Out = "",
  [string]$Formats = "setup,zip"
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$here = $PSScriptRoot
$root = (Resolve-Path "$here\..\..").Path
$pkgdir = "$root\packaging"
if (-not $Out) { $Out = "$root\dist" }
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path
function Log($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Want($f) { return (",$Formats," -like "*,$f,*") }

# ------------------------------------------------------------------ version
if ($env:NEWTOPO_VERSION) { $version = $env:NEWTOPO_VERSION }
else {
  Push-Location $root
  $meta = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
  Pop-Location
  $version = ($meta.packages | Where-Object name -eq 'newtopo').version
}
$numVersion = (($version -split '[-+]')[0]) + '.0'
$name = "newtopo-$version-windows-x64"

# ------------------------------------------------------------------ build
if ($BinDir) { $SkipBuild = $true }
if (-not $SkipBuild) {
  Log "cargo build --workspace --release"
  Push-Location $root
  cargo build --workspace --release --locked
  if ($LASTEXITCODE) { throw "cargo build failed" }
  Pop-Location
}
if (-not $BinDir) {
  $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "$root\target" }
  $BinDir = "$target\release"
}
if (-not (Test-Path "$BinDir\newtopo.exe")) { throw "$BinDir\newtopo.exe not found" }

$work = "$Out\.work-windows"
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
$stage = "$work\$name"
New-Item -ItemType Directory -Force "$stage\licenses" | Out-Null

$exes = @()
foreach ($b in 'newtopo', 'newtopo-cli', 'newtopo-import') {
  if (Test-Path "$BinDir\$b.exe") { Copy-Item "$BinDir\$b.exe" $stage; $exes += "$b.exe" }
  else { Write-Warning "$BinDir\$b.exe missing, not packaged" }
}
Copy-Item "$pkgdir\icons\newtopo.ico" $stage

# ------------------------------------------------------------------ tools
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vsPath = $null
if (Test-Path $vswhere) {
  $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}
$dumpbin = $null
if ($vsPath) {
  $dumpbin = Get-ChildItem "$vsPath\VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe" -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if (-not $dumpbin) { $dumpbin = (Get-Command dumpbin.exe -ErrorAction SilentlyContinue).Source }

function Get-Imports($file) {
  $lines = & $dumpbin /nologo /dependents $file
  if ($LASTEXITCODE) { throw "dumpbin failed on $file" }
  $lines | ForEach-Object { if ($_ -match '^\s+(\S+\.dll)\s*$') { $Matches[1] } }
}

# ------------------------------------------------------------------ OCCT DLLs
$libDirs = @()
if ($env:OCCT_DIR) {
  foreach ($d in "$env:OCCT_DIR\bin", "$env:OCCT_DIR\win64\vc14\bin") { if (Test-Path $d) { $libDirs += (Resolve-Path $d).Path } }
}
if ($env:NEWTOPO_BUNDLE_LIB_DIRS) {
  foreach ($d in $env:NEWTOPO_BUNDLE_LIB_DIRS -split ';') { if ($d -and (Test-Path $d)) { $libDirs += (Resolve-Path $d).Path } }
}
Log "bundling DLLs from: $(if ($libDirs) { $libDirs -join ', ' } else { '(none; OCCT_DIR unset)' })"
$bundled = 0
if ($dumpbin) {
  $queue = New-Object System.Collections.Queue
  foreach ($e in $exes) { $queue.Enqueue("$stage\$e") }
  while ($queue.Count) {
    $f = $queue.Dequeue()
    foreach ($dll in Get-Imports $f) {
      if (Test-Path "$stage\$dll") { continue }
      foreach ($d in $libDirs) {
        if (Test-Path "$d\$dll") {
          Copy-Item "$d\$dll" $stage; $queue.Enqueue("$stage\$dll"); $bundled++
          break
        }
      }
    }
  }
} elseif ($libDirs) {
  Write-Warning "dumpbin not found; copying every DLL from the OCCT bin dirs"
  foreach ($d in $libDirs) { Get-ChildItem "$d\*.dll" | ForEach-Object { Copy-Item $_.FullName $stage; $bundled++ } }
}
Write-Host "bundled $bundled DLLs"

# ------------------------------------------------------------------ MSVC runtime
# Rust (msvc target) and OCCT both link the dynamic CRT. Ship it app-local so
# the zip works without the VC++ redistributable installed.
$crt = $env:NEWTOPO_VCREDIST_DIR
if (-not $crt -and $vsPath) {
  $crt = Get-ChildItem "$vsPath\VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT" -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -notmatch '\\onecore\\' } |
    Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if (-not $crt) { throw "MSVC runtime DLLs not found (install VS Build Tools or set NEWTOPO_VCREDIST_DIR)" }
Log "MSVC runtime from $crt"
Copy-Item "$crt\*.dll" $stage

# ------------------------------------------------------------------ check
if ($dumpbin) {
  Log "checking DLL resolution (dumpbin /dependents)"
  $present = @{}
  Get-ChildItem $stage -File | ForEach-Object { $present[$_.Name.ToLower()] = $true }
  $sys = "$env:SystemRoot\System32"
  $bad = @()
  foreach ($f in Get-ChildItem $stage -File | Where-Object { $_.Extension -in '.exe', '.dll' }) {
    foreach ($dll in Get-Imports $f.FullName) {
      $l = $dll.ToLower()
      if ($present[$l] -or $l -like 'api-ms-win-*' -or $l -like 'ext-ms-*' -or (Test-Path "$sys\$dll")) { continue }
      $bad += "$($f.Name) -> $dll"
    }
  }
  if ($bad) { $bad | ForEach-Object { Write-Host "unresolved: $_" }; throw "DLL check failed" }
  foreach ($e in $exes) { Write-Host "--- $e imports: $((Get-Imports "$stage\$e") -join ', ')" }
}
# Smoke test with a minimal PATH: a missing DLL makes Windows exit 0xC0000135.
$oldPath = $env:PATH
try {
  $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
  foreach ($e in 'newtopo-cli.exe', 'newtopo-import.exe') {
    if (-not (Test-Path "$stage\$e")) { continue }
    $p = Start-Process -FilePath "$stage\$e" -ArgumentList '--help' -NoNewWindow -Wait -PassThru `
      -RedirectStandardOutput "$work\smoke.out" -RedirectStandardError "$work\smoke.err"
    if ($p.ExitCode -eq -1073741515 -or $p.ExitCode -eq -1073741701) { throw "$e failed to load its DLLs (exit $($p.ExitCode))" }
  }
} finally { $env:PATH = $oldPath }

# ------------------------------------------------------------------ licences
Copy-Item "$pkgdir\common\THIRD-PARTY-NOTICES.txt" $stage
$py = (Get-Command python -ErrorAction SilentlyContinue).Source
if (-not $py) { $py = (Get-Command py -ErrorAction SilentlyContinue).Source }
if ($py) { & $py -I "$pkgdir\common\meta.py" rust-notices "$stage\THIRD-PARTY-RUST.txt" x86_64-pc-windows-msvc }
else { Write-Warning "python not found; THIRD-PARTY-RUST.txt not generated" }
$lic = "$pkgdir\common\licenses"
if ($env:OCCT_DIR -and (Test-Path "$env:OCCT_DIR\share\doc\opencascade\LICENSE_LGPL_21.txt")) { $lic = "$env:OCCT_DIR\share\doc\opencascade" }
Copy-Item "$lic\LICENSE_LGPL_21.txt", "$lic\OCCT_LGPL_EXCEPTION.txt" "$stage\licenses"
if (Test-Path "$root\LICENSE") { Copy-Item "$root\LICENSE" "$stage\LICENSE.txt" }
else { Write-Warning "$root\LICENSE missing; package has no newtopo licence file" }

# ------------------------------------------------------------------ zip
if (Want 'zip') {
  Log "zip"
  $zip = "$Out\$name.zip"
  if (Test-Path $zip) { Remove-Item $zip }
  Compress-Archive -Path $stage -DestinationPath $zip -CompressionLevel Optimal
}

# ------------------------------------------------------------------ installer
if (Want 'setup') {
  Log "Inno Setup installer"
  $iscc = (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source
  if (-not $iscc) {
    $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
              "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
  }
  if (-not $iscc -and $env:CI) {
    choco install innosetup -y --no-progress | Out-Host
    $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
  }
  if (-not $iscc) { throw "Inno Setup 6 (ISCC.exe) not found; install it from https://jrsoftware.org/isinfo.php" }
  Write-Host "using $iscc"
  & $iscc /Qp "/DAppVersion=$version" "/DNumVersion=$numVersion" "/DStageDir=$stage" "/DOutputDir=$Out" "$here\newtopo.iss"
  if ($LASTEXITCODE) { throw "ISCC failed" }
}

Remove-Item -Recurse -Force $work
Log "done"
Get-ChildItem $Out -File | Format-Table Name, Length
