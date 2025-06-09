use anyhow::{Context, Result};
use clap::{ArgGroup, Parser, Subcommand};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

mod godo;
use godo::Godo;

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

    /// Skip confirmation prompts
    #[arg(long, global = true)]
    no_prompt: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a command in an isolated workspace
    Run {
        /// Keep the sandbox after the command exits
        #[arg(long)]
        keep: bool,

        /// Exclude directories that match glob (can be specified multiple times)
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,

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

        /// Force removal even if there are uncommitted changes
        #[arg(long)]
        force: bool,
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

fn main() -> Result<()> {
    // Handle errors with custom formatting
    if let Err(e) = run() {
        // Reset any existing colors
        print!("\x1b[0m");
        let _ = std::io::stdout().flush();

        // Print error in orange to stderr
        let mut stderr = StandardStream::stderr(ColorChoice::Auto);
        let _ = stderr.set_color(ColorSpec::new().set_fg(Some(Color::Rgb(255, 165, 0))));
        eprintln!("Error: {e:#}");
        let _ = stderr.reset();

        std::process::exit(1);
    }
    Ok(())
}

fn run() -> Result<()> {
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
    let repo_dir = cli.repo_dir.as_ref().map(|repo| expand_tilde(repo));

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
    let godo = Godo::new(godo_dir, repo_dir, color, cli.quiet, cli.no_prompt)
        .context("Failed to initialize godo")?;

    match cli.command {
        Commands::Run {
            keep,
            excludes,
            name,
            command,
        } => {
            godo.run(keep, &excludes, &name, &command)?;
        }
        Commands::List => {
            godo.list()?;
        }
        Commands::Rm { name, force } => {
            godo.remove(&name, force)?;
        }
        Commands::Prune => {
            godo.prune()?;
        }
    }

    Ok(())
}
