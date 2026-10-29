//! Document import in a separate, limited process ("worker").
//!
//! Task sheets come from untrusted sources. Their parsers (`lopdf` and
//! `pdf-extract` for the PDF text layer, Windows.Data.Pdf / PDFKit for
//! rendering scanned pages, the system OCR, the `image` crate, the DOCX
//! reader) run in a child process instead of the app: the app's own
//! executable, started with the hidden argument [`WORKER_ARG`] (handled in
//! `main.rs` before Tauri starts, so the child never opens a window and
//! needs no extra binary in the bundle).
//!
//! The child gets the file on stdin (a JSON header line, then the bytes)
//! and answers with JSON lines on stdout (progress, then the result or a
//! German error). Its limits:
//!
//! - memory: Windows job object with `JOB_OBJECT_LIMIT_PROCESS_MEMORY`
//!   (assigned before the file is sent); macOS/Linux `setrlimit` in the
//!   child; on all systems the parent also kills the child when its
//!   resident memory exceeds the limit (macOS hardly enforces rlimits),
//! - time: no progress for [`PHASE_TIMEOUT`] or longer than
//!   [`TOTAL_TIMEOUT`] in total → killed (plus RLIMIT_CPU and a deadline
//!   thread in the child, in case the app is gone),
//! - output: at most [`MAX_OUTPUT_BYTES`],
//! - lifetime: on Windows the job kills the child when the app ends; on
//!   macOS/Linux the child's own deadline ends it.
//!
//! Whatever happens in the child (abort on allocation failure, stack
//! overflow, endless loop), the app gets a German error message and keeps
//! running.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{DocsError, ImportResult, MAX_FILE_BYTES};
use crate::runner::Progress;

/// Hidden first argument that turns the app executable into the worker.
pub const WORKER_ARG: &str = "--code-pruefer-docs-worker";
/// Memory limit of the worker (OCR of a rendered page needs a few hundred
/// MB; hostile files are stopped here).
pub const MEMORY_LIMIT: u64 = 1536 * 1024 * 1024;
/// Longest time without a progress message (one page: render + OCR).
pub const PHASE_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest time for the whole import (up to 20 scanned pages).
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(300);
/// Largest answer (text ≤ 1 MB, preview ≤ 5 MB as base64, JSON escapes).
pub const MAX_OUTPUT_BYTES: u64 = 32 * 1024 * 1024;
/// Largest request header line.
const MAX_HEADER_BYTES: u64 = 64 * 1024;
/// Stack of the worker thread in the child (`pdf-extract` recurses).
const WORKER_STACK: usize = 64 * 1024 * 1024;
/// How long the parent waits for the child to exit after its answer.
const EXIT_GRACE: Duration = Duration::from_secs(5);
/// Debug builds only: makes the worker misbehave (`stack`, `alloc`,
/// `hang`, `flood`, `garbage`, `exit`, `chatty`) or skip the PDF checks
/// (`noguard`), for the integration tests in `tests/docs_worker.rs`.
pub const SELF_TEST_ENV: &str = "CODE_PRUEFER_DOCS_WORKER_TEST";

const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// The app executable (or, in tests, the built `code-pruefer` binary).
    pub exe: PathBuf,
    pub memory_limit: u64,
    pub phase_timeout: Duration,
    pub total_timeout: Duration,
    /// Extra environment for the child (tests: [`SELF_TEST_ENV`]).
    pub env: Vec<(String, String)>,
}

impl WorkerConfig {
    pub fn new(exe: PathBuf) -> Self {
        WorkerConfig {
            exe,
            memory_limit: MEMORY_LIMIT,
            phase_timeout: PHASE_TIMEOUT,
            total_timeout: TOTAL_TIMEOUT,
            env: Vec::new(),
        }
    }

