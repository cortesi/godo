#![deny(missing_docs)]
#![deny(rustdoc::missing_crate_level_docs)]
//! Terminal output abstractions and implementations for user-facing messages and prompts.
//!
//! This crate provides an [`Output`] trait that abstracts over how user messages
//! and interactive prompts are rendered. Implementations include:
//!
//! - [`Terminal`]: A color-capable terminal renderer for production use
//! - [`Quiet`]: A silent implementation that suppresses output (useful for tests)

use std::{
    io::{self, Write},
    result::Result as StdResult,
    time::Duration,
};

use crossterm::terminal;
use indicatif::{ProgressBar, ProgressStyle};
use inquire::{Confirm, InquireError, Select, ui::RenderConfig};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use thiserror::Error;

/// Default terminal width when detection fails.
const DEFAULT_WIDTH: usize = 80;

/// Minimum width before we disable wrapping entirely.
const MIN_WRAP_WIDTH: usize = 40;

/// Get the current terminal width, falling back to a default.
fn term_width() -> usize {
    terminal::size()
        .map(|(cols, _)| cols as usize)
        .unwrap_or(DEFAULT_WIDTH)
}

/// Errors produced by [`Output`] implementations when interacting with the user
/// or the terminal.
#[derive(Debug, Error)]
pub enum OutputError {
    /// The requested operation is not supported by this output backend.
    #[error("{0}")]
    Unsupported(&'static str),

    /// The caller supplied invalid input (e.g. empty options for a selector).
    #[error("{0}")]
    InvalidInput(&'static str),

    /// A terminal/TTY related failure occurred.
    #[error("Terminal error: {0}")]
    Terminal(String),

    /// Underlying I/O error while writing/reading to the terminal.
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// The user cancelled an interactive prompt.
    #[error("Selection cancelled")]
    Cancelled,
}

/// Convenience alias for output-related fallible operations.
pub type Result<T> = StdResult<T, OutputError>;

/// A handle to a running spinner animation.
///
/// The spinner will automatically stop and clear when dropped.
pub trait Spinner: Send {
    /// Stop the spinner and display a success message.
    fn finish_success(self: Box<Self>, msg: &str);
    /// Stop the spinner and display a failure message.
    fn finish_fail(self: Box<Self>, msg: &str);
    /// Stop the spinner and clear the line (no message).
    fn finish_clear(self: Box<Self>);
}

/// Abstraction over how user-facing messages and prompts are produced.
///
/// Implementations can render to a terminal, suppress output, or emit to other
/// formats (e.g. files or JSON) in the future.
pub trait Output: Send + Sync {
    /// Print an informational message (neutral, for status updates).
    fn message(&self, msg: &str) -> Result<()>;
    /// Print a success message (positive outcome).
    fn success(&self, msg: &str) -> Result<()>;
    /// Print a warning message (attention needed but not an error).
    fn warn(&self, msg: &str) -> Result<()>;
    /// Print an error/failure message (something went wrong).
    fn fail(&self, msg: &str) -> Result<()>;
    /// Print a key-value item where key is a label and value is content.
    /// The key is styled as a heading (dimmed) and the value as content.
    fn item(&self, key: &str, value: &str) -> Result<()>;
    /// Print a diff stat line with colored +insertions/-deletions.
    fn diff_stat(&self, label: &str, insertions: usize, deletions: usize) -> Result<()>;
    /// Print a commit line: hash subject +ins/-del
    fn commit(&self, hash: &str, subject: &str, insertions: usize, deletions: usize) -> Result<()>;
    /// Ask the user to confirm an action; returns `true` if confirmed.
    fn confirm(&self, prompt: &str) -> Result<bool>;
    /// Present a list of `options` and return the chosen index.
    fn select(&self, prompt: &str, options: Vec<String>) -> Result<usize>;
    /// Flush any buffered output.
    fn finish(&self) -> Result<()>;
    /// Create a nested output section with a header.
    fn section(&self, header: &str) -> Box<dyn Output>;
    /// Start a spinner with the given message.
    ///
    /// Returns a handle that can be used to stop the spinner with a final message.
    /// The spinner will animate until stopped.
    fn spinner(&self, msg: &str) -> Box<dyn Spinner>;
}

/// A no-op spinner for quiet mode.
struct QuietSpinner;

impl Spinner for QuietSpinner {
    fn finish_success(self: Box<Self>, _msg: &str) {}
    fn finish_fail(self: Box<Self>, _msg: &str) {}
    fn finish_clear(self: Box<Self>) {}
}

/// Output implementation that suppresses all messages and rejects interactive
/// prompts. Useful for non-interactive or test environments.
pub struct Quiet;

impl Output for Quiet {
    fn message(&self, _msg: &str) -> Result<()> {
        Ok(())
    }

    fn success(&self, _msg: &str) -> Result<()> {
        Ok(())
    }

    fn warn(&self, _msg: &str) -> Result<()> {
        Ok(())
    }

    fn fail(&self, _msg: &str) -> Result<()> {
        Ok(())
    }

    fn item(&self, _key: &str, _value: &str) -> Result<()> {
        Ok(())
    }

    fn diff_stat(&self, _label: &str, _insertions: usize, _deletions: usize) -> Result<()> {
        Ok(())
    }

    fn commit(
        &self,
        _hash: &str,
        _subject: &str,
        _insertions: usize,
        _deletions: usize,
    ) -> Result<()> {
        Ok(())
    }

    fn confirm(&self, _prompt: &str) -> Result<bool> {
        Err(OutputError::Unsupported(
            "Cannot prompt for confirmation in quiet mode",
        ))
    }

    fn select(&self, _prompt: &str, _options: Vec<String>) -> Result<usize> {
        Err(OutputError::Unsupported(
            "Cannot prompt for selection in quiet mode",
        ))
    }

    fn finish(&self) -> Result<()> {
        Ok(())
    }

    fn section(&self, _header: &str) -> Box<dyn Output> {
        Box::new(Self)
    }

    fn spinner(&self, _msg: &str) -> Box<dyn Spinner> {
        Box::new(QuietSpinner)
    }
}

/// A terminal spinner using indicatif.
struct TerminalSpinner {
    /// The underlying progress bar from indicatif.
    bar: ProgressBar,
}

impl Spinner for TerminalSpinner {
    fn finish_success(self: Box<Self>, msg: &str) {
        self.bar
            .set_style(ProgressStyle::with_template(&format!("\x1b[32m✓\x1b[0m {msg}")).unwrap());
        self.bar.finish();
    }

    fn finish_fail(self: Box<Self>, msg: &str) {
        self.bar
            .set_style(ProgressStyle::with_template(&format!("\x1b[31m✗\x1b[0m {msg}")).unwrap());
        self.bar.finish();
    }

    fn finish_clear(self: Box<Self>) {
        self.bar.finish_and_clear();
    }
}

/// Color-capable terminal renderer for user messages and prompts.
pub struct Terminal {
    /// Whether to emit ANSI color sequences when writing to stdout.
    color_choice: ColorChoice,
    /// The prefix string for indentation in nested sections.
    line_prefix: String,
}

impl Terminal {
    /// Create a new terminal output.
    ///
    /// - `color`: when `true`, always render colored output; when `false`,
    ///   disable ANSI colors.
    pub fn new(color: bool) -> Self {
        let color_choice = if color {
            ColorChoice::Always
        } else {
            ColorChoice::Never
        };
        Self {
            color_choice,
            line_prefix: String::new(),
        }
    }

    /// Build an `inquire` render configuration that matches this terminal's color mode.
    fn inquire_render_config(&self) -> RenderConfig<'static> {
        match self.color_choice {
            ColorChoice::Never => RenderConfig::empty(),
            ColorChoice::Always | ColorChoice::AlwaysAnsi | ColorChoice::Auto => {
                RenderConfig::default_colored()
            }
        }
    }

    /// Convert an `inquire` error into an [`OutputError`].
    fn map_inquire_error(err: InquireError) -> OutputError {
        match err {
            InquireError::IO(err) => OutputError::Io(err),
            InquireError::OperationCanceled | InquireError::OperationInterrupted => {
                OutputError::Cancelled
            }
            other => OutputError::Terminal(other.to_string()),
        }
    }

    /// Calculate available width for text after accounting for prefix.
    fn available_width(&self) -> usize {
        let prefix_width = self.line_prefix.chars().count();
        let total = term_width();
        if total > prefix_width + MIN_WRAP_WIDTH {
            total - prefix_width
        } else {
            total // Don't wrap if too narrow
        }
    }

    /// Wrap text to fit terminal width, respecting the current prefix.
    fn wrap_text(&self, text: &str) -> Vec<String> {
        let width = self.available_width();
        if width < MIN_WRAP_WIDTH {
            // Terminal too narrow, don't wrap
            return vec![text.to_string()];
        }

        textwrap::wrap(text, width)
            .into_iter()
            .map(|cow| cow.into_owned())
            .collect()
    }

    /// Write a line with the current prefix.
    fn write_prefixed_line(
        &self,
        stdout: &mut StandardStream,
        line: &str,
        is_first: bool,
    ) -> Result<()> {
        if !self.line_prefix.is_empty() {
            // For continuation lines within the same message, use the same prefix
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Rgb(100, 100, 100))))?;
            if is_first {
                write!(stdout, "{}", self.line_prefix)?;
            } else {
                // Continuation lines get the vertical bar prefix
                let cont_prefix = self.continuation_prefix();
                write!(stdout, "{}", cont_prefix)?;
            }
            stdout.reset()?;
        }
        write!(stdout, "{}", line)?;
        Ok(())
    }

    /// Get the prefix for continuation lines.
    fn continuation_prefix(&self) -> &str {
        &self.line_prefix
    }

    /// Write a message with color styling.
    fn write_message(&self, msg: &str, color: Option<Color>, dim: bool) -> Result<()> {
        let mut stdout = StandardStream::stdout(self.color_choice);
        let lines = self.wrap_text(msg);

        for (i, line) in lines.iter().enumerate() {
            let is_first = i == 0;

            // Write the tree prefix
            self.write_prefixed_line(&mut stdout, "", is_first)?;

            // Write the message text
            let mut spec = ColorSpec::new();
            if let Some(c) = color {
                spec.set_fg(Some(c));
            }
            if dim {
                spec.set_dimmed(true);
            }
            stdout.set_color(&spec)?;
            writeln!(stdout, "{}", line)?;
            stdout.reset()?;
        }

        stdout.flush()?;
        Ok(())
    }
}

