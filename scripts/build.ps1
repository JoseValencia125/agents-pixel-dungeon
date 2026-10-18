$ErrorActionPreference = 'Stop'
$ProjectDir = Split-Path -Parent $PSScriptRoot
Set-Location $ProjectDir

# Windows PowerShell 5.1 turns cargo's normal stderr output into errors under 'Stop'.
$ErrorActionPreference = 'Continue'
cargo build --release
$status = $LASTEXITCODE
$ErrorActionPreference = 'Stop'
if ($status -ne 0) { throw "cargo build failed (exit $status)" }

$Build = Join-Path $ProjectDir 'build'
New-Item -ItemType Directory -Force $Build | Out-Null
Copy-Item (Join-Path $ProjectDir 'target\release\agents-pixel-dungeon.exe') $Build -Force
Copy-Item (Join-Path $ProjectDir 'target\release\agents-pixel-dungeon-guild.exe') $Build -Force
Copy-Item (Join-Path $ProjectDir 'Resources\Icon\icon_256.png') (Join-Path $Build 'agents-pixel-dungeon.png') -Force

$Licenses = Join-Path $Build 'Licenses'
New-Item -ItemType Directory -Force $Licenses | Out-Null
Copy-Item (Join-Path $ProjectDir 'Resources\Licenses\*') $Licenses -Force
Copy-Item (Join-Path $ProjectDir 'LICENSE') (Join-Path $Licenses 'APP-MIT.txt') -Force
Copy-Item (Join-Path $ProjectDir 'NOTICE.md') (Join-Path $Licenses 'NOTICE.md') -Force

Write-Host "Built: $Build\agents-pixel-dungeon.exe"