    /// The running app as worker.
    pub fn current_exe() -> Result<Self, DocsError> {
        std::env::current_exe()
            .map(Self::new)
            .map_err(|e| start_error(&e))
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    file_name: String,
    size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Message {
    Progress { message: String },
    Done { result: Box<ImportResult> },
    Failed { error: WireError },
}

/// [`DocsError`] across the process boundary.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
enum WireError {
    TooLarge(u64),
    TooManyPages(usize),
    Unsupported,
    Mismatch(String),
    OcrUnavailable(String),
    Invalid(String),
}

impl From<DocsError> for WireError {
    fn from(e: DocsError) -> Self {
        match e {
            DocsError::TooLarge(n) => Self::TooLarge(n),
            DocsError::TooManyPages(n) => Self::TooManyPages(n),
            DocsError::Unsupported => Self::Unsupported,
            DocsError::Mismatch(ext) => Self::Mismatch(ext),
            DocsError::OcrUnavailable(m) => Self::OcrUnavailable(m),
            e @ (DocsError::Read(_) | DocsError::Invalid(_)) => Self::Invalid(e.to_string()),
        }
    }
}

impl From<WireError> for DocsError {
    fn from(e: WireError) -> Self {
        match e {
            WireError::TooLarge(n) => Self::TooLarge(n),
            WireError::TooManyPages(n) => Self::TooManyPages(n),
            WireError::Unsupported => Self::Unsupported,
            WireError::Mismatch(ext) => Self::Mismatch(ext),
            WireError::OcrUnavailable(m) => Self::OcrUnavailable(m),
            WireError::Invalid(m) => Self::Invalid(m),
        }
    }
}

/// Why the worker was stopped (or stopped by itself without answer).
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    Timeout(Duration),
    Memory(u64),
    Output,
    Protocol,
    Crashed(Option<i32>),
}

fn start_error(e: &dyn std::fmt::Display) -> DocsError {
    DocsError::Invalid(format!(
        "Der Lesevorgang für die Datei konnte nicht gestartet werden ({e})."
    ))
}

/// German message for a failed worker.
pub fn failure_message(file_name: &str, failure: &Failure) -> String {
    let subject = if super::is_pdf_name(file_name) {
        "Das PDF"
    } else {
        "Die Datei"
    };
    let detail = match failure {
        Failure::Timeout(t) => format!("das Lesen dauerte länger als {} s", t.as_secs()),
        Failure::Memory(limit) => format!(
            "das Lesen brauchte mehr als {} MB Arbeitsspeicher",
            limit / MIB
        ),
        Failure::Output => "das Lesen lieferte zu viele Daten".into(),
        Failure::Protocol => "das Lesen lieferte ungültige Daten".into(),
        Failure::Crashed(_) => "der Lesevorgang ist abgebrochen, z. B. wegen zu hohen \
                                Speicherbedarfs oder zu tiefer Verschachtelung"
            .into(),
    };
    format!(
        "{subject} konnte nicht sicher gelesen werden ({detail}). Vermutlich ist die Datei \
         beschädigt oder absichtlich manipuliert; der Code-Prüfer selbst läuft normal weiter. \
         Tipp: die Datei neu speichern (bei PDF z. B. über „Drucken als PDF“) oder den Text \
         direkt einfügen."
    )
}

// ---------------------------------------------------------------------------
// Parent side

/// Kills the child unless it has exited (every early return).
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

enum Event {
    Message(Message),
    Invalid,
    Overflow,
    Eof,
}

fn read_messages(stdout: ChildStdout, tx: mpsc::Sender<Event>) {
    let mut reader = BufReader::new(stdout.take(MAX_OUTPUT_BYTES + 1));
    let mut total = 0u64;
    loop {
        let mut line = Vec::new();
        let event = match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => Event::Eof,
            Ok(n) => {
                total += n as u64;
                if total > MAX_OUTPUT_BYTES {
                    Event::Overflow
                } else {
                    match serde_json::from_slice::<Message>(&line) {
                        Ok(m) => Event::Message(m),
                        Err(_) => Event::Invalid,
                    }
                }
            }
        };
        let last = !matches!(event, Event::Message(Message::Progress { .. }));
        if tx.send(event).is_err() || last {
            return;
        }
    }
}

