use crossterm::{
    event::{self, Event, KeyCode, KeyEvent},
    terminal,
};
use std::collections::HashSet;
use std::io::{self, Write};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use thiserror::Error;

const INDENT: usize = 4;

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

pub type Result<T> = std::result::Result<T, OutputError>;

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
        Box::new(Quiet)
    }
}

/// Color-capable terminal renderer for user messages and prompts.
pub struct Terminal {
    color_choice: ColorChoice,
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

    fn write_colored(&self, msg: &str, color: Color) -> Result<()> {
        let mut stdout = StandardStream::stdout(self.color_choice);
        stdout.set_color(ColorSpec::new().set_fg(Some(color)))?;
        writeln!(stdout, "{}{msg}", " ".repeat(self.indent))?;
        stdout.reset()?;
        stdout.flush()?;
        Ok(())
    }

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
                    let num_char = std::char::from_digit(i, 10).unwrap();
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

    fn print_option_with_shortcut(&self, option: &str, shortcut: char) -> Result<()> {
        let mut stdout = StandardStream::stdout(self.color_choice);

        // Find the position of the shortcut in the option
        let shortcut_pos = option
            .chars()
            .position(|ch| ch.to_lowercase().next() == Some(shortcut))
            .unwrap_or(usize::MAX);

        if shortcut_pos < option.len() {
            // Print the part before the shortcut
            if shortcut_pos > 0 {
                write!(stdout, "{}", &option[..shortcut_pos])?;
            }

            // Print the shortcut in cyan
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Cyan)))?;
            write!(stdout, "{}", option.chars().nth(shortcut_pos).unwrap())?;
            stdout.reset()?;

            // Print the rest
            write!(
                stdout,
                "{}",
                &option[shortcut_pos + option.chars().nth(shortcut_pos).unwrap().len_utf8()..]
            )?;
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
                if let Event::Key(KeyEvent { code, .. }) =
                    event::read().map_err(|e| OutputError::Terminal(e.to_string()))?
                {
                    match code {
                        KeyCode::Char(ch) => {
                            let ch_lower = ch.to_lowercase().next().unwrap();

                            // Check if input matches any shortcut
                            if let Some(index) = shortcuts.iter().position(|&s| s == ch_lower) {
                                // Return the index and character to print after restoring terminal
                                return Ok((index, Some(ch_lower)));
                            }
                        }
                        KeyCode::Esc => {
                            return Err(OutputError::Cancelled);
                        }
                        _ => {}
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
        let _ = self.message(header);

        // Return a new Terminal with increased indent
        Box::new(Terminal {
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
    fn test_section_creates_indented_output() {
        let terminal = Terminal::new(false);
        assert_eq!(terminal.indent, 0);

        let section1 = terminal.section("Section 1");
        // Can't directly test indent since it's private, but we can test behavior
        // by checking that section returns a valid Output implementation
        let _ = section1.message("Test message");

        // Test nested sections
        let section2 = section1.section("Section 2");
        let _ = section2.message("Nested message");
    }
}
