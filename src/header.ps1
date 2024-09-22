function New-TemporaryDirectory {
    $parent = [System.IO.Path]::GetTempPath()
    [string] $name = [System.Guid]::NewGuid()
    $tempDir = New-Item -ItemType Directory -Path (Join-Path $parent $name)
    return $tempDir.FullName
}

$TEMPDIR = New-TemporaryDirectory
$PREFIX = ""
$FORCE = $false
$VERBOSE = $false
$QUIET = $false
$UNPACK_SHELL = ""

$USAGE = @"
usage: $($MyInvocation.MyCommand.Name) [options]

Unpacks an environment packed using pixi-pack

-f, --force                 No error if environment already exists
-h, --help                  Print this help message and exit
-o, --output-directory <DIR> Where to unpack the environment
-s, --shell <SHELL>         Sets the shell [options: bash, zsh, xonsh, cmd, powershell, fish, nushell]
-v, --verbose               Increase logging verbosity
-q, --quiet                 Decrease logging verbosity
"@

# Parse command-line arguments
$args = $MyInvocation.UnboundArguments
for ($i = 0; $i -lt $args.Count; $i++) {
    switch ($args[$i]) {
        "-f" { $FORCE = $true }
        "--force" { $FORCE = $true }
        "-o" { $PREFIX = $args[++$i] }
        "--output-directory" { $PREFIX = $args[++$i] }
        "-s" { $UNPACK_SHELL = $args[++$i] }
        "--shell" { $UNPACK_SHELL = $args[++$i] }
        "-v" { $VERBOSE = $true }
        "--verbose" { $VERBOSE = $true }
        "-q" { $QUIET = $true }
        "--quiet" { $QUIET = $true }
        "-h" { Write-Output $USAGE; exit 2 }
        "--help" { Write-Output $USAGE; exit 2 }
    }
}

# Check if verbose and quiet are both set
if ($VERBOSE -and $QUIET) {
    Write-Error "ERROR: Verbose and quiet options cannot be used together."
    exit 1
}

# Step 1: Extract the archive and pixi-pack executable, and decode them
$scriptContent = Get-Content -Raw -Path $MyInvocation.MyCommand.Path
$lines = $scriptContent -split "`r?`n"

$headerLine = $null
$archiveLine = $null

# Find the lines where __END_HEADER__ and __END_ARCHIVE__ occur
for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -like "*__END_HEADER__*") {
        $headerLine = $i + 1
    }
    if ($lines[$i] -like "*__END_ARCHIVE__*") {
        $archiveLine = $i + 1
    }
}

if (-not $headerLine -or -not $archiveLine) {
    Write-Error "Markers __END_HEADER__ or __END_ARCHIVE__ not found."
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

# Step 2: Build the command with flags
$arguments = @("unpack")

# Use $PREFIX for output directory if it is provided
if ($PREFIX) {
    $arguments += "--output-directory"
    $arguments += $PREFIX
}

# Handle verbosity/quiet flags
if ($VERBOSE) {
    $arguments += "--verbose"
} elseif ($QUIET) {
    $arguments += "--quiet"
}

# Add shell flag if provided
if ($UNPACK_SHELL) {
    $arguments += "--shell"
    $arguments += $UNPACK_SHELL
}

# Finally, add the path to the archive
$arguments += $archivePath

& $pixiPackPath @arguments

exit 0

__END_HEADER__