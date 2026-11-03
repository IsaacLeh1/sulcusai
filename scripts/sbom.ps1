# SPDX-License-Identifier: AGPL-3.0-only
# Writes a bill of materials (CycloneDX 1.5 JSON) listing every Rust crate and
# npm package this version of SulcusAI is built from, with versions, licenses
# and, where the lock files have them, checksums. Nothing is built or installed:
# Rust crates come from `cargo metadata` (with Cargo.lock), npm packages from
# pnpm-lock.yaml.
#
#   powershell -File scripts\sbom.ps1                      # to %TEMP%\sulcusai-sbom.cdx.json
#   powershell -File scripts\sbom.ps1 -Out C:\x\sbom.json
#   powershell -File scripts\sbom.ps1 -Plain               # a plain list instead of CycloneDX
#
# pnpm-lock.yaml has no licenses, so npm licenses are read from node_modules
# when it exists (pass -NodeModules to point at another checkout's folder);
# without it they are recorded as NOASSERTION rather than guessed.

param(
    [string]$Out = (Join-Path $env:TEMP "sulcusai-sbom.cdx.json"),
    [switch]$Plain,
    [string]$NodeModules = "",
    # cargo metadata can wait on another cargo's lock; give up instead of hanging.
    [int]$CargoTimeoutSeconds = 60
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $NodeModules) { $NodeModules = Join-Path $root "node_modules" }

# CycloneDX wants SPDX ids or expressions; Cargo.toml files still use "A/B".
function ConvertTo-LicenseList($text) {
    if (-not $text) { return @(, @{ expression = "NOASSERTION" }) }
    $t = ($text -replace '\s*/\s*', ' OR ').Trim()
    if ($t -match '\s|\(') { return @(, @{ expression = $t }) }
    return @(, @{ license = @{ id = $t } })
}

function Get-LicenseText($licenses) { ($licenses | ForEach-Object { if ($_.expression) { $_.expression } else { $_.license.id } }) -join "; " }

