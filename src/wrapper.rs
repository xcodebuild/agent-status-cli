use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
#[cfg(not(windows))]
use signal_hook::consts::signal::SIGWINCH;
#[cfg(not(windows))]
use signal_hook::flag;

use crate::args::{Args, Status};
use crate::osc::OscFilter;
use crate::state::StateDetector;
use crate::terminal::{
    DebugLog, RawModeGuard, RgbColor, TerminalUi, TitleContext, is_iterm2, terminal_size,
};
use crate::tool::Tool;

const STATUS_CHANGE_BUFFER: Duration = Duration::from_millis(500);
const DEFAULT_SCREEN_SCAN_INTERVAL: Duration = Duration::from_millis(125);
const DEFAULT_TITLE_RENDER_DEBOUNCE: Duration = Duration::from_millis(100);
const DEFAULT_OUTPUT_FLUSH_INTERVAL: Duration = Duration::from_millis(8);
const ITERM2_SCREEN_SCAN_INTERVAL: Duration = Duration::from_millis(250);
const ITERM2_TITLE_RENDER_DEBOUNCE: Duration = Duration::from_millis(250);
const ITERM2_OUTPUT_FLUSH_INTERVAL: Duration = Duration::from_millis(32);
const DEFAULT_SCREEN_PARSE_BYTES_THRESHOLD: usize = 16 * 1024;
const ITERM2_SCREEN_PARSE_BYTES_THRESHOLD: usize = 64 * 1024;
const OUTPUT_FLUSH_BYTES_THRESHOLD: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingStatusChange {
    status: Status,
}

#[derive(Debug)]
struct BufferedStatus {
    displayed: Status,
    pending: Option<PendingStatusChange>,
}

impl BufferedStatus {
    fn new(displayed: Status) -> Self {
        Self {
            displayed,
            pending: None,
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

        let pending = PendingStatusChange { status: detected };
        self.pending = Some(pending);
        Some(pending)
    }

    fn commit_pending(&mut self) -> Option<PendingStatusChange> {
        let pending = self.pending?;
        self.displayed = pending.status;
        self.pending = None;
        Some(pending)
    }
}

#[derive(Debug)]
struct WorkerSchedule {
    running: bool,
    pending_screen: Vec<u8>,
    next_scan_at: Option<Instant>,
    next_status_commit_at: Option<Instant>,
    next_title_render_at: Option<Instant>,
}

impl WorkerSchedule {
    fn new() -> Self {
        Self {
            running: true,
            pending_screen: Vec::new(),
            next_scan_at: None,
            next_status_commit_at: None,
            next_title_render_at: None,
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let mut next = self.next_scan_at;
        next = earliest_deadline(next, self.next_status_commit_at);
        earliest_deadline(next, self.next_title_render_at)
    }

    fn enqueue_screen_bytes(&mut self, bytes: &[u8], now: Instant, profile: PerformanceProfile) {
        if bytes.is_empty() {
            return;
        }

        self.pending_screen.extend_from_slice(bytes);
        if self.pending_screen.len() >= profile.screen_parse_bytes_threshold {
            self.next_scan_at = Some(now);
        } else if self.next_scan_at.is_none() {
            self.next_scan_at = Some(now + profile.screen_scan_interval);
        }
    }

    fn schedule_status_commit(&mut self, now: Instant) {
        self.next_status_commit_at = Some(now + STATUS_CHANGE_BUFFER);
    }

    fn schedule_title_render(&mut self, now: Instant, interval: Duration) {
        if self.next_title_render_at.is_none() {
            self.next_title_render_at = Some(now + interval);
        }
    }
}

#[derive(Clone)]
struct WorkerActions {
    screen_bytes: Option<Vec<u8>>,
    render_status: bool,
    render_title: bool,
}

type WorkerSignal = Arc<(Mutex<WorkerSchedule>, Condvar)>;
type WorkerSync = (Mutex<WorkerSchedule>, Condvar);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PerformanceProfile {
    screen_scan_interval: Duration,
    screen_parse_bytes_threshold: usize,
    title_render_debounce: Duration,
    output_flush_interval: Duration,
    output_flush_bytes_threshold: usize,
}

impl PerformanceProfile {
    fn for_terminal(is_iterm2: bool) -> Self {
        if is_iterm2 {
            Self {
                screen_scan_interval: ITERM2_SCREEN_SCAN_INTERVAL,
                screen_parse_bytes_threshold: ITERM2_SCREEN_PARSE_BYTES_THRESHOLD,
                title_render_debounce: ITERM2_TITLE_RENDER_DEBOUNCE,
                output_flush_interval: ITERM2_OUTPUT_FLUSH_INTERVAL,
                output_flush_bytes_threshold: OUTPUT_FLUSH_BYTES_THRESHOLD,
            }
        } else {
            Self {
                screen_scan_interval: DEFAULT_SCREEN_SCAN_INTERVAL,
                screen_parse_bytes_threshold: DEFAULT_SCREEN_PARSE_BYTES_THRESHOLD,
                title_render_debounce: DEFAULT_TITLE_RENDER_DEBOUNCE,
                output_flush_interval: DEFAULT_OUTPUT_FLUSH_INTERVAL,
                output_flush_bytes_threshold: OUTPUT_FLUSH_BYTES_THRESHOLD,
            }
        }
    }
}

#[derive(Debug)]
struct OutputBuffer {
    running: bool,
    pending: Vec<u8>,
    next_flush_at: Option<Instant>,
}

impl OutputBuffer {
    fn new() -> Self {
        Self {
            running: true,
            pending: Vec::new(),
            next_flush_at: None,
        }
    }

