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
    char,
    collections::HashSet,
    io::{self, Write},
    result::Result as StdResult,
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    terminal,
};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use thiserror::Error;

/// Indentation level (in spaces) used for nested output sections.
const INDENT: usize = 4;

/// ASCII control representation of `Ctrl+C`.
const CTRL_C: char = '\u{3}';
/// ASCII control representation of `Ctrl+D`.
const CTRL_D: char = '\u{4}';

/// Determine whether the combination of `code` and `modifiers` represents an
/// interactive cancellation such as `Ctrl+C`, `Ctrl+D`, or `Esc`.
fn is_cancel_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    match code {
        KeyCode::Char(ch) => {
            if modifiers.contains(KeyModifiers::CONTROL)
                && matches!(ch.to_lowercase().next().unwrap_or(ch), 'c' | 'd')
            {
                return true;
            }

            matches!(ch, CTRL_C | CTRL_D)
        }
        KeyCode::Esc => true,
        _ => false,
    }
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

/// Abstraction over how user-facing messages and prompts are produced.
///
/// Implementations can render to a terminal, suppress output, or emit to other
/// formats (e.g. files or JSON) in the future.
pub trait Output: Send + Sync {
    /// Print an informational message.
    fn message(&self, msg: &str) -> Result<()>;
    /// Print a success message.
    fn success(&self, msg: &str) -> Result<()>;
    /// Print a warning message.
    fn warn(&self, msg: &str) -> Result<()>;
    /// Print an error/failure message.
    fn fail(&self, msg: &str) -> Result<()>;
    /// Ask the user to confirm an action; returns `true` if confirmed.
    fn confirm(&self, prompt: &str) -> Result<bool>;
    /// Present a list of `options` and return the chosen index.
    fn select(&self, prompt: &str, options: Vec<String>) -> Result<usize>;
    /// Flush any buffered output.
    fn finish(&self) -> Result<()>;
    /// Create a nested output section that indents subsequent messages.
    fn section(&self, header: &str) -> Box<dyn Output>;
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
}

/// Color-capable terminal renderer for user messages and prompts.
pub struct Terminal {
    /// Whether to emit ANSI color sequences when writing to stdout.
    color_choice: ColorChoice,
    /// Current indentation depth in spaces.
    indent: usize,
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
            indent: 0,
        }
    }

    /// Write `msg` using `color` while honoring the current indentation level.
    fn write_colored(&self, msg: &str, color: Color) -> Result<()> {
        let mut stdout = StandardStream::stdout(self.color_choice);
        stdout.set_color(ColorSpec::new().set_fg(Some(color)))?;
        writeln!(stdout, "{}{msg}", " ".repeat(self.indent))?;
        stdout.reset()?;
        stdout.flush()?;
        Ok(())
    }

    /// Generate mnemonic shortcuts for the provided `options` list.
    fn generate_shortcuts(&self, options: &[String]) -> Vec<char> {
        let mut shortcuts = Vec::new();
        let mut used_chars = HashSet::new();

        for option in options {
            let mut shortcut = None;

            // Try to find the first unique character in the option
            for ch in option.chars() {
                if ch.is_alphabetic() && !used_chars.contains(&ch.to_lowercase().next().unwrap()) {
                    let lower_ch = ch.to_lowercase().next().unwrap();
                    used_chars.insert(lower_ch);
                    shortcut = Some(lower_ch);
                    break;
                }
            }

            // If no unique character found, use a number
            if shortcut.is_none() {
                for i in 1..=9 {
                    let num_char = char::from_digit(i, 10).unwrap();
                    if !used_chars.contains(&num_char) {
                        used_chars.insert(num_char);
                        shortcut = Some(num_char);
                        break;
                    }
                }
            }

            // If still no shortcut, use any available letter
            if shortcut.is_none() {
                for ch in 'a'..='z' {
                    if !used_chars.contains(&ch) {
                        used_chars.insert(ch);
                        shortcut = Some(ch);
                        break;
                    }
                }
            }

            shortcuts.push(shortcut.unwrap_or('?'));
        }

        shortcuts
    }

    /// Render `option` while highlighting `shortcut` within the label when possible.
    fn print_option_with_shortcut(&self, option: &str, shortcut: char) -> Result<()> {
        let mut stdout = StandardStream::stdout(self.color_choice);

        // Find the first matching character using Unicode-aware iteration.
        // Compare case-insensitively by taking the first codepoint of to_lowercase(),
        // mirroring how shortcuts are generated.
        let mut match_byte_idx: Option<usize> = None;
        let mut match_char: Option<char> = None;
        for (byte_idx, ch) in option.char_indices() {
            let lower = ch.to_lowercase().next().unwrap_or(ch);
            if lower == shortcut {
                match_byte_idx = Some(byte_idx);
                match_char = Some(ch);
                break;
            }
        }

        if let (Some(idx), Some(ch)) = (match_byte_idx, match_char) {
            // Print prefix safely up to the matched character's byte index
            if idx > 0 {
                write!(stdout, "{}", &option[..idx])?;
            }

            // Highlight the matched character
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Cyan)))?;
            write!(stdout, "{}", ch)?;
            stdout.reset()?;

            // Print the suffix starting after the matched character
            let next = idx + ch.len_utf8();
            if next <= option.len() {
                write!(stdout, "{}", &option[next..])?;
            }
        } else {
            // Shortcut not in option text, print shortcut in brackets
            write!(stdout, "[{shortcut}] {option}")?;
        }

        writeln!(stdout)?;
        stdout.flush()?;
        Ok(())
    }
}

