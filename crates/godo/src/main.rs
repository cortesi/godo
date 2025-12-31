#![deny(missing_docs)]
#![deny(rustdoc::missing_crate_level_docs)]
//! Command-line interface for managing godo sandboxes via the libgodo crate.

mod args;
mod commands;
mod ui;
mod utils;

use std::{
    env,
    io::{self, IsTerminal, Write},
    sync::Arc,
};

use anyhow::{Context, Result};
use clap::Parser;
use godo_term::{Output, Quiet, Terminal};
use libgodo::{Godo, GodoError};

use args::{Cli, Commands, RunRequest};
use utils::{current_sandbox_name, expand_tilde};

/// Default directory for storing godo-managed sandboxes.
const DEFAULT_GODO_DIR: &str = "~/.godo";

/// CLI entrypoint.
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

        std::process::exit(exit_code);
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

    // Detect if we're running from within a sandbox
    let current_sandbox = current_sandbox_name(&godo_dir)?;

    // Per-command sandbox context checks
    match &cli.command {
        Commands::List => {}
        Commands::Diff { .. } => {}
        Commands::Run { name, .. } => {
            if let Some(ref current) = current_sandbox
                && current == name
            {
                anyhow::bail!(
                    "Cannot run sandbox '{}' from within itself. Exit the sandbox first.",
                    name
                );
            }
        }
        Commands::Remove { name, .. } => {
            if let Some(ref current) = current_sandbox
                && current == name
            {
                anyhow::bail!(
                    "Cannot remove sandbox '{}' while inside it. Exit the sandbox first.",
                    name
                );
            }
        }
        Commands::Clean { name } => {
            if let Some(ref current) = current_sandbox {
                if name.is_none() {
                    anyhow::bail!(
                        "Cannot run 'godo clean' (all sandboxes) from within a sandbox. \
                         Exit the sandbox first, or specify a sandbox name."
                    );
                }
                if name.as_ref() == Some(current) {
                    anyhow::bail!(
                        "Cannot clean sandbox '{}' while inside it. Exit the sandbox first.",
                        current
                    );
                }
            }
        }
    }

    // Determine repository directory
    let repo_dir = cli.repo_dir.as_ref().map(|repo| expand_tilde(repo));

    // Create Godo instance
    let godo = Godo::new(godo_dir, repo_dir).context("Failed to initialize godo")?;

    match cli.command {
        Commands::Run {
            keep,
            commit,
            sh,
            excludes,
            name,
            command,
        } => {
            commands::run::run(
                &godo,
                output.as_ref(),
                cli.no_prompt,
                RunRequest {
                    keep,
                    commit,
                    force_shell: sh,
                    excludes,
                    sandbox_name: name,
                    command,
                },
            )?;
        }
        Commands::List => {
            commands::list::list(&godo, output.as_ref())?;
        }
        Commands::Diff {
            name,
            base,
            pager,
            no_pager,
        } => {
            commands::diff::diff(
                &godo,
                output.as_ref(),
                name.as_deref(),
                base.as_deref(),
                pager,
                no_pager,
                current_sandbox.as_deref(),
            )?;
        }
        Commands::Remove { name, force } => {
            commands::remove::remove(&godo, output.as_ref(), name, force, cli.no_prompt)?;
        }
        Commands::Clean { name } => {
            commands::clean::clean(&godo, output.as_ref(), name.as_deref(), cli.no_prompt)?;
        }
    }

    output.finish()?;
    Ok(())
}