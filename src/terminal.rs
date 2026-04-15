use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use crossterm::terminal;

use crate::args::{ColorMode, Status, TitleMode};
use crate::tool::Tool;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

impl fmt::Display for RgbColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl std::str::FromStr for RgbColor {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let Some(hex) = input.strip_prefix('#') else {
            return Err(format!("invalid color '{input}', expected #RRGGBB"));
        };
        if hex.len() != 6 {
            return Err(format!("invalid color '{input}', expected #RRGGBB"));
        }

        let parse = |range: std::ops::Range<usize>| {
            u8::from_str_radix(&hex[range], 16)
                .map_err(|_| format!("invalid color '{input}', expected #RRGGBB"))
        };

        Ok(Self::new(parse(0..2)?, parse(2..4)?, parse(4..6)?))
    }
}

pub struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    pub fn new() -> Result<Self> {
        if io::stdin().is_terminal() {
            terminal::enable_raw_mode().context("failed to enable raw mode")?;
            Ok(Self { enabled: true })
        } else {
            Ok(Self { enabled: false })
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = terminal::disable_raw_mode();
        }
    }
}

pub fn terminal_size() -> Result<(u16, u16)> {
    terminal::size().context("failed to read terminal size")
}

#[derive(Clone, Debug)]
pub struct TitleContext {
    pub status: Status,
    pub state_label: String,
    pub title_token: String,
    pub cwd: String,
    pub tool: Tool,
    pub tool_title: String,
}

#[derive(Clone)]
pub struct TerminalUi {
    stdout: Arc<Mutex<io::Stdout>>,
    title_mode: TitleMode,
    color_mode: ColorMode,
    title_format: String,
    is_iterm2: bool,
    pushed_title_stack: bool,
}

impl TerminalUi {
    pub fn new(title_mode: TitleMode, color_mode: ColorMode, title_format: String) -> Self {
        Self {
            stdout: Arc::new(Mutex::new(io::stdout())),
            title_mode,
            color_mode,
            title_format,
            is_iterm2: std::env::var_os("TERM_PROGRAM")
                .and_then(|value| value.into_string().ok())
                .map(|value| value == "iTerm.app")
                .unwrap_or(false),
            pushed_title_stack: false,
        }
    }

    pub fn stdout(&self) -> Arc<Mutex<io::Stdout>> {
        Arc::clone(&self.stdout)
    }

    pub fn push_title_stack(&mut self) -> Result<()> {
        if self.title_mode == TitleMode::Off || self.pushed_title_stack {
            return Ok(());
        }
        let mut stdout = self.stdout.lock().unwrap();
        write_terminal_sequence(&mut *stdout, b"\x1b[22;0t")?;
        stdout.flush()?;
        self.pushed_title_stack = true;
        Ok(())
    }

    pub fn update(&self, context: &TitleContext, color: Option<RgbColor>) -> Result<()> {
        let mut stdout = self.stdout.lock().unwrap();

        if self.title_mode != TitleMode::Off {
            let title = render_title(&self.title_format, self.title_mode, context);
            let sequence = format!("\x1b]0;{}\x07", sanitize_title(&title));
            write_terminal_sequence(&mut *stdout, sequence.as_bytes())?;
        }

        if self.is_iterm2 && self.color_mode != ColorMode::Off {
            if let Some(color) = color {
                let sequence = format!(
                    "\x1b]6;1;bg;red;brightness;{}\x07\
                     \x1b]6;1;bg;green;brightness;{}\x07\
                     \x1b]6;1;bg;blue;brightness;{}\x07",
                    color.r, color.g, color.b
                );
                write_terminal_sequence(&mut *stdout, sequence.as_bytes())?;
            }
        }

        stdout.flush()?;
        Ok(())
    }

    pub fn restore(&self) -> Result<()> {
        let mut stdout = self.stdout.lock().unwrap();
        if self.title_mode != TitleMode::Off {
            write_terminal_sequence(&mut *stdout, b"\x1b[23;0t")?;
        }
        if self.is_iterm2 && self.color_mode != ColorMode::Off {
            write_terminal_sequence(&mut *stdout, b"\x1b]6;1;bg;*;default\x07")?;
        }
        stdout.flush()?;
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct DebugLog {
    file: Option<Arc<Mutex<std::fs::File>>>,
}

impl DebugLog {
    pub fn new(path: Option<&Path>) -> Result<Self> {
        let file = match path {
            Some(path) => Some(Arc::new(Mutex::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .with_context(|| format!("failed to open debug log {}", path.display()))?,
            ))),
            None => None,
        };

        Ok(Self { file })
    }

    pub fn write_line(&self, line: &str) {
        let Some(file) = &self.file else {
            return;
        };
        let mut file = file.lock().unwrap();
        let _ = writeln!(file, "{line}");
    }
}

fn render_title(template: &str, mode: TitleMode, context: &TitleContext) -> String {
    let template = match mode {
        TitleMode::Off => "",
        TitleMode::StatusOnly => "{title}",
        TitleMode::ToolOnly => "{tool_title}",
        TitleMode::Combined => template,
    };

    replace_template(template, context)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

fn replace_template(template: &str, context: &TitleContext) -> String {
    template
        .replace("{title}", &context.title_token)
        .replace("{icon}", &context.title_token)
        .replace("{state}", context.status.as_str())
        .replace("{label}", &context.state_label)
        .replace("{cwd}", &context.cwd)
        .replace("{tool}", context.tool.as_str())
        .replace("{tool_title}", &context.tool_title)
}

fn sanitize_title(input: &str) -> String {
    input
        .chars()
        .filter(|ch| *ch != '\u{7}' && *ch != '\u{1b}')
        .collect()
}

fn write_terminal_sequence(stdout: &mut impl Write, sequence: &[u8]) -> io::Result<()> {
    stdout.write_all(sequence)
}
