use anyhow::{Context, Result};
use clap::{ArgGroup, Parser, Subcommand};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

mod godo;
use godo::{Godo, RunOptions};

const DEFAULT_GODO_DIR: &str = "~/.godo";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(group(
    ArgGroup::new("color_mode")
        .args(["color", "no_color"])
))]
struct Cli {
    /// Override the godo directory location
    #[arg(long, global = true, value_name = "DIR")]
    dir: Option<String>,

    /// Override the repository directory (defaults to current git project)
    #[arg(long, global = true, value_name = "DIR")]
    repo_dir: Option<String>,

    /// Enable colored output
    #[arg(long, global = true)]
    color: bool,

    /// Disable colored output
    #[arg(long = "no-color", global = true)]
    no_color: bool,

    /// Suppress all output
    #[arg(long, global = true)]
    quiet: bool,

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

fn find_git_root(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir;
    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return None,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Determine godo directory (priority: CLI flag > env var > default)
    let godo_dir = if let Some(dir) = &cli.dir {
        expand_tilde(dir)
    } else if let Ok(env_dir) = std::env::var("GODO_DIR") {
        expand_tilde(&env_dir)
    } else {
        expand_tilde(DEFAULT_GODO_DIR)
    };

    // Determine repository directory
    let repo_dir = if let Some(repo) = &cli.repo_dir {
        expand_tilde(repo)
    } else {
        // Find git root from current directory
        let current_dir = std::env::current_dir().context("Failed to get current directory")?;

        find_git_root(&current_dir).context("Not in a git repository")?
    };

    // Determine color output preference
    let color = if cli.color {
        true
    } else if cli.no_color {
        false
    } else {
        // Auto-detect based on terminal
        std::io::stdout().is_terminal()
    };

    // Create Godo instance
    let godo =
        Godo::new(godo_dir, repo_dir, color, cli.quiet).context("Failed to initialize godo")?;

    match cli.command {
        Commands::Run {
            persist,
            copy,
            name,
            command,
        } => {
            let options = RunOptions {
                persist,
                copy,
                name,
                command,
            };

            godo.run(&options)?;
        }
        Commands::List => {
            godo.list()?;
        }
        Commands::Rm { name } => {
            godo.remove(&name)?;
        }
        Commands::Prune => {
            godo.prune()?;
        }
    }

    Ok(())
}