    fn enqueue(&mut self, bytes: &[u8], profile: PerformanceProfile, now: Instant) {
        if bytes.is_empty() {
            return;
        }

        self.pending.extend_from_slice(bytes);
        if self.pending.len() >= profile.output_flush_bytes_threshold {
            self.next_flush_at = Some(now);
        } else if self.next_flush_at.is_none() {
            self.next_flush_at = Some(now + profile.output_flush_interval);
        }
    }

    fn take_pending(&mut self) -> Option<Vec<u8>> {
        if self.pending.is_empty() {
            return None;
        }

        self.next_flush_at = None;
        Some(std::mem::take(&mut self.pending))
    }
}

type OutputSignal = Arc<(Mutex<OutputBuffer>, Condvar)>;
type OutputSync = (Mutex<OutputBuffer>, Condvar);

pub fn run(args: Args) -> Result<i32> {
    let debug = DebugLog::new(args.debug_log.as_deref())?;
    let _raw_mode = RawModeGuard::new()?;
    let profile = PerformanceProfile::for_terminal(is_iterm2());

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
    let worker_signal: WorkerSignal = Arc::new((Mutex::new(WorkerSchedule::new()), Condvar::new()));
    let output_signal: OutputSignal = Arc::new((Mutex::new(OutputBuffer::new()), Condvar::new()));

    debug.write_line(&format!(
        "performance_profile screen_scan_ms={} screen_parse_threshold={} title_debounce_ms={} output_flush_ms={} output_flush_threshold={}",
        profile.screen_scan_interval.as_millis(),
        profile.screen_parse_bytes_threshold,
        profile.title_render_debounce.as_millis(),
        profile.output_flush_interval.as_millis(),
        profile.output_flush_bytes_threshold
    ));

    render_ui(
        &ui,
        title_map.as_ref(),
        color_map.as_ref(),
        &cwd,
        args.tool,
        Status::Starting,
        tool_title.lock().unwrap().as_str(),
    )?;

    let state_thread = {
        let parser = Arc::clone(&parser);
        let detector = Arc::clone(&detector);
        let tool_title = Arc::clone(&tool_title);
        let saw_tool_title = Arc::clone(&saw_tool_title);
        let ui = Arc::clone(&ui);
        let buffered_status = Arc::clone(&buffered_status);
        let update_lock = Arc::clone(&update_lock);
        let title_map = Arc::clone(&title_map);
        let color_map = Arc::clone(&color_map);
        let worker_signal = Arc::clone(&worker_signal);
        let cwd = cwd.clone();
        let debug = debug.clone();
        let tool = args.tool;

        thread::spawn(move || -> Result<()> {
            loop {
                let actions = wait_for_worker_actions(worker_signal.as_ref());
                if actions.screen_bytes.is_none() && !actions.render_status && !actions.render_title {
                    break;
                }

                if let Some(screen_bytes) = actions.screen_bytes {
                    let screen_text = {
                        let mut parser = parser.lock().unwrap();
                        parser.process(&screen_bytes);
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
                        schedule_status_commit(worker_signal.as_ref());
                    }
                }

                if actions.render_status {
                    let Some(pending) = buffered_status.lock().unwrap().commit_pending() else {
                        continue;
                    };
                    let _update_guard = update_lock.lock().unwrap();
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
                        &current_title,
                    ) {
                        debug.write_line(&format!("buffered state update failed: {err}"));
                    }
                }

                if actions.render_title && !actions.render_status {
                    let _update_guard = update_lock.lock().unwrap();
                    let displayed_state = buffered_status.lock().unwrap().displayed();
                    let current_title = tool_title.lock().unwrap().clone();
                    if let Err(err) = render_ui(
                        &ui,
                        title_map.as_ref(),
                        color_map.as_ref(),
                        &cwd,
                        tool,
                        displayed_state,
                        &current_title,
                    ) {
                        debug.write_line(&format!("debounced title update failed: {err}"));
                    }
                }
            }

            Ok(())
        })
    };

