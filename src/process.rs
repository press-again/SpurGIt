//! Bounded-concurrency process runner.
//!
//! Commands are spawned as argument arrays (never shell strings) with
//! concurrent stdout/stderr drains, a captured exit status, and a bounded,
//! display-sanitized log tail. Repository selection is explicit: Git commands
//! carry `-C <worktree>` instead of relying on an inherited working
//! directory. `Batch` bounds how many commands run at once and can cancel
//! queued and running jobs; terminating a `wsl.exe` child tears down the
//! Linux session it owns, including SSH and credential helpers.
//!
//! Nothing here touches GPUI: callers run jobs from a background executor.
#![allow(dead_code)]

use std::borrow::Cow;
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::process::{Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

/// Conservative default global job cap when `GIS_JOBS` is absent (CONCEPT
/// "Concurrency and cancellation").
pub const DEFAULT_JOBS: usize = 4;

/// Default retained UI-log bytes per job. Older bytes are dropped; the final
/// error stays visible because the tail is retained.
pub const DEFAULT_LOG_TAIL: usize = 64 * 1024;

/// Bounded, sanitized tail of a job's combined output, for UI display.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogTail {
    pub text: String,
    /// Bytes dropped from the front because the tail cap was exceeded.
    pub omitted_bytes: u64,
}

impl LogTail {
    /// Display form, with a visible marker when earlier output was dropped.
    pub fn display(&self) -> String {
        if self.omitted_bytes == 0 {
            self.text.clone()
        } else {
            format!(
                "… {} bytes of earlier output omitted …\n{}",
                self.omitted_bytes, self.text
            )
        }
    }
}

/// Result of one finished (or cancelled) command.
#[derive(Debug, Clone, Default)]
pub struct Output {
    /// Process exit code; `None` when terminated without one.
    pub status: Option<i32>,
    /// The job was killed by `Job::cancel`, not by Git.
    pub cancelled: bool,
    /// The job reached `spawn`; a queued job cancelled before start is false.
    pub started: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// A capture limit dropped output bytes (the child was still drained to
    /// completion, so it could never block on a full pipe).
    pub capture_truncated: bool,
    /// Bounded, display-safe tail of stdout+stderr in arrival order.
    pub log: LogTail,
}

impl Output {
    /// A job cancelled before it ever started.
    pub fn not_started() -> Self {
        Self { cancelled: true, ..Default::default() }
    }

    pub fn success(&self) -> bool {
        self.started && !self.cancelled && self.status == Some(0)
    }

    pub fn stdout_text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.stdout)
    }

    pub fn stderr_text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.stderr)
    }

    /// One-line context for callers that only surface failures.
    pub fn error_context(&self) -> String {
        if self.cancelled {
            return if self.started {
                "cancelled".into()
            } else {
                "cancelled before start".into()
            };
        }
        let detail = sanitize(&self.stderr);
        let detail = detail.trim();
        match (self.status, detail.is_empty()) {
            (Some(code), false) => format!("exit {code}: {detail}"),
            (Some(code), true) => format!("exit {code}"),
            (None, false) => detail.to_string(),
            (None, true) => "process ended without an exit status".into(),
        }
    }
}

/// An argument-array command to run, plus its log-tail bound.
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    env_remove: Vec<OsString>,
    /// Data fed to the child's stdin; `None` closes stdin immediately.
    stdin: Option<Vec<u8>>,
    log_tail: usize,
    /// Cap on retained stdout/stderr bytes (`None` = unbounded). Preview
    /// commands use it so a huge diff is not captured whole before the parser
    /// applies its own limit; excess output is still drained and discarded.
    capture: Option<usize>,
    /// Publish stderr live through [`progress_lines`] while the command runs.
    progress: bool,
}

