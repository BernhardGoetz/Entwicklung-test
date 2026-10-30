//! llama.cpp `llama-server` as a sidecar process.
//!
//! Release builds ship the official llama.cpp binaries (version and SHA-256
//! pinned in `scripts/fetch-llama-server.sh`) in `<resource_dir>/llama/`.
//! The server listens on 127.0.0.1 on a free port and requires a random API
//! key (passed in the environment, not on the command line), so other local
//! programs cannot use it. Before the first request the app checks that the
//! answering server is its own: a random model alias (`--alias`) must come
//! back from the authenticated `/v1/models`. One server runs at a time (for
//! one model); it is restarted when the model changes and killed on cancel
//! and when the app exits. If the app itself dies, a Windows job object
//! kills the server with it; on macOS/Linux the next app start kills a
//! leftover server found via its pid file (after checking its executable).
//!
//! Models without a verified catalog hash (imported, unpinned) run with
//! llama.cpp's built-in ChatML template instead of the Jinja template in
//! the file ([`ChatTemplate`]).

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

use super::AiError;
use crate::runner::languages::{override_from, DEV_OVERRIDES};

/// Pinned llama.cpp release (same as `version` in
/// `scripts/fetch-llama-server.sh`, which also holds the SHA-256 per asset).
pub const LLAMA_CPP_VERSION: &str = "b11700";
/// Subdirectory of the resource dir with llama-server and its libraries.
pub const RUNTIME_SUBDIR: &str = "llama";
/// Path to a llama-server binary (debug builds only, e.g. on Linux).
pub const SERVER_ENV: &str = "CODE_PRUEFER_LLAMA_SERVER";
/// Context size in tokens (prompt + answer) at least.
pub const CTX_SIZE: u32 = 8192;
/// Context per parallel slot; with `--kv-unified` the slots share one pool,
/// so a single long request can still use the whole context.
pub const CTX_PER_SLOT: u32 = 4096;
/// Loading a 7B model from a slow disk can take a while.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(120);
const HEALTH_INTERVAL: Duration = Duration::from_millis(300);
/// Upper bound for one completion (CPU, 7B, ~1500 tokens).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Lines of server log kept for error messages.
const STDERR_LINES: usize = 40;
/// Windows `STATUS_DLL_NOT_FOUND` (0xC0000135) as exit code.
const STATUS_DLL_NOT_FOUND: i32 = 0xC000_0135_u32 as i32;
/// Environment variable llama-server reads the API key from (b11700:
/// `--api-key`, "env: LLAMA_API_KEY").
pub const API_KEY_ENV: &str = "LLAMA_API_KEY";
/// Pid file of the running server in the app data directory.
pub const PID_FILE: &str = "llama-server.pid";

static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();
static STATE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Sets the app's resource directory (called once at app start).
pub fn init_resource_dir(dir: PathBuf) {
    let _ = RESOURCE_DIR.set(dir);
}

/// Sets the directory for the pid file and kills a server left over from a
/// crashed app (called once at app start).
pub fn init_state_dir(dir: PathBuf) {
    let pid_file = dir.join(PID_FILE);
    if let Ok(binary) = server_path() {
        if let Some(pid) = kill_stale(&pid_file, &binary) {
            eprintln!("llama-server: übrig gebliebenen Prozess {pid} beendet");
        }
    }
    let _ = STATE_DIR.set(dir);
}

pub fn server_binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// Finds llama-server: the dev override (if allowed) or
/// `<resource_dir>/llama/llama-server[.exe]`. Pure apart from `is_file`.
pub fn locate_server(
    resource_dir: Option<&Path>,
    env_override: Option<String>,
    allow_override: bool,
) -> Result<PathBuf, AiError> {
    if let Some(p) = override_from(env_override, "", allow_override) {
        let p = PathBuf::from(p);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(AiError::RuntimeMissing(format!(
                "{SERVER_ENV}={} existiert nicht",
                p.display()
            )))
        };
    }
    let Some(dir) = resource_dir else {
        return Err(AiError::RuntimeMissing("Ressourcenordner unbekannt".into()));
    };
    let p = dir.join(RUNTIME_SUBDIR).join(server_binary_name());
    if p.is_file() {
        Ok(p)
    } else {
        Err(AiError::RuntimeMissing(format!("gesucht: {}", p.display())))
    }
}

pub fn server_path() -> Result<PathBuf, AiError> {
    locate_server(
        RESOURCE_DIR.get().map(PathBuf::as_path),
        std::env::var(SERVER_ENV).ok(),
        DEV_OVERRIDES,
    )
}

/// Total context for `parallel` slots: `parallel` × [`CTX_PER_SLOT`], at
/// least [`CTX_SIZE`].
pub fn ctx_size(parallel: usize) -> u32 {
    (parallel.max(1) as u32 * CTX_PER_SLOT).max(CTX_SIZE)
}

/// Threads for prompt processing (batches): all logical cores; this part
/// is compute-bound and profits from hyper-threads.
pub fn batch_thread_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or_else(|_| thread_count())
        .clamp(1, 64)
}

/// Threads for inference: the physical cores (hyper-threads do not help).
pub fn thread_count() -> usize {
    sysinfo::System::physical_core_count()
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(4)
        .clamp(1, 32)
}

