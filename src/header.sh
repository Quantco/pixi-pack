#!/bin/sh

set -euo pipefail
TEMPDIR="$(mktemp -d)"
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
# Parse command-line arguments
while [[ $# -gt 0 ]]; do
  case "$1" in
    -h)
      echo "$USAGE"
      exit 0
      ;;
    -f)
      FORCE=1
      shift
      ;;
    -v)
      VERBOSE=1
      shift
      ;;
    -o)
      if [[ -n "$2" && "$2" != -* ]]; then
        PREFIX="$2"
        shift 2
      else
        echo "Option -o requires an argument" >&2
        echo "$USAGE" >&2
        exit 1
      fi
      ;;
    -s)
      if [[ -n "$2" && "$2" != -* ]]; then
        UNPACK_SHELL="$2"
        shift 2
      else
        echo "Option -s requires an argument" >&2
        echo "$USAGE" >&2
        exit 1
      fi
      ;;
    -q)
      QUIET=1
      shift
      ;;
    -*)
      echo "Invalid option: $1" >&2
      echo "$USAGE" >&2
      exit 1
      ;;
    *)
      # Stop parsing options when encountering a non-option argument
      break
      ;;
  esac
done

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

archive_begin=$(($(grep -anm 1 "^@@END_HEADER@@" "$0" | sed 's/:.*//') + 2))
archive_end=$(($(grep -anm 1 "^@@END_ARCHIVE@@" "$0" | sed 's/:.*//') - 1))

echo "Unpacking payload ..."
echo $(tail -n +$archive_begin "$0" | head -n $(($archive_end - $archive_begin + 1))) > "$TEMPDIR/archive_temp"
base64 -d "$TEMPDIR/archive_temp" > "$TEMPDIR/archive.tar"

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
