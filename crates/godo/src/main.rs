#![deny(missing_docs)]
#![deny(rustdoc::missing_crate_level_docs)]
//! Command-line interface for managing godo sandboxes via the libgodo crate.

use std::{
    env,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use anyhow::{Context, Result};
use clap::{ArgGroup, Parser, Subcommand};
use libgodo::{Godo, GodoError, Output, Quiet, Terminal};

/// Default directory for storing godo-managed sandboxes.
const DEFAULT_GODO_DIR: &str = "~/.godo";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(group(
    ArgGroup::new("color_mode")
        .args(["color", "no_color"])
))]
/// Top-level CLI options for godo.
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
    /// The primary command to execute.
    command: Commands,
}

#[derive(Subcommand)]
/// CLI subcommands supported by godo.
enum Commands {
    /// Run a command in an isolated workspace
    Run {
        /// Keep the sandbox after the command exits
        #[arg(long)]
        keep: bool,

        /// Automatically commit all changes with the specified message after command exits
        #[arg(long)]
        commit: Option<String>,

        /// Force shell evaluation with $SHELL -c
        #[arg(long = "sh")]
        sh: bool,

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

    /// Diff a sandbox against its recorded base commit
    Diff {
        /// Name of the sandbox to diff
        name: String,

        /// Override the base commit used for diffing
        #[arg(long, value_name = "COMMIT")]
        base: Option<String>,

        /// Override the pager command for diff output
        #[arg(long, value_name = "CMD", conflicts_with = "no_pager")]
        pager: Option<String>,

        /// Disable paging for diff output
        #[arg(long = "no-pager", conflicts_with = "pager")]
        no_pager: bool,
    },

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

/// Expand a leading `~` in a filesystem path using the `HOME` environment variable.
fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~")
        && let Ok(home) = env::var("HOME")
    {
        return PathBuf::from(path.replacen("~", &home, 1));
    }
    PathBuf::from(path)
}

/// Check if the current working directory is inside a godo sandbox.
fn is_inside_godo_sandbox(godo_dir: &Path) -> Result<bool> {
    let current_dir = env::current_dir()?;
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
        io::stdout().is_terminal()
    };

    // Create output handler for potential error messages
    let output: Arc<dyn Output> = if cli.quiet {
        Arc::new(Quiet)
    } else {
        Arc::new(Terminal::new(color))
    };

    // Handle errors with custom formatting
    if let Err(e) = run(cli, &output) {
        // Reset any existing colors only if color was enabled and stdout is a TTY
        if color && io::stdout().is_terminal() {
            print!("\x1b[0m");
            if let Err(flush_err) = io::stdout().flush() {
                eprintln!("Failed to flush stdout while resetting colors: {flush_err}");
            }
        }

        let exit_code = match e.downcast_ref::<GodoError>() {
            Some(err @ GodoError::CommandExit { .. }) => err.exit_code(),
            Some(err @ GodoError::UserAborted) => {
                if let Err(finish_err) = output.finish() {
                    eprintln!("Failed to flush output handler: {finish_err:#}");
                }
                err.exit_code()
            }
            Some(err) => {
                // Use the output handler to display the error
                if let Err(display_err) = output.fail(&format!("{e:#}")) {
                    eprintln!("Failed to report error via output handler: {display_err:#}");
                }
                if let Err(finish_err) = output.finish() {
                    eprintln!("Failed to flush output handler: {finish_err:#}");
                }
                err.exit_code()
            }
            None => {
                // Use the output handler to display the error
                if let Err(display_err) = output.fail(&format!("{e:#}")) {
                    eprintln!("Failed to report error via output handler: {display_err:#}");
                }
                if let Err(finish_err) = output.finish() {
                    eprintln!("Failed to flush output handler: {finish_err:#}");
                }
                1
            }
        };

        process::exit(exit_code);
    }
    Ok(())
}

/// Execute the selected CLI command using the provided output implementation.
fn run(cli: Cli, output: &Arc<dyn Output>) -> Result<()> {
    // Determine godo directory (priority: CLI flag > env var > default)
    let godo_dir = if let Some(dir) = &cli.dir {
        expand_tilde(dir)
    } else if let Ok(env_dir) = env::var("GODO_DIR") {
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
    let godo = Godo::new(godo_dir, repo_dir, Arc::clone(output), cli.no_prompt)
        .context("Failed to initialize godo")?;

    match cli.command {
        Commands::Run {
            keep,
            commit,
            sh,
            excludes,
            name,
            command,
        } => {
            godo.run(keep, commit, sh, &excludes, &name, &command)?;
        }
        Commands::List => {
            godo.list()?;
        }
        Commands::Diff {
            name,
            base,
            pager,
            no_pager,
        } => {
            godo.diff(&name, base.as_deref(), pager, no_pager)?;
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
