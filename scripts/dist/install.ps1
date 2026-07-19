# Vaporly installer for Windows. Downloads the latest setup .exe and runs it.
#
# Usage:
#   powershell -c "irm https://github.com/pohsuchenwork/vaporly/releases/latest/download/install.ps1 | iex"
#
# Note: while the repository is private these downloads need a GitHub login.
# They become anonymous once the repo is public.
$ErrorActionPreference = "Stop"

$Repo = "pohsuchenwork/vaporly"

switch ($env:PROCESSOR_ARCHITECTURE) {
  "AMD64" { $Arch = "x64" }
  "ARM64" { $Arch = "arm64" }
  default { throw "Unsupported architecture: $($env:PROCESSOR_ARCHITECTURE)" }
}

Write-Host ">> Finding the latest Vaporly setup for $Arch..."
$api = "https://api.github.com/repos/$Repo/releases/latest"
$release = Invoke-RestMethod -Uri $api -Headers @{ "User-Agent" = "vaporly-installer" }

$asset = $release.assets |
  Where-Object { $_.name -like "*-setup.exe" -and $_.name -like "*$Arch*" } |
  Select-Object -First 1
if (-not $asset) {
  $asset = $release.assets | Where-Object { $_.name -like "*-setup.exe" } | Select-Object -First 1
}
if (-not $asset) { throw "No setup .exe found on the latest release." }

$dest = Join-Path $env:TEMP $asset.name
Write-Host ">> Downloading $($asset.name)..."
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $dest

Write-Host ">> Launching the installer..."
Start-Process -FilePath $dest -Wait

Write-Host ""
Write-Host "Done. Launch Vaporly from the Start menu, then hold your dictation hotkey and speak."
