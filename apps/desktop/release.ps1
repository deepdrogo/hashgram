<#
.SYNOPSIS
  Reproducible release build of Hashgram One for Windows. Prints the SHA-256
  of every artefact and enforces the installer size budget (< 45 MB).

.DESCRIPTION
  Steps: pnpm install (frozen lockfile) -> typecheck + frontend tests ->
  sidecars -> desktop crate tests (incl. the no-plaintext audit) ->
  `tauri build` (NSIS + MSI, per-user) -> optional Authenticode -> checksums
  -> updater manifest. Code signing and updater signing are applied when the
  owner supplies the keys through the environment; otherwise the script says
  so in the release notes it writes, About shows "unsigned preview", and
  SmartScreen will warn on first run.

  Authenticode is architected but disabled until a certificate exists: set
  HASHGRAM_CODESIGN_THUMBPRINT and the same script signs the .exe/.msi and
  bakes HASHGRAM_CODESIGNED=1 into the binary (About -> "signed").

  Environment (all optional):
    TAURI_SIGNING_PRIVATE_KEY           minisign private key (updater manifest signing;
                                        the owner keeps this offline and only sets it on
                                        the release machine for the duration of the build)
    TAURI_SIGNING_PRIVATE_KEY_PATH      alternatively, a file holding that key
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD  its password
    HASHGRAM_CODESIGN_THUMBPRINT        SHA-1 thumbprint of an EV/OV certificate in the
                                        user's certificate store (signtool)
    HASHGRAM_COMMIT                     commit to bake in (default: git rev-parse)

.EXAMPLE
  pwsh apps/desktop/release.ps1
  pwsh apps/desktop/release.ps1 -SkipInstall
#>
[CmdletBinding()]
param(
    [switch]$SkipInstall,
    [string]$OutDir = "",
    # GitHub repository whose Releases host the installer and latest.json.
    [string]$Repo = "deepdrogo/hashgram_windows"
)

