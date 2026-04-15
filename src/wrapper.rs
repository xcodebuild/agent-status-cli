use std::io::{self, Read, Write};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
#[cfg(not(windows))]
use signal_hook::consts::signal::SIGWINCH;
#[cfg(not(windows))]
use signal_hook::flag;

use crate::args::{Args, Status};
use crate::osc::OscFilter;
use crate::state::StateDetector;
use crate::terminal::{DebugLog, RawModeGuard, RgbColor, TerminalUi, TitleContext, terminal_size};
use crate::tool::Tool;

const STATUS_CHANGE_BUFFER: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingStatusChange {
    generation: u64,
    status: Status,
}

#[derive(Debug)]
struct BufferedStatus {
    displayed: Status,
    pending: Option<PendingStatusChange>,
    next_generation: u64,
}

impl BufferedStatus {
    fn new(displayed: Status) -> Self {
        Self {
            displayed,
            pending: None,
            next_generation: 0,
        }
    }

    fn displayed(&self) -> Status {
        self.displayed
    }

    fn observe(&mut self, detected: Status) -> Option<PendingStatusChange> {
        if detected == self.displayed {
            self.pending = None;
            return None;
        }

        if self.pending.is_some_and(|pending| pending.status == detected) {
            return None;
        }

        self.next_generation += 1;
        let pending = PendingStatusChange {
            generation: self.next_generation,
            status: detected,
        };
        self.pending = Some(pending);
        Some(pending)
    }

    fn commit(&mut self, pending: PendingStatusChange) -> bool {
        if self.pending != Some(pending) || self.displayed == pending.status {
            return false;
        }

        self.displayed = pending.status;
        self.pending = None;
        true
    }
}