/// Which chat template llama-server uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatTemplate {
    /// The Jinja template in the GGUF file (catalog models with verified
    /// SHA-256: Qwen, ChatML).
    FromModel,
    /// llama.cpp's built-in ChatML template (`--chat-template chatml`); the
    /// file's template is not even parsed. For models without a verified
    /// hash, whose template could be anything.
    BuiltinChatml,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    pub model: PathBuf,
    pub port: u16,
    /// Not on the command line: [`API_KEY_ENV`] (see [`server_env`]).
    pub api_key: String,
    /// Random model name; `/v1/models` must return it ([`is_our_server`]).
    pub alias: String,
    pub template: ChatTemplate,
    pub threads: usize,
    pub threads_batch: usize,
    /// Parallel slots (solutions generated at the same time).
    pub parallel: usize,
    /// KV cache as q8_0 (half the memory of f16). Needs a head size that is
    /// a multiple of 32; otherwise the start is retried without.
    pub kv_quant: bool,
}

/// Command line of llama-server. Every flag was checked against
/// `llama-server --help` of the pinned build (b11700): `--flash-attn
/// on|off|auto`, `--cache-type-k/-v` (q8_0 allowed), `--threads-batch`,
/// `--parallel`, `--kv-unified`, `--alias`, `--chat-template chatml`
/// (built-in template; with jinja on, the name selects llama.cpp's own
/// ChatML source, the model's template is not read: common/chat.cpp
/// `common_chat_templates_init`). The API key is not an argument (visible
/// to other processes) but [`API_KEY_ENV`].
pub fn server_args(c: &ServerConfig) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-m".into(),
        c.model.display().to_string(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        c.port.to_string(),
        "--alias".into(),
        c.alias.clone(),
        "--ctx-size".into(),
        ctx_size(c.parallel).to_string(),
        "--threads".into(),
        c.threads.to_string(),
        "--threads-batch".into(),
        c.threads_batch.to_string(),
        // Solutions are generated at the same time (continuous batching):
        // on a CPU, memory bandwidth limits, so N sequences cost little
        // more than one. One KV pool for all slots, so every slot can use
        // the whole context (without it each slot gets ctx / N).
        "--parallel".into(),
        c.parallel.max(1).to_string(),
        "--kv-unified".into(),
        "--flash-attn".into(),
        "on".into(),
        "--jinja".into(),
        "--no-webui".into(),
        // Never download anything (e.g. via -hf); models come from us.
        "--offline".into(),
    ];
    if c.template == ChatTemplate::BuiltinChatml {
        args.extend(["--chat-template".into(), "chatml".into()]);
    }
    // No GPU offload on Windows/Linux (CPU build); Metal on macOS is used
    // automatically by the official build.
    if !cfg!(target_os = "macos") {
        args.extend(["--n-gpu-layers".into(), "0".into()]);
    }
    if c.kv_quant {
        // Half the memory of the f16 default: leaves more for Docker.
        for flag in ["--cache-type-k", "--cache-type-v"] {
            args.extend([flag.into(), "q8_0".into()]);
        }
    }
    args
}

/// Environment changes for llama-server: the API key, and no inherited
/// `LLAMA_*` variables (llama.cpp reads options such as `LLAMA_ARG_HOST`
/// from the environment). Returns (variables to remove, variables to set).
pub fn server_env(
    c: &ServerConfig,
    inherited: impl IntoIterator<Item = String>,
) -> (Vec<String>, Vec<(String, String)>) {
    let remove = inherited
        .into_iter()
        .filter(|k| k.to_ascii_uppercase().starts_with("LLAMA_"))
        .collect();
    (remove, vec![(API_KEY_ENV.into(), c.api_key.clone())])
}

/// Whether the `/v1/models` answer names our random alias (OpenAI format
/// `data[].id`, llama.cpp `models[].name`/`model`, plus `aliases`).
pub fn is_our_server(models_json: &Value, alias: &str) -> bool {
    let named = |m: &Value| {
        ["id", "name", "model"]
            .iter()
            .any(|k| m.get(k).and_then(Value::as_str) == Some(alias))
            || m.get("aliases")
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(alias)))
    };
    ["data", "models"].iter().any(|list| {
        models_json
            .get(list)
            .and_then(Value::as_array)
            .is_some_and(|ms| ms.iter().any(named))
    })
}

/// Contents of the pid file: process id and executable, so a reused pid
/// of another program is never killed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PidRecord {
    pub pid: u32,
    pub exe: PathBuf,
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Kills the server recorded in `pid_file` if that process still runs
/// *our* llama-server binary (`binary`), then removes the file. Returns the
/// killed pid.
pub fn kill_stale(pid_file: &Path, binary: &Path) -> Option<u32> {
    let record: Option<PidRecord> = std::fs::read(pid_file)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok());
    let _ = std::fs::remove_file(pid_file);
    let record = record?;
    if !same_file(&record.exe, binary) {
        return None;
    }
    let pid = sysinfo::Pid::from_u32(record.pid);
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always),
    );
    let process = system.process(pid)?;
    if !process.exe().is_some_and(|exe| same_file(exe, binary)) {
        return None;
    }
    // Its own process group (see `start_server`): end the whole group.
    #[cfg(unix)]
    // SAFETY: plain syscall; the pid was just verified to be our server.
    unsafe {
        libc::kill(-(record.pid as libc::pid_t), libc::SIGKILL);
    }
    process.kill();
    Some(record.pid)
}

fn pid_file() -> Option<PathBuf> {
    STATE_DIR.get().map(|d| d.join(PID_FILE))
}

