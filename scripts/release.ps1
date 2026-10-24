# SPDX-License-Identifier: AGPL-3.0-only
# Builds a signed SulcusAI installer and the latest.json the app's updater
# reads. With -Publish it also creates the GitHub release vX.Y.Z (on the
# public IsaacLeh1/sulcusai repo) with both files attached.
#
#   powershell -File scripts\release.ps1                  # build and sign only
#   powershell -File scripts\release.ps1 -Publish -Notes "What's new"
#
# The notes go in the app first (Settings > Update notes): before building,
# py -3.12 -I scripts\add_update_notes.py <notes.md>, then commit. A build
# without this version's notes stops; publishing uses them unless -Notes or
# -NotesFile says otherwise.
#
# The signing key lives at %USERPROFILE%\.tauri\sulcusai.key (never commit
# it; without it, installed copies can never update again).
# The Microsoft and Google sign-in registrations are built in from
# %USERPROFILE%\.tauri\sulcusai-oauth.env (KEY=value lines:
# SULCUSAI_MS_CLIENT_ID, SULCUSAI_GOOGLE_CLIENT_ID, SULCUSAI_GOOGLE_CLIENT_SECRET).

param(
    [switch]$Publish,
    [string]$Notes = "",
    # The notes from a file. Passing them with -Notes from another PowerShell
    # cuts them at the first double quote (Windows PowerShell 5.1 argument quoting).
    [string]$NotesFile = "",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$keyFile = Join-Path $env:USERPROFILE ".tauri\sulcusai.key"
if (-not (Test-Path $keyFile)) { throw "Signing key not found at $keyFile" }

$version = (Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
$tag = "v$version"
$bundle = Join-Path $root "src-tauri\target\release\bundle\nsis"
$exeName = "SulcusAI_${version}_x64-setup.exe"
$exe = Join-Path $bundle $exeName
$sig = "$exe.sig"

# What's published must be this commit, built from it, under one number.
$dirty = git -C $root status --porcelain --untracked-files=no
if ($dirty) { throw "There are uncommitted changes. Commit them first, so the build matches the tag:`n$($dirty -join "`n")" }
$cargoVersion = (Select-String -Path (Join-Path $root "src-tauri\Cargo.toml") -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$packageVersion = (Get-Content (Join-Path $root "package.json") -Raw | ConvertFrom-Json).version
if ($cargoVersion -ne $version -or $packageVersion -ne $version) {
    throw "The version numbers differ: tauri.conf.json $version, Cargo.toml $cargoVersion, package.json $packageVersion."
}

# The app shows every version's notes under Settings > Update notes, from
# src/updateNotes.json: this version's must be in it before it's built.
$notesJson = Join-Path $root "src\updateNotes.json"
$bundled = ([System.IO.File]::ReadAllText($notesJson, [System.Text.Encoding]::UTF8) | ConvertFrom-Json) | Where-Object { $_.version -eq $version } | Select-Object -First 1
if (-not $bundled -or -not $bundled.notes.Trim()) {
    throw "src\updateNotes.json has no notes for $version. Write them, add them with: py -3.12 -I scripts\add_update_notes.py <notes.md>, and commit, so the app shows them under Settings > Update notes."
}

if (-not $SkipBuild) {
    $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $keyFile -Raw
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
    $oauthFile = Join-Path $env:USERPROFILE ".tauri\sulcusai-oauth.env"
    if (Test-Path $oauthFile) {
        foreach ($line in Get-Content $oauthFile) {
            if ($line -match '^\s*(SULCUSAI_[A-Z_]+)\s*=\s*(.*?)\s*$' -and $Matches[2]) {
                Set-Item -Path "Env:$($Matches[1])" -Value $Matches[2]
            }
        }
    } else {
        Write-Warning "No $oauthFile; this build won't have Microsoft or Google sign-in built in."
    }
    Push-Location $root
    try {
        pnpm tauri build
        if ($LASTEXITCODE -ne 0) { throw "The build failed." }
    } finally {
        Pop-Location
        Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
    }
}
if (-not (Test-Path $sig)) { throw "No signature next to $exeName; was it built with the signing key?" }
# Publishing an earlier build: only one made after the last commit.
if ($SkipBuild -and $Publish) {
    $committed = [DateTimeOffset]::FromUnixTimeSeconds([long](git -C $root log -1 --format=%ct)).LocalDateTime
    if ((Get-Item $exe).LastWriteTime -lt $committed) { throw "$exeName was built before the last commit. Build again (without -SkipBuild) so it matches." }
}

if ($NotesFile) { $Notes = [System.IO.File]::ReadAllText((Resolve-Path $NotesFile), [System.Text.Encoding]::UTF8).Trim() }
# The release says what the app shows for this version, unless told otherwise.
if (-not $Notes) { $Notes = $bundled.notes.Trim() }

$latest = [ordered]@{
    version   = $version
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = [ordered]@{
        "windows-x86_64" = [ordered]@{
            signature = (Get-Content $sig -Raw).Trim()
            url       = "https://github.com/IsaacLeh1/sulcusai/releases/download/$tag/$exeName"
        }
    }
}
$latestPath = Join-Path $bundle "latest.json"
# UTF-8 without a byte-order mark, which the updater's JSON parser needs.
[System.IO.File]::WriteAllText($latestPath, ($latest | ConvertTo-Json -Depth 5), (New-Object System.Text.UTF8Encoding $false))
Write-Host "Built and signed $exeName; wrote latest.json"

if ($Publish) {
    # The tag goes on this commit, not on whatever main is on GitHub: the
    # macOS and Linux builds start from the tag (0.9.14's first went out on
    # 0.9.13's code because main wasn't pushed yet).
    $commit = (git -C $root rev-parse HEAD).Trim()
    if (-not (git -C $root branch -r --contains $commit)) { throw "Push $commit to GitHub first: the release tag goes on it." }
    $title = "SulcusAI $version"
    $body = if ($Notes) { $Notes } else { "SulcusAI $version" }
    # From a file, so quotes in the notes reach GitHub intact.
    $bodyFile = Join-Path $env:TEMP "sulcusai-notes-$version.md"
    [System.IO.File]::WriteAllText($bodyFile, $body, (New-Object System.Text.UTF8Encoding $false))
    gh release create $tag $exe $latestPath --repo IsaacLeh1/sulcusai --target $commit --title $title --notes-file $bodyFile
    Remove-Item $bodyFile -ErrorAction SilentlyContinue
    if ($LASTEXITCODE -ne 0) { throw "Publishing the release failed." }
    Write-Host "Published $tag. Installed copies will find it on their next check."
}
