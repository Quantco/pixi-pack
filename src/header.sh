#!/bin/sh

set -eu

INSTALLER_NAME="__NAME__"
INSTALLER_VERSION="__VERSION__"
INSTALLER_PLATFORM="__PLAT__"
PREFIX="__DEFAULT_PREFIX__"
BATCH=0
FORCE=0

USAGE="
usage: $0 [options]

Unpacks ${INSTALLER_NAME} ${INSTALLER_VERSION}

-b           run in batch mode (without manual intervention)
-f           no error if install prefix already exists
-h           print this help message and exit
-p PREFIX    install prefix, defaults to $PREFIX
"

while getopts "bfhp:" x; do
    case "$x" in
        h)
            echo "$USAGE"
            exit 2
            ;;
        b)
            BATCH=1
            ;;
        f)
            FORCE=1
            ;;
        p)
            PREFIX="$OPTARG"
            ;;
        ?)
            echo "ERROR: did not recognize option '$x', please try -h" >&2
            exit 1
            ;;
    esac
done

if [ "$FORCE" = "0" ] && [ -e "$PREFIX" ]; then
    echo "ERROR: File or directory already exists: '$PREFIX'" >&2
    echo "If you want to update an existing installation, use the -f option." >&2
    exit 1
fi

if ! mkdir -p "$PREFIX"; then
    echo "ERROR: Could not create directory: '$PREFIX'" >&2
    exit 1
fi

PREFIX=$(cd "$PREFIX"; pwd | sed 's@//@/@')
export PREFIX

echo "PREFIX=$PREFIX"

extract_range () {
    dd if="$0" bs=1 skip="$1" count="$((${2}-${1}))" 2>/dev/null
}

last_line=$(grep -anm 1 '^@@END_HEADER@@' "$0" | sed 's/:.*//')
boundary=$(head -n "${last_line}" "$0" | wc -c | sed 's/ //g')

cd "$PREFIX"

echo "Unpacking payload ..."
extract_range $boundary | tar -xzf -

echo "Installation completed."

if [ "$BATCH" = "0" ]; then
    echo "Thank you for installing ${INSTALLER_NAME}!"
fi

exit 0
@@END_HEADER@@