pub fn run(args: Args) -> Result<i32> {
    let debug = DebugLog::new(args.debug_log.as_deref())?;
    let _raw_mode = RawModeGuard::new()?;

    let (cols, rows) = terminal_size()?;
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("failed to open PTY")?;

    let command = build_command(&args);
    debug.write_line(&format!(
        "launch tool={} cli_bin={} args={:?}",
        args.tool, args.cli_bin, command
    ));

    let mut child = pair
        .slave
        .spawn_command(command)
        .context("failed to spawn wrapped CLI")?;
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;
    let writer = pair
        .master
        .take_writer()
        .context("failed to open PTY writer")?;

    let mut ui = TerminalUi::new(args.title_mode, args.color_mode, args.title_format.clone());
    ui.push_title_stack()?;
    let ui = Arc::new(ui);
    let stdout = ui.stdout();

    let cwd = std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| ".".to_owned());

    let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 10_000)));
    let detector = Arc::new(Mutex::new(StateDetector::new(args.tool)));
    let tool_title = Arc::new(Mutex::new(cwd.clone()));
    let saw_tool_title = Arc::new(AtomicBool::new(false));
    let buffered_status = Arc::new(Mutex::new(BufferedStatus::new(Status::Starting)));
    let update_lock = Arc::new(Mutex::new(()));
    let title_map = Arc::new(args.title_map.clone());
    let color_map = Arc::new(args.color_map.clone());

    render_ui(
        &ui,
        title_map.as_ref(),
        color_map.as_ref(),
        &cwd,
        args.tool,
        Status::Starting,
        tool_title.lock().unwrap().clone(),
    )?;

    let stdout_thread = {
        let stdout = Arc::clone(&stdout);
        let parser = Arc::clone(&parser);
        let detector = Arc::clone(&detector);
        let tool_title = Arc::clone(&tool_title);
        let saw_tool_title = Arc::clone(&saw_tool_title);
        let ui = Arc::clone(&ui);
        let buffered_status = Arc::clone(&buffered_status);
        let update_lock = Arc::clone(&update_lock);
        let title_map = Arc::clone(&title_map);
        let color_map = Arc::clone(&color_map);
        let cwd = cwd.clone();
        let debug = debug.clone();
        let tool = args.tool;

        thread::spawn(move || -> Result<()> {
            let mut reader = reader;
            let mut filter = OscFilter::default();
            let mut buffer = [0_u8; 8192];

            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let filtered = filter.feed(&buffer[..n]);
                        let title_changed = filtered.title.is_some();
                        if let Some(title) = filtered.title.clone() {
                            *tool_title.lock().unwrap() = title;
                            saw_tool_title.store(true, Ordering::Relaxed);
                        }

                        {
                            let mut stdout = stdout.lock().unwrap();
                            stdout.write_all(&filtered.passthrough)?;
                            stdout.flush()?;
                        }

                        if !filtered.passthrough.is_empty() {
                            let mut parser = parser.lock().unwrap();
                            parser.process(&filtered.passthrough);
                        }

                        let screen_text = {
                            let parser = parser.lock().unwrap();
                            parser.screen().contents()
                        };
                        let title_seen = saw_tool_title.load(Ordering::Relaxed);
                        let next_state = detector.lock().unwrap().detect(&screen_text, title_seen);

                        let pending = buffered_status.lock().unwrap().observe(next_state);

                        if let Some(pending) = pending {
                            debug.write_line(&format!(
                                "buffering_state={} delay_ms={}",
                                pending.status.as_str(),
                                STATUS_CHANGE_BUFFER.as_millis()
                            ));
                            let buffered_status = Arc::clone(&buffered_status);
                            let update_lock = Arc::clone(&update_lock);
                            let tool_title = Arc::clone(&tool_title);
                            let ui = Arc::clone(&ui);
                            let title_map = Arc::clone(&title_map);
                            let color_map = Arc::clone(&color_map);
                            let cwd = cwd.clone();
                            let debug = debug.clone();

                            thread::spawn(move || {
                                thread::sleep(STATUS_CHANGE_BUFFER);

                                let _update_guard = update_lock.lock().unwrap();
                                let committed = buffered_status.lock().unwrap().commit(pending);
                                if !committed {
                                    return;
                                }

                                let current_title = tool_title.lock().unwrap().clone();
                                debug.write_line(&format!(
                                    "state={} tool_title={} buffered_ms={}",
                                    pending.status.as_str(),
                                    current_title,
                                    STATUS_CHANGE_BUFFER.as_millis()
                                ));
                                if let Err(err) = render_ui(
                                    &ui,
                                    title_map.as_ref(),
                                    color_map.as_ref(),
                                    &cwd,
                                    tool,
                                    pending.status,
                                    current_title,
                                ) {
                                    debug.write_line(&format!("buffered state update failed: {err}"));
                                }
                            });
                        }

                        if title_changed {
                            let _update_guard = update_lock.lock().unwrap();
                            let displayed_state = buffered_status.lock().unwrap().displayed();
                            let current_title = tool_title.lock().unwrap().clone();
                            render_ui(
                                &ui,
                                title_map.as_ref(),
                                color_map.as_ref(),
                                &cwd,
                                tool,
                                displayed_state,
                                current_title,
                            )?;
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) if err.raw_os_error() == Some(5) => break,
                    Err(err) => return Err(err).context("error reading PTY output"),
                }
            }

            Ok(())
        })
    };

    let stdin_thread = thread::spawn(move || -> Result<()> {
        let mut stdin = io::stdin();
        let mut writer = writer;
        let mut buffer = [0_u8; 4096];

        loop {
            match stdin.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => writer.write_all(&buffer[..n])?,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err).context("error reading stdin"),
            }
        }

        Ok(())
    });

    let resize_thread = spawn_resize_thread(pair.master, Arc::clone(&parser), debug.clone())?;

    let exit_status = child.wait().context("failed waiting for child process")?;
    let _ = stdout_thread
        .join()
        .map_err(|_| anyhow!("stdout thread panicked"))??;
    let _stdin_thread = stdin_thread;
    resize_thread.store(false, Ordering::Relaxed);
    let _ = ui.restore();

    Ok(exit_status.exit_code().try_into().unwrap_or(i32::MAX))
}