# ---- Rust ----
$cargoDir = Join-Path $root "src-tauri"
$cargoOut = [IO.Path]::GetTempFileName()
$cargoErr = [IO.Path]::GetTempFileName()
try {
    $p = Start-Process -FilePath "cargo" -ArgumentList @("metadata", "--format-version", "1", "--locked", "--manifest-path", (Join-Path $cargoDir "Cargo.toml")) `
        -NoNewWindow -PassThru -RedirectStandardOutput $cargoOut -RedirectStandardError $cargoErr
    # Windows PowerShell 5.1 reports a null ExitCode unless the handle was touched.
    $null = $p.Handle
    if (-not $p.WaitForExit($CargoTimeoutSeconds * 1000)) {
        Stop-Process -Id $p.Id -Force
        throw "cargo metadata did not finish in $CargoTimeoutSeconds s (another cargo may hold the lock); stopped it."
    }
    $p.WaitForExit()
    if ($p.ExitCode -ne 0) { throw "cargo metadata failed: $(Get-Content $cargoErr -Raw)" }
    # ConvertFrom-Json is case-insensitive and rejects feature names that differ only by case.
    Add-Type -AssemblyName System.Web.Extensions
    $serializer = New-Object System.Web.Script.Serialization.JavaScriptSerializer
    $serializer.MaxJsonLength = [int]::MaxValue
    $meta = $serializer.DeserializeObject([IO.File]::ReadAllText($cargoOut))
} finally {
    Remove-Item $cargoOut, $cargoErr -ErrorAction SilentlyContinue
}

$checksums = @{}
$lockText = [IO.File]::ReadAllText((Join-Path $cargoDir "Cargo.lock"))
foreach ($m in [regex]::Matches($lockText, '(?m)^name = "([^"]+)"\r?\nversion = "([^"]+)"\r?\n(?:source = "[^"]*"\r?\n)?checksum = "([0-9a-f]{64})"')) {
    $checksums["$($m.Groups[1].Value)@$($m.Groups[2].Value)"] = $m.Groups[3].Value
}

$rootPkgId = $meta["resolve"]["root"]
$rust = foreach ($pkg in $meta["packages"]) {
    if ($pkg["id"] -eq $rootPkgId) { continue }
    [pscustomobject]@{
        name = $pkg["name"]; version = $pkg["version"]; licenses = (ConvertTo-LicenseList $pkg["license"])
        sha256 = $checksums["$($pkg["name"])@$($pkg["version"])"]; ecosystem = "cargo"
    }
}
$rust = @($rust | Sort-Object name, version)

# ---- npm ----
# pnpm-lock.yaml starts with a small document for pnpm itself; the project's
# packages are in the last document.
$lockDocs = [IO.File]::ReadAllText((Join-Path $root "pnpm-lock.yaml")) -split '(?m)^---\s*$'
$lock = $lockDocs[$lockDocs.Count - 1]
$section = [regex]::Match($lock, '(?ms)^packages:\r?\n(.*?)(?=^snapshots:|\z)').Groups[1].Value
$npm = New-Object System.Collections.Generic.List[object]
$pnpmStore = Join-Path $NodeModules ".pnpm"
foreach ($m in [regex]::Matches($section, "(?m)^  '?((?:@[^/']+/)?[^@'\s]+)@([^':\s]+)'?:\r?\n\s+resolution: \{([^}]*)\}")) {
    $name = $m.Groups[1].Value; $version = $m.Groups[2].Value
    $integrity = [regex]::Match($m.Groups[3].Value, 'integrity: (\S+?)(?:,|$)').Groups[1].Value
    $license = $null
    if (Test-Path $pnpmStore) {
        $dir = Get-ChildItem -Path $pnpmStore -Directory -Filter ("{0}@{1}*" -f ($name -replace '/', '+'), $version) -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($dir) {
            $pj = Join-Path $dir.FullName "node_modules\$($name -replace '/', '\')\package.json"
            if (Test-Path $pj) {
                try {
                    $j = [IO.File]::ReadAllText($pj) | ConvertFrom-Json
                    if ($j.license -is [string]) { $license = $j.license }
                    elseif ($j.license.type) { $license = $j.license.type }
                    elseif ($j.licenses) { $license = (@($j.licenses | ForEach-Object { $_.type }) -join " OR ") }
                } catch { }
            }
        }
    }
    $npm.Add([pscustomobject]@{
        name = $name; version = $version; licenses = (ConvertTo-LicenseList $license)
        integrity = $integrity; ecosystem = "npm"
    })
}
$npm = @($npm | Sort-Object name, version)

# ---- Output ----
$appVersion = (Get-Content (Join-Path $cargoDir "tauri.conf.json") -Raw | ConvertFrom-Json).version
$commit = (& git -C $root rev-parse --short HEAD 2>$null)
$stamp = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

function ConvertTo-Purl($c) {
    if ($c.ecosystem -eq "cargo") { return "pkg:cargo/$($c.name)@$($c.version)" }
    $n = $c.name -replace '^@', '%40'
    return "pkg:npm/$n@$($c.version)"
}

if ($Plain) {
    $doc = [ordered]@{
        app = "SulcusAI"; version = $appVersion; commit = $commit; generated = $stamp
        crates = @($rust | ForEach-Object { [ordered]@{ name = $_.name; version = $_.version; license = (Get-LicenseText $_.licenses); sha256 = $_.sha256 } })
        npm    = @($npm  | ForEach-Object { [ordered]@{ name = $_.name; version = $_.version; license = (Get-LicenseText $_.licenses); integrity = $_.integrity } })
    }
} else {
    $components = foreach ($c in @($rust) + @($npm)) {
        $comp = [ordered]@{
            type = "library"; "bom-ref" = (ConvertTo-Purl $c); name = $c.name; version = $c.version
            purl = (ConvertTo-Purl $c); licenses = @($c.licenses)
        }
        if ($c.ecosystem -eq "cargo" -and $c.sha256) { $comp.hashes = @(, [ordered]@{ alg = "SHA-256"; content = $c.sha256 }) }
        if ($c.ecosystem -eq "npm" -and $c.integrity -match '^sha512-(.+)$') {
            $comp.hashes = @(, [ordered]@{ alg = "SHA-512"; content = (([Convert]::FromBase64String($Matches[1]) | ForEach-Object { $_.ToString("x2") }) -join "") })
        }
        $comp
    }
    $doc = [ordered]@{
        bomFormat = "CycloneDX"; specVersion = "1.5"; version = 1
        serialNumber = "urn:uuid:$([guid]::NewGuid())"
        metadata = [ordered]@{
            timestamp = $stamp
            component = [ordered]@{ type = "application"; name = "SulcusAI"; version = $appVersion; licenses = @(, @{ license = @{ id = "AGPL-3.0-only" } }) }
            properties = @(, [ordered]@{ name = "git:commit"; value = "$commit" })
        }
        components = @($components)
    }
}

$json = $doc | ConvertTo-Json -Depth 10
# No byte-order mark: other tools read this as plain UTF-8.
[IO.File]::WriteAllText($Out, $json, (New-Object Text.UTF8Encoding($false)))

$unknown = @(@($rust) + @($npm) | Where-Object { (Get-LicenseText $_.licenses) -eq "NOASSERTION" }).Count
Write-Host "Wrote $Out"
Write-Host ("Rust crates: {0}  npm packages: {1}  without a license: {2}" -f $rust.Count, $npm.Count, $unknown)
