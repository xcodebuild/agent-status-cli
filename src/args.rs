use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;

use crate::terminal::RgbColor;
use crate::tool::Tool;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Status {
    Starting,
    Busy,
    Ready,
    Error,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Starting => "starting",
            Status::Busy => "busy",
            Status::Ready => "ready",
            Status::Error => "error",
        }
    }

    pub fn default_label(self) -> &'static str {
        match self {
            Status::Starting => "Starting",
            Status::Busy => "Busy",
            Status::Ready => "Ready",
            Status::Error => "Error",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Status {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input {
            "starting" => Ok(Status::Starting),
            "busy" => Ok(Status::Busy),
            "ready" => Ok(Status::Ready),
            "error" => Ok(Status::Error),
            other => Err(format!(
                "invalid state '{other}', expected one of: starting, busy, ready, error"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TitleMode {
    Off,
    StatusOnly,
    ToolOnly,
    Combined,
}

impl std::str::FromStr for TitleMode {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input {
            "off" => Ok(TitleMode::Off),
            "status" => Ok(TitleMode::StatusOnly),
            "tool" => Ok(TitleMode::ToolOnly),
            "combined" => Ok(TitleMode::Combined),
            other => Err(format!(
                "invalid title mode '{other}', expected one of: off, status, tool, combined"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorMode {
    Off,
    Auto,
    On,
}

impl std::str::FromStr for ColorMode {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input {
            "off" => Ok(ColorMode::Off),
            "auto" => Ok(ColorMode::Auto),
            "on" => Ok(ColorMode::On),
            other => Err(format!(
                "invalid color mode '{other}', expected one of: off, auto, on"
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Args {
    pub tool: Tool,
    pub cli_bin: String,
    pub title_mode: TitleMode,
    pub color_mode: ColorMode,
    pub title_format: String,
    pub title_map: BTreeMap<Status, String>,
    pub color_map: BTreeMap<Status, RgbColor>,
    pub keep_alt_screen: bool,
    pub debug_log: Option<PathBuf>,
    pub passthrough_args: Vec<OsString>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            tool: Tool::Codex,
            cli_bin: Tool::Codex.default_bin().to_owned(),
            title_mode: TitleMode::Combined,
            color_mode: ColorMode::Auto,
            title_format: "{title} {tool_title}".to_owned(),
            title_map: BTreeMap::from([
                (Status::Starting, "⏳".to_owned()),
                (Status::Busy, "⚙️".to_owned()),
                (Status::Ready, "🟢".to_owned()),
                (Status::Error, "🔴".to_owned()),
            ]),
            color_map: BTreeMap::from([
                (Status::Starting, RgbColor::new(255, 167, 38)),
                (Status::Busy, RgbColor::new(230, 122, 0)),
                (Status::Ready, RgbColor::new(0, 158, 95)),
                (Status::Error, RgbColor::new(213, 0, 0)),
            ]),
            keep_alt_screen: false,
            debug_log: None,
            passthrough_args: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub enum ParseOutcome {
    Run(Args),
    Help(String),
}

pub fn parse_env_args() -> Result<ParseOutcome, String> {
    parse_args(env::args_os())
}

pub fn parse_args<I>(iter: I) -> Result<ParseOutcome, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = Args::default();
    let mut argv = iter.into_iter().peekable();
    let program = argv
        .next()
        .unwrap_or_else(|| OsString::from("agent-status-cli"));
    let mut saw_tool = false;
    let mut saw_wrapper_arg = false;

    let mut passthrough = Vec::new();
    if argv.peek().is_none() {
        return Ok(ParseOutcome::Help(help_text(&program)));
    }

    while let Some(raw) = argv.next() {
        if raw == OsStr::new("--") {
            passthrough.extend(argv);
            break;
        }

        let Some(arg) = raw.to_str() else {
            passthrough.push(raw);
            passthrough.extend(argv);
            break;
        };

        if !arg.starts_with('-') || arg == "-" {
            passthrough.push(raw);
            passthrough.extend(argv);
            break;
        }

        if arg == "-h" || arg == "--help" {
            return Ok(ParseOutcome::Help(help_text(&program)));
        }

        let (flag, inline_value) = split_long_option(arg);
        match flag {
            "--tool" => {
                saw_wrapper_arg = true;
                let value = next_value(flag, inline_value, &mut argv)?;
                let tool: Tool = value.parse()?;
                args.tool = tool;
                saw_tool = true;
                if args.cli_bin == Tool::Codex.default_bin()
                    || args.cli_bin == Tool::Claude.default_bin()
                {
                    args.cli_bin = tool.default_bin().to_owned();
                }
            }
            "--cli-bin" | "--codex-bin" => {
                saw_wrapper_arg = true;
                let value = next_value(flag, inline_value, &mut argv)?;
                args.cli_bin = value;
            }
            "--title-mode" => {
                saw_wrapper_arg = true;
                let value = next_value(flag, inline_value, &mut argv)?;
                args.title_mode = value.parse()?;
            }
            "--color-mode" => {
                saw_wrapper_arg = true;
                let value = next_value(flag, inline_value, &mut argv)?;
                args.color_mode = value.parse()?;
            }
            "--title-format" => {
                saw_wrapper_arg = true;
                args.title_format = next_value(flag, inline_value, &mut argv)?;
            }
            "--title-map" => {
                saw_wrapper_arg = true;
                let value = next_value(flag, inline_value, &mut argv)?;
                let (state, mapped) = parse_status_mapping(&value)?;
                args.title_map.insert(state, mapped);
            }
            "--color-map" => {
                saw_wrapper_arg = true;
                let value = next_value(flag, inline_value, &mut argv)?;
                let (state, mapped) = parse_color_mapping(&value)?;
                args.color_map.insert(state, mapped);
            }
            "--keep-alt-screen" => {
                saw_wrapper_arg = true;
                if inline_value.is_some() {
                    return Err("--keep-alt-screen does not accept a value".to_owned());
                }
                args.keep_alt_screen = true;
            }
            "--debug-log" => {
                saw_wrapper_arg = true;
                let value = next_value(flag, inline_value, &mut argv)?;
                args.debug_log = Some(PathBuf::from(value));
            }
            _ => {
                passthrough.push(raw);
                passthrough.extend(argv);
                break;
            }
        }
    }

    args.passthrough_args = passthrough;
    if !saw_tool && (saw_wrapper_arg || !args.passthrough_args.is_empty()) {
        return Err("missing required --tool <codex|claude>".to_owned());
    }
    Ok(ParseOutcome::Run(args))
}

fn split_long_option(input: &str) -> (&str, Option<String>) {
    if let Some((flag, value)) = input.split_once('=') {
        (flag, Some(value.to_owned()))
    } else {
        (input, None)
    }
}

fn next_value<I>(flag: &str, inline_value: Option<String>, iter: &mut I) -> Result<String, String>
where
    I: Iterator<Item = OsString>,
{
    if let Some(value) = inline_value {
        return Ok(value);
    }

    let Some(next) = iter.next() else {
        return Err(format!("missing value for {flag}"));
    };
    let Some(value) = next.to_str() else {
        return Err(format!("invalid non-utf8 value for {flag}"));
    };
    Ok(value.to_owned())
}

fn parse_status_mapping(input: &str) -> Result<(Status, String), String> {
    let Some((state, value)) = input.split_once('=') else {
        return Err(format!(
            "invalid title mapping '{input}', expected STATE=VALUE"
        ));
    };
    let status: Status = state.parse()?;
    if value.is_empty() {
        return Err(format!(
            "invalid title mapping '{input}', title value must not be empty"
        ));
    }
    Ok((status, value.to_owned()))
}

fn parse_color_mapping(input: &str) -> Result<(Status, RgbColor), String> {
    let Some((state, value)) = input.split_once('=') else {
        return Err(format!(
            "invalid color mapping '{input}', expected STATE=#RRGGBB"
        ));
    };
    let status: Status = state.parse()?;
    let color = value.parse()?;
    Ok((status, color))
}

pub fn help_text(program: &OsStr) -> String {
    let program = program.to_string_lossy();
    format!(
        "\
{program} wraps a supported interactive CLI in a PTY and mirrors its visible state into the terminal tab title and iTerm2 tab color.

Usage:
  {program} [wrapper options] [tool args...]
  {program} [wrapper options] -- [tool args...]

Wrapper options:
  --tool <codex|claude>       Select which CLI to wrap. Required
  --cli-bin <path-or-name>    Override the executable used for the selected tool
  --title-mode <mode>         off | status | tool | combined. Default: combined
  --color-mode <mode>         off | auto | on. Default: auto
  --title-format <template>   Template fields: {{title}} {{icon}} {{state}} {{label}} {{cwd}} {{tool}} {{tool_title}}
  --title-map <state=value>   Override one title mapping. Repeatable
  --color-map <state=#RRGGBB> Override one color mapping. Repeatable
  --keep-alt-screen           Do not inject tool-specific no-alt-screen arguments
  --debug-log <path>          Write wrapper debug logs to a file
  -h, --help                  Show this help

Defaults:
  starting=⏳
  busy=⚙️
  ready=🟢
  error=🔴

Examples:
  {program} --tool codex
  {program} --tool codex --title-map ready=✅ --color-map error=#d50000
  {program} --tool claude --title-format \"{{title}} {{tool_title}}\"
  {program} --tool codex -- --model gpt-5
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(argv: &[&str]) -> Args {
        match parse_args(argv.iter().map(|value| OsString::from(value))).unwrap() {
            ParseOutcome::Run(args) => args,
            ParseOutcome::Help(_) => panic!("expected run args"),
        }
    }

    #[test]
    fn parses_front_loaded_wrapper_args_and_passthrough() {
        let args = parse_ok(&[
            "tool",
            "--tool",
            "claude",
            "--title-map",
            "ready=✅",
            "--color-map=error=#112233",
            "resume",
            "--continue",
        ]);

        assert_eq!(args.tool, Tool::Claude);
        assert_eq!(args.cli_bin, "claude");
        assert_eq!(args.title_map.get(&Status::Ready).unwrap(), "✅");
        assert_eq!(
            *args.color_map.get(&Status::Error).unwrap(),
            RgbColor::new(0x11, 0x22, 0x33)
        );
        assert_eq!(
            args.passthrough_args,
            vec![OsString::from("resume"), OsString::from("--continue")]
        );
    }

    #[test]
    fn rejects_invalid_state_names() {
        let err = parse_args(
            ["tool", "--title-map", "broken=oops"]
                .into_iter()
                .map(OsString::from),
        )
        .unwrap_err();
        assert!(err.contains("invalid state 'broken'"));
    }

    #[test]
    fn rejects_invalid_color_values() {
        let err = parse_args(
            ["tool", "--color-map", "ready=green"]
                .into_iter()
                .map(OsString::from),
        )
        .unwrap_err();
        assert!(err.contains("invalid color 'green'"));
    }

    #[test]
    fn prints_help_when_no_args_are_provided() {
        match parse_args(["tool"].into_iter().map(OsString::from)).unwrap() {
            ParseOutcome::Help(help) => assert!(help.contains("Usage:")),
            ParseOutcome::Run(_) => panic!("expected help"),
        }
    }

    #[test]
    fn requires_explicit_tool_selection() {
        let err = parse_args(
            ["tool", "--title-map", "ready=✅", "resume"]
                .into_iter()
                .map(OsString::from),
        )
        .unwrap_err();
        assert!(err.contains("missing required --tool"));
    }
}