impl Command {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_os_string(),
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
            stdin: None,
            log_tail: DEFAULT_LOG_TAIL,
            capture: None,
            progress: false,
        }
    }

    /// Show this command's `--progress` output in the busy dialog.
    pub fn live_progress(mut self) -> Self {
        self.progress = true;
        self
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args.extend(args.into_iter().map(|a| a.as_ref().to_os_string()));
        self
    }

    /// Set a variable in the spawned process's environment (in addition to the
    /// inherited environment).
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env.push((key.as_ref().to_os_string(), value.as_ref().to_os_string()));
        self
    }

    /// Remove an inherited variable from the spawned process's environment.
    pub fn env_remove(mut self, key: impl AsRef<OsStr>) -> Self {
        self.env_remove.push(key.as_ref().to_os_string());
        self
    }

    /// Feed `data` to the child's stdin (then end it). Lets callers move
    /// arguments off the command line — a Windows `CreateProcess` limit that
    /// ref-heavy `git log --stdin` calls would otherwise hit (os error 206).
    pub fn stdin(mut self, data: Vec<u8>) -> Self {
        self.stdin = Some(data);
        self
    }

    pub fn log_tail_bytes(mut self, bytes: usize) -> Self {
        self.log_tail = bytes;
        self
    }

    /// Retain at most `bytes` of stdout/stderr per stream (for preview
    /// commands whose consumers apply their own limit). Excess output is
    /// drained and dropped so the child cannot block on a full pipe.
    pub fn capture_limit(mut self, bytes: usize) -> Self {
        self.capture = Some(bytes);
        self
    }

    /// Run in its own worker thread and return immediately.
    pub fn spawn(self) -> Job {
        let job = Job::new();
        let worker = job.clone();
        thread::spawn(move || {
            register_running(&worker);
            let result = self.run(&worker);
            unregister_running(&worker);
            journal_record(&self, &result);
            worker.publish(result);
        });
        job
    }

    /// Run to completion on the calling thread. The result is moved out, not
    /// cloned (see [`Job::take_output`]).
    pub fn output(self) -> io::Result<Output> {
        self.spawn().take_output()
    }

    fn run(&self, job: &Job) -> io::Result<Output> {
        if self.progress {
            PROGRESS.lock().unwrap().clear();
        }
        let mut child = self.std_command().spawn()?;
        job.set_pid(child.id());
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let tail = Arc::new(Mutex::new(TailBuf::new(self.log_tail)));
        let out_reader = spawn_drain(stdout, tail.clone(), self.capture, false);
        let err_reader = spawn_drain(stderr, tail.clone(), self.capture, self.progress);
        if let Some(data) = &self.stdin {
            // Feed only after the output drains are running, so a burst of
            // child output cannot deadlock against this write.
            if let Some(mut pipe) = child.stdin.take() {
                let _ = pipe.write_all(data);
            }
        }
        let status = child.wait();
        let (stdout, out_dropped) = join_drain(out_reader)?;
        let (mut stderr, err_dropped) = join_drain(err_reader)?;
        if self.progress {
            PROGRESS.lock().unwrap().clear();
            // Errors and the operation log only need the final state of
            // each progress line.
            stderr = collapse_redraws(&stderr);
        }
        let status = status?;
        let log = tail.lock().unwrap().finish();
        Ok(Output {
            status: status.code(),
            cancelled: job.was_killed(),
            started: true,
            stdout,
            stderr,
            capture_truncated: out_dropped || err_dropped,
            log,
        })
    }

    fn std_command(&self) -> StdCommand {
        let mut c = StdCommand::new(&self.program);
        c.args(&self.args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in &self.env_remove {
            c.env_remove(key);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // GUI launches have no console; hide the per-command console flash.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            c.creation_flags(CREATE_NO_WINDOW);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Own process group, so a cancel can kill Git's helpers (ssh,
            // credential helpers) that would otherwise keep the pipes open.
            c.process_group(0);
        }
        c
    }
}

/// Retained output per stream in one journal record; the panel caps what it
/// shows anyway, and preview commands can capture megabytes.
pub const JOURNAL_STREAM_CAP: usize = 4096;
/// Ring size. A workspace scan storm can turn this over mid-operation, so
/// panel entries snapshot their records at completion instead of ids.
pub const JOURNAL_CAP: usize = 200;

/// One finished command for the operation log: the exact argv plus capped
/// output, so a failure carries its own evidence.
#[derive(Debug, Clone)]
pub struct CommandRecord {
    pub id: u64,
    pub argv: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
    pub success: bool,
    pub cancelled: bool,
}

struct Journal {
    next_id: u64,
    records: std::collections::VecDeque<CommandRecord>,
}

static JOURNAL: Mutex<Journal> = Mutex::new(Journal {
    next_id: 0,
    records: std::collections::VecDeque::new(),
});

/// Currently running `spawn()` command, if any. Pump cancel kills
/// whatever sits here: usually the queued operation (refresh Batch queries
/// deliberately never register). Best-effort by construction — a diff
/// reload racing the click dies instead, and simply reloads.
static RUNNING: Mutex<Option<Job>> = Mutex::new(None);

fn register_running(job: &Job) {
    *RUNNING.lock().unwrap() = Some(job.clone());
}

fn unregister_running(job: &Job) {
    let mut slot = RUNNING.lock().unwrap();
    if slot
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(&current.inner, &job.inner))
    {
        *slot = None;
    }
}

/// Live stderr of the running [`Command::live_progress`] command. One slot is
/// enough: dialog operations run one at a time.
static PROGRESS: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// Only the last lines are shown, so older output can go.
const PROGRESS_CAP: usize = 8 * 1024;

/// The last `max` non-empty lines of the running command's progress, each in
/// its latest redrawn state.
pub fn progress_lines(max: usize) -> Vec<String> {
    let text = sanitize(&collapse_redraws(&PROGRESS.lock().unwrap()));
    let lines: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    lines[lines.len().saturating_sub(max)..].to_vec()
}

/// Terminal-style carriage returns: text after a `\r` replaces the current
/// line, so a progress counter keeps only its latest state.
fn collapse_redraws(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut line_start = 0;
    let mut rest = bytes.iter().peekable();
    while let Some(&byte) = rest.next() {
        match byte {
            b'\r' if rest.peek().is_some_and(|&&next| next != b'\n') => {
                out.truncate(line_start)
            }
            b'\r' => {}
            b'\n' => {
                out.push(b'\n');
                line_start = out.len();
            }
            _ => out.push(byte),
        }
    }
    out
}