impl Output for Terminal {
    fn message(&self, msg: &str) -> Result<()> {
        // Neutral informational message - dimmed to reduce visual noise
        self.write_message(msg, None, true)
    }

    fn success(&self, msg: &str) -> Result<()> {
        self.write_message(msg, Some(Color::Green), false)
    }

    fn warn(&self, msg: &str) -> Result<()> {
        self.write_message(msg, Some(Color::Yellow), false)
    }

    fn fail(&self, msg: &str) -> Result<()> {
        self.write_message(msg, Some(Color::Red), false)
    }

    fn item(&self, key: &str, value: &str) -> Result<()> {
        let mut stdout = StandardStream::stdout(self.color_choice);

        // Write prefix if we're in a section
        if !self.line_prefix.is_empty() {
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Rgb(100, 100, 100))))?;
            write!(stdout, "{}", self.line_prefix)?;
            stdout.reset()?;
        }

        // Write key dimmed
        stdout.set_color(ColorSpec::new().set_dimmed(true))?;
        write!(stdout, "{}: ", key)?;
        stdout.reset()?;

        // Write value in normal style
        writeln!(stdout, "{}", value)?;
        stdout.flush()?;
        Ok(())
    }

    fn diff_stat(&self, label: &str, insertions: usize, deletions: usize) -> Result<()> {
        let mut stdout = StandardStream::stdout(self.color_choice);

        // Write prefix if we're in a section
        if !self.line_prefix.is_empty() {
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Rgb(100, 100, 100))))?;
            write!(stdout, "{}", self.line_prefix)?;
            stdout.reset()?;
        }

        // Write the label dimmed (consistent with item)
        stdout.set_color(ColorSpec::new().set_dimmed(true))?;
        write!(stdout, "{} ", label)?;
        stdout.reset()?;

        // Write insertions in green
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green)))?;
        write!(stdout, "+{}", insertions)?;
        stdout.reset()?;

        write!(stdout, "/")?;

        // Write deletions in red
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
        write!(stdout, "-{}", deletions)?;
        stdout.reset()?;

        writeln!(stdout)?;
        stdout.flush()?;
        Ok(())
    }

    fn commit(&self, hash: &str, subject: &str, insertions: usize, deletions: usize) -> Result<()> {
        let mut stdout = StandardStream::stdout(self.color_choice);

        // Write prefix if we're in a section
        if !self.line_prefix.is_empty() {
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Rgb(100, 100, 100))))?;
            write!(stdout, "{}", self.line_prefix)?;
            stdout.reset()?;
        }

        // Write hash in yellow
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Yellow)))?;
        write!(stdout, "{}", hash)?;
        stdout.reset()?;

        // Write subject
        write!(stdout, " {}", subject)?;

        // Write stats if non-zero
        if insertions > 0 || deletions > 0 {
            write!(stdout, " ")?;
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green)))?;
            write!(stdout, "+{}", insertions)?;
            stdout.reset()?;
            write!(stdout, "/")?;
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
            write!(stdout, "-{}", deletions)?;
            stdout.reset()?;
        }

        writeln!(stdout)?;
        stdout.flush()?;
        Ok(())
    }

    fn confirm(&self, prompt: &str) -> Result<bool> {
        Confirm::new(prompt)
            .with_default(false)
            .with_render_config(self.inquire_render_config())
            .prompt_skippable()
            .map_err(Self::map_inquire_error)?
            .ok_or(OutputError::Cancelled)
    }

    fn select(&self, prompt: &str, options: Vec<String>) -> Result<usize> {
        if options.is_empty() {
            return Err(OutputError::InvalidInput(
                "No options provided for selection",
            ));
        }

        Select::new(prompt, options)
            .without_filtering()
            .with_vim_mode(true)
            .with_help_message("↑↓/j/k to move, enter to select, esc to cancel")
            .with_render_config(self.inquire_render_config())
            .raw_prompt_skippable()
            .map_err(Self::map_inquire_error)?
            .map(|answer| answer.index)
            .ok_or(OutputError::Cancelled)
    }

    fn finish(&self) -> Result<()> {
        io::stdout().flush()?;
        Ok(())
    }

    #[allow(clippy::let_underscore_must_use)]
    fn section(&self, header: &str) -> Box<dyn Output> {
        let mut stdout = StandardStream::stdout(self.color_choice);

        // Print section header with current prefix
        if !self.line_prefix.is_empty() {
            let _ = stdout.set_color(ColorSpec::new().set_fg(Some(Color::Rgb(100, 100, 100))));
            let _ = write!(stdout, "{}", self.line_prefix);
            let _ = stdout.reset();
        }

        // Section header in bold
        let _ = stdout.set_color(ColorSpec::new().set_bold(true));
        let _ = writeln!(stdout, "{}", header);
        let _ = stdout.reset();
        let _ = stdout.flush();

        // Build the new prefix for children - simple indentation
        let new_prefix = format!("{}   ", self.line_prefix);

        Box::new(Self {
            color_choice: self.color_choice,
            line_prefix: new_prefix,
        })
    }

    fn spinner(&self, msg: &str) -> Box<dyn Spinner> {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
        bar.set_message(msg.to_string());
        bar.enable_steady_tick(Duration::from_millis(80));
        Box::new(TerminalSpinner { bar })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quiet_confirm_returns_error() {
        let quiet = Quiet;
        let result = quiet.confirm("Test prompt?");
        assert!(result.is_err());

        if let Err(e) = result {
            assert!(matches!(e, OutputError::Unsupported(_)));
            assert_eq!(
                e.to_string(),
                "Cannot prompt for confirmation in quiet mode"
            );
        }
    }

    #[test]
    fn test_quiet_select_returns_error() {
        let quiet = Quiet;
        let options = vec!["Option 1".to_string(), "Option 2".to_string()];
        let result = quiet.select("Choose an option:", options);
        assert!(result.is_err());

        if let Err(e) = result {
            assert!(matches!(e, OutputError::Unsupported(_)));
            assert_eq!(e.to_string(), "Cannot prompt for selection in quiet mode");
        }
    }

    #[test]
    fn test_select_empty_options_error() {
        let terminal = Terminal::new(false);
        let result = terminal.select("Choose:", vec![]);
        assert!(result.is_err());

        if let Err(e) = result {
            assert!(matches!(e, OutputError::InvalidInput(_)));
            assert_eq!(e.to_string(), "No options provided for selection");
        }
    }

    #[test]
    fn test_section_creates_nested_output() {
        let terminal = Terminal::new(false);

        let section1 = terminal.section("Section 1");
        section1
            .message("Test message")
            .expect("section message succeeds");

        // Test nested sections
        let section2 = section1.section("Section 2");
        section2
            .message("Nested message")
            .expect("nested section message succeeds");
    }

    #[test]
    fn test_wrap_text() {
        let terminal = Terminal::new(false);
        // With default width, short text shouldn't wrap
        let lines = terminal.wrap_text("short");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "short");
    }
}
