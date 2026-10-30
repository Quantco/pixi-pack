#!/usr/bin/env bash
# Create an application from examples/app and check that it starts the entry point.
# Usage: tests/app/smoke-test.sh <path to pixi-pack> <path to pixi-launcher>
set -euo pipefail

pixi_pack="$1"
pixi_launcher="$2"
# Allow passing the same paths on Windows, where the executables end with `.exe`. Git Bash
# finds `foo.exe` for `-f foo`, but pixi-pack does not.
if [ -f "$pixi_launcher.exe" ]; then pixi_launcher="$pixi_launcher.exe"; fi
work="$(mktemp -d)"
marker="$work/marker.txt"
identifier="org.example.pixi-pack-smoke-test"

# Same app as the example, but the entry point writes a marker file instead of opening a window.
sed -e "s/^identifier = .*/identifier = \"$identifier\"/" \
  -e 's/^args = .*/args = ["-c", "import sys, tkinter; open(sys.argv[1], \\"w\\").write(sys.prefix)"]/' \
  examples/app/app.toml >examples/app/smoke-test.toml
cleanup() {
  rm -f examples/app/smoke-test.toml
  if [ -n "${mount:-}" ]; then hdiutil detach "$mount" >/dev/null || true; fi
}
trap cleanup EXIT

"$pixi_pack" --create-app examples/app/smoke-test.toml --pixi-launcher-source "$pixi_launcher" \
  -o "$work/out" examples/app/pixi.toml
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
  data_dir="${XDG_DATA_HOME:-$HOME/.local/share}/$identifier"
  # Test the AppImage first, then the Debian package, extracted instead of installed.
  appimage=("$work"/out/*.AppImage)
  chmod +x "${appimage[0]}"
  APPIMAGE_EXTRACT_AND_RUN=1 "${appimage[0]}" "$marker"
  deb=("$work"/out/*.deb)
  dpkg-deb -x "${deb[0]}" "$work/deb"
  run() { "$work/deb/usr/bin/idle-demo" "$marker"; }
  ;;
MINGW* | MSYS* | CYGWIN*)
  data_dir="$LOCALAPPDATA/$identifier"
  # NSIS installs silently with /S, for the current user into %LOCALAPPDATA%\<product name>.
  installer=("$work"/out/*-setup.exe)
  # Git Bash would turn `/S` into a path.
  MSYS2_ARG_CONV_EXCL='*' "${installer[0]}" /S
  run() { "$LOCALAPPDATA/IDLE Demo/IDLE Demo.exe" "$marker"; }
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
    cat "$data_dir/launcher.log" || true
    exit 1
  fi
  echo "entry point ran in $(cat "$marker")"
  rm "$marker"
}

if [ -n "${appimage:-}" ]; then
  check
fi
# First launch unpacks, second launch reuses the environment.
run
check
run
check
