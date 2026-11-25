param(
    [string]$SourceDir = "",
    [switch]$Force
)

$ErrorActionPreference = "Stop"

$repoDesktopDir = Resolve-Path (Join-Path $PSScriptRoot "..")
$targetDir = Join-Path $repoDesktopDir "src-tauri\bin\ffmpeg"
New-Item -ItemType Directory -Path $targetDir -Force | Out-Null

$targetFfmpeg = Join-Path $targetDir "ffmpeg.exe"
$targetFfprobe = Join-Path $targetDir "ffprobe.exe"

function Test-MediaBin([string]$exePath) {
    if (-not (Test-Path -LiteralPath $exePath -PathType Leaf)) {
        return $false
    }
    $null = & $exePath -version 2>$null
    return $LASTEXITCODE -eq 0
}

if (-not $Force -and (Test-MediaBin $targetFfmpeg) -and (Test-MediaBin $targetFfprobe)) {
    Write-Host "FFmpeg runtime already staged in $targetDir"
    exit 0
}

function Copy-FromBinDir([string]$binDir) {
    $srcFfmpeg = Join-Path $binDir "ffmpeg.exe"
    $srcFfprobe = Join-Path $binDir "ffprobe.exe"
    if (-not (Test-Path -LiteralPath $srcFfmpeg -PathType Leaf) -or -not (Test-Path -LiteralPath $srcFfprobe -PathType Leaf)) {
        throw "Could not find ffmpeg.exe and ffprobe.exe in $binDir"
    }
    Copy-Item -LiteralPath $srcFfmpeg -Destination $targetFfmpeg -Force
    Copy-Item -LiteralPath $srcFfprobe -Destination $targetFfprobe -Force

    $parentDir = Split-Path -Parent $binDir
    foreach ($candidate in @(
        (Join-Path $binDir "LICENSE"),
        (Join-Path $binDir "LICENSE.txt"),
        (Join-Path $parentDir "LICENSE"),
        (Join-Path $parentDir "LICENSE.txt")
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            Copy-Item -LiteralPath $candidate -Destination (Join-Path $targetDir "LICENSE.txt") -Force
            break
        }
    }
}

if ($SourceDir) {
    Copy-FromBinDir $SourceDir
} else {
    $cmdFfmpeg = Get-Command ffmpeg.exe -ErrorAction SilentlyContinue
    $cmdFfprobe = Get-Command ffprobe.exe -ErrorAction SilentlyContinue
    if ($cmdFfmpeg -and $cmdFfprobe -and (Split-Path -Parent $cmdFfmpeg.Source) -eq (Split-Path -Parent $cmdFfprobe.Source)) {
        Copy-FromBinDir (Split-Path -Parent $cmdFfmpeg.Source)
    } else {
        $zipUrl = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"
        $shaUrl = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip.sha256"
        $tempDir = Join-Path $env:TEMP ("zibblu-ffmpeg-" + [guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
        try {
            $zipPath = Join-Path $tempDir "ffmpeg.zip"
            $extractDir = Join-Path $tempDir "extract"
            Write-Host "Downloading official Gyan.dev FFmpeg release essentials..."
            Invoke-WebRequest -Uri $zipUrl -OutFile $zipPath -UseBasicParsing
            $expectedSha = (Invoke-WebRequest -Uri $shaUrl -UseBasicParsing).Content.Trim().Split()[0].ToLowerInvariant()
            $actualSha = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($expectedSha.Length -ne 64 -or $actualSha -ne $expectedSha) {
                throw "SHA-256 mismatch for downloaded FFmpeg archive (expected $expectedSha, got $actualSha)"
            }
            Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force
            $binFound = Get-ChildItem -Path $extractDir -Filter "ffmpeg.exe" -Recurse | Select-Object -First 1
            if (-not $binFound) {
                throw "Extracted archive did not contain ffmpeg.exe"
            }
            Copy-FromBinDir $binFound.Directory.FullName
        } finally {
            Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

if (-not (Test-MediaBin $targetFfmpeg) -or -not (Test-MediaBin $targetFfprobe)) {
    throw "Staged FFmpeg or FFprobe binaries failed `-version` verification"
}

Write-Host "Successfully staged FFmpeg runtime to $targetDir"
