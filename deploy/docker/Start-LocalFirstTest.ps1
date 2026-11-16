param(
    [ValidatePattern('^zibblu-issue46(-[a-z0-9]{1,12})?$')]
    [string]$ProjectName = 'zibblu-issue46',
    [ValidateRange(1024, 65535)]
    [int]$HttpPort = 17841,
    [ValidateRange(1024, 65535)]
    [int]$DbPort = 17846,
    [switch]$ShortTtl
)

$ErrorActionPreference = 'Stop'
$database = $ProjectName.Replace('-', '_')
$settings = @{
    POSTGRES_USER = 'zibblu'
    POSTGRES_DB = $database
    POSTGRES_PASSWORD = "${database}_fixture_only"
    ZIBBLU_HTTP_PORT = "$HttpPort"
    ZIBBLU_TEST_DB_PORT = "$DbPort"
    ZIBBLU_TEXT_TTL_SECONDS = '259200'
    ZIBBLU_FILE_TTL_SECONDS = '600'
    ZIBBLU_UPLOAD_TTL_SECONDS = '120'
    ZIBBLU_DEVICE_IDLE_SECONDS = '3600'
    ZIBBLU_DEVICE_LIMIT = '5'
    ZIBBLU_CLEANUP_INTERVAL_SECONDS = '1'
}
if ($ShortTtl) {
    $settings.ZIBBLU_TEXT_TTL_SECONDS = '6'
    $settings.ZIBBLU_FILE_TTL_SECONDS = '15'
    $settings.ZIBBLU_UPLOAD_TTL_SECONDS = '8'
    $settings.ZIBBLU_DEVICE_IDLE_SECONDS = '60'
}
$previous = @{}
$stack = @('--context', 'desktop-linux', 'compose', '-p', $ProjectName,
    '-f', "$PSScriptRoot/compose.yaml", '-f', "$PSScriptRoot/compose.local-first-test.yaml")
try {
    foreach ($key in $settings.Keys) {
        $previous[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
        [Environment]::SetEnvironmentVariable($key, $settings[$key], 'Process')
    }
    docker @stack config --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Invalid isolated Compose configuration.' }
    docker @stack up -d --build --wait --wait-timeout 120
    if ($LASTEXITCODE -ne 0) { throw 'The isolated Zibblu stack did not start.' }
    Write-Output "http://127.0.0.1:$HttpPort / PostgreSQL $database / Compose $ProjectName"
} finally {
    foreach ($key in $previous.Keys) {
        [Environment]::SetEnvironmentVariable($key, $previous[$key], 'Process')
    }
}
