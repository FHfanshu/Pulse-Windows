# Run from the repository root after the x64 Tauri build.
# -RequireUpdaterArtifacts: a release build, which must also carry the installer's signature and the
# update feed (latest.json) the in-app updater reads.
param([switch]$RequireUpdaterArtifacts)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$version = (Get-Content -LiteralPath (Join-Path $repoRoot 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version
$buildRoot = Join-Path $repoRoot 'target/x86_64-pc-windows-msvc/release'
$installer = @(Get-ChildItem -LiteralPath (Join-Path $buildRoot 'bundle/nsis') -Filter '*-setup.exe' -File)
if ($installer.Count -ne 1) { throw 'Expected exactly one NSIS installer' }
$executable = Join-Path $buildRoot 'pulse.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw "Missing application: $executable" }
$output = Join-Path $repoRoot 'release-artifacts'
New-Item -ItemType Directory -Path $output -Force | Out-Null
$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("pulse-package-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    $setupName = "Pulse_${version}_x64-setup.exe"
    Copy-Item -LiteralPath $installer[0].FullName -Destination (Join-Path $output $setupName)
    $signature = "$($installer[0].FullName).sig"
    if (Test-Path -LiteralPath $signature) {
        Copy-Item -LiteralPath $signature -Destination (Join-Path $output "$setupName.sig")
        node (Join-Path $repoRoot 'scripts/make-latest-json.mjs') (Join-Path $output $setupName) (Join-Path $output "$setupName.sig") (Join-Path $output 'latest.json')
        if ($LASTEXITCODE -ne 0) { throw 'Could not write latest.json' }
    } elseif ($RequireUpdaterArtifacts) {
        throw "Missing updater signature: $signature (is TAURI_SIGNING_PRIVATE_KEY set?)"
    }
    Copy-Item -LiteralPath $executable -Destination (Join-Path $stage 'pulse.exe')
    foreach ($name in @('LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md', 'THIRD_PARTY_LICENSES.md')) {
        Copy-Item -LiteralPath (Join-Path $repoRoot $name) -Destination (Join-Path $stage $name)
    }
    @"
Pulse for Windows $version
Extract the entire archive and run pulse.exe on Windows 10/11 x64.
Microsoft Edge WebView2 Runtime is required.
Settings and cache: %APPDATA%\Pulse
Downloads and source: https://github.com/FHfanshu/Pulse-Windows
This build is unsigned. See the included license and notices.
"@ | Set-Content -LiteralPath (Join-Path $stage 'README.txt') -Encoding utf8
    $zip = Join-Path $output "Pulse_${version}_windows-x64.zip"
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -Force
    $files = @("Pulse_${version}_x64-setup.exe", "Pulse_${version}_windows-x64.zip")
    $checksums = foreach ($file in $files) {
        $hash = (Get-FileHash -LiteralPath (Join-Path $output $file) -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $file"
    }
    $checksums | Set-Content -LiteralPath (Join-Path $output 'SHA256SUMS.txt') -Encoding ascii
    Get-ChildItem -LiteralPath $output -File | Select-Object Name, Length
} finally {
    # Only remove the unique staging folder allocated above, never a caller-supplied path.
    $resolvedStage = [System.IO.Path]::GetFullPath($stage)
    $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    if (-not $resolvedStage.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or
        -not (Split-Path -Leaf $resolvedStage).StartsWith('pulse-package-')) { throw 'Unsafe staging path' }
    Remove-Item -LiteralPath $resolvedStage -Recurse -Force
}
