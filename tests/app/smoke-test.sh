#!/usr/bin/env bash
# Create an application from examples/app and check that it starts the entry point.
# Usage: tests/app/smoke-test.sh <path to pixi-pack> <path to pixi-launcher> <path to pixi-unpack>
# With DOWNLOAD_PACKAGES=1, the packages are downloaded on first launch instead of included.
set -euo pipefail

pixi_pack="$1"
pixi_launcher="$2"
pixi_unpack="$3"
# Allow passing the same paths on Windows, where the executables end with `.exe`. Git Bash
# finds `foo.exe` for `-f foo`, but pixi-pack does not.
if [ -f "$pixi_launcher.exe" ]; then pixi_launcher="$pixi_launcher.exe"; fi
if [ -f "$pixi_unpack.exe" ]; then pixi_unpack="$pixi_unpack.exe"; fi
if [ "$(uname -s)" = Linux ]; then
  # The Flatpak sandbox has its own /tmp, but can write to the home directory.
  work="$(mktemp -d -p "$HOME")"
else
  work="$(mktemp -d)"
fi
marker="$work/marker.txt"
identifier="org.example.pixi-pack-smoke-test"

# Same app as the example, but the entry point writes a marker file instead of opening a window.
# `CONDA_PREFIX` is only set if the environment was activated.
sed -e "s/^identifier = .*/identifier = \"$identifier\"/" \
  -e 's/^args = .*/args = ["-c", "import os, sys, tkinter; open(sys.argv[1], \\"w\\").write(os.environ[\\"CONDA_PREFIX\\"])"]/' \
  examples/app/app.toml >examples/app/smoke-test.toml
if [ "${DOWNLOAD_PACKAGES:-}" = 1 ]; then
  { echo "download-packages = true"; cat examples/app/smoke-test.toml; } >"$work/smoke-test.toml"
  mv "$work/smoke-test.toml" examples/app/smoke-test.toml
fi
cleanup() {
  rm -f examples/app/smoke-test.toml
  if [ -n "${mount:-}" ]; then hdiutil detach "$mount" >/dev/null || true; fi
  if [ -n "${flatpak:-}" ]; then
    flatpak uninstall --user --noninteractive "$identifier" || true
    rm -rf "$HOME/.var/app/$identifier"
  fi
  if [ -n "${uninstaller:-}" ]; then MSYS2_ARG_CONV_EXCL='*' "$uninstaller" /S || true; fi
  if [ -n "${data_dir:-}" ]; then rm -rf "$data_dir"; fi
  if [ "$(uname -s)" = Linux ]; then rm -rf "$work"; fi
}
trap cleanup EXIT

"$pixi_pack" --create-app examples/app/smoke-test.toml --pixi-launcher-source "$pixi_launcher" \
  --pixi-unpack-source "$pixi_unpack" -o "$work/out" examples/app/pixi.toml
ls -l "$work/out"

# Dialogs would block forever in CI.
export PIXI_LAUNCHER_NO_UI=1

case "$(uname -s)" in
Darwin)
  data_dir="$HOME/Library/Application Support/$identifier"
  mount="$work/mnt"
  mkdir "$mount"
  hdiutil attach -quiet -nobrowse -readonly -mountpoint "$mount" "$work"/out/*.dmg
  run() { "$mount/IDLE Demo.app/Contents/MacOS/IDLE Demo" "$marker"; }
  ;;
Linux)
  if [ "${DOWNLOAD_PACKAGES:-}" = 1 ]; then
    # The launcher unpacks the environment into the Flatpak's data directory.
    data_dir="$HOME/.var/app/$identifier/data/$identifier"
  else
    # The environment is unpacked while building the Flatpak, there is no launcher.
    data_dir=""
  fi
  flatpak=("$work"/out/*.flatpak)
  flatpak install --user --noninteractive "${flatpak[0]}"
  run() { flatpak run "$identifier" "$marker"; }
  ;;
MINGW* | MSYS* | CYGWIN*)
  data_dir="$LOCALAPPDATA/$identifier"
  # NSIS installs silently with /S, for the current user into %LOCALAPPDATA%\<product name>.
  installer=("$work"/out/*-setup.exe)
  # Git Bash would turn `/S` into a path.
  MSYS2_ARG_CONV_EXCL='*' "${installer[0]}" /S
  run() { "$LOCALAPPDATA/IDLE Demo/IDLE Demo.exe" "$marker"; }
  uninstaller="$LOCALAPPDATA/IDLE Demo/uninstall.exe"
  ;;
esac

check() {
  # On Windows the launcher returns right after starting the application.
  for _ in $(seq 60); do
    [ -s "$marker" ] && break
    sleep 1
  done
  if [ ! -s "$marker" ]; then
    echo "entry point did not run, launcher log:"
    if [ -n "$data_dir" ]; then cat "$data_dir/launcher.log" || true; fi
    exit 1
  fi
  echo "entry point ran in the activated environment $(cat "$marker")"
  rm "$marker"
}

# First launch unpacks, second launch reuses the environment.
run
check
run
check
