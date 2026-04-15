use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TabColors {
    pub active: RgbColor,
    pub inactive: RgbColor,
}

impl TabColors {
    pub const fn same(color: RgbColor) -> Self {
        Self {
            active: color,
            inactive: color,
        }
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

pub fn is_iterm2() -> bool {
    std::env::var_os("TERM_PROGRAM")
        .and_then(|value| value.into_string().ok())
        .map(|value| value == "iTerm.app")
        .unwrap_or(false)
}

fn is_kitty() -> bool {
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("TERM")
            .and_then(|value| value.into_string().ok())
            .map(|value| value == "xterm-kitty")
            .unwrap_or(false)
}

fn kitty_remote_control_addr() -> Option<String> {
    std::env::var_os("KITTY_LISTEN_ON").and_then(|value| value.into_string().ok())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TabColorBackend {
    None,
    Osc6,
    Kitty,
}

fn detect_tab_color_backend() -> TabColorBackend {
    if is_iterm2() {
        TabColorBackend::Osc6
    } else if is_kitty() {
        TabColorBackend::Kitty
    } else {
        TabColorBackend::None
    }
}

fn tab_color_backend_for_mode(
    color_mode: ColorMode,
    detected_backend: TabColorBackend,
) -> TabColorBackend {
    match color_mode {
        ColorMode::Off => TabColorBackend::None,
        ColorMode::Auto => detected_backend,
        ColorMode::On => match detected_backend {
            TabColorBackend::Kitty => TabColorBackend::Kitty,
            TabColorBackend::Osc6 | TabColorBackend::None => TabColorBackend::Osc6,
        },
    }
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

pub struct TerminalUi {
    stdout: Arc<Mutex<io::Stdout>>,
    last_rendered: Arc<Mutex<RenderedUi>>,
    kitty_tab_color_updater: Option<KittyTabColorUpdater>,
    title_mode: TitleMode,
    color_mode: ColorMode,
    title_format: String,
    tab_color_backend: TabColorBackend,
    pushed_title_stack: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RenderedUi {
    title: Option<String>,
    color: Option<TabColors>,
}

struct KittyTabColorUpdater {
    state: Arc<(Mutex<KittyTabColorState>, Condvar)>,
}

#[derive(Debug, Default)]
struct KittyTabColorState {
    running: bool,
    desired_color: Option<Option<TabColors>>,
}

impl KittyTabColorUpdater {
    fn new(remote_control_addr: String) -> Self {
        let state = Arc::new((
            Mutex::new(KittyTabColorState {
                running: true,
                desired_color: None,
            }),
            Condvar::new(),
        ));
        let worker_state = Arc::clone(&state);
        thread::spawn(move || {
            run_kitty_tab_color_worker(worker_state, remote_control_addr);
        });
        Self { state }
    }

    fn submit(&self, color: Option<TabColors>) {
        let (lock, condvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.desired_color = Some(color);
        condvar.notify_one();
    }
}

impl Drop for KittyTabColorUpdater {
    fn drop(&mut self) {
        let (lock, condvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.running = false;
        condvar.notify_one();
    }
}

impl TerminalUi {
    pub fn new(title_mode: TitleMode, color_mode: ColorMode, title_format: String) -> Self {
        let tab_color_backend = detect_tab_color_backend();
        let kitty_tab_color_updater = match tab_color_backend {
            TabColorBackend::Kitty => kitty_remote_control_addr().map(KittyTabColorUpdater::new),
            TabColorBackend::None | TabColorBackend::Osc6 => None,
        };
        Self {
            stdout: Arc::new(Mutex::new(io::stdout())),
            last_rendered: Arc::new(Mutex::new(RenderedUi::default())),
            kitty_tab_color_updater,
            title_mode,
            color_mode,
            title_format,
            tab_color_backend,
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
        let title = (self.title_mode != TitleMode::Off)
            .then(|| render_title(&self.title_format, self.title_mode, context));
        let tab_color_backend = tab_color_backend_for_mode(self.color_mode, self.tab_color_backend);
        let color = resolved_tab_colors(tab_color_backend, color);
        let mut last_rendered = self.last_rendered.lock().unwrap();
        let mut stdout = self.stdout.lock().unwrap();
        let mut wrote_output = false;

        if title != last_rendered.title {
            if let Some(title) = title.as_deref() {
                let sequence = format!("\x1b]0;{}\x07", sanitize_title(title));
                write_terminal_sequence(&mut *stdout, sequence.as_bytes())?;
                wrote_output = true;
            }
            last_rendered.title = title;
        }

        if color != last_rendered.color {
            match tab_color_backend {
                TabColorBackend::None => {}
                TabColorBackend::Osc6 => {
                    if let Some(color) = color {
                        let sequence = format!(
                            "\x1b]6;1;bg;red;brightness;{}\x07\
                             \x1b]6;1;bg;green;brightness;{}\x07\
                             \x1b]6;1;bg;blue;brightness;{}\x07",
                            color.active.r, color.active.g, color.active.b
                        );
                        write_terminal_sequence(&mut *stdout, sequence.as_bytes())?;
                    } else {
                        write_terminal_sequence(&mut *stdout, b"\x1b]6;1;bg;*;default\x07")?;
                    }
                    last_rendered.color = color;
                    wrote_output = true;
                }
                TabColorBackend::Kitty => {
                    if let Some(updater) = &self.kitty_tab_color_updater {
                        updater.submit(color);
                    } else {
                        write_terminal_sequence(
                            &mut *stdout,
                            kitty_local_set_tab_color_sequence(color).as_bytes(),
                        )?;
                        wrote_output = true;
                    }
                    last_rendered.color = color;
                }
            }
        }

        if wrote_output {
            stdout.flush()?;
        }
        Ok(())
    }

    pub fn restore(&self) -> Result<()> {
        let mut last_rendered = self.last_rendered.lock().unwrap();
        let mut stdout = self.stdout.lock().unwrap();
        let tab_color_backend = tab_color_backend_for_mode(self.color_mode, self.tab_color_backend);
        let mut wrote_output = false;
        if self.title_mode != TitleMode::Off {
            write_terminal_sequence(&mut *stdout, b"\x1b[23;0t")?;
            wrote_output = true;
        }
        if last_rendered.color.is_some() {
            match tab_color_backend {
                TabColorBackend::None => {}
                TabColorBackend::Osc6 => {
                    write_terminal_sequence(&mut *stdout, b"\x1b]6;1;bg;*;default\x07")?;
                    wrote_output = true;
                }
                TabColorBackend::Kitty => {
                    if let Some(updater) = &self.kitty_tab_color_updater {
                        updater.submit(None);
                    } else {
                        write_terminal_sequence(
                            &mut *stdout,
                            kitty_local_set_tab_color_sequence(None).as_bytes(),
                        )?;
                        wrote_output = true;
                    }
                    last_rendered.color = None;
                }
            }
        }
        if wrote_output {
            stdout.flush()?;
        }
        *last_rendered = RenderedUi::default();
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

fn resolved_tab_colors(
    backend: TabColorBackend,
    color: Option<RgbColor>,
) -> Option<TabColors> {
    let color = color?;
    Some(match backend {
        TabColorBackend::None => return None,
        TabColorBackend::Osc6 => TabColors::same(color),
        TabColorBackend::Kitty => TabColors {
            active: color,
            inactive: dim_tab_color(color),
        },
    })
}

fn dim_tab_color(color: RgbColor) -> RgbColor {
    const INACTIVE_PERCENT: u16 = 55;

    let scale = |component: u8| ((u16::from(component) * INACTIVE_PERCENT) / 100) as u8;
    RgbColor::new(scale(color.r), scale(color.g), scale(color.b))
}

fn kitty_local_set_tab_color_sequence(color: Option<TabColors>) -> String {
    let colors = match color {
        Some(color) => format!(
            "\"active_bg\":{},\"inactive_bg\":{},\"active_fg\":null,\"inactive_fg\":null",
            rgb_color_to_kitty_int(color.active),
            rgb_color_to_kitty_int(color.inactive)
        ),
        None => "\"active_bg\":null,\"inactive_bg\":null,\"active_fg\":null,\"inactive_fg\":null"
            .to_owned(),
    };

    let kitty_window_id = std::env::var_os("KITTY_WINDOW_ID")
        .and_then(|value| value.into_string().ok())
        .map(|value| format!(",\"kitty_window_id\":{value}"))
        .unwrap_or_default();

    format!(
        "\x1bP@kitty-cmd{{\"cmd\":\"set-tab-color\",\"version\":[0,14,2],\"no_response\":true{kitty_window_id},\"payload\":{{\"self\":true,\"colors\":{{{colors}}}}}}}\x1b\\"
    )
}

fn rgb_color_to_kitty_int(color: RgbColor) -> u32 {
    (u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)
}

fn run_kitty_tab_color_worker(
    state: Arc<(Mutex<KittyTabColorState>, Condvar)>,
    remote_control_addr: String,
) {
    loop {
        let color = {
            let (lock, condvar) = &*state;
            let mut state = lock.lock().unwrap();
            while state.running && state.desired_color.is_none() {
                state = condvar.wait(state).unwrap();
            }
            if !state.running {
                return;
            }
            state.desired_color.take().unwrap()
        };

        let _ = spawn_kitty_tab_color_update(&Some(remote_control_addr.clone()), color);
    }
}

fn spawn_kitty_tab_color_update(
    remote_control_addr: &Option<String>,
    color: Option<TabColors>,
) -> io::Result<()> {
    let _child = Command::new("kitten")
        .args(kitty_set_tab_color_args(remote_control_addr, color))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

fn kitty_set_tab_color_args(
    remote_control_addr: &Option<String>,
    color: Option<TabColors>,
) -> Vec<String> {
    let mut args = vec!["@".to_owned()];
    if let Some(remote_control_addr) = remote_control_addr {
        args.push("--to".to_owned());
        args.push(remote_control_addr.to_owned());
    }
    args.push("set-tab-color".to_owned());
    args.push("--self".to_owned());
    match color {
        Some(color) => args.extend([
            format!("active_bg={}", color.active),
            format!("inactive_bg={}", color.inactive),
            "active_fg=NONE".to_owned(),
            "inactive_fg=NONE".to_owned(),
        ]),
        None => args.extend([
            "active_bg=NONE".to_owned(),
            "inactive_bg=NONE".to_owned(),
            "active_fg=NONE".to_owned(),
            "inactive_fg=NONE".to_owned(),
        ]),
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_terminal_env(
        term_program: Option<&str>,
        term: Option<&str>,
        kitty_listen_on: Option<&str>,
        f: impl FnOnce(),
    ) {
        let _guard = env_lock().lock().unwrap();
        let original_kitty_window_id = std::env::var_os("KITTY_WINDOW_ID");
        let original_term_program = std::env::var_os("TERM_PROGRAM");
        let original_term = std::env::var_os("TERM");
        let original_kitty_listen_on = std::env::var_os("KITTY_LISTEN_ON");

        unsafe {
            std::env::remove_var("KITTY_WINDOW_ID");
            match term_program {
                Some(value) => std::env::set_var("TERM_PROGRAM", value),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match term {
                Some(value) => std::env::set_var("TERM", value),
                None => std::env::remove_var("TERM"),
            }
            match kitty_listen_on {
                Some(value) => std::env::set_var("KITTY_LISTEN_ON", value),
                None => std::env::remove_var("KITTY_LISTEN_ON"),
            }
        }

        f();

        unsafe {
            match original_kitty_window_id {
                Some(value) => std::env::set_var("KITTY_WINDOW_ID", value),
                None => std::env::remove_var("KITTY_WINDOW_ID"),
            }
            match original_term_program {
                Some(value) => std::env::set_var("TERM_PROGRAM", value),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match original_term {
                Some(value) => std::env::set_var("TERM", value),
                None => std::env::remove_var("TERM"),
            }
            match original_kitty_listen_on {
                Some(value) => std::env::set_var("KITTY_LISTEN_ON", value),
                None => std::env::remove_var("KITTY_LISTEN_ON"),
            }
        }
    }

    #[test]
    fn detects_iterm2_tab_color_backend() {
        with_terminal_env(Some("iTerm.app"), None, None, || {
            assert_eq!(detect_tab_color_backend(), TabColorBackend::Osc6);
        });
    }

    #[test]
    fn rejects_unsupported_terminal_program_for_tab_color_backend() {
        with_terminal_env(Some("ghostty"), None, None, || {
            assert_eq!(detect_tab_color_backend(), TabColorBackend::None);
        });
    }

    #[test]
    fn rejects_terminal_app_tab_color_backend() {
        with_terminal_env(Some("Apple_Terminal"), Some("xterm-256color"), None, || {
            assert_eq!(detect_tab_color_backend(), TabColorBackend::None);
        });
    }

    #[test]
    fn detects_kitty_tab_color_backend_from_term() {
        with_terminal_env(None, Some("xterm-kitty"), Some("unix:/tmp/kitty"), || {
            assert_eq!(detect_tab_color_backend(), TabColorBackend::Kitty);
        });
    }

    #[test]
    fn detects_kitty_tab_color_backend_without_remote_control_addr() {
        with_terminal_env(None, Some("xterm-kitty"), None, || {
            assert_eq!(detect_tab_color_backend(), TabColorBackend::Kitty);
        });
    }

    #[test]
    fn detects_kitty_tab_color_backend_from_window_id_and_remote_control_addr() {
        let _guard = env_lock().lock().unwrap();
        let original_window_id = std::env::var_os("KITTY_WINDOW_ID");
        let original_term_program = std::env::var_os("TERM_PROGRAM");
        let original_term = std::env::var_os("TERM");
        let original_kitty_listen_on = std::env::var_os("KITTY_LISTEN_ON");
        unsafe {
            std::env::set_var("KITTY_WINDOW_ID", "17");
            std::env::remove_var("TERM_PROGRAM");
            std::env::remove_var("TERM");
            std::env::set_var("KITTY_LISTEN_ON", "unix:/tmp/kitty");
        }

        assert_eq!(detect_tab_color_backend(), TabColorBackend::Kitty);

        unsafe {
            match original_window_id {
                Some(value) => std::env::set_var("KITTY_WINDOW_ID", value),
                None => std::env::remove_var("KITTY_WINDOW_ID"),
            }
            match original_term_program {
                Some(value) => std::env::set_var("TERM_PROGRAM", value),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match original_term {
                Some(value) => std::env::set_var("TERM", value),
                None => std::env::remove_var("TERM"),
            }
            match original_kitty_listen_on {
                Some(value) => std::env::set_var("KITTY_LISTEN_ON", value),
                None => std::env::remove_var("KITTY_LISTEN_ON"),
            }
        }
    }

    #[test]
    fn rejects_unknown_terminal_for_tab_color_backend() {
        with_terminal_env(Some("WezTerm"), Some("xterm-256color"), None, || {
            assert_eq!(detect_tab_color_backend(), TabColorBackend::None);
        });
    }

    #[test]
    fn auto_mode_uses_detected_tab_color_backend() {
        assert_eq!(
            tab_color_backend_for_mode(ColorMode::Auto, TabColorBackend::Osc6),
            TabColorBackend::Osc6
        );
        assert_eq!(
            tab_color_backend_for_mode(ColorMode::Auto, TabColorBackend::Kitty),
            TabColorBackend::Kitty
        );
        assert_eq!(
            tab_color_backend_for_mode(ColorMode::Auto, TabColorBackend::None),
            TabColorBackend::None
        );
    }

    #[test]
    fn on_mode_falls_back_to_osc6_when_terminal_is_unknown() {
        assert_eq!(
            tab_color_backend_for_mode(ColorMode::On, TabColorBackend::None),
            TabColorBackend::Osc6
        );
    }

    #[test]
    fn on_mode_keeps_kitty_backend_when_available() {
        assert_eq!(
            tab_color_backend_for_mode(ColorMode::On, TabColorBackend::Kitty),
            TabColorBackend::Kitty
        );
    }

    #[test]
    fn builds_kitty_set_tab_color_args() {
        assert_eq!(
            kitty_set_tab_color_args(
                &Some("unix:/tmp/kitty".to_owned()),
                Some(TabColors {
                    active: RgbColor::new(1, 2, 3),
                    inactive: RgbColor::new(4, 5, 6),
                })
            ),
            vec![
                "@".to_owned(),
                "--to".to_owned(),
                "unix:/tmp/kitty".to_owned(),
                "set-tab-color".to_owned(),
                "--self".to_owned(),
                "active_bg=#010203".to_owned(),
                "inactive_bg=#040506".to_owned(),
                "active_fg=NONE".to_owned(),
                "inactive_fg=NONE".to_owned(),
            ]
        );
    }

    #[test]
    fn derives_dim_inactive_tab_color_for_kitty() {
        assert_eq!(
            resolved_tab_colors(TabColorBackend::Kitty, Some(RgbColor::new(0x20, 0x80, 0xff))),
            Some(TabColors {
                active: RgbColor::new(0x20, 0x80, 0xff),
                inactive: RgbColor::new(0x11, 0x46, 0x8c),
            })
        );
    }

    #[test]
    fn builds_local_kitty_remote_control_sequence() {
        let _guard = env_lock().lock().unwrap();
        let original_window_id = std::env::var_os("KITTY_WINDOW_ID");
        unsafe {
            std::env::set_var("KITTY_WINDOW_ID", "17");
        }

        assert_eq!(
            kitty_local_set_tab_color_sequence(Some(TabColors {
                active: RgbColor::new(1, 2, 3),
                inactive: RgbColor::new(4, 5, 6),
            })),
            "\u{1b}P@kitty-cmd{\"cmd\":\"set-tab-color\",\"version\":[0,14,2],\"no_response\":true,\"kitty_window_id\":17,\"payload\":{\"self\":true,\"colors\":{\"active_bg\":66051,\"inactive_bg\":263430,\"active_fg\":null,\"inactive_fg\":null}}}\u{1b}\\"
        );

        unsafe {
            match original_window_id {
                Some(value) => std::env::set_var("KITTY_WINDOW_ID", value),
                None => std::env::remove_var("KITTY_WINDOW_ID"),
            }
        }
    }

    #[test]
    fn builds_kitty_reset_tab_color_args() {
        assert_eq!(
            kitty_set_tab_color_args(&Some("unix:/tmp/kitty".to_owned()), None),
            vec![
                "@".to_owned(),
                "--to".to_owned(),
                "unix:/tmp/kitty".to_owned(),
                "set-tab-color".to_owned(),
                "--self".to_owned(),
                "active_bg=NONE".to_owned(),
                "inactive_bg=NONE".to_owned(),
                "active_fg=NONE".to_owned(),
                "inactive_fg=NONE".to_owned(),
            ]
        );
    }

    #[test]
    fn builds_local_kitty_set_tab_color_args_without_listen_on() {
        assert_eq!(
            kitty_set_tab_color_args(
                &None,
                Some(TabColors {
                    active: RgbColor::new(1, 2, 3),
                    inactive: RgbColor::new(4, 5, 6),
                })
            ),
            vec![
                "@".to_owned(),
                "set-tab-color".to_owned(),
                "--self".to_owned(),
                "active_bg=#010203".to_owned(),
                "inactive_bg=#040506".to_owned(),
                "active_fg=NONE".to_owned(),
                "inactive_fg=NONE".to_owned(),
            ]
        );
    }
}