/// Bumped on every cancel request, so multi-step work (backup capture, then
/// the mutation) can notice a cancel that killed one of its earlier commands.
static CANCEL_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Current cancel generation; compare a snapshot before each further step.
pub fn cancel_generation() -> u64 {
    CANCEL_GEN.load(std::sync::atomic::Ordering::SeqCst)
}

/// Kill the registered running command, if there is one.
pub fn cancel_running_command() -> bool {
    CANCEL_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let job = RUNNING.lock().unwrap().take();
    match job {
        Some(job) => {
            job.cancel();
            true
        }
        None => false,
    }
}

fn capped(bytes: &[u8]) -> (String, bool) {
    if bytes.len() > JOURNAL_STREAM_CAP {
        (
            String::from_utf8_lossy(&bytes[..JOURNAL_STREAM_CAP]).into_owned(),
            true,
        )
    } else {
        (String::from_utf8_lossy(bytes).into_owned(), false)
    }
}

fn journal_record(command: &Command, result: &io::Result<Output>) {
    let mut argv = vec![command.program.to_string_lossy().into_owned()];
    argv.extend(
        command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned()),
    );
    let (stdout, stderr, truncated, success, cancelled) = match result {
        Ok(out) => {
            let (stdout, out_dropped) = capped(&out.stdout);
            let (stderr, err_dropped) = capped(&out.stderr);
            (
                stdout,
                stderr,
                out_dropped || err_dropped || out.capture_truncated,
                out.success(),
                out.cancelled,
            )
        }
        Err(err) => (String::new(), err.to_string(), false, false, false),
    };
    journal_push(argv, stdout, stderr, truncated, success, cancelled);
}

fn journal_push(
    argv: Vec<String>,
    stdout: String,
    stderr: String,
    truncated: bool,
    success: bool,
    cancelled: bool,
) -> u64 {
    let mut journal = JOURNAL.lock().unwrap();
    journal.next_id += 1;
    let id = journal.next_id;
    journal.records.push_back(CommandRecord {
        id,
        argv,
        stdout,
        stderr,
        truncated,
        success,
        cancelled,
    });
    while journal.records.len() > JOURNAL_CAP {
        journal.records.pop_front();
    }
    id
}

/// Assigned ids so far; a caller snapshots `journal_since(len)` around its
/// own work to attribute the commands it spawned.
pub fn journal_len() -> u64 {
    JOURNAL.lock().unwrap().next_id
}

/// Records assigned after `id`, oldest first.
pub fn journal_since(id: u64) -> Vec<CommandRecord> {
    JOURNAL
        .lock()
        .unwrap()
        .records
        .iter()
        .filter(|record| record.id > id)
        .cloned()
        .collect()
}

/// `journal_since` narrowed to one repository: git calls carry
/// `-C <worktree>`, host reads carry paths under it. Both Linux and Windows
/// spellings match: native Windows Git receives translated (`C:\…`) paths,
/// so a Linux-only comparison misses every fast-path call.
pub fn journal_since_for(id: u64, worktree: &str) -> Vec<CommandRecord> {
    let prefix = format!("{worktree}/");
    journal_since(id)
        .into_iter()
        .filter(|record| {
            record.argv.iter().any(|arg| {
                arg == worktree
                    || arg.starts_with(&prefix)
                    || crate::model::to_linux_path(arg)
                        .is_some_and(|linux| linux == worktree || linux.starts_with(&prefix))
            })
        })
        .collect()
}

