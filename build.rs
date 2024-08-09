use std::process::Command;

fn main() {
    let extractor_path = "./extractor";

    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--manifest-path",
            &format!("{}/Cargo.toml", extractor_path),
        ])
        .status()
        .expect("Failed to build extractor");

    if !status.success() {
        panic!("Failed to compile the extractor project.");
    }
}
