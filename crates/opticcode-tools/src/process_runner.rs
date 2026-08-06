#[cfg(windows)]
mod windows;

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::Serialize;

#[cfg(not(windows))]
use std::process::Child;

#[cfg(windows)]
use self::windows::ProcessTree;

pub const DEFAULT_PROCESS_TIMEOUT_SECONDS: u64 = 10 * 60;
pub const DEFAULT_PROCESS_TIMEOUT: Duration = Duration::from_secs(DEFAULT_PROCESS_TIMEOUT_SECONDS);
pub const MAX_PROCESS_TIMEOUT_SECONDS: u64 = 60 * 60;
pub const MAX_PROCESS_TIMEOUT: Duration = Duration::from_secs(MAX_PROCESS_TIMEOUT_SECONDS);
pub const DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES: usize = 1024 * 1024;
pub const MAX_PROCESS_OUTPUT_LIMIT_BYTES: usize = 16 * 1024 * 1024;

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(20);
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessLaunchMode {
    Direct,
    WindowsCommandScript,
}

#[derive(Debug, Clone)]
pub struct ProcessRequest {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub working_directory: PathBuf,
    pub timeout: Duration,
    /// Maximum retained bytes for each stream. Readers continue draining after the limit.
    pub output_limit_bytes: usize,
    pub launch_mode: ProcessLaunchMode,
    pub environment: Vec<(OsString, OsString)>,
}

