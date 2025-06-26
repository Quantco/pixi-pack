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
pixi_pack_start=$((archive_end + 2))

sed -n "$archive_begin,${archive_end}p" "$0" | base64 -d > "$TEMPDIR/archive.tar"
sed -n "$pixi_pack_start,\$p" "$0" | base64 -d > "$TEMPDIR/pixi-unpack"

chmod +x "$TEMPDIR/pixi-unpack"

"$TEMPDIR/pixi-unpack" "$@" "$TEMPDIR/archive.tar"

rm -rf "$TEMPDIR"

exit 0
# shellcheck disable=SC2317
@@END_HEADER@@
