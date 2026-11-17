<#
  Smoke test of a packaged newtopo on Windows.

    smoke.ps1 -Dir <folder with newtopo*.exe> -Models <models-dir> -Work <work-dir>

  Runs newtopo-cli info / run / validate / export (exercising newtopo-import
  next to the exe and the bundled OCCT and MSVC runtime DLLs), then starts the
  GUI in its headless screenshot mode. PATH is reduced to the Windows system
  folders, so nothing from the build machine can be picked up. A DLL load
  failure is fatal; a GUI without a usable GPU adapter only warns, unless
  NEWTOPO_SMOKE_GPU=1.
#>
param(
  [Parameter(Mandatory)][string]$Dir,
  [Parameter(Mandatory)][string]$Models,
  [Parameter(Mandatory)][string]$Work,
  [int]$GuiTimeout = 300,
  [switch]$SkipGui
)
$ErrorActionPreference = 'Stop'
$DllNotFound = -1073741515   # 0xC0000135 STATUS_DLL_NOT_FOUND
$EntryNotFound = -1073741511 # 0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND
$BadImage = -1073741701      # 0xC000007B STATUS_INVALID_IMAGE_FORMAT

New-Item -ItemType Directory -Force $Work | Out-Null
$Work = (Resolve-Path $Work).Path
Copy-Item "$Models\filleted_box.step" $Work
Set-Content -Encoding ascii "$Work\script.json" '{"version":1,"input":"filleted_box.step","ops":[{"op":"slice","body":1,"count":10}]}'

$cli = Join-Path $Dir 'newtopo-cli.exe'
$gui = Join-Path $Dir 'newtopo.exe'
$oldPath = $env:PATH
Remove-Item Env:NEWTOPO_IMPORT_HELPER -ErrorAction SilentlyContinue
$env:PATH = "$env:SystemRoot\System32;$env:SystemRoot;$env:SystemRoot\System32\WindowsPowerShell\v1.0"

# Not "Cli": that name is a built-in alias of Clear-Item and would win.
function Invoke-Cli([string]$title, [string[]]$cliArgs) {
  Write-Host "`n--- $title"
  & $cli @cliArgs
  $rc = $LASTEXITCODE
  if ($rc -in $DllNotFound, $EntryNotFound, $BadImage) { throw "newtopo-cli failed to load its DLLs (exit $rc)" }
  if ($rc -ne 0) { throw "newtopo-cli $title failed (exit $rc)" }
}
try {
  Invoke-Cli 'info filleted_box.step' @('info', "$Work\filleted_box.step")
  Invoke-Cli 'run script.json -o out.ntopo' @('run', "$Work\script.json", '-o', "$Work\out.ntopo")
  if (-not (Test-Path "$Work\out.ntopo")) { throw "out.ntopo missing" }
  Invoke-Cli 'validate out.ntopo' @('validate', "$Work\out.ntopo")
  Invoke-Cli 'export out.ntopo -o out.fbx' @('export', "$Work\out.ntopo", '-o', "$Work\out.fbx")
  $magic = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes("$Work\out.fbx")[0..17])
  if ($magic -ne 'Kaydara FBX Binary') { throw "out.fbx is not binary FBX" }
  Get-ChildItem "$Work\out.ntopo", "$Work\out.fbx" | Format-Table Name, Length
  Write-Host "SMOKE CLI OK: $cli"
  if ($SkipGui) { return }

  Write-Host "`n--- newtopo.exe --shot (headless, ${GuiTimeout}s limit)"
  New-Item -ItemType Directory -Force "$Work\shots" | Out-Null
  $p = Start-Process -FilePath $gui -ArgumentList '--shot', "`"$Work\shots`"" -PassThru -NoNewWindow `
    -RedirectStandardOutput "$Work\gui.out" -RedirectStandardError "$Work\gui.err"
  $null = $p.Handle  # cache the handle so ExitCode is available later
  if (-not $p.WaitForExit($GuiTimeout * 1000)) {
    $p.Kill()
    Get-Content "$Work\gui.err" -ErrorAction SilentlyContinue
    Write-Warning "GUI still running after ${GuiTimeout}s; it loaded its DLLs but did not finish"
    return
  }
  $rc = $p.ExitCode
  Get-Content "$Work\gui.out", "$Work\gui.err" -ErrorAction SilentlyContinue
  if ($rc -in $DllNotFound, $EntryNotFound, $BadImage) { throw "newtopo.exe failed to load its DLLs (exit $rc)" }
  $n = @(Get-ChildItem "$Work\shots" -Filter *.png -ErrorAction SilentlyContinue).Count
  if ($rc -eq 0 -and $n -gt 0) { Write-Host "SMOKE GUI OK: $n screenshots" }
  elseif ($env:NEWTOPO_SMOKE_GPU -eq '1') { throw "GUI exited $rc with $n screenshots" }
  else { Write-Warning "GUI loaded but exited $rc with $n screenshots (no usable GPU adapter?)" }
} finally {
  $env:PATH = $oldPath
}
