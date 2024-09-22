#!/bin/sh

set -eu
TEMPDIR=$(mktemp -d)
PREFIX=""
FORCE=0
VERBOSE=0
QUIET=0
UNPACK_SHELL=""

USAGE="
usage: $0 [options]

Unpacks an environment packed using pixi-pack

-f, --force                 No error if environment already exists
-h, --help                  Print this help message and exit
-o, --output-directory <DIR> Where to unpack the environment
-s, --shell <SHELL>         Sets the shell [options: bash, zsh, xonsh, cmd, powershell, fish, nushell]
-v, --verbose               Increase logging verbosity
-q, --quiet                 Decrease logging verbosity
"
# Parse command-line options
while getopts ":hfvo:s:q" opt; do
  case ${opt} in
    h )
      echo "$USAGE"
      exit 0
      ;;
    f )
      FORCE=1
      ;;
    v )
      VERBOSE=1
      ;;
    o )
      PREFIX="$OPTARG"
      ;;
    s )
      UNPACK_SHELL="$OPTARG"
      ;;
    q )
      QUIET=1
      ;;
    \? )
      echo "Invalid option: -$OPTARG" >&2
      echo "$USAGE" >&2
      exit 1
      ;;
    : )
      echo "Option -$OPTARG requires an argument" >&2
      echo "$USAGE" >&2
      exit 1
      ;;
  esac
done
shift $((OPTIND -1))

# Validate shell option if provided
if [ -n "$UNPACK_SHELL" ]; then
  case "$UNPACK_SHELL" in
    bash|zsh|xonsh|cmd|powershell|fish|nushell)
      ;;
    *)
      echo "Invalid shell option: $UNPACK_SHELL" >&2
      echo "Valid options are: bash, zsh, xonsh, cmd, powershell, fish, nushell" >&2
      exit 1
      ;;
  esac
fi

if [ "$FORCE" = "0" ] && [ -n "$PREFIX" ] && [ -e "$PREFIX" ]; then
    echo "ERROR: File or directory already exists: '$PREFIX'" >&2
    echo "If you want to update an existing environment, use the -f option." >&2
    exit 1
fi

if [ "$FORCE" = "1" ] && [ -n "$PREFIX" ] && [ -e "$PREFIX" ]; then
    rm -rf "$PREFIX"
fi

archive_begin=$(($(grep -anm 1 "^@@END_HEADER@@" "$0" | sed 's/:.*//') + 1))
archive_end=$(($(grep -anm 1 "^@@END_ARCHIVE@@" "$0" | sed 's/:.*//') - 1))

echo "Unpacking payload ..."
tail -n +$archive_begin "$0" | head -n $(($archive_end - $archive_begin + 1)) | base64 -d > "$TEMPDIR/archive.tar"

pixi_pack_start=$(($archive_end + 2))

tail -n +$pixi_pack_start "$0" | base64 -d > "$TEMPDIR/pixi-pack"
chmod +x "$TEMPDIR/pixi-pack"
if [ "$VERBOSE" = "1" ] && [ "$QUIET" = "1" ]; then
    printf "ERROR: Verbose and quiet options cannot be used together.\n" >&2
    exit 1
fi

VERBOSITY_FLAG=""
[ "$VERBOSE" = "1" ] && VERBOSITY_FLAG="--verbose"
[ "$QUIET" = "1" ] && VERBOSITY_FLAG="--quiet"

OUTPUT_DIR_FLAG=""
[ -n "$PREFIX" ] && OUTPUT_DIR_FLAG="--output-directory $PREFIX"

SHELL_FLAG=""
[ -n "$UNPACK_SHELL" ] && SHELL_FLAG="--shell $UNPACK_SHELL"

CMD="\"$TEMPDIR/pixi-pack\" unpack $OUTPUT_DIR_FLAG $VERBOSITY_FLAG"
if [ -n "$UNPACK_SHELL" ]; then
    CMD="$CMD --shell $UNPACK_SHELL"
fi
CMD="$CMD \"$TEMPDIR/archive.tar\""

# Execute the command
eval "$CMD"

exit 0
@@END_HEADER@@