impl ProcessRequest {
    pub fn new(program: impl Into<PathBuf>, working_directory: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            working_directory: working_directory.into(),
            timeout: DEFAULT_PROCESS_TIMEOUT,
            output_limit_bytes: DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES,
            launch_mode: ProcessLaunchMode::Direct,
            environment: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Success,
    Failed,
    TimedOut,
    Cancelled,
}

impl ProcessStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessTreeStrategy {
    WindowsJobObject,
    DirectChild,
}

impl ProcessTreeStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WindowsJobObject => "windows_job_object",
            Self::DirectChild => "direct_child",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessTermination {
    pub attempted: bool,
    pub succeeded: bool,
    pub strategy: ProcessTreeStrategy,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessOutputStats {
    pub limit_bytes_per_stream: usize,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_retained_bytes: usize,
    pub stderr_retained_bytes: usize,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub output_truncated: bool,
    pub capture_errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub process_id: Option<u32>,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub duration: Duration,
    pub stdout: String,
    pub stderr: String,
    pub output: ProcessOutputStats,
    pub termination: ProcessTermination,
}

impl ProcessResult {
    pub fn success(&self) -> bool {
        self.status == ProcessStatus::Success
    }

    pub fn timed_out(&self) -> bool {
        self.status == ProcessStatus::TimedOut
    }

    pub fn cancelled(&self) -> bool {
        self.status == ProcessStatus::Cancelled
    }
}

#[derive(Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl std::fmt::Debug for CancellationToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CancellationToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

pub fn run_process(request: &ProcessRequest) -> Result<ProcessResult> {
    run_process_with_cancellation(request, None)
}

pub fn run_process_with_cancellation(
    request: &ProcessRequest,
    cancellation: Option<&CancellationToken>,
) -> Result<ProcessResult> {
    validate_request(request)?;
    let started_at = Instant::now();
    let default_strategy = platform_tree_strategy();

    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Ok(ProcessResult {
            process_id: None,
            status: ProcessStatus::Cancelled,
            exit_code: None,
            duration: started_at.elapsed(),
            stdout: String::new(),
            stderr: String::new(),
            output: empty_output_stats(request.output_limit_bytes),
            termination: ProcessTermination {
                attempted: false,
                succeeded: false,
                strategy: default_strategy,
                error: None,
            },
        });
    }

    let mut command = build_command(request);
    command
        .current_dir(&request.working_directory)
        .envs(request.environment.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let tree = ProcessTree::new().context("failed to create process tree guard")?;
    let mut child = command.spawn().with_context(|| {
        format!(
            "failed to start process `{}` in {}",
            request.program.display(),
            request.working_directory.display()
        )
    })?;
    let process_id = child.id();

    let mut initial_exit = None;
    let tree_attached = match tree.assign(&child) {
        Ok(()) => true,
        Err(assign_error) => match child.try_wait() {
            Ok(Some(status)) => {
                initial_exit = Some(status);
                false
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("failed to attach process {process_id} to its tree guard: {assign_error}");
            }
        },
    };

    let stdout = child
        .stdout
        .take()
        .context("child stdout pipe was not available")?;
    let stderr = child
        .stderr
        .take()
        .context("child stderr pipe was not available")?;
    let stdout_capture = CaptureTask::start(stdout, request.output_limit_bytes);
    let stderr_capture = CaptureTask::start(stderr, request.output_limit_bytes);

    let mut requested_status = None;
    let exit_status = if let Some(status) = initial_exit {
        Some(status)
    } else {
        loop {
            if let Some(status) = child
                .try_wait()
                .with_context(|| format!("failed to poll process {process_id}"))?
            {
                break Some(status);
            }

            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                requested_status = Some(ProcessStatus::Cancelled);
                break None;
            }

            if started_at.elapsed() >= request.timeout {
                requested_status = Some(ProcessStatus::TimedOut);
                break None;
            }

            let remaining = request.timeout.saturating_sub(started_at.elapsed());
            thread::sleep(PROCESS_POLL_INTERVAL.min(remaining));
        }
    };

    let mut termination = ProcessTermination {
        attempted: requested_status.is_some(),
        succeeded: false,
        strategy: if tree_attached {
            platform_tree_strategy()
        } else {
            ProcessTreeStrategy::DirectChild
        },
        error: None,
    };

    let final_exit = if requested_status.is_some() {
        match tree.terminate(&mut child) {
            Ok(()) => termination.succeeded = true,
            Err(error) => {
                termination.error = Some(error.to_string());
                if let Err(fallback_error) = child.kill() {
                    termination.error = Some(format!(
                        "{}; direct-child fallback failed: {fallback_error}",
                        termination
                            .error
                            .as_deref()
                            .unwrap_or("tree termination failed")
                    ));
                }
            }
        }
        #[cfg(windows)]
        drop(tree);
        Some(
            child
                .wait()
                .with_context(|| format!("failed to wait for terminated process {process_id}"))?,
        )
    } else {
        #[cfg(windows)]
        drop(tree);
        exit_status
    };

    let stdout_capture = stdout_capture.finish("stdout");
    let stderr_capture = stderr_capture.finish("stderr");
    let mut capture_errors = Vec::new();
    if let Some(error) = stdout_capture.error {
        capture_errors.push(error);
    }
    if let Some(error) = stderr_capture.error {
        capture_errors.push(error);
    }

    let mut status = requested_status.unwrap_or_else(|| status_from_exit(final_exit.as_ref()));
    if status == ProcessStatus::Success && !capture_errors.is_empty() {
        status = ProcessStatus::Failed;
    }

    let stdout = String::from_utf8_lossy(&stdout_capture.snapshot.bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_capture.snapshot.bytes).into_owned();
    let stdout_retained_bytes = stdout_capture.snapshot.bytes.len();
    let stderr_retained_bytes = stderr_capture.snapshot.bytes.len();
    let stdout_truncated = stdout_capture.snapshot.truncated;
    let stderr_truncated = stderr_capture.snapshot.truncated;

    Ok(ProcessResult {
        process_id: Some(process_id),
        status,
        exit_code: final_exit.as_ref().and_then(ExitStatus::code),
        duration: started_at.elapsed(),
        stdout,
        stderr,
        output: ProcessOutputStats {
            limit_bytes_per_stream: request.output_limit_bytes,
            stdout_bytes: stdout_capture.snapshot.total_bytes,
            stderr_bytes: stderr_capture.snapshot.total_bytes,
            stdout_retained_bytes,
            stderr_retained_bytes,
            stdout_truncated,
            stderr_truncated,
            output_truncated: stdout_truncated || stderr_truncated,
            capture_errors,
        },
        termination,
    })
}

fn validate_request(request: &ProcessRequest) -> Result<()> {
    if request.program.as_os_str().is_empty() {
        bail!("process program cannot be empty");
    }
    if request.timeout.is_zero() {
        bail!("process timeout must be greater than zero");
    }
    if request.timeout > MAX_PROCESS_TIMEOUT {
        bail!(
            "process timeout {} seconds exceeds the maximum of {} seconds",
            request.timeout.as_secs_f64(),
            MAX_PROCESS_TIMEOUT_SECONDS
        );
    }
    if request.output_limit_bytes == 0 {
        bail!("process output limit must be greater than zero");
    }
    if request.output_limit_bytes > MAX_PROCESS_OUTPUT_LIMIT_BYTES {
        bail!(
            "process output limit {} exceeds the maximum of {} bytes per stream",
            request.output_limit_bytes,
            MAX_PROCESS_OUTPUT_LIMIT_BYTES
        );
    }
    Ok(())
}

fn build_command(request: &ProcessRequest) -> Command {
    #[cfg(windows)]
    if request.launch_mode == ProcessLaunchMode::WindowsCommandScript {
        let mut command = Command::new("cmd.exe");
        command
            .arg("/D")
            .arg("/C")
            .arg(&request.program)
            .args(&request.args);
        return command;
    }

    let mut command = Command::new(&request.program);
    command.args(&request.args);
    command
}

fn status_from_exit(exit_status: Option<&ExitStatus>) -> ProcessStatus {
    if exit_status.is_some_and(ExitStatus::success) {
        ProcessStatus::Success
    } else {
        ProcessStatus::Failed
    }
}

fn empty_output_stats(limit: usize) -> ProcessOutputStats {
    ProcessOutputStats {
        limit_bytes_per_stream: limit,
        stdout_bytes: 0,
        stderr_bytes: 0,
        stdout_retained_bytes: 0,
        stderr_retained_bytes: 0,
        stdout_truncated: false,
        stderr_truncated: false,
        output_truncated: false,
        capture_errors: Vec::new(),
    }
}

#[cfg(windows)]
fn platform_tree_strategy() -> ProcessTreeStrategy {
    ProcessTreeStrategy::WindowsJobObject
}

#[cfg(not(windows))]
fn platform_tree_strategy() -> ProcessTreeStrategy {
    ProcessTreeStrategy::DirectChild
}

#[cfg(not(windows))]
struct ProcessTree;

#[cfg(not(windows))]
impl ProcessTree {
    fn new() -> io::Result<Self> {
        Ok(Self)
    }

