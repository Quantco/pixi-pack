#!/bin/sh

set -eu

TEMPDIR=`mktemp -d`
PREFIX="env"
FORCE=0
INSTALLER="rattler"  # Default to rattler
CREATE_ACTIVATION_SCRIPT=false

# Pixi Constants ./lib.rs
PIXI_PACK_CHANNEL_DIRECTORY=""
PIXI_PACK_METADATA_PATH=""
PIXI_PACK_DEFAULT_VERSION=""

USAGE="
usage: $0 [options]

Unpacks an environment packed with pixi-pack

-f           no error if environment already exists
-h           print this help message and exit
-p ENV       environment prefix, defaults to $PREFIX
-i INSTALLER create the environment using the specified installer defaulting to $INSTALLER
-a           create an activation script to activate the environment
"

while getopts ":fhai:p:" x; do
    case "$x" in
        f)
            FORCE=1
            ;;
        p)
            PREFIX="$OPTARG"
            ;;
        i)
            INSTALLER="$OPTARG"
            ;;
        a)
            CREATE_ACTIVATION_SCRIPT=true
            ;;
        h)
            echo "$USAGE"
            exit 2
            ;;
    esac
done

if [ "$INSTALLER" != "rattler" ] && [ "$INSTALLER" != "conda" ] && [ "$INSTALLER" != "micromamba" ]; then
    echo "ERROR: Invalid installer: '$INSTALLER'" >&2
    exit 1
fi

if [ "$FORCE" = "0" ] && [ -e "$PREFIX" ]; then
    echo "ERROR: File or directory already exists: '$PREFIX'" >&2
    echo "If you want to update an existing environment, use the -f option." >&2
    exit 1
fi

if [ "$FORCE" = "1" ] && [ -e "$PREFIX" ]; then
    rm -rf "$PREFIX"
fi

if [ "$CREATE_ACTIVATION_SCRIPT" = true ] && [ "$INSTALLER" = "conda" ]; then
    echo "ERROR: Activation script creation is only supported with rattler or micromamba as the installer." >&2
    exit 1
fi

mkdir -p "$PREFIX"
PREFIX="$(realpath "$PREFIX")"
PARENT_DIR="$(dirname "$PREFIX")"

archive_begin=$(($(grep -anm 1 "^@@END_HEADER@@" "$0" | sed 's/:.*//') + 1))
archive_end=$(($(grep -anm 1 "^@@END_ARCHIVE@@" "$0" | sed 's/:.*//') - 1))

echo "Unpacking payload ..."
tail -n +$archive_begin "$0" | head -n $(($archive_end - $archive_begin + 1)) | tar -xvf - -C "$TEMPDIR"

echo "Creating environment using $INSTALLER"

if [ "$INSTALLER" = "rattler" ]; then
    (
        ls $TEMPDIR

        export PIXI_PACK_CHANNEL_DIRECTORY=$PIXI_PACK_CHANNEL_DIRECTORY
        export PIXI_PACK_METADATA_PATH=$PIXI_PACK_METADATA_PATH
        export PIXI_PACK_DEFAULT_VERSION=$PIXI_PACK_DEFAULT_VERSION

        rattler_start=$(($archive_end + 2))

        tail -n +$rattler_start "$0" > "$TEMPDIR/rattler"
        chmod +x "$TEMPDIR/rattler"

        "$TEMPDIR/rattler" "unpack" "$TEMPDIR" "$PREFIX"
        echo "Environment created at $PREFIX"

        if [ "$CREATE_ACTIVATION_SCRIPT" = true ]; then
            "$TEMPDIR/rattler" "create-script" "$PARENT_DIR" "$PREFIX"
            echo "Activation script created at $PARENT_DIR/activate.sh"
        fi
    )
elif [ "$INSTALLER" = "conda" ]; then
    cd $TEMPDIR
    conda env create -p $PREFIX --file environment.yml
    echo "Environment created at $PREFIX"
elif [ "$INSTALLER" = "micromamba" ]; then
    cd $TEMPDIR
    micromamba create -p $PREFIX --file environment.yml

    echo "Environment created at $PREFIX"

    if [ "$CREATE_ACTIVATION_SCRIPT" = true ]; then
        micromamba shell activate -p $PREFIX > $PARENTDIR/activate.sh
        echo "Activation script created at $PARENTDIR/activate.sh"
    fi
fi

cd $PARENT_DIR

exit 0
@@END_HEADER@@