/// Imports `bytes` (named `file_name`) in a worker process.
pub fn import(
    config: &WorkerConfig,
    file_name: &str,
    bytes: &[u8],
    on_progress: impl FnMut(Progress),
) -> Result<ImportResult, DocsError> {
    match run(config, file_name, bytes, on_progress) {
        Ok(answer) => answer,
        Err(RunError::Start(e)) => Err(e),
        Err(RunError::Failed(f)) => Err(DocsError::Invalid(failure_message(file_name, &f))),
    }
}

enum RunError {
    Start(DocsError),
    Failed(Failure),
}

fn run(
    config: &WorkerConfig,
    file_name: &str,
    bytes: &[u8],
    mut on_progress: impl FnMut(Progress),
) -> Result<Result<ImportResult, DocsError>, RunError> {
    let mut cmd = Command::new(&config.exe);
    cmd.arg(WORKER_ARG)
        .arg(config.memory_limit.to_string())
        .arg(config.total_timeout.as_secs().max(1).to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_remove(SELF_TEST_ENV);
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let child = cmd.spawn().map_err(|e| RunError::Start(start_error(&e)))?;
    let mut child = ChildGuard(child);
    // The job must hold the child before it gets the file: the worker
    // reads nothing else before.
    #[cfg(windows)]
    let _job = {
        use std::os::windows::io::AsRawHandle;
        crate::proc_guard::KillJob::new(Some(config.memory_limit))
            .and_then(|job| job.assign(child.0.as_raw_handle()).map(|_| job))
            .map_err(|e| RunError::Start(start_error(&e)))?
    };

    let mut stdin = child.0.stdin.take().expect("piped stdin");
    let stdout = child.0.stdout.take().expect("piped stdout");
    let mut header = serde_json::to_vec(&Request {
        file_name: file_name.to_string(),
        size: bytes.len() as u64,
    })
    .expect("request is serializable");
    header.push(b'\n');

    std::thread::scope(|scope| {
        scope.spawn(move || {
            // Errors mean the child is gone; that shows up on stdout.
            let _ = stdin
                .write_all(&header)
                .and_then(|_| stdin.write_all(bytes));
        });
        let (tx, rx) = mpsc::channel();
        scope.spawn(move || read_messages(stdout, tx));
        let answer = supervise(config, &mut child, &rx, &mut on_progress);
        if answer.is_err() {
            let _ = child.0.kill();
        }
        let _ = child.0.wait();
        answer
    })
}

fn supervise(
    config: &WorkerConfig,
    child: &mut ChildGuard,
    rx: &mpsc::Receiver<Event>,
    on_progress: &mut impl FnMut(Progress),
) -> Result<Result<ImportResult, DocsError>, RunError> {
    let started = Instant::now();
    let mut last_progress = started;
    let mut last_memory_check = started;
    let mut system = sysinfo::System::new();
    let pid = child.0.id();
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Message(Message::Progress { message })) => {
                last_progress = Instant::now();
                on_progress(Progress::Phase { message });
            }
            Ok(Event::Message(Message::Done { result })) => {
                wait_for_exit(child);
                return Ok(Ok(*result));
            }
            Ok(Event::Message(Message::Failed { error })) => {
                wait_for_exit(child);
                return Ok(Err(error.into()));
            }
            Ok(Event::Invalid) => return Err(RunError::Failed(Failure::Protocol)),
            Ok(Event::Overflow) => return Err(RunError::Failed(Failure::Output)),
            Ok(Event::Eof) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Without an answer: crashed (abort, stack overflow, killed
                // by a limit). Give it a moment to be reaped.
                let status = wait_until(child, Instant::now() + Duration::from_secs(2));
                return Err(RunError::Failed(Failure::Crashed(
                    status.and_then(|s| s.code()),
                )));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        if now.duration_since(started) > config.total_timeout {
            return Err(RunError::Failed(Failure::Timeout(config.total_timeout)));
        }
        if now.duration_since(last_progress) > config.phase_timeout {
            return Err(RunError::Failed(Failure::Timeout(config.phase_timeout)));
        }
        if now.duration_since(last_memory_check) >= crate::proc_guard::MEMORY_POLL {
            last_memory_check = now;
            if crate::proc_guard::resident_bytes(&mut system, pid)
                .is_some_and(|rss| rss > config.memory_limit)
            {
                return Err(RunError::Failed(Failure::Memory(config.memory_limit)));
            }
        }
    }
}