    fn assign(&self, _child: &Child) -> io::Result<()> {
        Ok(())
    }

    fn terminate(&self, child: &mut Child) -> io::Result<()> {
        child.kill()
    }
}

struct CaptureTask {
    state: Arc<Mutex<BoundedTail>>,
    completion: Receiver<io::Result<()>>,
    handle: Option<JoinHandle<()>>,
}

impl CaptureTask {
    fn start<R>(mut reader: R, limit: usize) -> Self
    where
        R: Read + Send + 'static,
    {
        let state = Arc::new(Mutex::new(BoundedTail::new(limit)));
        let thread_state = Arc::clone(&state);
        let (sender, completion) = mpsc::channel();
        let handle = thread::spawn(move || {
            let result = drain_reader(&mut reader, &thread_state);
            let _ = sender.send(result);
        });
        Self {
            state,
            completion,
            handle: Some(handle),
        }
    }

    fn finish(mut self, stream_name: &str) -> FinishedCapture {
        let completion = self.completion.recv_timeout(OUTPUT_DRAIN_TIMEOUT);
        let drain_timed_out = matches!(&completion, Err(RecvTimeoutError::Timeout));
        let mut error = match completion {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(format!("{stream_name} capture failed: {error}")),
            Err(RecvTimeoutError::Timeout) => Some(format!(
                "{stream_name} capture did not close within {} ms",
                OUTPUT_DRAIN_TIMEOUT.as_millis()
            )),
            Err(RecvTimeoutError::Disconnected) => {
                Some(format!("{stream_name} capture thread disconnected"))
            }
        };

        if !drain_timed_out
            && self
                .handle
                .take()
                .is_some_and(|handle| handle.join().is_err())
        {
            error = Some(format!("{stream_name} capture thread panicked"));
        }

        let snapshot = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot();
        FinishedCapture { snapshot, error }
    }
}

fn drain_reader<R>(reader: &mut R, state: &Arc<Mutex<BoundedTail>>) -> io::Result<()>
where
    R: Read,
{
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(&chunk[..read]);
    }
}

