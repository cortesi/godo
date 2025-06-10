use std::io::{self, Write};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

/// Trait for handling output in different contexts (terminal, no output, etc.)
/// Future implementations could include file output, JSON output, etc.
pub trait Output: Send + Sync {
    fn message(&self, msg: &str) -> io::Result<()>;
    fn success(&self, msg: &str) -> io::Result<()>;
    fn warn(&self, msg: &str) -> io::Result<()>;
    fn fail(&self, msg: &str) -> io::Result<()>;
}

pub struct Quiet;

impl Output for Quiet {
    fn message(&self, _msg: &str) -> io::Result<()> {
        Ok(())
    }

    fn success(&self, _msg: &str) -> io::Result<()> {
        Ok(())
    }

    fn warn(&self, _msg: &str) -> io::Result<()> {
        Ok(())
    }

    fn fail(&self, _msg: &str) -> io::Result<()> {
        Ok(())
    }
}

pub struct Terminal {
    color_choice: ColorChoice,
}

impl Terminal {
    pub fn new(color: bool) -> Self {
        let color_choice = if color {
            ColorChoice::Always
        } else {
            ColorChoice::Never
        };
        Self { color_choice }
    }

    fn write_colored(&self, msg: &str, color: Color) -> io::Result<()> {
        let mut stdout = StandardStream::stdout(self.color_choice);
        stdout.set_color(ColorSpec::new().set_fg(Some(color)))?;
        writeln!(stdout, "{msg}")?;
        stdout.reset()?;
        stdout.flush()
    }
}

impl Output for Terminal {
    fn message(&self, msg: &str) -> io::Result<()> {
        self.write_colored(msg, Color::Cyan)
    }

    fn success(&self, msg: &str) -> io::Result<()> {
        self.write_colored(msg, Color::Green)
    }

    fn warn(&self, msg: &str) -> io::Result<()> {
        self.write_colored(msg, Color::Rgb(255, 165, 0)) // Orange
    }

    fn fail(&self, msg: &str) -> io::Result<()> {
        self.write_colored(msg, Color::Red)
    }
}
