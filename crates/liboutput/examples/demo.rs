//! Test harness for exercising liboutput presentation.
//!
//! This tool provides a way to visually test and iterate on output formatting
//! without running the full godo application.
//!
//! Run with: `cargo run --example demo -- <command>`

use clap::{Parser, Subcommand};
use liboutput::{Output, Terminal};

/// Test harness for liboutput presentation
#[derive(Parser)]
#[command(name = "demo")]
#[command(about = "Exercise liboutput presentation features")]
struct Cli {
    /// Disable colors in output
    #[arg(long)]
    no_color: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show all message types
    Messages,
    /// Test the selection prompt
    Select,
    /// Test the confirmation prompt
    Confirm,
    /// Test nested sections
    Sections,
    /// Simulate a realistic godo workflow
    Workflow,
    /// Test text wrapping with long messages
    Wrap,
    /// Run all demos
    All,
}

/// Demonstrate all message types.
fn demo_messages(output: &dyn Output) {
    println!("\n=== Message Types ===\n");
    output.message("Status update: checking prerequisites").unwrap();
    output.message("Cloning repository to sandbox...").unwrap();
    output.success("Sandbox created successfully").unwrap();
    output.warn("Branch has diverged from main").unwrap();
    output.fail("Could not connect to remote").unwrap();
}

/// Demonstrate text wrapping.
fn demo_wrap(output: &dyn Output) {
    println!("\n=== Text Wrapping ===\n");
    output.message("This is a short message.").unwrap();
    output.message(
        "This is a much longer message that should wrap to multiple lines when \
         the terminal is narrow enough. It contains enough text to demonstrate \
         how the wrapping behaves with the current terminal width.",
    )
    .unwrap();
    output
        .success(
            "Operation completed successfully after processing all 47 files in the \
         repository and validating the configuration against the schema.",
        )
        .unwrap();
    output
        .warn(
            "The configuration file contains deprecated options that will be removed \
         in a future version. Please update your configuration to use the new format.",
        )
        .unwrap();
}

/// Demonstrate the selection prompt.
fn demo_select(output: &dyn Output) {
    println!("\n=== Selection Prompt ===\n");

    let options = vec![
        "Create a new sandbox".to_string(),
        "List existing sandboxes".to_string(),
        "Remove a sandbox".to_string(),
        "Clean all sandboxes".to_string(),
    ];

    match output.select("What would you like to do?", options) {
        Ok(idx) => output
            .success(&format!("You selected option {}", idx + 1))
            .unwrap(),
        Err(e) => output.warn(&format!("Selection cancelled: {}", e)).unwrap(),
    }
}

/// Demonstrate the confirmation prompt.
fn demo_confirm(output: &dyn Output) {
    println!("\n=== Confirmation Prompt ===\n");

    match output.confirm("Are you sure you want to proceed?") {
        Ok(true) => output.success("Confirmed").unwrap(),
        Ok(false) => output.message("Declined").unwrap(),
        Err(e) => output.warn(&format!("Cancelled: {}", e)).unwrap(),
    }
}

/// Demonstrate nested sections.
fn demo_sections(output: &dyn Output) {
    println!("\n=== Nested Sections ===\n");

    output.message("Starting operation").unwrap();

    let section1 = output.section("Phase 1: Setup");
    section1.message("Checking prerequisites").unwrap();
    section1.message("Validating configuration").unwrap();
    section1.success("Setup complete").unwrap();

    let section2 = output.section("Phase 2: Execution");
    section2.message("Processing files").unwrap();

    let subsection = section2.section("Batch 1");
    subsection.message("Processing file1.rs").unwrap();
    subsection.message("Processing file2.rs").unwrap();
    subsection.warn("file3.rs has warnings").unwrap();
    subsection.success("Batch complete").unwrap();

    section2.success("All batches processed").unwrap();

    output.success("Operation finished").unwrap();
}

/// Simulate a realistic godo workflow.
fn demo_workflow(output: &dyn Output) {
    println!("\n=== Simulated Godo Workflow ===\n");

    // Simulating: godo run feature-xyz
    output
        .message("Creating sandbox feature-xyz with branch godo/feature-xyz")
        .unwrap();
    output.message("Cloning tree to sandbox...").unwrap();
    output.success("Sandbox ready").unwrap();

    // Simulate some work happening...
    println!();
    output.message("Running command in sandbox...").unwrap();
    println!("  $ cargo test");
    println!("  running 42 tests");
    println!("  test result: ok. 42 passed");
    println!();

    // Simulating cleanup with sections
    let cleanup = output.section("Cleaning up sandbox: feature-xyz");
    cleanup.message("Checking for uncommitted changes").unwrap();
    cleanup.warn("You have uncommitted changes").unwrap();

    // Would normally prompt here
    cleanup.message("Staging and committing changes...").unwrap();
    cleanup.success("Committed with message: WIP").unwrap();
    cleanup.message("Removing worktree").unwrap();
    cleanup
        .success("Sandbox cleaned up, branch godo/feature-xyz kept")
        .unwrap();
}

/// Run all demos in sequence.
fn demo_all(output: &dyn Output) {
    demo_messages(output);
    demo_wrap(output);
    demo_sections(output);
    demo_workflow(output);
    // Skip interactive demos in "all" mode
    println!("\n(Skipping interactive demos: select, confirm)\n");
}

fn main() {
    let cli = Cli::parse();
    let output = Terminal::new(!cli.no_color);

    match cli.command {
        Some(Commands::Messages) => demo_messages(&output),
        Some(Commands::Select) => demo_select(&output),
        Some(Commands::Confirm) => demo_confirm(&output),
        Some(Commands::Sections) => demo_sections(&output),
        Some(Commands::Workflow) => demo_workflow(&output),
        Some(Commands::Wrap) => demo_wrap(&output),
        Some(Commands::All) => demo_all(&output),
        None => {
            println!("demo: Test harness for liboutput\n");
            println!("Run with --help for usage information.\n");
            // Quick preview
            demo_messages(&output);
        }
    }
}