struct FinishedCapture {
    snapshot: CaptureSnapshot,
    error: Option<String>,
}

struct CaptureSnapshot {
    bytes: Vec<u8>,
    total_bytes: u64,
    truncated: bool,
}

struct BoundedTail {
    bytes: VecDeque<u8>,
    total_bytes: u64,
    limit: usize,
}

impl BoundedTail {
    fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(limit.min(64 * 1024)),
            total_bytes: 0,
            limit,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        self.total_bytes = self
            .total_bytes
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));

        if chunk.len() >= self.limit {
            self.bytes.clear();
            self.bytes.extend(&chunk[chunk.len() - self.limit..]);
            return;
        }

        let overflow = self
            .bytes
            .len()
            .saturating_add(chunk.len())
            .saturating_sub(self.limit);
        if overflow > 0 {
            self.bytes.drain(..overflow);
        }
        self.bytes.extend(chunk);
    }

    fn snapshot(&self) -> CaptureSnapshot {
        let bytes = self.bytes.iter().copied().collect::<Vec<_>>();
        CaptureSnapshot {
            truncated: self.total_bytes > bytes.len() as u64,
            total_bytes: self.total_bytes,
            bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        run_process, run_process_with_cancellation, CancellationToken, ProcessRequest,
        ProcessStatus, MAX_PROCESS_OUTPUT_LIMIT_BYTES, MAX_PROCESS_TIMEOUT_SECONDS,
    };
    use std::ffi::OsString;
    use std::io::{self, Write};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    const HELPER_MODE_ENV: &str = "OPTICCODE_PROCESS_RUNNER_TEST_MODE";
    const HELPER_TEST_NAME: &str = "process_runner::tests::process_fixture";

    #[test]
    fn process_fixture() {
        let Ok(mode) = std::env::var(HELPER_MODE_ENV) else {
            return;
        };

        match mode.as_str() {
            "success" => {
                println!("fixture stdout");
                eprintln!("fixture stderr");
            }
            "failure" => {
                eprintln!("fixture failed");
                std::process::exit(7);
            }
            "output" => {
                io::stdout().write_all(&vec![b'x'; 64 * 1024]).unwrap();
                io::stderr().write_all(&vec![b'y'; 32 * 1024]).unwrap();
            }
            "blocked" | "grandchild" => thread::sleep(Duration::from_secs(30)),
            "parent_with_child" => {
                let mut child = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        HELPER_TEST_NAME,
                        "--nocapture",
                        "--test-threads=1",
                    ])
                    .env(HELPER_MODE_ENV, "grandchild")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                let mut stdout = io::stdout().lock();
                writeln!(stdout, "DESCENDANT_PID={}", child.id()).unwrap();
                stdout.flush().unwrap();
                thread::sleep(Duration::from_secs(30));
                let _ = child.wait();
            }
            other => panic!("unknown process fixture mode: {other}"),
        }
    }

    #[test]
    fn captures_successful_process_output() {
        let result = run_process(&helper_request("success", Duration::from_secs(5), 4096)).unwrap();

        assert_eq!(result.status, ProcessStatus::Success);
        assert!(result.success());
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("fixture stdout"));
        assert!(result.stderr.contains("fixture stderr"));
        assert!(!result.output.output_truncated);
    }

    #[test]
    fn reports_failed_process_exit() {
        let result = run_process(&helper_request("failure", Duration::from_secs(5), 4096)).unwrap();

        assert_eq!(result.status, ProcessStatus::Failed);
        assert!(!result.success());
        assert_eq!(result.exit_code, Some(7));
        assert!(result.stderr.contains("fixture failed"));
    }

    #[test]
    fn drains_but_bounds_large_output() {
        let limit = 1024;
        let result = run_process(&helper_request("output", Duration::from_secs(5), limit)).unwrap();

        assert_eq!(result.status, ProcessStatus::Success);
        assert!(result.output.output_truncated);
        assert!(result.output.stdout_truncated);
        assert!(result.output.stderr_truncated);
        assert!(result.output.stdout_bytes >= 64 * 1024);
        assert!(result.output.stderr_bytes >= 32 * 1024);
        assert!(result.output.stdout_retained_bytes <= limit);
        assert!(result.output.stderr_retained_bytes <= limit);
    }

    #[test]
    fn times_out_blocked_process() {
        let started_at = Instant::now();
        let result =
            run_process(&helper_request("blocked", Duration::from_millis(250), 4096)).unwrap();

        assert_eq!(result.status, ProcessStatus::TimedOut);
        assert!(result.timed_out());
        assert!(result.termination.attempted);
        assert!(result.termination.succeeded);
        assert!(started_at.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn distinguishes_cancellation_from_timeout() {
        let cancellation = CancellationToken::new();
        let cancellation_trigger = cancellation.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            cancellation_trigger.cancel();
        });

        let result = run_process_with_cancellation(
            &helper_request("blocked", Duration::from_secs(5), 4096),
            Some(&cancellation),
        )
        .unwrap();
        canceller.join().unwrap();

        assert_eq!(result.status, ProcessStatus::Cancelled);
        assert!(result.cancelled());
        assert!(!result.timed_out());
        assert!(result.termination.attempted);
        assert!(result.termination.succeeded);
    }

    #[test]
    fn honours_cancellation_before_spawn() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let request = ProcessRequest::new(
            "program-that-must-not-be-spawned",
            std::env::current_dir().unwrap(),
        );

        let result = run_process_with_cancellation(&request, Some(&cancellation)).unwrap();

        assert_eq!(result.status, ProcessStatus::Cancelled);
        assert_eq!(result.process_id, None);
        assert!(!result.termination.attempted);
        assert!(!result.termination.succeeded);
    }

    #[test]
    fn rejects_excessive_output_limit_before_spawn() {
        let mut request = ProcessRequest::new(
            "program-that-must-not-be-spawned",
            std::env::current_dir().unwrap(),
        );
        request.output_limit_bytes = MAX_PROCESS_OUTPUT_LIMIT_BYTES + 1;

        let error = run_process(&request).unwrap_err();

        assert!(error.to_string().contains("exceeds the maximum"));
    }

    #[test]
    fn rejects_excessive_timeout_before_spawn() {
        let mut request = ProcessRequest::new(
            "program-that-must-not-be-spawned",
            std::env::current_dir().unwrap(),
        );
        request.timeout = Duration::from_secs(MAX_PROCESS_TIMEOUT_SECONDS + 1);

        let error = run_process(&request).unwrap_err();

        assert!(error.to_string().contains("timeout"));
        assert!(error.to_string().contains("exceeds the maximum"));
    }

    #[cfg(windows)]
    #[test]
    fn terminates_windows_descendant_tree() {
        let result = run_process(&helper_request(
            "parent_with_child",
            Duration::from_secs(1),
            4096,
        ))
        .unwrap();
        assert_eq!(result.status, ProcessStatus::TimedOut);
        assert!(result.termination.succeeded);

        let descendant_pid = result
            .stdout
            .lines()
            .find_map(|line| line.split_once("DESCENDANT_PID=").map(|(_, value)| value))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or_else(|| {
                panic!(
                    "fixture should report its descendant PID; stdout={:?}; stderr={:?}",
                    result.stdout, result.stderr
                )
            });

        let deadline = Instant::now() + Duration::from_secs(2);
        while windows_process_is_active(descendant_pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!windows_process_is_active(descendant_pid));
    }

    fn helper_request(mode: &str, timeout: Duration, output_limit: usize) -> ProcessRequest {
        let mut request = ProcessRequest::new(
            std::env::current_exe().expect("test executable should be available"),
            std::env::current_dir().expect("current directory should be available"),
        );
        request.args = [
            "--exact",
            HELPER_TEST_NAME,
            "--nocapture",
            "--test-threads=1",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        request.timeout = timeout;
        request.output_limit_bytes = output_limit;
        request
            .environment
            .push((OsString::from(HELPER_MODE_ENV), OsString::from(mode)));
        request
    }

    #[cfg(windows)]
    fn windows_process_is_active(process_id: u32) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
            if process.is_null() {
                return false;
            }
            let mut exit_code = 0_u32;
            let read = GetExitCodeProcess(process, &mut exit_code);
            let _ = CloseHandle(process);
            read != 0 && exit_code == STILL_ACTIVE as u32
        }
    }
}
