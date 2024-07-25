#!/bin/sh

set -eu

TEMPDIR=`mktemp -d`
PREFIX="env"
FORCE=0
INSTALLER="conda"  # Default to conda
CREATE_ACTIVATION_SCRIPT=false
PARENT_DIR="$(dirname "$0")"

USAGE="
usage: $0 [options]

Unpacks an environment packed with pixi-pack

-f           no error if environment already exists
-h           print this help message and exit
-p ENV       environment prefix, defaults to $PREFIX
-i INSTALLER create the environment using the specified installer defaulting to $INSTALLER
-a           create an activation script to activate the environment
"

create_activation_script() {
    local destination="$1"
    local prefix="$2"
    local shell=$(basename "$3")

    case "$shell" in
        bash | zsh | fish)
            extension="sh"
            ;;
        *)
            echo "Unsupported shell: $shell" >&2
            return 1
            ;;
    esac

    activate_path="${destination}/activate.${extension}"

    activation_dir="${prefix}/etc/conda/activate.d"
    deactivation_dir="${prefix}/etc/conda/deactivate.d"
    env_vars_dir="${prefix}/etc/conda/env_vars.d"
    state_file="${prefix}/conda-meta/state"

    touch "$activate_path"
    echo "export PATH=\"$prefix/bin:\$PATH\"" >> "$activate_path"
    echo "export CONDA_PREFIX=\"$prefix\"" >> "$activate_path"

    # https://docs.rs/rattler_shell/latest/src/rattler_shell/activation.rs.html#335
    if [ -d "$activation_dir" ]; then
        for file in "${activation_dir}/*"; do
            echo ". \"$file\"" >> "$activate_path"
        done
    fi

    # https://docs.rs/rattler_shell/latest/src/rattler_shell/activation.rs.html#337
    if [ -d "$deactivation_dir" ]; then
        for file in "${deactivation_dir}/*"; do
            echo ". \"$file\"" >> "$activate_path"
        done
    fi

    # https://docs.rs/rattler_shell/latest/src/rattler_shell/activation.rs.html#191
    if [ -d "$env_vars_dir" ]; then
        env_var_files=$(find "$env_vars_dir" -type f | sort)

        for file in $env_var_files; do
            if jq empty "$file" 2>/dev/null; then
                jq -r 'to_entries | map("\(.key)=\(.value)") | .[]' "$file" | while IFS="=" read -r key value; do
                    # Remove quotes from the value
                    value=$(echo "$value" | sed 's/^"//; s/"$//')
                    echo "export $key=\"$value\"" >> "$activate_path"
                done
            else
                echo "WARNING: Invalid JSON file: $file" >&2
            fi
        done
    fi

    # https://docs.rs/rattler_shell/latest/src/rattler_shell/activation.rs.html#236
    if [ -e "$state_file" ]; then
        if ! state_json=$(jq '.' "$state_file" 2>/dev/null); then
            echo "WARNING: Invalid JSON in state file: $state_file" >&2
        else
            echo "$state_json" | jq -r '.env_vars // {} | to_entries | map("\(.key)=\(.value)") | .[]' | while IFS="=" read -r key value; do
                if [ -n "$key" ]; then
                    if grep -q "export $key=" "$activate_path"; then
                        echo "WARNING: environment variable $key already defined in packages (path: $state_file)" >&2
                    fi
                    if [ -n "$value" ]; then
                        echo "export ${key}=\"$value\"" >> "$activate_path"
                    else
                        echo "WARNING: environment variable $key has no string value (path: $state_file)" >&2
                    fi
                fi
            done
        fi
    fi

    chmod +x "$activate_path"
    echo "Activation script created at $activate_path"
}

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

if [ "$INSTALLER" != "conda" ] && [ "$INSTALLER" != "micromamba" ]; then
    echo "ERROR: Invalid installer: '$INSTALLER'" >&2
    exit 1
fi

if [ "$FORCE" = "0" ] && [ -e "$PREFIX" ]; then
    echo "ERROR: File or directory already exists: '$PREFIX'" >&2
    echo "If you want to update an existing environment, use the -f option." >&2
    exit 1
elif [ "$FORCE" = "1" ] && [ -e "$PREFIX" ]; then
    rm -rf "$PREFIX"
fi

PREFIX="$PARENT_DIR/$PREFIX"

last_line=$(($(grep -anm 1 '^@@END_HEADER@@' "$0" | sed 's/:.*//') + 1))

echo "Unpacking payload ..."
tail -n +$last_line "$0" | tar -xvf -C "$TEMPDIR"

echo "Creating environment using $INSTALLER"

cd $TEMPDIR

if [ "$INSTALLER" = "conda" ]; then
    conda env create -p $PREFIX --file environment.yml
else
    micromamba create -p $PREFIX --file environment.yml
fi

cd $PARENT_DIR

if [ "$CREATE_ACTIVATION_SCRIPT" = true ]; then
    create_activation_script "$PARENT_DIR" "$PREFIX" "$SHELL"
fi

exit 0
@@END_HEADER@@