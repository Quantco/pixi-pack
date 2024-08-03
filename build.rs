use std::process::Command;

fn main() {
    // Compile the extractor
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--manifest-path",
            "extractor/Cargo.toml",
        ])
        .status()
        .expect("Failed to compile extractor");

    if !status.success() {
        panic!("Failed to compile extractor");
    }
}