fn wait_until(child: &mut ChildGuard, deadline: Instant) -> Option<ExitStatus> {
    loop {
        if let Ok(Some(status)) = child.0.try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_exit(child: &mut ChildGuard) {
    if wait_until(child, Instant::now() + EXIT_GRACE).is_none() {
        let _ = child.0.kill();
    }
}

// ---------------------------------------------------------------------------
// Child side

/// Runs the worker if the process was started with [`WORKER_ARG`] and
/// returns its exit code; `None` for a normal app start.
pub fn run_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(WORKER_ARG)) {
        return None;
    }
    let mut number = || args.next().and_then(|a| a.to_str()?.parse::<u64>().ok());
    let memory = number().unwrap_or(MEMORY_LIMIT);
    let seconds = number().unwrap_or(TOTAL_TIMEOUT.as_secs());
    Some(child_main(memory, seconds))
}

fn send(message: &Message) {
    let Ok(mut line) = serde_json::to_vec(message) else {
        return;
    };
    line.push(b'\n');
    let mut out = std::io::stdout().lock();
    if out.write_all(&line).and_then(|_| out.flush()).is_err() {
        // The app is gone.
        std::process::exit(4);
    }
}

fn read_request(mut input: impl Read) -> Result<(String, Vec<u8>), String> {
    let mut header = Vec::new();
    (&mut input)
        .take(MAX_HEADER_BYTES)
        .read_until_newline(&mut header)
        .map_err(|e| e.to_string())?;
    let request: Request = serde_json::from_slice(&header).map_err(|e| e.to_string())?;
    if request.size > MAX_FILE_BYTES {
        return Err("Datei zu groß".into());
    }
    let mut bytes = Vec::with_capacity(request.size as usize);
    input
        .take(request.size)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != request.size {
        return Err("Datei unvollständig übertragen".into());
    }
    Ok((request.file_name, bytes))
}

trait ReadLine {
    fn read_until_newline(&mut self, buf: &mut Vec<u8>) -> std::io::Result<()>;
}

impl<R: Read> ReadLine for R {
    /// Byte by byte: a buffered reader would swallow the file that follows.
    fn read_until_newline(&mut self, buf: &mut Vec<u8>) -> std::io::Result<()> {
        let mut byte = [0u8; 1];
        loop {
            if self.read(&mut byte)? == 0 || byte[0] == b'\n' {
                return Ok(());
            }
            buf.push(byte[0]);
        }
    }
}

fn child_main(memory: u64, seconds: u64) -> i32 {
    #[cfg(unix)]
    crate::proc_guard::limit_current_process(memory, seconds.saturating_add(5));
    #[cfg(not(unix))]
    let _ = memory;
    // Own deadline, in case the app is gone and nobody kills this process.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(seconds.saturating_add(10)));
        std::process::exit(3);
    });
    let (file_name, bytes) = match read_request(std::io::stdin().lock()) {
        Ok(request) => request,
        Err(e) => {
            send(&Message::Failed {
                error: WireError::Invalid(format!("Interner Fehler beim Import: {e}")),
            });
            return 2;
        }
    };
    let self_test = if cfg!(debug_assertions) {
        std::env::var(SELF_TEST_ENV).ok()
    } else {
        None
    };
    let worker = std::thread::Builder::new()
        .name("docs-worker".into())
        .stack_size(WORKER_STACK)
        .spawn(move || serve(&file_name, &bytes, self_test.as_deref()));
    match worker.map(|w| w.join()) {
        Ok(Ok(())) => 0,
        _ => {
            send(&Message::Failed {
                error: WireError::Invalid("Interner Fehler beim Import.".into()),
            });
            1
        }
    }
}

fn serve(file_name: &str, bytes: &[u8], self_test: Option<&str>) {
    match self_test {
        Some("noguard") => super::pdf::skip_guard_for_self_test(),
        Some(mode) => misbehave(mode),
        None => {}
    }
    let result = super::import_bytes(file_name, bytes, |p| {
        if let Progress::Phase { message } = p {
            send(&Message::Progress { message });
        }
    });
    send(&match result {
        Ok(result) => Message::Done {
            result: Box::new(result),
        },
        Err(e) => Message::Failed { error: e.into() },
    });
}

