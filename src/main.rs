use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}


#[derive(Subcommand)]
enum Commands {
    /// Pack a pixi environment
    Pack {
        /// Environment to pack
        #[arg(short, long)]
        environment: String,

        /// Platform to pack
        #[arg(short, long)]
        platform: String,
    },
    /// Unpack a pixi environment
    Unpack {
        // TODO
    }
}

fn main() {
    let cli = Cli::parse();
    match &cli.command {
        Some(Commands::Pack { environment, platform }) => {
            println!("Pack environment: {}, platform: {}", environment, platform);
        }
        Some(Commands::Unpack {}) => {
            println!("Unpack environment");
        }
        None => {
            println!("No command specified");
        }
    }
}

fn pack(environment: String, platform: String) -> Result<()> {
    Ok(())
}
