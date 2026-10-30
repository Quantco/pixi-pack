# Application

Packs a Python environment as an application that starts IDLE, Python's built-in Tk editor.
The same `app.toml` creates a `.dmg` on macOS, an installer on Windows and a Flatpak on Linux.

```bash
cargo build --release --features app
./target/release/pixi-pack --create-app examples/app/app.toml \
    --pixi-launcher-source target/release/pixi-launcher \
    --pixi-unpack-source target/release/pixi-unpack examples/app/pixi.toml
```

Building the Flatpak requires `flatpak` and `flatpak-builder`, e.g. `sudo apt install flatpak flatpak-builder`.
Install and start it with:

```bash
flatpak install --user idle-demo_1.0.0_x86_64.flatpak
flatpak run org.example.idle-demo
```

On macOS and Windows, the first launch unpacks the environment into the user's data directory, e.g. `~/Library/Application Support/org.example.idle-demo` on macOS.
The Flatpak contains the unpacked environment.