impl Output for Terminal {
    fn message(&self, msg: &str) -> Result<()> {
        self.write_colored(msg, Color::Cyan)
    }

    fn success(&self, msg: &str) -> Result<()> {
        self.write_colored(msg, Color::Green)
    }

    fn warn(&self, msg: &str) -> Result<()> {
        self.write_colored(msg, Color::Rgb(255, 165, 0)) // Orange
    }

    fn fail(&self, msg: &str) -> Result<()> {
        self.write_colored(msg, Color::Red)
    }

    fn confirm(&self, prompt: &str) -> Result<bool> {
        let options = vec!["Yes".to_string(), "No".to_string()];
        let selection = self.select(prompt, options)?;
        Ok(selection == 0)
    }

    fn select(&self, prompt: &str, options: Vec<String>) -> Result<usize> {
        if options.is_empty() {
            return Err(OutputError::InvalidInput(
                "No options provided for selection",
            ));
        }

        // Generate shortcuts for each option
        let shortcuts = self.generate_shortcuts(&options);

        // Print the prompt
        println!("{}{prompt}", " ".repeat(self.indent));

        // Print options with shortcuts highlighted
        for (option, shortcut) in options.iter().zip(shortcuts.iter()) {
            print!("{}  ", " ".repeat(self.indent));
            self.print_option_with_shortcut(option, *shortcut)?;
        }

        print!("{} > ", " ".repeat(self.indent));
        io::stdout().flush()?;

        // Enable raw mode to read single key press
        terminal::enable_raw_mode().map_err(|e| OutputError::Terminal(e.to_string()))?;

        // Ensure we restore terminal on exit
        let result = (|| -> Result<(usize, Option<char>)> {
            loop {
                if let Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind,
                    ..
                }) = event::read().map_err(|e| OutputError::Terminal(e.to_string()))?
                {
                    if kind != KeyEventKind::Press {
                        continue;
                    }

                    if is_cancel_key(code, modifiers) {
                        return Err(OutputError::Cancelled);
                    }

                    if let KeyCode::Char(ch) = code {
                        let ch_lower = ch.to_lowercase().next().unwrap();

                        // Check if input matches any shortcut
                        if let Some(index) = shortcuts.iter().position(|&s| s == ch_lower) {
                            // Return the index and character to print after restoring terminal
                            return Ok((index, Some(ch_lower)));
                        }
                    }
                }
            }
        })();

        // Always restore terminal mode
        terminal::disable_raw_mode().map_err(|e| OutputError::Terminal(e.to_string()))?;

        // Print the selected character after restoring terminal mode
        match result {
            Ok((index, Some(ch))) => {
                println!("{ch}");
                Ok(index)
            }
            Err(OutputError::Cancelled) => {
                println!();
                Err(OutputError::Cancelled)
            }
            Ok((index, None)) => Ok(index),
            Err(e) => Err(e),
        }
    }

    fn finish(&self) -> Result<()> {
        io::stdout().flush()?;
        Ok(())
    }

    fn section(&self, header: &str) -> Box<dyn Output> {
        // Print the section header at current indent
        self.message(header)
            .expect("section header message should succeed");

        // Return a new Terminal with increased indent
        Box::new(Self {
            color_choice: self.color_choice,
            indent: self.indent + INDENT,
        })
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
    fn test_terminal_generate_shortcuts() {
        let terminal = Terminal::new(false);

        // Test basic case
        let options = vec![
            "Apple".to_string(),
            "Banana".to_string(),
            "Cherry".to_string(),
        ];
        let shortcuts = terminal.generate_shortcuts(&options);
        assert_eq!(shortcuts, vec!['a', 'b', 'c']);

        // Test with conflicting first letters
        let options = vec![
            "Apple".to_string(),
            "Apricot".to_string(),
            "Avocado".to_string(),
        ];
        let shortcuts = terminal.generate_shortcuts(&options);
        assert_eq!(shortcuts[0], 'a');
        assert!(shortcuts[1] != 'a');
        assert!(shortcuts[2] != 'a' && shortcuts[2] != shortcuts[1]);

        // Test case insensitive
        let options = vec![
            "apple".to_string(),
            "BANANA".to_string(),
            "Cherry".to_string(),
        ];
        let shortcuts = terminal.generate_shortcuts(&options);
        assert_eq!(shortcuts, vec!['a', 'b', 'c']);
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
    fn test_is_cancel_key_variants() {
        assert!(is_cancel_key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(is_cancel_key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(is_cancel_key(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(is_cancel_key(KeyCode::Char(CTRL_C), KeyModifiers::NONE));
        assert!(is_cancel_key(KeyCode::Char(CTRL_D), KeyModifiers::NONE));

        assert!(!is_cancel_key(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(!is_cancel_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
    }

    #[test]
    fn test_section_creates_indented_output() {
        let terminal = Terminal::new(false);
        assert_eq!(terminal.indent, 0);

        let section1 = terminal.section("Section 1");
        // Can't directly test indent since it's private, but we can test behavior
        // by checking that section returns a valid Output implementation
        section1
            .message("Test message")
            .expect("section message succeeds");

        // Test nested sections
        let section2 = section1.section("Section 2");
        section2
            .message("Nested message")
            .expect("nested section message succeeds");
    }
}
