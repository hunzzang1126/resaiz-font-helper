# resaiz Font Helper: install on Windows and start it at login.
# Usage (PowerShell): .\install-win.ps1 [path\to\resaiz-font-helper.exe]
param([string]$Source = (Join-Path $PSScriptRoot "resaiz-font-helper.exe"))
$ErrorActionPreference = "Stop"
if (-not (Test-Path $Source)) { Write-Error "binary not found: $Source" }
$destDir = Join-Path $env:LOCALAPPDATA "resaiz"
$dest = Join-Path $destDir "resaiz-font-helper.exe"
New-Item -ItemType Directory -Force -Path $destDir | Out-Null
Get-Process resaiz-font-helper -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Copy-Item $Source $dest -Force
Unblock-File $dest -ErrorAction SilentlyContinue
$startup = [Environment]::GetFolderPath("Startup")
$shortcut = Join-Path $startup "resaiz Font Helper.lnk"
$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut($shortcut)
$lnk.TargetPath = $dest
$lnk.WindowStyle = 7
$lnk.Description = "resaiz Font Helper (local font bridge)"
$lnk.Save()
Start-Process -FilePath $dest -WindowStyle Hidden
Start-Sleep -Seconds 1
try {
  $h = Invoke-RestMethod http://127.0.0.1:57731/v1/health
  Write-Host "resaiz Font Helper is running: $($h.fonts) fonts"
} catch {
  Write-Host "installed; the helper starts within a few seconds"
}
Write-Host "It starts automatically at login. To remove: .\uninstall-win.ps1"