    let output_thread = {
        let stdout = Arc::clone(&stdout);
        let output_signal = Arc::clone(&output_signal);

        thread::spawn(move || run_output_worker(stdout, output_signal.as_ref()))
    };

    let stdout_thread = {
        let tool_title = Arc::clone(&tool_title);
        let saw_tool_title = Arc::clone(&saw_tool_title);
        let worker_signal = Arc::clone(&worker_signal);
        let output_signal = Arc::clone(&output_signal);
        let profile = profile;

        thread::spawn(move || -> Result<()> {
            let mut reader = reader;
            let mut filter = OscFilter::default();
            let mut buffer = [0_u8; 8192];

            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let filtered = filter.feed(&buffer[..n]);
                        let mut title_changed = false;
                        if let Some(title) = filtered.title {
                            let mut current_title = tool_title.lock().unwrap();
                            if *current_title != title {
                                *current_title = title;
                                title_changed = true;
                            }
                            saw_tool_title.store(true, Ordering::Relaxed);
                        }

                        queue_output(output_signal.as_ref(), &filtered.passthrough, profile);

                        if !filtered.passthrough.is_empty() {
                            schedule_screen_scan(
                                worker_signal.as_ref(),
                                &filtered.passthrough,
                                profile,
                            );
                        }

                        if title_changed {
                            schedule_title_render(
                                worker_signal.as_ref(),
                                profile.title_render_debounce,
                            );
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
    stop_output(output_signal.as_ref());
    let _ = output_thread
        .join()
        .map_err(|_| anyhow!("output thread panicked"))??;
    stop_worker(worker_signal.as_ref());
    let _ = state_thread
        .join()
        .map_err(|_| anyhow!("state thread panicked"))??;
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
    tool_title: &str,
) -> Result<()> {
    ui.update(
        &TitleContext {
            status,
            state_label: status.default_label(),
            title_token: title_map[&status].as_str(),
            cwd,
            tool,
            tool_title,
        },
        color_map.get(&status).copied(),
    )
}

fn wait_for_worker_actions(worker_signal: &WorkerSync) -> WorkerActions {
    let (lock, condvar) = worker_signal;
    let mut schedule = lock.lock().unwrap();

    loop {
        let now = Instant::now();
        let scan_screen = schedule.next_scan_at.is_some_and(|deadline| deadline <= now);
        let render_status = schedule
                .next_status_commit_at
                .is_some_and(|deadline| deadline <= now);
        let render_title = schedule
                .next_title_render_at
                .is_some_and(|deadline| deadline <= now);

        if scan_screen || render_status || render_title {
            let screen_bytes = if scan_screen {
                schedule.next_scan_at = None;
                Some(std::mem::take(&mut schedule.pending_screen))
            } else {
                None
            };
            if render_status {
                schedule.next_status_commit_at = None;
            }
            if render_title {
                schedule.next_title_render_at = None;
            }
            return WorkerActions {
                screen_bytes,
                render_status,
                render_title,
            };
        }

        if !schedule.running {
            return WorkerActions {
                screen_bytes: None,
                render_status: false,
                render_title: false,
            };
        }

        if let Some(deadline) = schedule.next_deadline() {
            let timeout = deadline.saturating_duration_since(now);
            let (next_schedule, _) = condvar.wait_timeout(schedule, timeout).unwrap();
            schedule = next_schedule;
        } else {
            schedule = condvar.wait(schedule).unwrap();
        }
    }
}

fn schedule_screen_scan(worker_signal: &WorkerSync, bytes: &[u8], profile: PerformanceProfile) {
    if bytes.is_empty() {
        return;
    }

    let (lock, condvar) = worker_signal;
    let mut schedule = lock.lock().unwrap();
    schedule.enqueue_screen_bytes(bytes, Instant::now(), profile);
    condvar.notify_one();
}

fn schedule_status_commit(worker_signal: &WorkerSync) {
    let (lock, condvar) = worker_signal;
    let mut schedule = lock.lock().unwrap();
    schedule.schedule_status_commit(Instant::now());
    condvar.notify_one();
}

fn schedule_title_render(worker_signal: &WorkerSync, interval: Duration) {
    let (lock, condvar) = worker_signal;
    let mut schedule = lock.lock().unwrap();
    schedule.schedule_title_render(Instant::now(), interval);
    condvar.notify_one();
}

fn stop_worker(worker_signal: &WorkerSync) {
    let (lock, condvar) = worker_signal;
    let mut schedule = lock.lock().unwrap();
    schedule.running = false;
    condvar.notify_one();
}

fn run_output_worker(stdout: Arc<Mutex<io::Stdout>>, output_signal: &OutputSync) -> Result<()> {
    while let Some(chunk) = wait_for_output_chunk(output_signal) {
        let mut stdout = stdout.lock().unwrap();
        stdout.write_all(&chunk)?;
        stdout.flush()?;
    }

    Ok(())
}

fn wait_for_output_chunk(output_signal: &OutputSync) -> Option<Vec<u8>> {
    let (lock, condvar) = output_signal;
    let mut output = lock.lock().unwrap();

    loop {
        let now = Instant::now();
        let flush_due = output.next_flush_at.is_some_and(|deadline| deadline <= now);

        if (!output.running || flush_due) && !output.pending.is_empty() {
            return output.take_pending();
        }

        if !output.running {
            return None;
        }

        if let Some(deadline) = output.next_flush_at {
            let timeout = deadline.saturating_duration_since(now);
            let (next_output, _) = condvar.wait_timeout(output, timeout).unwrap();
            output = next_output;
        } else {
            output = condvar.wait(output).unwrap();
        }
    }
}

fn queue_output(output_signal: &OutputSync, bytes: &[u8], profile: PerformanceProfile) {
    if bytes.is_empty() {
        return;
    }

    let (lock, condvar) = output_signal;
    let mut output = lock.lock().unwrap();
    output.enqueue(bytes, profile, Instant::now());
    condvar.notify_one();
}

fn stop_output(output_signal: &OutputSync) {
    let (lock, condvar) = output_signal;
    let mut output = lock.lock().unwrap();
    output.running = false;
    condvar.notify_one();
}

fn earliest_deadline(current: Option<Instant>, next: Option<Instant>) -> Option<Instant> {
    match (current, next) {
        (Some(current), Some(next)) => Some(current.min(next)),
        (Some(current), None) => Some(current),
        (None, Some(next)) => Some(next),
        (None, None) => None,
    }
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
        let _pending = buffered.observe(Status::Busy).unwrap();

        assert_eq!(buffered.displayed(), Status::Ready);
        assert_eq!(buffered.observe(Status::Ready), None);
        assert_eq!(buffered.commit_pending(), None);
        assert_eq!(buffered.displayed(), Status::Ready);
    }

    #[test]
    fn commits_latest_pending_state() {
        let mut buffered = BufferedStatus::new(Status::Ready);
        let _first = buffered.observe(Status::Busy).unwrap();
        let second = buffered.observe(Status::Error).unwrap();

        assert_eq!(buffered.commit_pending(), Some(second));
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

    #[test]
    fn uses_more_conservative_profile_for_iterm2() {
        let profile = PerformanceProfile::for_terminal(true);

        assert_eq!(profile.screen_scan_interval, ITERM2_SCREEN_SCAN_INTERVAL);
        assert_eq!(
            profile.screen_parse_bytes_threshold,
            ITERM2_SCREEN_PARSE_BYTES_THRESHOLD
        );
        assert_eq!(profile.title_render_debounce, ITERM2_TITLE_RENDER_DEBOUNCE);
        assert_eq!(profile.output_flush_interval, ITERM2_OUTPUT_FLUSH_INTERVAL);
        assert_eq!(
            profile.output_flush_bytes_threshold,
            OUTPUT_FLUSH_BYTES_THRESHOLD
        );
    }

    #[test]
    fn worker_schedule_batches_screen_updates_until_deadline() {
        let mut schedule = WorkerSchedule::new();
        let profile = PerformanceProfile::for_terminal(false);
        let now = Instant::now();

        schedule.enqueue_screen_bytes(b"abc", now, profile);

        assert_eq!(schedule.pending_screen, b"abc");
        assert_eq!(schedule.next_scan_at, Some(now + profile.screen_scan_interval));
    }

    #[test]
    fn worker_schedule_flushes_large_screen_batches_immediately() {
        let mut schedule = WorkerSchedule::new();
        let profile = PerformanceProfile {
            screen_parse_bytes_threshold: 4,
            ..PerformanceProfile::for_terminal(false)
        };
        let now = Instant::now();

        schedule.enqueue_screen_bytes(b"abcd", now, profile);

        assert_eq!(schedule.pending_screen, b"abcd");
        assert_eq!(schedule.next_scan_at, Some(now));
    }

    #[test]
    fn output_buffer_batches_small_writes_until_deadline() {
        let mut output = OutputBuffer::new();
        let profile = PerformanceProfile::for_terminal(false);
        let now = Instant::now();

        output.enqueue(b"abc", profile, now);

        assert_eq!(output.pending, b"abc");
        assert_eq!(
            output.next_flush_at,
            Some(now + profile.output_flush_interval)
        );
    }

    #[test]
    fn output_buffer_flushes_large_batches_immediately() {
        let mut output = OutputBuffer::new();
        let profile = PerformanceProfile {
            output_flush_bytes_threshold: 4,
            ..PerformanceProfile::for_terminal(false)
        };
        let now = Instant::now();

        output.enqueue(b"abcd", profile, now);

        assert_eq!(output.pending, b"abcd");
        assert_eq!(output.next_flush_at, Some(now));
        assert_eq!(output.take_pending(), Some(b"abcd".to_vec()));
        assert_eq!(output.next_flush_at, None);
    }
}
