# Application

Packs a Python environment as an application that starts IDLE, Python's built-in Tk editor.
The same `app.toml` creates a `.dmg` on macOS, an installer on Windows and an AppImage and Debian package on Linux.

```bash
cargo build --release --features app
./target/release/pixi-pack --create-app examples/app/app.toml \
    --pixi-launcher-source target/release/pixi-launcher examples/app/pixi.toml
```

The first launch unpacks the environment into the user's data directory, e.g. `~/Library/Application Support/org.example.idle-demo` on macOS.
