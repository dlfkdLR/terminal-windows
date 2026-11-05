# terminal-windows installer for Windows (x64).
#   irm https://github.com/dlfkdLR/terminal-windows/releases/latest/download/install.ps1 | iex
# Verifies the SHA-256 against SHA256SUMS and installs to %LOCALAPPDATA%\Programs\terminal-windows.
$ErrorActionPreference = 'Stop'
$repo = if ($env:TW_REPO) { $env:TW_REPO } else { 'dlfkdLR/terminal-windows' }
$base = "https://github.com/$repo/releases/latest/download"
$asset = 'terminal-windows-x86_64-pc-windows-msvc.zip'
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
New-Item -ItemType Directory $tmp | Out-Null
try {
  Invoke-WebRequest "$base/$asset" -OutFile "$tmp\$asset" -UseBasicParsing
  Invoke-WebRequest "$base/SHA256SUMS" -OutFile "$tmp\SHA256SUMS" -UseBasicParsing
  $line = Get-Content "$tmp\SHA256SUMS" | Where-Object { $_ -match [regex]::Escape($asset) }
  if (-not $line) { throw "$asset is not listed in SHA256SUMS" }
  $expected = ($line -split '\s+')[0].ToLower()
  $actual = (Get-FileHash "$tmp\$asset" -Algorithm SHA256).Hash.ToLower()
  if ($expected -ne $actual) { throw "SHA-256 mismatch for $asset" }
  $dest = Join-Path $env:LOCALAPPDATA 'Programs\terminal-windows'
  New-Item -ItemType Directory -Force $dest | Out-Null
  Expand-Archive "$tmp\$asset" -DestinationPath $dest -Force
  Write-Host "Installed to $dest\terminal-windows.exe (SHA-256 verified)"
  if (($env:Path -split ';') -notcontains $dest) { Write-Host "Add $dest to your PATH." }
  Write-Host "Windows hosts are experimental; see docs/compatibility.md."
} finally { Remove-Item -Recurse -Force $tmp }
