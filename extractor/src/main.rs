use anyhow::{anyhow, Result};
// use rattler::install::Installer;
// use rattler::package_cache::PackageCache;
use std::env;
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        return Err(anyhow!("Usage: {} <input_dir> <output_dir>", args[0]));
    }

    let _archive_dir = Path::new(&args[1]);
    let _output_dir = Path::new(&args[2]);

    Ok(())
}