fn write_pid_file(pid: u32, binary: &Path) {
    if let Some(path) = pid_file() {
        let record = PidRecord {
            pid,
            exe: binary.to_path_buf(),
        };
        if let Ok(raw) = serde_json::to_vec(&record) {
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
            let _ = std::fs::write(path, raw);
        }
    }
}

fn remove_pid_file() {
    if let Some(path) = pid_file() {
        let _ = std::fs::remove_file(path);
    }
}

/// The server refused the quantized KV cache (head size not a multiple of
/// the q8_0 block size, e.g. tiny test models).
pub fn kv_quant_unsupported(log_tail: &str) -> bool {
    log_tail.contains("cache type") && log_tail.contains("does not divide")
}

/// 32 random bytes as hex.
pub fn random_api_key() -> Result<String, AiError> {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf)
        .map_err(|e| AiError::Start(format!("Zufallszahlen nicht verfügbar: {e}")))?;
    Ok(super::hex(&buf))
}

/// A random model alias (identity check of the server, [`is_our_server`]).
pub fn random_alias() -> Result<String, AiError> {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf)
        .map_err(|e| AiError::Start(format!("Zufallszahlen nicht verfügbar: {e}")))?;
    Ok(format!("code-pruefer-{}", super::hex(&buf)))
}

/// A free TCP port on 127.0.0.1 (released again before llama-server binds
/// it; a failed bind shows up as a start error, and a foreign program that
/// grabs the port in between is detected by the alias check).
pub fn free_port() -> Result<u16, AiError> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| AiError::Start(format!("Kein freier Port: {e}")))
}

/// German explanation for an early exit of llama-server.
pub fn explain_exit(code: Option<i32>, log_tail: &str) -> String {
    if code == Some(STATUS_DLL_NOT_FOUND) {
        return "Eine benötigte Systembibliothek fehlt. Bitte die „Microsoft Visual C++ \
                Redistributable (x64)“ von Microsoft installieren \
                (https://aka.ms/vs/17/release/vc_redist.x64.exe) und es erneut versuchen."
            .into();
    }
    let lower = log_tail.to_ascii_lowercase();
    let hint = if lower.contains("failed to load model")
        || lower.contains("error loading model")
        || lower.contains("invalid magic")
    {
        " Die Modelldatei ist beschädigt oder passt nicht zu dieser KI-Laufzeit."
    } else if lower.contains("bind") || lower.contains("address already in use") {
        " Der Port war belegt – bitte erneut versuchen."
    } else if lower.contains("illegal instruction") {
        " Der Prozessor unterstützt die nötigen Befehle (z. B. AVX2) nicht."
    } else {
        ""
    };
    let code = code.map_or("unbekannt".to_string(), |c| c.to_string());
    let tail: String = log_tail
        .lines()
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    format!("llama-server wurde beendet (Exit-Code {code}).{hint}\n{tail}")
        .trim_end()
        .to_string()
}

/// Address and key of the running server.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: String,
}

struct Running {
    child: tokio::process::Child,
    model: PathBuf,
    parallel: usize,
    template: ChatTemplate,
    endpoint: Endpoint,
    /// Kills the server with the app (Windows).
    #[cfg(windows)]
    _job: Option<crate::proc_guard::KillJob>,
}

/// A started server (child process and how to reach it).
struct Started {
    child: tokio::process::Child,
    endpoint: Endpoint,
    #[cfg(windows)]
    job: Option<crate::proc_guard::KillJob>,
}

static SERVER: tokio::sync::Mutex<Option<Running>> = tokio::sync::Mutex::const_new(None);

fn local_http(timeout: Duration) -> Result<reqwest::Client, AiError> {
    reqwest::Client::builder()
        // Never route 127.0.0.1 through a configured proxy.
        .no_proxy()
        .timeout(timeout)
        .build()
        .map_err(|e| AiError::Http(e.to_string()))
}

/// Starts llama-server for `model` with `parallel` slots (or reuses the
/// running one) and waits until the model is loaded.
pub async fn ensure_running(
    model: &Path,
    parallel: usize,
    template: ChatTemplate,
    cancel: &CancellationToken,
    mut on_status: impl FnMut(String),
) -> Result<Endpoint, AiError> {
    let parallel = parallel.max(1);
    let mut guard = SERVER.lock().await;
    if let Some(r) = guard.as_mut() {
        let alive = matches!(r.child.try_wait(), Ok(None));
        if alive && r.model == model && r.parallel == parallel && r.template == template {
            return Ok(r.endpoint.clone());
        }
    }
    if let Some(old) = guard.take() {
        stop(old).await;
    }

    let binary = server_path()?;
    on_status("KI-Modell wird geladen …".into());
    let mut kv_quant = true;
    loop {
        let config = ServerConfig {
            model: model.to_path_buf(),
            port: free_port()?,
            api_key: random_api_key()?,
            alias: random_alias()?,
            template,
            threads: thread_count(),
            threads_batch: batch_thread_count(),
            parallel,
            kv_quant,
        };
        match start_server(&binary, &config, cancel, &mut on_status).await {
            Ok(started) => {
                let endpoint = started.endpoint.clone();
                *guard = Some(Running {
                    child: started.child,
                    model: model.to_path_buf(),
                    parallel,
                    template,
                    endpoint: endpoint.clone(),
                    #[cfg(windows)]
                    _job: started.job,
                });
                return Ok(endpoint);
            }
            Err(StartFailure::KvQuant) if kv_quant => kv_quant = false,
            Err(StartFailure::KvQuant) => {
                return Err(AiError::Start(
                    "Der KV-Cache konnte nicht angelegt werden.".into(),
                ))
            }
            Err(StartFailure::Error(e)) => return Err(e),
        }
    }
}

