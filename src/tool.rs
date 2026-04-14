use std::ffi::OsString;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tool {
    Codex,
    Claude,
}

impl Tool {
    pub fn default_bin(self) -> &'static str {
        match self {
            Tool::Codex => "codex",
            Tool::Claude => "claude",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Tool::Codex => "codex",
            Tool::Claude => "claude",
        }
    }

    pub fn injected_args(self) -> &'static [&'static str] {
        match self {
            Tool::Codex => &["--no-alt-screen"],
            Tool::Claude => &[],
        }
    }

    pub fn should_inject_alt_screen_flag(self, args: &[OsString]) -> bool {
        match self {
            Tool::Codex => !args.iter().any(|arg| arg == "--no-alt-screen"),
            Tool::Claude => false,
        }
    }
}

impl fmt::Display for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Tool {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input {
            "codex" => Ok(Tool::Codex),
            "claude" | "claude-code" => Ok(Tool::Claude),
            other => Err(format!(
                "invalid tool '{other}', expected one of: codex, claude"
            )),
        }
    }
}