/// Drain one stream. Retains at most `capture` bytes when a cap is set (the
/// second tuple element reports whether bytes were dropped); the rest is read
/// and discarded so the child never blocks on a full pipe. A `live` stream is
/// also published to [`progress_lines`] as it arrives.
fn spawn_drain(
    mut reader: impl Read + Send + 'static,
    tail: Arc<Mutex<TailBuf>>,
    capture: Option<usize>,
    live: bool,
) -> thread::JoinHandle<io::Result<(Vec<u8>, bool)>> {
    thread::spawn(move || {
        let mut raw = Vec::new();
        let mut dropped = false;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    match capture {
                        Some(limit) if raw.len() >= limit => dropped = true,
                        Some(limit) => {
                            let room = limit - raw.len();
                            if n > room {
                                raw.extend_from_slice(&buf[..room]);
                                dropped = true;
                            } else {
                                raw.extend_from_slice(&buf[..n]);
                            }
                        }
                        None => raw.extend_from_slice(&buf[..n]),
                    }
                    tail.lock().unwrap().push(&buf[..n]);
                    if live {
                        let mut progress = PROGRESS.lock().unwrap();
                        progress.extend_from_slice(&buf[..n]);
                        let excess = progress.len().saturating_sub(PROGRESS_CAP);
                        progress.drain(..excess);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok((raw, dropped))
    })
}

fn join_drain(
    handle: thread::JoinHandle<io::Result<(Vec<u8>, bool)>>,
) -> io::Result<(Vec<u8>, bool)> {
    match handle.join() {
        Ok(result) => result,
        Err(_) => Err(io::Error::other("stream reader panicked")),
    }
}

/// Handle to one running, queued, or finished command.
#[derive(Clone)]
pub struct Job {
    inner: Arc<JobInner>,
}

struct JobInner {
    shared: Mutex<Option<Result<Output, String>>>,
    done: Condvar,
    /// Set when the worker published its result, whether or not a caller has
    /// consumed it (a taken result is `shared == None` again, so completion
    /// cannot be derived from that).
    finished: AtomicBool,
    cancel_requested: AtomicBool,
    killed: AtomicBool,
    pid: AtomicU32,
}

impl Job {
    fn new() -> Self {
        Self {
            inner: Arc::new(JobInner {
                shared: Mutex::new(None),
                done: Condvar::new(),
                finished: AtomicBool::new(false),
                cancel_requested: AtomicBool::new(false),
                killed: AtomicBool::new(false),
                pid: AtomicU32::new(0),
            }),
        }
    }

    /// Ask the running process tree to terminate. A queued job never starts.
    pub fn cancel(&self) {
        self.request_cancel();
        self.kill_running();
    }

    /// Flag only, without killing: `Batch::cancel` flags every job before the
    /// first kill so a freed permit cannot start a queued job.
    fn request_cancel(&self) {
        self.inner.cancel_requested.store(true, Ordering::SeqCst);
    }

    fn kill_running(&self) {
        let pid = self.inner.pid.load(Ordering::SeqCst);
        if pid != 0 {
            // Mark before killing: the worker may observe the exit as soon as
            // the kill lands.
            self.inner.killed.store(true, Ordering::SeqCst);
            kill_pid(pid);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancel_requested.load(Ordering::SeqCst)
    }

    /// Wait for the job to finish or be cancelled before start.
    pub fn wait(&self) -> io::Result<Output> {
        let mut guard = self.inner.shared.lock().unwrap();
        while !self.inner.finished.load(Ordering::SeqCst) {
            guard = self.inner.done.wait(guard).unwrap();
        }
        match guard.clone() {
            Some(Ok(output)) => Ok(output),
            Some(Err(message)) => Err(io::Error::other(message)),
            None => Err(io::Error::other("command output already taken")),
        }
    }

    /// [`wait`](Self::wait) that moves the result out instead of cloning it —
    /// important for one-shot callers, where the clone would briefly retain a
    /// second copy of a large stdout buffer. A second call reports that the
    /// output was already taken; repeatable readers use `wait`.
    pub fn take_output(&self) -> io::Result<Output> {
        let mut guard = self.inner.shared.lock().unwrap();
        while !self.inner.finished.load(Ordering::SeqCst) {
            guard = self.inner.done.wait(guard).unwrap();
        }
        match guard.take() {
            Some(Ok(output)) => Ok(output),
            Some(Err(message)) => Err(io::Error::other(message)),
            None => Err(io::Error::other("command output already taken")),
        }
    }

    fn set_pid(&self, pid: u32) {
        self.inner.pid.store(pid, Ordering::SeqCst);
        if self.is_cancelled() {
            self.inner.killed.store(true, Ordering::SeqCst);
            kill_pid(pid);
        }
    }

    fn was_killed(&self) -> bool {
        self.inner.killed.load(Ordering::SeqCst)
    }

    fn publish(&self, result: io::Result<Output>) {
        let mut guard = self.inner.shared.lock().unwrap();
        if guard.is_none() {
            *guard = Some(result.map_err(|e| e.to_string()));
        }
        self.inner.finished.store(true, Ordering::SeqCst);
        self.inner.done.notify_all();
    }
}

#[cfg(windows)]
fn kill_pid(pid: u32) {
    use std::os::windows::process::CommandExt as _;
    // /T also reaps Windows-side descendants so no writer keeps the pipes open.
    // CREATE_NO_WINDOW: the helper must not flash a console window.
    let _ = StdCommand::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .status();
}

#[cfg(not(windows))]
fn kill_pid(pid: u32) {
    // Negative pid = the whole process group created in `std_command`.
    let _ = StdCommand::new("kill")
        .args(["-9", "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// A group of jobs sharing one concurrency cap. `cancel` cancels the jobs
/// submitted so far; the batch stays usable afterwards.
pub struct Batch {
    inner: Arc<BatchInner>,
}

struct BatchInner {
    permits: Mutex<usize>,
    permit_ready: Condvar,
    jobs: Mutex<Vec<Job>>,
}

impl Batch {
    pub fn new(max_jobs: usize) -> Self {
        Self {
            inner: Arc::new(BatchInner {
                permits: Mutex::new(max_jobs.max(1)),
                permit_ready: Condvar::new(),
                jobs: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Queue a command, returning immediately.
    pub fn submit(&self, command: Command) -> Job {
        let job = Job::new();
        self.inner.jobs.lock().unwrap().push(job.clone());
        let inner = self.inner.clone();
        let worker = job.clone();
        thread::spawn(move || {
            let ready = {
                let mut permits = inner.permits.lock().unwrap();
                loop {
                    if worker.is_cancelled() {
                        break false;
                    }
                    if *permits > 0 {
                        *permits -= 1;
                        break true;
                    }
                    permits = inner.permit_ready.wait(permits).unwrap();
                }
            };
            if !ready {
                worker.publish(Ok(Output::not_started()));
                return;
            }
            if worker.is_cancelled() {
                // Cancelled between taking the permit and starting.
                *inner.permits.lock().unwrap() += 1;
                inner.permit_ready.notify_one();
                worker.publish(Ok(Output::not_started()));
                return;
            }
            let result = command.run(&worker);
            unregister_running(&worker);
            journal_record(&command, &result);
            *inner.permits.lock().unwrap() += 1;
            inner.permit_ready.notify_one();
            worker.publish(result);
        });
        job
    }

    /// Cancel queued and running jobs submitted so far.
    pub fn cancel(&self) {
        let jobs = self.inner.jobs.lock().unwrap().clone();
        for job in &jobs {
            job.request_cancel();
        }
        for job in &jobs {
            job.kill_running();
        }
        self.inner.permit_ready.notify_all();
    }
}

/// `GIS_JOBS` parsing: absent means [`DEFAULT_JOBS`]; zero, negative, empty,
/// and malformed values are rejected visibly (CONCEPT).
pub fn parse_jobs(raw: Option<&str>) -> Result<usize, String> {
    match raw.map(str::trim) {
        None => Ok(DEFAULT_JOBS),
        Some("") => Err("GIS_JOBS is empty; expected a positive number".into()),
        Some(value) => match value.parse::<i64>() {
            Ok(n) if n > 0 => Ok(n as usize),
            Ok(n) => Err(format!("GIS_JOBS must be positive, got {n}")),
            Err(_) => Err(format!("GIS_JOBS must be a positive number, got {value:?}")),
        },
    }
}

pub fn jobs_from_env() -> Result<usize, String> {
    parse_jobs(std::env::var("GIS_JOBS").ok().as_deref())
}

/// Rolling tail of combined output, bounded to `max` bytes.
struct TailBuf {
    bytes: VecDeque<u8>,
    omitted: u64,
    max: usize,
}

impl TailBuf {
    fn new(max: usize) -> Self {
        Self { bytes: VecDeque::new(), omitted: 0, max: max.max(1) }
    }

    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend(chunk.iter().copied());
        let length = self.bytes.len();
        if length > self.max {
            let drop = length - self.max;
            self.bytes.drain(..drop);
            self.omitted += drop as u64;
        }
    }

    fn finish(&mut self) -> LogTail {
        LogTail { text: sanitize(self.bytes.make_contiguous()), omitted_bytes: self.omitted }
    }
}

/// Make arbitrary process bytes safe for a rendered text surface: lossy UTF-8,
/// ANSI/OSC escape sequences removed, other control characters replaced with
/// spaces. Arguments passed to Git are never touched by this.
pub fn sanitize(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\n' => out.push('\n'),
            // Progress redraws would flood the log; drop the carriage return.
            '\r' => {}
            '\u{1b}' => match chars.next() {
                // CSI: consume through the final byte.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: consume through BEL or ST.
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod wsl_support {
    use std::io::Write;
    use std::process::{Command as StdCommand, Stdio};
    use std::time::{Duration, Instant};

    /// Fixture commands bypass the application runner on purpose.
    /// Off Windows the "Linux side" is this machine, so fixtures run directly.
    pub fn wsl_command(args: &[&str]) -> StdCommand {
        #[cfg(windows)]
        {
            let mut c = StdCommand::new("wsl.exe");
            c.args(["-d", crate::git::distro(), "-e"]).args(args);
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = StdCommand::new(args[0]);
            c.args(&args[1..]);
            c
        }
    }

    pub fn wsl(args: &[&str]) -> std::process::Output {
        wsl_command(args).output().expect("failed to launch fixture command")
    }

    pub fn must(args: &[&str]) -> Vec<u8> {
        let out = wsl(args);
        assert!(
            out.status.success(),
            "wsl {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    /// Run with a disposable HOME and no system Git config.
    pub fn must_env(home: &str, args: &[&str]) -> Vec<u8> {
        let home = format!("HOME={home}");
        let mut full = vec!["env", home.as_str(), "GIT_CONFIG_NOSYSTEM=1"];
        full.extend_from_slice(args);
        let out = wsl(&full);
        assert!(
            out.status.success(),
            "wsl env {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    pub fn write_file(path: &str, bytes: &[u8]) {
        let mut child = wsl_command(&["tee", path])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to launch tee");
        child.stdin.take().expect("stdin is piped").write_all(bytes).unwrap();
        assert!(child.wait().unwrap().success(), "failed to write {path}");
    }

    pub fn write_script(path: &str, text: &str) {
        write_file(path, text.as_bytes());
        must(&["chmod", "+x", path]);
    }

    pub fn exists(path: &str) -> bool {
        wsl(&["test", "-e", path]).status.success()
    }

    pub fn temp_dir(tag: &str) -> String {
        // Git reports real paths, and /tmp is a symlink on macOS.
        let root = if cfg!(target_os = "macos") { "/private/tmp" } else { "/tmp" };
        let dir = format!("{root}/spur-jobs-{tag}-{}", std::process::id());
        must(&["rm", "-rf", &dir]);
        must(&["mkdir", "-p", &dir]);
        dir
    }

    pub fn pgrep(pattern: &str) -> String {
        String::from_utf8_lossy(&wsl(&["pgrep", "-f", pattern]).stdout).into_owned()
    }

    /// Bounded poll for an external condition; not a timing assertion.
    pub fn poll_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        condition()
    }

    /// Generous diagnostic watchdog: a hung scenario aborts with a message
    /// instead of stalling the suite forever. Threads die with the process.
    pub fn watchdog(seconds: u64) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(seconds));
            eprintln!("watchdog: test did not finish within {seconds}s; aborting");
            std::process::exit(101);
        });
    }

    /// Kills helpers even when a test panics mid-scenario.
    pub struct Cleanup {
        pattern: String,
        sentinel: Option<std::process::Child>,
    }

    impl Cleanup {
        pub fn new(pattern: impl Into<String>) -> Self {
            Self { pattern: pattern.into(), sentinel: None }
        }

        pub fn track_sentinel(&mut self, child: std::process::Child) {
            self.sentinel = Some(child);
        }

        pub fn sentinel_alive(&mut self) -> bool {
            self.sentinel
                .as_mut()
                .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
        }

        pub fn kill_sentinel(&mut self) {
            if let Some(mut child) = self.sentinel.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = wsl(&["pkill", "-f", &self.pattern]);
            self.kill_sentinel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_terminal_sequences_and_controls() {
        let raw = b"plain \x1b[31mred\x1b[0m\x07bell\x08back\x1b]0;title\x07 after \xc3\x28 bad\nnext\rline\n";
        let text = sanitize(raw);
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert!(!text.contains('\u{7}'), "{text:?}");
        assert!(!text.contains('\u{8}'), "{text:?}");
        assert!(!text.contains("title"), "{text:?}");
        assert!(text.contains("plain red bell"), "{text:?}");
        assert!(text.contains("after \u{fffd}( bad"), "{text:?}");
        assert_eq!(text, "plain red bell back after \u{fffd}( bad\nnextline\n");
    }

    #[test]
    fn log_tail_keeps_the_end_and_counts_omitted_bytes() {
        let mut tail = TailBuf::new(8);
        tail.push(b"aaaa");
        tail.push(b"bbbb");
        tail.push(b"cccc");
        let finished = tail.finish();
        assert_eq!(finished.text, "bbbbcccc");
        assert_eq!(finished.omitted_bytes, 4);
        let display = finished.display();
        assert!(display.contains("4 bytes"), "{display:?}");
        assert!(display.ends_with("bbbbcccc"), "{display:?}");
    }

    #[test]
    fn progress_redraws_keep_only_their_latest_state() {
        let stream = b"Counting objects:  50% (2/4)\rCounting objects: 100% (4/4)\r\
Counting objects: 100% (4/4), done.\nWriting objects:  20% (1/5)\rWriting objects:  60% (3/5)\r";
        assert_eq!(
            collapse_redraws(stream),
            b"Counting objects: 100% (4/4), done.\nWriting objects:  60% (3/5)"
        );
        // A CRLF line ending is a line ending, not a redraw.
        assert_eq!(collapse_redraws(b"one\r\ntwo\r\n"), b"one\ntwo\n");
    }

    #[test]
    fn parse_jobs_accepts_only_positive_numbers() {
        assert_eq!(parse_jobs(None), Ok(DEFAULT_JOBS));
        assert_eq!(parse_jobs(Some(" 8 ")), Ok(8));
        assert_eq!(parse_jobs(Some("1")), Ok(1));
        assert!(parse_jobs(Some("0")).is_err());
        assert!(parse_jobs(Some("-3")).is_err());
        assert!(parse_jobs(Some("2.5")).is_err());
        assert!(parse_jobs(Some("many")).is_err());
        assert!(parse_jobs(Some("")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn local_command_captures_status_and_both_streams() {
        let out = Command::new("cmd")
            .args(["/c", "echo to-stdout & echo to-stderr 1>&2 & exit 3"])
            .output()
            .expect("cmd should run");
        assert_eq!(out.status, Some(3));
        assert!(!out.success());
        assert!(out.stdout_text().contains("to-stdout"));
        assert!(out.stderr_text().contains("to-stderr"));
        assert!(out.log.text.contains("to-stdout") && out.log.text.contains("to-stderr"));
        assert!(out.error_context().starts_with("exit 3"));
    }

    #[test]
    fn journal_rings_ids_and_filters_by_worktree() {
        // Pure ring behavior through the real push path. Other tests may
        // record concurrently, so assert shapes, never exact counts.
        let argv = vec![
            "git".to_string(),
            "-C".to_string(),
            "/journal-test-w".to_string(),
        ];
        for _ in 0..JOURNAL_CAP + 5 {
            journal_push(argv.clone(), String::new(), String::new(), false, true, false);
        }
        let ring = JOURNAL.lock().unwrap();
        assert!(ring.records.len() <= JOURNAL_CAP, "the ring must stay capped");
        drop(ring);
        let head = journal_len();
        let recent = journal_since(head.saturating_sub(3));
        assert!(
            recent.windows(2).all(|pair| pair[0].id < pair[1].id),
            "ranges stay id-ordered"
        );
        let ours: Vec<_> = journal_since_for(0, "/journal-test-w");
        assert!(!ours.is_empty(), "the -C worktree must match");
        assert!(
            ours.iter().all(|record| record.argv[2] == "/journal-test-w"),
            "no stranger may pass the filter"
        );
        assert!(journal_since_for(0, "/journal-test-other-not-present").is_empty());
        // Leave the ring clean for other tests.
        JOURNAL.lock().unwrap().records.clear();
    }

    #[test]
    fn journal_matches_translated_native_paths() {
        // Native Windows Git receives C:\ paths while the worktree identity
        // stays /mnt/…: both spellings must attribute, or ran operations on
        // Windows-mounted repositories expand to nothing.
        let floor = journal_len();
        journal_push(
            vec![
                "C:\\Program Files\\Git\\bin\\git.exe".to_string(),
                "-C".to_string(),
                "C:\\work\\repo".to_string(),
                "fetch".to_string(),
            ],
            String::new(),
            String::new(),
            false,
            true,
            false,
        );
        let records = journal_since_for(floor, "/mnt/c/work/repo");
        assert_eq!(records.len(), 1, "{records:?}");
        assert!(journal_since_for(floor, "/mnt/c/other").is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn journal_records_real_commands_with_argv_and_output() {
        let before = journal_len();
        let out = Command::new("cmd")
            .args(["/c", "echo journal-me"])
            .output()
            .expect("cmd should run");
        assert!(out.success());
        // Other tests may record concurrently; find ours by argv.
        let mine = journal_since(before)
            .into_iter()
            .find(|record| record.argv == vec!["cmd", "/c", "echo journal-me"])
            .expect("our command must be journaled");
        assert!(mine.stdout.contains("journal-me"));
        assert!(mine.success && !mine.cancelled && !mine.truncated);
    }

    #[cfg(windows)]
    #[test]
    fn command_feeds_large_stdin_without_deadlock() {
        // 80 kB on stdin — the size class that must not ride the command line.
        // findstr echoes every line, proving stdin was written and then closed.
        let mut payload = String::new();
        for i in 0..2000 {
            payload.push_str(&format!("line-{i:04}\n"));
        }
        let out = Command::new("cmd")
            .args(["/c", "findstr", "."])
            .stdin(payload.into_bytes())
            .output()
            .expect("cmd should run");
        assert!(out.success(), "{}", out.error_context());
        let text = out.stdout_text();
        assert!(text.contains("line-0000"), "first stdin line missing: {text:?}");
        assert!(text.contains("line-1999"), "last stdin line missing: {text:?}");
    }

    #[cfg(windows)]
    #[test]
    fn capture_limit_keeps_a_prefix_and_reports_truncation() {
        // A command that writes far more than the cap; the excess must not be
        // retained, but the child still drains to completion.
        let out = Command::new("cmd")
            .args(["/c", "for /L %i in (1,1,5000) do @echo 012345678901234567890123456789"])
            .capture_limit(256)
            .output()
            .expect("cmd should run");
        assert!(out.success(), "{}", out.error_context());
        assert!(out.capture_truncated);
        assert!(out.stdout.len() <= 256, "capture grew to {}", out.stdout.len());
        // take_output already consumed the result; a second read reports so.
        let job = Command::new("cmd").args(["/c", "echo once"]).spawn();
        assert!(job.take_output().is_ok());
        assert!(job.take_output().is_err(), "output was handed out twice");
    }

    #[cfg(windows)]
    #[test]
    fn batch_cancel_stops_a_queued_job_before_it_starts() {
        let batch = Batch::new(1);
        let running =
            batch.submit(Command::new("cmd").args(["/c", "ping -n 30 127.0.0.1 >nul"]));
        let queued = batch.submit(Command::new("cmd").args(["/c", "echo started"]));
        batch.cancel();
        // The first job may be mid-spawn or still queued; either way it is
        // cancelled and never reported as success.
        let first = running.wait().unwrap();
        assert!(first.cancelled && !first.success());
        let second = queued.wait().unwrap();
        assert!(!second.started && second.cancelled && second.status.is_none());
        assert!(running.clone().wait().unwrap().cancelled);
    }
}

/// Real-process integration tests against the WSL distro; ignored
/// by default: run with `cargo test -- --ignored`.
#[cfg(test)]
mod wsl_tests {
    use super::*;
    use crate::process::wsl_support as wsl;
    use std::process::Stdio;
    use std::time::Duration;

    fn wsl_job(args: &[&str]) -> Command {
        #[cfg(windows)]
        {
            Command::new("wsl.exe").args(["-d", crate::git::distro(), "-e"]).args(args)
        }
        #[cfg(not(windows))]
        {
            Command::new(args[0]).args(&args[1..])
        }
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn cancellation_stops_owned_work_and_releases_permits() {
        wsl::watchdog(120);
        let dir = wsl::temp_dir("r12a");
        let ready = format!("{dir}/ready");
        let child_started = format!("{dir}/child-started");
        let queued_started = format!("{dir}/queued-started");
        let parent_script = format!("{dir}/parent.sh");
        let queued_script = format!("{dir}/queued.sh");
        wsl::write_script(
            &parent_script,
            &format!(
                r#"#!/bin/bash
if [ "$1" = child ]; then
  touch "{child_started}"
  echo child-ready
  sleep 600
  exit 0
fi
"$0" child &
echo parent-ready
touch "{ready}"
sleep 600
"#
            ),
        );
        wsl::write_script(
            &queued_script,
            &format!("#!/bin/bash\ntouch \"{queued_started}\"\necho queued-ran\n"),
        );

        let sentinel = wsl::wsl_command(&["sleep", "600"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sentinel should launch");
        let mut cleanup = wsl::Cleanup::new(dir.clone());
        cleanup.track_sentinel(sentinel);

        let batch = Batch::new(1);
        let running = batch.submit(wsl_job(&[parent_script.as_str()]));
        assert!(
            wsl::poll_until(Duration::from_secs(15), || wsl::exists(&ready)),
            "helper never announced readiness"
        );
        assert!(wsl::exists(&child_started), "helper child never started");
        let queued = batch.submit(wsl_job(&[queued_script.as_str()]));
        batch.cancel();

        let first = running.wait().unwrap();
        assert!(first.started && first.cancelled && !first.success(), "{first:?}");
        assert!(
            first.stdout_text().contains("parent-ready"),
            "pipes did not drain after cancellation: {:?}",
            first.log.display()
        );

        let second = queued.wait().unwrap();
        assert!(!second.started && second.cancelled && !second.success());
        assert!(!wsl::exists(&queued_started), "queued job started after cancellation");

        assert!(
            wsl::poll_until(Duration::from_secs(10), || wsl::pgrep(&parent_script).is_empty()),
            "owned processes still running: {}",
            wsl::pgrep(&parent_script)
        );
        assert!(cleanup.sentinel_alive(), "sentinel was killed");
        cleanup.kill_sentinel();

        // Permits and guards were released: the batch still runs normal work.
        let next = batch.submit(wsl_job(&["git", "--version"]));
        let next = next.wait().unwrap();
        assert!(next.success(), "{}", next.error_context());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn cancellation_does_not_rewrite_completed_work() {
        wsl::watchdog(120);
        let batch = Batch::new(1);
        let job = batch.submit(wsl_job(&["git", "--version"]));
        let first = job.wait().unwrap();
        assert!(first.success(), "{}", first.error_context());
        batch.cancel();
        let again = job.wait().unwrap();
        assert!(again.success() && !again.cancelled, "completed work was rewritten");
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn output_pressure_drains_and_bounds_the_log() {
        wsl::watchdog(120);
        let dir = wsl::temp_dir("r13");
        let script = format!("{dir}/pressure.sh");
        let _cleanup = wsl::Cleanup::new(dir.clone());
        wsl::write_script(
            &script,
            r#"#!/bin/bash
(
  dd if=/dev/zero bs=8192 count=192 2>/dev/null | tr '\0' 'A'
  printf '\303\050'
  printf '\033[31mred\033[0m\007\010'
  printf 'STDOUT-END\n'
) &
(
  dd if=/dev/zero bs=8192 count=192 2>/dev/null | tr '\0' 'B' >&2
  printf '\303\050' >&2
  printf '\033[31mred\033[0m\007\010' >&2
) &
wait
printf 'FINAL-ERROR-MARKER\n' >&2
exit 3
"#,
        );

        // A 1 MiB tail covers the final marker while still proving the log is
        // bounded; the helper writes ~3 MiB in total.
        let out = wsl_job(&[script.as_str()]).log_tail_bytes(1024 * 1024).output().unwrap();
        assert_eq!(out.status, Some(3), "log: {}", out.log.display());
        assert!(!out.success());
        assert!(out.stdout.ends_with(b"STDOUT-END\n"), "stdout was not fully drained");
        assert!(
            out.stderr_text().contains("FINAL-ERROR-MARKER"),
            "stderr was not fully drained"
        );
        assert!(
            out.log.omitted_bytes > 0,
            "log was not bounded: {} bytes kept",
            out.log.text.len()
        );
        assert!(out.log.text.contains("FINAL-ERROR-MARKER"));
        let display = out.log.display();
        assert!(!display.contains('\u{1b}') && !display.contains('\u{7}'), "control bytes leaked");
    }
}