enum StartFailure {
    /// Retry without the quantized KV cache.
    KvQuant,
    Error(AiError),
}

impl From<AiError> for StartFailure {
    fn from(e: AiError) -> Self {
        Self::Error(e)
    }
}

/// Spawns llama-server, waits for `/health` and checks that the answering
/// server is ours (alias via authenticated `/v1/models`).
async fn start_server(
    binary: &Path,
    config: &ServerConfig,
    cancel: &CancellationToken,
    on_status: &mut impl FnMut(String),
) -> Result<Started, StartFailure> {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args(server_args(config))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let (remove, set) = server_env(config, std::env::vars().map(|(k, _)| k));
    for k in remove {
        cmd.env_remove(k);
    }
    cmd.envs(set);
    // Shared libraries (ggml-*.dll / .dylib) live next to the binary.
    if let Some(dir) = binary.parent() {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                                     // Own process group: a stale server is killed as a group
                                     // (`kill_stale`), and a terminal's Ctrl+C reaches only the app, which
                                     // then ends the server itself.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|e| AiError::Start(format!("{}: {e}", binary.display())))?;
    // Windows: the job kills the server when the app ends in any way. If
    // it cannot be created, the server still runs (and is ended on exit).
    #[cfg(windows)]
    let job = child.raw_handle().and_then(|handle| {
        let job = crate::proc_guard::KillJob::new(None).ok()?;
        job.assign(handle).ok()?;
        Some(job)
    });
    if let Some(pid) = child.id() {
        write_pid_file(pid, binary);
    }

    let log = Arc::new(Mutex::new(VecDeque::<String>::new()));
    if let Some(stderr) = child.stderr.take() {
        let log = log.clone();
        // Must keep draining stderr, or the server blocks on a full pipe.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut l) = log.lock() {
                    if l.len() >= STDERR_LINES {
                        l.pop_front();
                    }
                    l.push_back(line);
                }
            }
        });
    }
    let log_tail = || {
        log.lock()
            .map(|l| l.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    };

    let endpoint = Endpoint {
        base_url: format!("http://127.0.0.1:{}", config.port),
        api_key: config.api_key.clone(),
    };
    let http = local_http(Duration::from_secs(2))?;
    let started = Instant::now();
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill().await;
            remove_pid_file();
            return Err(AiError::Cancelled.into());
        }
        if let Ok(Some(status)) = child.try_wait() {
            remove_pid_file();
            // Give the log reader a moment to collect the last lines.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let tail = log_tail();
            if config.kv_quant && kv_quant_unsupported(&tail) {
                return Err(StartFailure::KvQuant);
            }
            return Err(AiError::Start(explain_exit(status.code(), &tail)).into());
        }
        if let Ok(resp) = http
            .get(format!("{}/health", endpoint.base_url))
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Err(e) = verify_identity(&http, &endpoint, &config.alias, &mut child).await {
                    let _ = child.kill().await;
                    remove_pid_file();
                    return Err(e.into());
                }
                return Ok(Started {
                    child,
                    endpoint,
                    #[cfg(windows)]
                    job,
                });
            }
        }
        let waited = started.elapsed();
        if waited >= HEALTH_TIMEOUT {
            let _ = child.kill().await;
            remove_pid_file();
            return Err(AiError::Start(format!(
                "Das Modell wurde nach {} s noch nicht geladen.\n{}",
                waited.as_secs(),
                log_tail()
            ))
            .into());
        }
        on_status(format!("KI-Modell wird geladen … ({} s)", waited.as_secs()));
        tokio::time::sleep(HEALTH_INTERVAL).await;
    }
}

/// After `/health`: the server on the port must know our API key and
/// report our random alias, and our child must still be running (a foreign
/// program that took the port would get the key, but could not know the
/// alias; then llama-server itself fails to bind and exits).
async fn verify_identity(
    http: &reqwest::Client,
    endpoint: &Endpoint,
    alias: &str,
    child: &mut tokio::process::Child,
) -> Result<(), AiError> {
    let foreign = || {
        AiError::Start(
            "Auf dem Port der KI-Laufzeit antwortet ein anderes Programm. Bitte erneut \
             versuchen; tritt das wiederholt auf, den Rechner neu starten."
                .into(),
        )
    };
    let resp = http
        .get(format!("{}/v1/models", endpoint.base_url))
        .bearer_auth(&endpoint.api_key)
        .send()
        .await
        .map_err(|_| foreign())?;
    if !resp.status().is_success() {
        return Err(foreign());
    }
    let body: Value = resp.json().await.map_err(|_| foreign())?;
    if !is_our_server(&body, alias) || !matches!(child.try_wait(), Ok(None)) {
        return Err(foreign());
    }
    Ok(())
}

/// Starts llama-server for the selected model in the background, so the
/// first request does not wait for the model to load (task confirmed,
/// model selected). Returns `false` if nothing is to be done (no model,
/// no runtime, already running).
pub async fn warm_up(cancel: &CancellationToken) -> Result<bool, AiError> {
    let model = super::models::selected_model()?;
    server_path()?;
    {
        let mut guard = SERVER.lock().await;
        if let Some(r) = guard.as_mut() {
            if matches!(r.child.try_wait(), Ok(None))
                && r.model == model.path
                && r.parallel == model.server_slots()
                && r.template == model.chat_template()
            {
                return Ok(false);
            }
        }
    }
    ensure_running(
        &model.path,
        model.server_slots(),
        model.chat_template(),
        cancel,
        |_| {},
    )
    .await?;
    Ok(true)
}