/// Debug self-tests: the failure modes the parent must survive.
fn misbehave(mode: &str) {
    fn deep(n: u64) -> u64 {
        let frame = std::hint::black_box([n; 64]);
        if n == u64::MAX {
            return frame[1];
        }
        deep(n + 1).wrapping_add(frame[3])
    }
    match mode {
        "stack" => {
            std::hint::black_box(deep(0));
        }
        "alloc" => {
            let mut hoard = Vec::new();
            loop {
                hoard.push(std::hint::black_box(vec![1u8; 32 * 1024 * 1024]));
            }
        }
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        "chatty" => loop {
            send(&Message::Progress {
                message: "…".into(),
            });
            std::thread::sleep(Duration::from_millis(20));
        },
        "flood" => {
            let chunk = vec![b'x'; 1024 * 1024];
            let mut out = std::io::stdout().lock();
            while out.write_all(&chunk).is_ok() {}
            std::process::exit(0);
        }
        "garbage" => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(b"kein JSON\n").and_then(|_| out.flush());
            std::thread::sleep(Duration::from_secs(3600));
        }
        "exit" => std::process::exit(0),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_cross_the_process_boundary() {
        for e in [
            DocsError::TooManyPages(21),
            DocsError::Mismatch("png".into()),
            DocsError::OcrUnavailable("keine OCR".into()),
            DocsError::Invalid("kaputt".into()),
            DocsError::TooLarge(21),
            DocsError::Unsupported,
        ] {
            let text = e.to_string();
            let wire: WireError = e.into();
            let json = serde_json::to_string(&Message::Failed { error: wire }).unwrap();
            let Message::Failed { error } = serde_json::from_str(&json).unwrap() else {
                panic!("{json}");
            };
            assert_eq!(DocsError::from(error).to_string(), text);
        }
        let read = DocsError::Read(std::io::Error::other("weg"));
        assert_eq!(
            DocsError::from(WireError::from(read)).to_string(),
            "Die Datei konnte nicht gelesen werden: weg"
        );
    }

    #[test]
    fn request_is_header_line_plus_bytes() {
        let mut input = br#"{"fileName":"a.pdf","size":5}"#.to_vec();
        input.push(b'\n');
        input.extend_from_slice(b"%PDF-rest-ignored");
        let (name, bytes) = read_request(input.as_slice()).unwrap();
        assert_eq!(name, "a.pdf");
        assert_eq!(bytes, b"%PDF-");
        // Truncated and oversized requests are refused.
        assert!(read_request(&br#"{"fileName":"a.pdf","size":5}"#[..]).is_err());
        let big = format!(
            "{{\"fileName\":\"a.pdf\",\"size\":{}}}\n",
            MAX_FILE_BYTES + 1
        );
        assert!(read_request(big.as_bytes()).is_err());
    }

    #[test]
    fn failure_messages_are_german() {
        let m = failure_message("blatt.pdf", &Failure::Memory(MEMORY_LIMIT));
        assert!(
            m.starts_with(
                "Das PDF konnte nicht sicher gelesen werden (das Lesen brauchte mehr als 1536 MB"
            ),
            "{m}"
        );
        let m = failure_message("bild.png", &Failure::Timeout(Duration::from_secs(60)));
        assert!(
            m.starts_with("Die Datei konnte nicht sicher gelesen werden"),
            "{m}"
        );
        assert!(m.contains("60 s"), "{m}");
        assert!(failure_message("a.pdf", &Failure::Crashed(None)).contains("abgebrochen"));
    }

    #[test]
    fn missing_worker_binary_is_an_error() {
        let config = WorkerConfig::new(PathBuf::from("/nicht/vorhanden/code-pruefer"));
        let e = import(&config, "a.pdf", b"%PDF-1.4", |_| {})
            .unwrap_err()
            .to_string();
        assert!(e.contains("konnte nicht gestartet werden"), "{e}");
    }
}
