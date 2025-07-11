use crate::output::Output;
use anyhow::{Context, Result};
use clap::{ArgGroup, Parser, Subcommand};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod git;
mod godo;
mod output;
use godo::{Godo, GodoError};

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

        /// Automatically commit all changes with the specified message after command exits
        #[arg(long)]
        commit: Option<String>,

        /// Exclude directories that match glob (can be specified multiple times)
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,

        /// Name of the sandbox
        name: String,

        /// Command to execute (if omitted, opens interactive shell)
        command: Vec<String>,
    },

    /// Show existing sandboxes
    #[command(alias = "ls")]
    List,

    /// Delete a named sandbox
    #[command(alias = "rm")]
    Remove {
        /// Name of the sandbox to remove
        name: String,

        /// Force removal even if there are uncommitted changes
        #[arg(long)]
        force: bool,
    },

    /// Clean up a sandbox by removing worktree but keeping the branch
    Clean {
        /// Name of the sandbox to clean (if not specified, cleans all sandboxes)
        name: Option<String>,
    },
}

fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(path.replacen("~", &home, 1));
        }
    }
    PathBuf::from(path)
}

/// Check if the current working directory is inside a godo sandbox
fn is_inside_godo_sandbox(godo_dir: &Path) -> Result<bool> {
    let current_dir = std::env::current_dir()?;
    let canonical_godo_dir = godo_dir
        .canonicalize()
        .unwrap_or_else(|_| godo_dir.to_path_buf());

    // Check if current directory is under the godo directory
    if let Ok(canonical_current) = current_dir.canonicalize() {
        return Ok(canonical_current.starts_with(&canonical_godo_dir));
    }

    Ok(false)
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Determine color output preference early for error handling
    let color = if cli.color {
        true
    } else if cli.no_color {
        false
    } else {
        // Auto-detect based on terminal
        std::io::stdout().is_terminal()
    };

    // Create output handler for potential error messages
    let output: Arc<dyn Output> = if cli.quiet {
        Arc::new(crate::output::Quiet)
    } else {
        Arc::new(crate::output::Terminal::new(color))
    };

    // Handle errors with custom formatting
    if let Err(e) = run(cli, output.clone()) {
        // Reset any existing colors
        print!("\x1b[0m");
        let _ = std::io::stdout().flush();

        // Check if this is a GodoError::CommandExit to get the specific exit code
        let exit_code = if let Some(GodoError::CommandExit { code }) = e.downcast_ref::<GodoError>()
        {
            // Don't display error message for command exit errors
            *code
        } else {
            // Use the output handler to display the error
            let _ = output.fail(&format!("{e:#}"));
            let _ = output.finish();
            1
        };

        std::process::exit(exit_code);
    }
    Ok(())
}

fn run(cli: Cli, output: Arc<dyn Output>) -> Result<()> {
    // Determine godo directory (priority: CLI flag > env var > default)
    let godo_dir = if let Some(dir) = &cli.dir {
        expand_tilde(dir)
    } else if let Ok(env_dir) = std::env::var("GODO_DIR") {
        expand_tilde(&env_dir)
    } else {
        expand_tilde(DEFAULT_GODO_DIR)
    };

    // Check if we're running from inside a godo sandbox
    if is_inside_godo_sandbox(&godo_dir)? {
        anyhow::bail!("Cannot run godo from within a godo sandbox");
    }

    // Determine repository directory
    let repo_dir = cli.repo_dir.as_ref().map(|repo| expand_tilde(repo));

    // Create Godo instance with the same output object
    let godo = Godo::new(godo_dir, repo_dir, output.clone(), cli.no_prompt)
        .context("Failed to initialize godo")?;

    match cli.command {
        Commands::Run {
            keep,
            commit,
            excludes,
            name,
            command,
        } => {
            godo.run(keep, commit, &excludes, &name, &command)?;
        }
        Commands::List => {
            godo.list()?;
        }
        Commands::Remove { name, force } => {
            godo.remove(&name, force)?;
        }
        Commands::Clean { name } => {
            godo.clean(name.as_deref())?;
        }
    }

    output.finish()?;
    Ok(())
}