$ErrorActionPreference = "Continue"
$here = $PSScriptRoot
Set-Location $here
$root = Resolve-Path (Join-Path $here "..\..")
$commit = if ($env:HASHGRAM_COMMIT) { $env:HASHGRAM_COMMIT } else { (git -C $root rev-parse --short=12 HEAD).Trim() }
$env:HASHGRAM_COMMIT = $commit
if ($OutDir -eq "") { $OutDir = Join-Path $root "dist\desktop" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Step([string]$m) { Write-Host ""; Write-Host "==> $m" }
function Fail([string]$m) { Write-Host "release: $m"; exit 1 }

Step "toolchain"
rustc --version; cargo --version; node --version; pnpm --version
Write-Host "commit $commit"

if (-not $SkipInstall) {
    Step "pnpm install --frozen-lockfile"
    pnpm install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { Fail "pnpm install failed" }
}

Step "typecheck"
pnpm exec tsc --noEmit
if ($LASTEXITCODE -ne 0) { Fail "typecheck failed" }

Step "frontend tests"
pnpm exec vitest run
if ($LASTEXITCODE -ne 0) { Fail "frontend tests failed" }

Step "rule checks (no hardcoded hosts, no telemetry, no CDN)"
$appsDir = Join-Path $root "apps"
$ipv4 = Get-ChildItem $appsDir -Recurse -File -Include *.rs,*.ts,*.tsx,*.json,*.html,*.css `
    | Where-Object { $_.FullName -notmatch "node_modules|\\target\\|\\dist\\" } `
    | Select-String -Pattern "\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b" `
    | Where-Object { $_.Line -notmatch "127\.0\.0\.1|0\.0\.0\.0|version|\d+\.\d+\.\d+\.\d+\s*(MiB|MB)" }
if ($ipv4) { $ipv4 | ForEach-Object { Write-Host "  $($_.Path):$($_.LineNumber): $($_.Line.Trim())" }; Fail "hardcoded IPv4 address in apps/" }
$cdn = Get-ChildItem $appsDir -Recurse -File -Include *.ts,*.tsx,*.html,*.css `
    | Where-Object { $_.FullName -notmatch "node_modules|\\dist\\" } `
    | Select-String -Pattern "fonts\.googleapis|cdn\.jsdelivr|unpkg\.com|sentry\.io|googletagmanager|google-analytics|segment\.com|mixpanel|posthog"
if ($cdn) { $cdn | ForEach-Object { Write-Host "  $($_.Path):$($_.LineNumber)" }; Fail "third-party CDN/analytics reference in apps/" }

Step "sidecars: hashgram-node and the service wrapper (release)"
Push-Location (Join-Path $root "node")
cargo build --release -p hashgram-node -p hashgram-node-service
if ($LASTEXITCODE -ne 0) { Pop-Location; Fail "sidecar build failed" }
Pop-Location
$bin = Join-Path $here "src-tauri\binaries"
New-Item -ItemType Directory -Force -Path $bin | Out-Null
Copy-Item (Join-Path $root "node\target\release\hashgram-node.exe") (Join-Path $bin "hashgram-node-x86_64-pc-windows-msvc.exe") -Force
Copy-Item (Join-Path $root "node\target\release\hashgram-node-service.exe") (Join-Path $bin "hashgram-node-service-x86_64-pc-windows-msvc.exe") -Force

Step "rust tests (desktop crate)"
Push-Location (Join-Path $root "node")
cargo test -p hashgram-desktop
if ($LASTEXITCODE -ne 0) { Pop-Location; Fail "rust tests failed" }
Pop-Location

$updater = $false
if (-not $env:TAURI_SIGNING_PRIVATE_KEY -and $env:TAURI_SIGNING_PRIVATE_KEY_PATH -and (Test-Path $env:TAURI_SIGNING_PRIVATE_KEY_PATH)) {
    $env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content $env:TAURI_SIGNING_PRIVATE_KEY_PATH -Raw).Trim()
}
if ($env:TAURI_SIGNING_PRIVATE_KEY) {
    $updater = $true
    Write-Host "updater artefacts: signing key present, will produce and sign .sig files"
} else {
    Write-Host "updater artefacts: no TAURI_SIGNING_PRIVATE_KEY; unsigned builds cannot be published as updates"
}

if ($env:HASHGRAM_CODESIGN_THUMBPRINT) {
    # Baked into the binary at compile time: About shows "signed".
    $env:HASHGRAM_CODESIGNED = "1"
    Write-Host "code signing: thumbprint present, artefacts will be Authenticode-signed after the build"
} else {
    Remove-Item Env:HASHGRAM_CODESIGNED -ErrorAction SilentlyContinue
    Write-Host "code signing: disabled (no HASHGRAM_CODESIGN_THUMBPRINT); About will say 'unsigned preview'"
}

Step "tauri build (nsis, msi)"
# PowerShell mangles quoted JSON on the command line; hand Tauri a file.
$cfgFile = Join-Path $env:TEMP "hashgram-tauri-override.json"
$cfgJson = if ($updater) { '{"bundle":{"createUpdaterArtifacts":true}}' } else { '{"bundle":{"createUpdaterArtifacts":false}}' }
Set-Content -Path $cfgFile -Value $cfgJson -Encoding ascii
pnpm tauri build --config $cfgFile
if ($LASTEXITCODE -ne 0) { Fail "tauri build failed" }

$bundle = Join-Path $root "node\target\release\bundle"
# Only this version's files: the bundle directory keeps older builds around.
$version = (Get-Content (Join-Path $here "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
$mine = "*_${version}_*"
$artefacts = @()
$artefacts += Get-ChildItem (Join-Path $bundle "nsis") -Filter *.exe -ErrorAction SilentlyContinue | Where-Object { $_.Name -like $mine }
$artefacts += Get-ChildItem (Join-Path $bundle "msi") -Filter *.msi -ErrorAction SilentlyContinue | Where-Object { $_.Name -like $mine }
$artefacts += Get-ChildItem (Join-Path $bundle "nsis") -Filter *.sig -ErrorAction SilentlyContinue | Where-Object { $_.Name -like $mine }
$artefacts += Get-ChildItem (Join-Path $bundle "msi") -Filter *.sig -ErrorAction SilentlyContinue | Where-Object { $_.Name -like $mine }
if (-not $artefacts) { Fail "no artefacts for version $version found under $bundle" }

if ($env:HASHGRAM_CODESIGN_THUMBPRINT) {
    Step "code signing"
    foreach ($a in $artefacts | Where-Object { $_.Extension -in ".exe", ".msi" }) {
        signtool sign /sha1 $env:HASHGRAM_CODESIGN_THUMBPRINT /tr http://timestamp.digicert.com /td sha256 /fd sha256 $a.FullName
        if ($LASTEXITCODE -ne 0) { Fail "signtool failed on $($a.Name)" }
    }
} else {
    Write-Host "code signing: no HASHGRAM_CODESIGN_THUMBPRINT; artefacts are unsigned (SmartScreen will warn)"
}

Step "artefacts and SHA-256"
$notes = @()
$notes += "Hashgram One for Windows $version - commit $commit - built $(Get-Date -Format s)"
$notes += "Installs per user (no admin), data in %LOCALAPPDATA%\Hashgram\data; a v0.1.x vault is migrated in place."
$notes += ""
$failures = 0
$publishedArtefacts = @()
foreach ($a in $artefacts) {
    # GitHub's release upload API silently rewrites spaces in asset names to
    # dots. Normalize before publishing so latest.json always names the real
    # downloadable asset (and local/CI releases produce identical names).
    $publishName = $a.Name -replace " ", "."
    $destination = Join-Path $OutDir $publishName
    Copy-Item $a.FullName $destination -Force
    $published = Get-Item $destination
    $publishedArtefacts += $published
    $sha = (Get-FileHash $published.FullName -Algorithm SHA256).Hash.ToLower()
    $mb = [math]::Round($published.Length / 1MB, 2)
    $line = "{0}  {1}  {2} MB" -f $sha, $published.Name, $mb
    Write-Host $line
    $notes += $line
    if ($published.Extension -eq ".exe" -and $published.Length -gt 45MB) { Write-Host "  [FAIL] installer exceeds the 45 MB budget"; $failures++ }
}

# Updater manifest: the app fetches <release>/latest/download/latest.json and
# follows `url` only when `signature` verifies against the compiled-in key.
if ($updater) {
    Step "updater manifest (latest.json)"
    $setup = $publishedArtefacts | Where-Object { $_.Extension -eq ".exe" } | Select-Object -First 1
    $sigFile = "$($setup.FullName).sig"
    if (-not (Test-Path $sigFile)) { Fail "missing signature $sigFile" }
    $manifest = [ordered]@{
        version  = $version
        notes    = "Hashgram One for Windows $version (commit $commit). SHA-256 of the installer: " + (Get-FileHash $setup.FullName -Algorithm SHA256).Hash.ToLower()
        pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
        platforms = [ordered]@{
            "windows-x86_64" = [ordered]@{
                signature = (Get-Content $sigFile -Raw).Trim()
                url       = "https://github.com/$Repo/releases/download/v$version/$($setup.Name)"
            }
        }
    }
    $manifestPath = Join-Path $OutDir "latest.json"
    [IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding($false)))
    Write-Host "latest.json -> $($manifest.platforms['windows-x86_64'].url)"
    $notes += "latest.json  version $version  url $($manifest.platforms['windows-x86_64'].url)"
}
$notes += ""
$notes += "Code signing: " + $(if ($env:HASHGRAM_CODESIGN_THUMBPRINT) { "signed (Authenticode, SHA-256, timestamped); About shows 'signed'" } else { "UNSIGNED PREVIEW - Windows SmartScreen will warn on first run until an EV/OV certificate is used; About shows 'unsigned preview'" })
$notes += "Updater: " + $(if ($updater) { "signed .sig files and latest.json produced; publish them with the installer under GitHub release v$version" } else { "not produced (no minisign key on this machine)" })
$notes += "Verify: (Get-FileHash <file> -Algorithm SHA256).Hash.ToLower()"
$notes | Set-Content (Join-Path $OutDir "RELEASE_NOTES.txt") -Encoding ascii
$notes | ForEach-Object { $_ } | Select-Object -Skip 2 | Where-Object { $_ -match "^[0-9a-f]{64}" } | Set-Content (Join-Path $OutDir "SHA256SUMS.txt") -Encoding ascii
Write-Host ""
Write-Host "artefacts in $OutDir"
if ($failures -gt 0) { exit 1 } else { exit 0 }
