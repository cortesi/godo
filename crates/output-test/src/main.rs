//! Test harness for exercising liboutput presentation.
//!
//! This tool provides a way to visually test and iterate on output formatting
//! without running the full godo application.

use clap::{Parser, Subcommand};
use liboutput::{Output, Terminal};

/// Test harness for liboutput presentation
#[derive(Parser)]
#[command(name = "output-test")]
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
    /// Run all demos
    All,
}

/// Demonstrate all message types.
fn demo_messages(output: &dyn Output) {
    println!("\n=== Message Types ===\n");
    output.message("This is an informational message").unwrap();
    output.success("This is a success message").unwrap();
    output.warn("This is a warning message").unwrap();
    output.fail("This is a failure/error message").unwrap();
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

/// Demonstrate conflicting shortcuts in selection.
fn demo_select_conflicts(output: &dyn Output) {
    println!("\n=== Selection with Conflicting Shortcuts ===\n");

    let options = vec![
        "Apple".to_string(),
        "Apricot".to_string(),
        "Avocado".to_string(),
        "Banana".to_string(),
    ];

    match output.select("Pick a fruit (note: all start with 'A' except Banana):", options) {
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
        Ok(true) => output.success("You confirmed the action").unwrap(),
        Ok(false) => output.message("You declined the action").unwrap(),
        Err(e) => output.warn(&format!("Confirmation cancelled: {}", e)).unwrap(),
    }
}

/// Demonstrate nested sections.
fn demo_sections(output: &dyn Output) {
    println!("\n=== Nested Sections ===\n");

    output.message("Top-level message").unwrap();

    let section1 = output.section("Section 1: Setup");
    section1.message("Checking prerequisites...").unwrap();
    section1.success("All prerequisites met").unwrap();

    let subsection = section1.section("Subsection 1.1: Details");
    subsection.message("Running detailed checks").unwrap();
    subsection.warn("Minor issue detected").unwrap();
    subsection.success("Issue resolved").unwrap();

    let section2 = output.section("Section 2: Execution");
    section2.message("Running main operation...").unwrap();
    section2.success("Operation completed successfully").unwrap();

    output.success("All sections complete").unwrap();
}

/// Run all demos in sequence.
fn demo_all(output: &dyn Output) {
    demo_messages(output);
    demo_sections(output);
    demo_select(output);
    demo_select_conflicts(output);
    demo_confirm(output);
}

fn main() {
    let cli = Cli::parse();
    let output = Terminal::new(!cli.no_color);

    match cli.command {
        Some(Commands::Messages) => demo_messages(&output),
        Some(Commands::Select) => demo_select(&output),
        Some(Commands::Confirm) => demo_confirm(&output),
        Some(Commands::Sections) => demo_sections(&output),
        Some(Commands::All) => demo_all(&output),
        None => {
            // Default: show a brief overview
            println!("output-test: Test harness for liboutput\n");
            println!("Usage: output-test <COMMAND>\n");
            println!("Commands:");
            println!("  messages   Show all message types");
            println!("  select     Test the selection prompt");
            println!("  confirm    Test the confirmation prompt");
            println!("  sections   Test nested sections");
            println!("  all        Run all demos\n");
            println!("Options:");
            println!("  --no-color Disable colors in output");
            println!("  --help     Print help\n");

            // Quick preview of message types
            println!("Quick preview of message types:\n");
            demo_messages(&output);
        }
    }
}