#[cfg(not(windows))]
fn spawn_resize_thread(
    master: Box<dyn MasterPty + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    debug: DebugLog,
) -> Result<Arc<AtomicBool>> {
    let resize_flag = Arc::new(AtomicBool::new(false));
    flag::register(SIGWINCH, Arc::clone(&resize_flag)).context("failed to register SIGWINCH")?;

    let keep_running = Arc::new(AtomicBool::new(true));
    let running = Arc::clone(&keep_running);
    thread::spawn(move || -> Result<()> {
        while running.load(Ordering::Relaxed) {
            if resize_flag.swap(false, Ordering::Relaxed) {
                apply_resize(master.as_ref(), parser.as_ref(), &debug)?;
            }
            thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    });

    Ok(keep_running)
}

#[cfg(windows)]
fn spawn_resize_thread(
    master: Box<dyn MasterPty + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    debug: DebugLog,
) -> Result<Arc<AtomicBool>> {
    let keep_running = Arc::new(AtomicBool::new(true));
    let running = Arc::clone(&keep_running);
    thread::spawn(move || -> Result<()> {
        let mut last_size = terminal_size().ok();

        while running.load(Ordering::Relaxed) {
            let current_size = terminal_size().ok();
            if let Some(size) = current_size {
                if last_size != Some(size) {
                    apply_resize(master.as_ref(), parser.as_ref(), &debug)?;
                    last_size = Some(size);
                }
            }
            thread::sleep(Duration::from_millis(150));
        }
        Ok(())
    });

    Ok(keep_running)
}

fn apply_resize(
    master: &dyn MasterPty,
    parser: &Mutex<vt100::Parser>,
    debug: &DebugLog,
) -> Result<()> {
    let (cols, rows) = terminal_size()?;
    master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("failed to resize PTY")?;
    parser.lock().unwrap().screen_mut().set_size(rows, cols);
    debug.write_line(&format!("resize rows={} cols={}", rows, cols));
    Ok(())
}

fn build_command(args: &Args) -> CommandBuilder {
    let mut command = CommandBuilder::new(&args.cli_bin);
    if let Ok(cwd) = std::env::current_dir() {
        command.cwd(cwd);
    }
    if !args.keep_alt_screen
        && args
            .tool
            .should_inject_alt_screen_flag(&args.passthrough_args)
    {
        for extra in args.tool.injected_args() {
            command.arg(extra);
        }
    }
    for arg in &args.passthrough_args {
        command.arg(arg);
    }
    command
}

fn render_ui(
    ui: &TerminalUi,
    title_map: &BTreeMap<Status, String>,
    color_map: &BTreeMap<Status, RgbColor>,
    cwd: &str,
    tool: Tool,
    status: Status,
    tool_title: String,
) -> Result<()> {
    ui.update(
        &TitleContext {
            status,
            state_label: status.default_label().to_owned(),
            title_token: title_map[&status].clone(),
            cwd: cwd.to_owned(),
            tool,
            tool_title,
        },
        color_map.get(&status).copied(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_new_state_until_timer_fires() {
        let mut buffered = BufferedStatus::new(Status::Ready);

        let pending = buffered.observe(Status::Busy);

        assert_eq!(pending.map(|change| change.status), Some(Status::Busy));
        assert_eq!(buffered.displayed(), Status::Ready);
    }

    #[test]
    fn drops_pending_change_when_state_bounces_back() {
        let mut buffered = BufferedStatus::new(Status::Ready);
        let pending = buffered.observe(Status::Busy).unwrap();

        assert_eq!(buffered.displayed(), Status::Ready);
        assert_eq!(buffered.observe(Status::Ready), None);
        assert!(!buffered.commit(pending));
        assert_eq!(buffered.displayed(), Status::Ready);
    }

    #[test]
    fn ignores_stale_timer_after_newer_transition_is_buffered() {
        let mut buffered = BufferedStatus::new(Status::Ready);
        let first = buffered.observe(Status::Busy).unwrap();
        let second = buffered.observe(Status::Error).unwrap();

        assert!(!buffered.commit(first));
        assert!(buffered.commit(second));
        assert_eq!(buffered.displayed(), Status::Error);
    }

    #[test]
    fn build_command_sets_child_cwd_to_wrapper_cwd() {
        let args = Args::default();

        let command = build_command(&args);

        assert_eq!(
            command.get_cwd().map(|cwd| cwd.as_os_str()),
            std::env::current_dir().ok().as_deref().map(|cwd| cwd.as_os_str())
        );
    }

    #[test]
    fn build_command_keeps_codex_default_screen_mode() {
        let args = Args::default();

        let command = build_command(&args);

        let argv: Vec<_> = command
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, vec!["codex"]);
    }

    #[test]
    fn build_command_preserves_explicit_no_alt_screen_passthrough() {
        let args = Args {
            passthrough_args: vec!["--no-alt-screen".into()],
            ..Args::default()
        };

        let command = build_command(&args);

        let argv: Vec<_> = command
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, vec!["codex", "--no-alt-screen"]);
    }
}
