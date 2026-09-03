# Remove resaiz Font Helper from Windows.
Get-Process resaiz-font-helper -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
$startup = [Environment]::GetFolderPath("Startup")
Remove-Item (Join-Path $startup "resaiz Font Helper.lnk") -ErrorAction SilentlyContinue
Remove-Item (Join-Path $env:LOCALAPPDATA "resaiz") -Recurse -Force -ErrorAction SilentlyContinue
Write-Host "resaiz Font Helper removed"
