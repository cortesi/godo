use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_GODO_DIR: &str = "~/.godo";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Override the godo directory location
    #[arg(long, global = true, value_name = "DIR")]
    dir: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a command in an isolated workspace
    Run {
        /// Keep the sandbox after the command exits
        #[arg(long)]
        persist: bool,

        /// Copy only directories that match glob (can be specified multiple times)
        #[arg(long, value_name = "GLOB")]
        copy: Vec<String>,

        /// Name of the sandbox
        name: String,

        /// Command to execute (if omitted, opens interactive shell)
        command: Vec<String>,
    },

    /// Show existing sandboxes
    List,

    /// Delete a named sandbox
    Rm {
        /// Name of the sandbox to remove
        name: String,
    },

    /// Remove sandboxes whose branch no longer exists
    Prune,
}

fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(path.replacen("~", &home, 1));
        }
    }
    PathBuf::from(path)
}

fn ensure_godo_directory(godo_dir: &Path) -> std::io::Result<()> {
    // Create main godo directory
    fs::create_dir_all(godo_dir)?;

    // Create sandboxes subdirectory
    let sandboxes_dir = godo_dir.join("sandboxes");
    fs::create_dir_all(sandboxes_dir)?;

    Ok(())
}

fn main() {
    let cli = Cli::parse();

    // Determine godo directory (priority: CLI flag > env var > default)
    let godo_dir = if let Some(dir) = &cli.dir {
        expand_tilde(dir)
    } else if let Ok(env_dir) = std::env::var("GODO_DIR") {
        expand_tilde(&env_dir)
    } else {
        expand_tilde(DEFAULT_GODO_DIR)
    };

    // Ensure godo directory exists
    if let Err(e) = ensure_godo_directory(&godo_dir) {
        eprintln!("Failed to create godo directory: {e}");
        std::process::exit(1);
    }

    match cli.command {
        Commands::Run {
            persist,
            copy,
            name,
            command,
        } => {
            println!("run");
        }
        Commands::List => {
            println!("list");
        }
        Commands::Rm { name } => {
            println!("rm");
        }
        Commands::Prune => {
            println!("prune");
        }
    }
}
