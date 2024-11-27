#!/usr/bin/env bash

set -euo pipefail
TEMPDIR="$(mktemp -d)"
USAGE="
Usage: $0 [OPTIONS]

Arguments:
  Path to an environment packed using pixi-pack

Options:
  -o, --output-directory <DIR>    Where to unpack the environment. The environment will be unpacked into a subdirectory of this path [default: env]
  -e, --env-name <NAME>           Name of the environment [default: env]
  -s, --shell <SHELL>             Sets the shell [options: bash, zsh, xonsh, cmd, powershell, fish, nushell]
  -v, --verbose                   Increase logging verbosity
  -q, --quiet                     Decrease logging verbosity
  -h, --help                      Print help
"

# Check for help flag
for arg in "$@"; do
  if [ "$arg" = "-h" ] || [ "$arg" = "--help" ]; then
    echo "$USAGE"
    exit 0
  fi
done

archive_begin=$(grep -anm 1 "^@@END_HEADER@@" "$0" | awk -F: '{print $1}')
archive_end=$(grep -anm 1 "^@@END_ARCHIVE@@" "$0" | awk -F: '{print $1}')

if [ -z "$archive_begin" ] || [ -z "$archive_end" ]; then
  echo "ERROR: Markers @@END_HEADER@@ or @@END_ARCHIVE@@ not found." >&2
  exit 1
fi

archive_begin=$((archive_begin + 2))
archive_end=$((archive_end - 1))
pixi_pack_start=$(($archive_end + 2))

echo "Unpacking payload ..."
echo $(tail -n +$archive_begin "$0" | head -n $(($archive_end - $archive_begin + 1))) > "$TEMPDIR/archive_temp"
echo $(tail -n +$pixi_pack_start "$0") > "$TEMPDIR/pixi-pack_temp"

if [[ $(base64 --version | grep -q 'GNU') ]]; then
  # BSD/macOS version
  base64 -d -i "$TEMPDIR/archive_temp" > "$TEMPDIR/archive.tar"
  base64 -d -i "$TEMPDIR/pixi-pack_temp" > "$TEMPDIR/pixi-pack"
else
  # GNU version
  base64 -d "$TEMPDIR/archive_temp" > "$TEMPDIR/archive.tar"
  base64 -d "$TEMPDIR/pixi-pack_temp" > "$TEMPDIR/pixi-pack"
fi

chmod +x "$TEMPDIR/pixi-pack"

CMD="\"$TEMPDIR/pixi-pack\" unpack $@ \"$TEMPDIR/archive.tar\""

# Execute the command
eval "$CMD"

exit 0
@@END_HEADER@@