async fn stop(mut r: Running) {
    let _ = r.child.kill().await;
    remove_pid_file();
}

/// Stops the server, if any (cancel, model change, app exit).
pub async fn shutdown() {
    if let Some(r) = SERVER.lock().await.take() {
        stop(r).await;
    }
}

// ---------------------------------------------------------------------------
// Chat client

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn system(s: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: s.into(),
        }
    }
    pub fn user(s: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: s.into(),
        }
    }
    pub fn assistant(s: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: s.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_tokens: u32,
    /// JSON schema the answer must follow (grammar-constrained by llama-server).
    pub json_schema: Option<Value>,
    pub seed: Option<u64>,
    /// Start of the answer, written into the prompt after the chat template
    /// (e.g. the opening code fence): the model continues from there and the
    /// response content starts with it.
    pub prefill: Option<String>,
    /// Generation stops at the first of these strings (e.g. the closing
    /// code fence: no explanation after the code).
    pub stop: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatResponse {
    pub content: String,
    pub completion_tokens: Option<u64>,
    pub tokens_per_second: Option<f64>,
    /// "stop" or "length" (answer cut off at `max_tokens`).
    pub finish_reason: Option<String>,
    /// The stop string that ended the answer (not part of `content`).
    pub stopping_word: Option<String>,
}

/// A chat model. Implemented by [`LlamaClient`]; mocked in tests.
pub trait Llm {
    fn chat(&self, req: &ChatRequest)
        -> impl Future<Output = Result<ChatResponse, AiError>> + Send;
}

// The client uses llama-server's native endpoints: `/apply-template`
// renders the messages with the model's chat template, `/completion`
// generates with an optional `json_schema`, from which the server builds a
// grammar that constrains sampling. (The OpenAI-compatible chat endpoint
// of the pinned build wraps that grammar into its chat-format parser,
// which failed with a ChatML test model; the native path does not.)

/// Body for `/apply-template`.
pub fn template_body(req: &ChatRequest) -> Value {
    json!({ "messages": req.messages })
}

/// Body for `/completion` with the rendered prompt.
pub fn completion_body(prompt: &str, req: &ChatRequest) -> Value {
    let mut body = json!({
        "prompt": prompt,
        "temperature": req.temperature,
        "n_predict": req.max_tokens,
        "stream": false,
        "cache_prompt": true,
    });
    if let Some(schema) = &req.json_schema {
        body["json_schema"] = schema.clone();
    }
    if let Some(seed) = req.seed {
        body["seed"] = json!(seed);
    }
    if !req.stop.is_empty() {
        body["stop"] = json!(req.stop);
    }
    body
}

/// Prompt for `/completion`: the rendered template plus the prefill.
pub fn full_prompt(template: &str, req: &ChatRequest) -> String {
    format!("{template}{}", req.prefill.as_deref().unwrap_or(""))
}

/// The complete answer: prefill, generated text and the stop string that
/// ended it (so a code block keeps its closing fence, e.g. for the repair
/// conversation).
pub fn assemble_answer(req: &ChatRequest, mut r: ChatResponse) -> ChatResponse {
    let prefill = req.prefill.as_deref().unwrap_or("");
    let stop = r.stopping_word.as_deref().unwrap_or("");
    r.content = format!("{prefill}{}{stop}", r.content);
    r
}

#[derive(Deserialize)]
struct RawTemplate {
    prompt: String,
}

#[derive(Deserialize)]
struct RawCompletion {
    #[serde(default)]
    content: String,
    tokens_predicted: Option<u64>,
    stop_type: Option<String>,
    #[serde(default)]
    stopping_word: String,
    timings: Option<RawTimings>,
}
#[derive(Deserialize)]
struct RawTimings {
    predicted_per_second: Option<f64>,
}

pub fn parse_template(raw: &str) -> Result<String, AiError> {
    serde_json::from_str::<RawTemplate>(raw)
        .map(|t| t.prompt)
        .map_err(|e| AiError::Http(format!("Unerwartete Antwort ({e})")))
}

/// Parses a `/completion` answer (incl. `timings` for tokens/s).
pub fn parse_completion(raw: &str) -> Result<ChatResponse, AiError> {
    let r: RawCompletion = serde_json::from_str(raw)
        .map_err(|e| AiError::Http(format!("Unerwartete Antwort ({e})")))?;
    Ok(ChatResponse {
        content: r.content,
        completion_tokens: r.tokens_predicted,
        tokens_per_second: r
            .timings
            .and_then(|t| t.predicted_per_second)
            .filter(|v| v.is_finite() && *v > 0.0),
        stopping_word: (r.stop_type.as_deref() == Some("word") && !r.stopping_word.is_empty())
            .then_some(r.stopping_word),
        // "limit": cut off at n_predict.
        finish_reason: r.stop_type.map(|t| {
            if t == "limit" {
                "length".to_string()
            } else {
                "stop".to_string()
            }
        }),
    })
}

/// HTTP client for the running llama-server.
pub struct LlamaClient {
    http: reqwest::Client,
    endpoint: Endpoint,
    cancel: CancellationToken,
    /// `/apply-template` results by exact message list. The template output
    /// depends on every message, so only identical requests (the N
    /// solution requests of one run, JSON retries) share an entry; a cache
    /// "per prompt type" with placeholders would be wrong for templates
    /// that escape or transform the content. The lock is held during the
    /// request, so concurrent identical requests cost one call.
    templates: tokio::sync::Mutex<HashMap<String, String>>,
    template_calls: AtomicUsize,
}

impl LlamaClient {
    pub fn new(endpoint: Endpoint, cancel: CancellationToken) -> Result<Self, AiError> {
        Ok(Self {
            http: local_http(REQUEST_TIMEOUT)?,
            endpoint,
            cancel,
            templates: tokio::sync::Mutex::new(HashMap::new()),
            template_calls: AtomicUsize::new(0),
        })
    }

    /// Number of `/apply-template` requests sent (cache misses).
    pub fn template_calls(&self) -> usize {
        self.template_calls.load(AtomicOrdering::Relaxed)
    }

    async fn template(&self, req: &ChatRequest) -> Result<String, AiError> {
        let key = serde_json::to_string(&req.messages).map_err(|e| AiError::Http(e.to_string()))?;
        let mut cache = self.templates.lock().await;
        if let Some(p) = cache.get(&key) {
            return Ok(p.clone());
        }
        self.template_calls.fetch_add(1, AtomicOrdering::Relaxed);
        let prompt = parse_template(&self.post("/apply-template", &template_body(req)).await?)?;
        cache.insert(key, prompt.clone());
        Ok(prompt)
    }

    async fn post(&self, path: &str, body: &Value) -> Result<String, AiError> {
        let resp = self
            .http
            .post(format!("{}{path}", self.endpoint.base_url))
            .bearer_auth(&self.endpoint.api_key)
            .json(body)
            .send()
            .await
            .map_err(|e| AiError::Http(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| AiError::Http(e.to_string()))?;
        if !status.is_success() {
            let snippet: String = text.chars().take(300).collect();
            return Err(AiError::Http(format!("HTTP {status}: {snippet}")));
        }
        Ok(text)
    }
}

impl Llm for LlamaClient {
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, AiError> {
        let send = async {
            let prompt = full_prompt(&self.template(req).await?, req);
            let raw = self
                .post("/completion", &completion_body(&prompt, req))
                .await?;
            Ok(assemble_answer(req, parse_completion(&raw)?))
        };
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(AiError::Cancelled),
            r = send => r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_version_matches_fetch_script() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let script =
            std::fs::read_to_string(root.join("../scripts/fetch-llama-server.sh")).unwrap();
        assert!(
            script.contains(&format!("version=\"{LLAMA_CPP_VERSION}\"")),
            "fetch-llama-server.sh pins another llama.cpp version"
        );
        let notices = std::fs::read_to_string(root.join("../THIRD_PARTY_NOTICES.txt")).unwrap();
        assert!(notices.contains(&format!("Release {LLAMA_CPP_VERSION}")));
    }

    #[test]
    fn locate_server_prefers_allowed_override() {
        let dir = std::env::temp_dir().join(format!("cp-llama-{}", std::process::id()));
        let bin_dir = dir.join(RUNTIME_SUBDIR);
        std::fs::create_dir_all(&bin_dir).unwrap();
        // Missing binary: clear German message with the searched path.
        let e = locate_server(Some(&dir), None, true)
            .unwrap_err()
            .to_string();
        assert!(e.starts_with("KI-Laufzeit nicht gefunden"), "{e}");
        assert!(e.contains(server_binary_name()), "{e}");
        std::fs::write(bin_dir.join(server_binary_name()), b"x").unwrap();
        assert_eq!(
            locate_server(Some(&dir), None, true).unwrap(),
            bin_dir.join(server_binary_name())
        );
        // Override only when allowed (debug builds).
        let other = dir.join("other-server");
        std::fs::write(&other, b"x").unwrap();
        let ov = Some(other.display().to_string());
        assert_eq!(locate_server(Some(&dir), ov.clone(), true).unwrap(), other);
        assert_eq!(
            locate_server(Some(&dir), ov, false).unwrap(),
            bin_dir.join(server_binary_name())
        );
        let e = locate_server(Some(&dir), Some("/nicht/da".into()), true).unwrap_err();
        assert!(e.to_string().contains(SERVER_ENV));
        assert!(locate_server(None, None, true).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn config(template: ChatTemplate) -> ServerConfig {
        ServerConfig {
            model: "/m/model.gguf".into(),
            port: 4242,
            api_key: "k".repeat(64),
            alias: "code-pruefer-abc".into(),
            template,
            threads: 6,
            threads_batch: 12,
            parallel: 4,
            kv_quant: true,
        }
    }

    #[test]
    fn server_args_bind_localhost_with_key() {
        let args = server_args(&config(ChatTemplate::FromModel));
        let pair = |flag: &str| {
            let i = args.iter().position(|a| a == flag).expect(flag);
            args[i + 1].clone()
        };
        assert_eq!(pair("-m"), "/m/model.gguf");
        assert_eq!(pair("--host"), "127.0.0.1");
        assert_eq!(pair("--port"), "4242");
        assert_eq!(pair("--alias"), "code-pruefer-abc");
        // The key is never on the command line.
        assert!(!args
            .iter()
            .any(|a| a == "--api-key" || a.contains(&"k".repeat(64))));
        assert!(!args.iter().any(|a| a == "--chat-template"));
        assert_eq!(pair("--ctx-size"), "16384");
        assert_eq!(pair("--threads"), "6");
        assert_eq!(pair("--threads-batch"), "12");
        assert_eq!(pair("--parallel"), "4");
        assert_eq!(pair("--flash-attn"), "on");
        assert_eq!(pair("--cache-type-k"), "q8_0");
        assert_eq!(pair("--cache-type-v"), "q8_0");
        for flag in ["--no-webui", "--offline", "--jinja", "--kv-unified"] {
            assert!(args.iter().any(|a| a == flag), "{flag}");
        }
        let plain = server_args(&ServerConfig {
            kv_quant: false,
            ..config(ChatTemplate::FromModel)
        });
        assert!(!plain.iter().any(|a| a.starts_with("--cache-type")));
    }

    #[test]
    fn unverified_models_get_the_builtin_template() {
        let args = server_args(&config(ChatTemplate::BuiltinChatml));
        let i = args.iter().position(|a| a == "--chat-template").unwrap();
        assert_eq!(args[i + 1], "chatml");
        assert!(!args.iter().any(|a| a == "--chat-template-file"));
    }

    #[test]
    fn api_key_in_environment_and_foreign_llama_variables_removed() {
        let (remove, set) = server_env(
            &config(ChatTemplate::FromModel),
            [
                "PATH",
                "LLAMA_ARG_HOST",
                "llama_arg_port",
                "LLAMA_API_KEY",
                "HOME",
            ]
            .map(String::from),
        );
        assert_eq!(
            remove,
            ["LLAMA_ARG_HOST", "llama_arg_port", "LLAMA_API_KEY"]
        );
        assert_eq!(set, [(API_KEY_ENV.to_string(), "k".repeat(64))]);
    }

    #[test]
    fn identity_check_needs_our_alias() {
        // b11700 answers with both lists (tools/server: get_res_models).
        let ours = json!({
            "models": [{"name": "code-pruefer-abc", "model": "code-pruefer-abc"}],
            "object": "list",
            "data": [{"id": "code-pruefer-abc", "aliases": ["code-pruefer-abc"]}]
        });
        assert!(is_our_server(&ours, "code-pruefer-abc"));
        assert!(is_our_server(
            &json!({"data": [{"id": "x", "aliases": ["code-pruefer-abc"]}]}),
            "code-pruefer-abc"
        ));
        assert!(!is_our_server(&ours, "code-pruefer-other"));
        assert!(!is_our_server(&json!({"data": []}), "code-pruefer-abc"));
        assert!(!is_our_server(
            &json!("code-pruefer-abc"),
            "code-pruefer-abc"
        ));
        let a = random_alias().unwrap();
        assert!(a.starts_with("code-pruefer-") && a.len() == 13 + 32, "{a}");
        assert_ne!(a, random_alias().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn stale_server_is_killed_only_if_it_is_ours() {
        let dir = std::env::temp_dir().join(format!("code-pruefer-pid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join(PID_FILE);
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let exe = {
            let mut system = sysinfo::System::new();
            let p = sysinfo::Pid::from_u32(pid);
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[p]),
                true,
                sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always),
            );
            system
                .process(p)
                .and_then(|p| p.exe())
                .unwrap()
                .to_path_buf()
        };
        let write = |exe: &Path| {
            let raw = serde_json::to_vec(&PidRecord {
                pid,
                exe: exe.to_path_buf(),
            })
            .unwrap();
            std::fs::write(&pid_file, raw).unwrap();
        };
        // Recorded for another binary: the process is left alone.
        write(Path::new("/anderes/llama-server"));
        assert_eq!(kill_stale(&pid_file, &exe), None);
        assert!(!pid_file.exists());
        assert!(matches!(child.try_wait(), Ok(None)));
        // Recorded binary is not what the pid runs now (pid reused).
        write(&exe);
        assert_eq!(
            kill_stale(&pid_file, Path::new("/anderes/llama-server")),
            None
        );
        assert!(matches!(child.try_wait(), Ok(None)));
        // Ours: killed.
        write(&exe);
        assert_eq!(kill_stale(&pid_file, &exe), Some(pid));
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(child.try_wait().unwrap().is_some());
        // No pid file, garbage: nothing happens.
        assert_eq!(kill_stale(&pid_file, &exe), None);
        std::fs::write(&pid_file, "kaputt").unwrap();
        assert_eq!(kill_stale(&pid_file, &exe), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn context_grows_with_slots() {
        assert_eq!(ctx_size(0), CTX_SIZE);
        assert_eq!(ctx_size(1), CTX_SIZE);
        assert_eq!(ctx_size(2), CTX_SIZE);
        assert_eq!(ctx_size(4), 4 * CTX_PER_SLOT);
        assert_eq!(ctx_size(6), 6 * CTX_PER_SLOT);
        assert!(batch_thread_count() >= 1);
        // The b11700 message for head sizes q8_0 cannot hold.
        assert!(kv_quant_unsupported(
            "E llama_init_from_model: K cache type q8_0 with block size 32 does not divide n_embd_head_k=16"
        ));
        assert!(!kv_quant_unsupported("error loading model"));
    }

    #[test]
    fn prefill_and_stop_make_a_complete_code_block() {
        let req = ChatRequest {
            messages: vec![ChatMessage::user("u")],
            temperature: 0.7,
            max_tokens: 10,
            json_schema: None,
            seed: None,
            prefill: Some("```python\n".into()),
            stop: vec!["\n```".into()],
        };
        assert_eq!(full_prompt("<t>", &req), "<t>```python\n");
        let b = completion_body("p", &req);
        assert_eq!(b["stop"], json!(["\n```"]));
        // Stopped at the closing fence: the fence is put back.
        let raw = r#"{"content":"print(1)","stop_type":"word","stopping_word":"\n```"}"#;
        let r = assemble_answer(&req, parse_completion(raw).unwrap());
        assert_eq!(r.content, "```python\nprint(1)\n```");
        assert_eq!(
            crate::ai::prompts::extract_code(&r.content, "python").as_deref(),
            Some("print(1)\n")
        );
        // Cut off at the token limit: no fence appended.
        let raw = r#"{"content":"print(","stop_type":"limit","stopping_word":""}"#;
        let r = assemble_answer(&req, parse_completion(raw).unwrap());
        assert_eq!(r.content, "```python\nprint(");
        assert_eq!(r.finish_reason.as_deref(), Some("length"));
    }

    /// Minimal llama-server stand-in: counts requests per path.
    async fn fake_server(counts: Arc<Mutex<HashMap<String, usize>>>) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let counts = counts.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).to_string();
                        let Some(end) = text.find("\r\n\r\n") else {
                            continue;
                        };
                        let len: usize = text[..end]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if buf.len() < end + 4 + len {
                            continue;
                        }
                        let path = text.split_whitespace().nth(1).unwrap_or("").to_string();
                        *counts.lock().unwrap().entry(path.clone()).or_default() += 1;
                        let body = if path == "/apply-template" {
                            r#"{"prompt":"<p>"}"#
                        } else {
                            r#"{"content":"x","tokens_predicted":1,"stop_type":"eos"}"#
                        };
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                        return;
                    }
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn template_is_fetched_once_per_message_list() {
        let counts = Arc::new(Mutex::new(HashMap::new()));
        let addr = fake_server(counts.clone()).await;
        let client = LlamaClient::new(
            Endpoint {
                base_url: format!("http://{addr}"),
                api_key: "k".into(),
            },
            CancellationToken::new(),
        )
        .unwrap();
        let req = |u: &str| ChatRequest {
            messages: vec![ChatMessage::system("s"), ChatMessage::user(u)],
            temperature: 0.7,
            max_tokens: 5,
            json_schema: None,
            seed: None,
            prefill: None,
            stop: Vec::new(),
        };
        // Three identical solution requests at the same time, then another prompt.
        let same = req("task");
        let rs = futures_util::future::join_all((0..3).map(|_| client.chat(&same))).await;
        assert!(rs.iter().all(|r| r.as_ref().unwrap().content == "x"));
        client.chat(&req("other")).await.unwrap();
        let c = counts.lock().unwrap().clone();
        assert_eq!(c.get("/apply-template"), Some(&2), "{c:?}");
        assert_eq!(c.get("/completion"), Some(&4), "{c:?}");
        assert_eq!(client.template_calls(), 2);
    }

    #[test]
    fn api_keys_are_random_hex() {
        let a = random_api_key().unwrap();
        let b = random_api_key().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn free_port_is_nonzero() {
        assert!(free_port().unwrap() > 0);
    }

    #[test]
    fn exit_explanations() {
        let m = explain_exit(Some(-1073741515), "");
        assert!(m.contains("Visual C++"), "{m}");
        let m = explain_exit(Some(1), "a\nllama_model_load: error loading model: x\n");
        assert!(m.contains("beschädigt"), "{m}");
        assert!(m.contains("Exit-Code 1"), "{m}");
        let m = explain_exit(None, "");
        assert!(m.contains("unbekannt"), "{m}");
    }

    #[test]
    fn request_bodies() {
        let req = ChatRequest {
            messages: vec![ChatMessage::system("s"), ChatMessage::user("u")],
            temperature: 0.7,
            max_tokens: 100,
            json_schema: Some(json!({"type": "object"})),
            seed: Some(7),
            prefill: None,
            stop: Vec::new(),
        };
        let t = template_body(&req);
        assert_eq!(t["messages"][0]["role"], "system");
        assert_eq!(t["messages"][1]["content"], "u");
        let b = completion_body("<p>", &req);
        assert_eq!(b["prompt"], "<p>");
        assert_eq!(b["n_predict"], 100);
        assert_eq!(b["stream"], false);
        assert_eq!(b["seed"], 7);
        assert_eq!(b["json_schema"]["type"], "object");
        let plain = completion_body(
            "x",
            &ChatRequest {
                json_schema: None,
                seed: None,
                ..req
            },
        );
        assert!(plain.get("json_schema").is_none());
        assert!(plain.get("seed").is_none());
    }

    #[test]
    fn parse_answers() {
        assert_eq!(
            parse_template(r#"{"prompt":"<|im_start|>user"}"#).unwrap(),
            "<|im_start|>user"
        );
        assert!(parse_template("{}").is_err());
        let raw = r#"{"index":0,"content":"hallo","tokens_predicted":12,"stop":true,
            "stop_type":"eos","timings":{"predicted_per_second":21.5,"prompt_n":3}}"#;
        let r = parse_completion(raw).unwrap();
        assert_eq!(r.content, "hallo");
        assert_eq!(r.completion_tokens, Some(12));
        assert_eq!(r.tokens_per_second, Some(21.5));
        assert_eq!(r.finish_reason.as_deref(), Some("stop"));
        let r = parse_completion(r#"{"content":"x","stop_type":"limit"}"#).unwrap();
        assert_eq!(r.finish_reason.as_deref(), Some("length"));
        assert_eq!(r.tokens_per_second, None);
        assert!(parse_completion("kaputt").is_err());
    }
}
