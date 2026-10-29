# macOS application

Packs a Python environment as a macOS application that starts IDLE, Python's built-in Tk editor.

```bash
cargo build --release --features app
./target/release/pixi-pack --create-app examples/macos-app/app.toml examples/macos-app/pixi.toml
open "IDLE Demo.app"
```

The first launch unpacks the environment into `~/Library/Application Support/org.example.idle-demo`.
