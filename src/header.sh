#!/bin/sh

set -eu

TEMPDIR=`mktemp -d`
PREFIX="env"
FORCE=0
INSTALLER="conda"  # Default to conda
CREATE_ACTIVATION_SCRIPT=false
PARENT_DIR="$(pwd)"

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
        bash)
            extension="sh"
            ;;
        zsh)
            extension="zsh"
            ;;
        fish)
            extension="fish"
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
        declare -A env_vars

        env_var_files=($(find "$env_vars_dir" -type f | sort))

        for file in "${env_var_files[@]}"; do
            if jq empty "$file" 2>/dev/null; then
                while IFS="=" read -r key value; do
                    # Remove quotes from the value
                    value="${value%\"}"
                    value="${value#\"}"
                    env_vars["$key"]="$value"
                done < <(jq -r 'to_entries | map("\(.key)=\(.value)") | .[]' "$file")
            else
                echo "WARNING: Invalid JSON file: $file" >&2
            fi
        done

        for key in "${!env_vars[@]}"; do
            echo "export $key=\"${env_vars[$key]}\"" >> "$activate_path"
        done
    fi

    # https://docs.rs/rattler_shell/latest/src/rattler_shell/activation.rs.html#236
    if [ -e "$state_file" ]; then
        if ! state_json=$(jq '.' "$state_file" 2>/dev/null); then
            echo "WARNING: Invalid JSON in state file: $state_file" >&2
        else
            state_env_vars=$(echo "$state_json" | jq -r '.env_vars // {}')
            while IFS="=" read -r key value; do
                if [ -n "$key" ]; then
                    if [ -n "${env_vars[$key]}" ]; then
                        echo "WARNING: environment variable $key already defined in packages (path: $state_file)" >&2
                    fi
                    if [ -n "$value" ]; then
                        env_vars["${key^^}"]="$value"
                    else
                        echo "WARNING: environment variable $key has no string value (path: $state_file)" >&2
                    fi
                fi
            done < <(echo "$state_env_vars" | jq -r 'to_entries | map("\(.key)=\(.value)") | .[]')
        fi
        echo "export CONDA_ENV_STATE_FILE=\"$state_file\"" >> "$activate_path"
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

PREFIX="$(pwd)/$PREFIX"

last_line=$(($(grep -anm 1 '^@@END_HEADER@@' "$0" | sed 's/:.*//') + 1))

echo "Unpacking payload ..."
tail -n +$last_line "$0" | tar xzv -C "$TEMPDIR"

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
