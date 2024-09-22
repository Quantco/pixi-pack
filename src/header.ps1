Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

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

if (-not $FORCE -and (Test-Path $PREFIX)) {
    Write-Error "ERROR: File or directory already exists: '$PREFIX'"
    Write-Error "If you want to update an existing environment, use the -f option."
    exit 1
}

if ($FORCE -and (Test-Path $PREFIX)) {
    Remove-Item -Recurse -Force $PREFIX
}

Write-Host "Unpacking payload ..."
$scriptContent = Get-Content -Raw -Path $MyInvocation.MyCommand.Path
$headerEnd = $scriptContent.IndexOf("__END_HEADER__")
$archiveEnd = $scriptContent.IndexOf("__END_ARCHIVE__", $headerEnd)

# Extract the base64-encoded archive data between __END_HEADER__ and __END_ARCHIVE__
$archiveContent = $scriptContent.Substring($headerEnd + "__END_HEADER__".Length, $archiveEnd - $headerEnd - "__END_HEADER__".Length)
[System.IO.File]::WriteAllBytes("$TEMPDIR\archive.tar", [System.Convert]::FromBase64String($archiveContent.Trim()))

Write-Host "Creating environment..."

# Extract the base64-encoded pixi-pack binary after __END_ARCHIVE__
$pixiPackContent = $scriptContent.Substring($archiveEnd + "__END_ARCHIVE__".Length)
[System.IO.File]::WriteAllBytes("$TEMPDIR\pixi-pack.exe", [System.Convert]::FromBase64String($pixiPackContent.Trim()))

if ($VERBOSE -and $QUIET) {
    Write-Error "ERROR: Verbose and quiet options cannot be used together."
    exit 1
}

$VERBOSITY_FLAG = ""
if ($VERBOSE) { $VERBOSITY_FLAG = "--verbose" }
if ($QUIET) { $VERBOSITY_FLAG = "--quiet" }

$OUTPUT_DIR_FLAG = ""
if ($PREFIX) { $OUTPUT_DIR_FLAG = "--output-directory $PREFIX" }

$SHELL_FLAG = ""
if ($UNPACK_SHELL) { $SHELL_FLAG = "--shell $UNPACK_SHELL" }

$CMD = "& `"$TEMPDIR\pixi-pack.exe`" unpack $OUTPUT_DIR_FLAG $VERBOSITY_FLAG $SHELL_FLAG `"$TEMPDIR\archive.tar`""

# Execute the command
Invoke-Expression $CMD

exit 0

function New-TemporaryDirectory {
    $parent = [System.IO.Path]::GetTempPath()
    [string] $name = [System.Guid]::NewGuid()
    New-Item -ItemType Directory -Path (Join-Path $parent $name)
}

__END_HEADER__