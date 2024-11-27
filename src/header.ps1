function New-TemporaryDirectory {
    $parent = [System.IO.Path]::GetTempPath()
    [string] $name = [System.Guid]::NewGuid()
    $tempDir = New-Item -ItemType Directory -Path (Join-Path $parent $name)
    return $tempDir.FullName
}

$TEMPDIR = New-TemporaryDirectory
$USAGE = @"
Usage: $($MyInvocation.MyCommand.Name) [OPTIONS]

Arguments:
    Path to an environment packed using pixi-pack

Options:
    -o, --output-directory <DIR>    Where to unpack the environment. The environment will be unpacked into a subdirectory of this path [default: env]
    -e, --env-name <NAME>           Name of the environment [default: env]
    -s, --shell <SHELL>             Sets the shell [options: bash, zsh, xonsh, cmd, powershell, fish, nushell]
    -v, --verbose                   Increase logging verbosity
    -q, --quiet                     Decrease logging verbosity
    -h, --help                      Print help
"@

foreach ($arg in $args) {
    if ($arg -eq "-h" -or $arg -eq "--help") {
        Write-Output $USAGE
        exit 0
    }
}

# Extract the archive and pixi-pack executable, and decode them
$scriptContent = Get-Content -Raw -Path $MyInvocation.MyCommand.Path
$lines = $scriptContent -split "`r?`n"

$headerLine = $null
$archiveLine = $null

# Find the lines where __END_HEADER__ and __END_ARCHIVE__ occur
for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -like "*__END_HEADER__*") {
        $headerLine = $i + 2
    }
    if ($lines[$i] -like "*__END_ARCHIVE__*") {
        $archiveLine = $i + 1
    }
}

if (-not $headerLine -or -not $archiveLine) {
    Write-Error "ERROR: Markers __END_HEADER__ or __END_ARCHIVE__ not found."
    exit 1
}

# Extract Base64 content for the tar archive
$archiveContent = $lines[($headerLine)..($archiveLine - 2)] -join ""
$archiveContent = $archiveContent.Trim()

# Decode Base64 content into tar file
try {
    $decodedArchive = [System.Convert]::FromBase64String($archiveContent)
    $archivePath = "$TEMPDIR\archive.tar"
    [System.IO.File]::WriteAllBytes($archivePath, $decodedArchive)
} catch {
    Write-Error "Failed to decode Base64 archive content: $_"
    exit 1
}

# Extract Base64 content for pixi-pack executable
$pixiPackContent = $lines[($archiveLine)..($lines.Count - 1)] -join ""
$pixiPackContent = $pixiPackContent.Trim()

# Decode Base64 content into the pixi-pack executable file
try {
    $decodedPixiPack = [System.Convert]::FromBase64String($pixiPackContent)
    $pixiPackPath = "$TEMPDIR\pixi-pack.exe"
    [System.IO.File]::WriteAllBytes($pixiPackPath, $decodedPixiPack)
} catch {
    Write-Error "Failed to decode Base64 pixi-pack content: $_"
    exit 1
}

# Build the command with flags
$arguments = @("unpack")
$arguments += $args | Join-String -Separator ' '

# Add the path to the archive
$arguments += $archivePath

& $pixiPackPath @arguments

Remove-Item -Path $TEMPDIR -Recurse -Force

exit 0

__END_HEADER__
