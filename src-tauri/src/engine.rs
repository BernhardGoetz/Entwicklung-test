//! Container engine choice (Docker Desktop or Podman), starting the chosen
//! engine on demand and engine-specific connection endpoints.
//!
//! The choice is read from, in order of precedence:
//! 1. `engine.json` in the app config directory (`{"engine":"podman"}`),
//!    written by the engine switch in the app;
//! 2. on Windows the registry value `HKCU\Software\Code-Pruefer\Engine`
//!    (`docker` | `podman`), written by the installer;
//! 3. the default: Docker.
//!
//! The choice only orders the connection candidates (the chosen engine is
//! tried first) and decides which engine is started; an explicit
//! `DOCKER_HOST` still wins exclusively (see `runner::docker::candidate_hosts`).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::runner::docker;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    #[default]
    Docker,
    Podman,
}

impl Engine {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "docker" => Some(Self::Docker),
            "podman" => Some(Self::Podman),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Docker => "Docker Desktop",
            Self::Podman => "Podman",
        }
    }

    pub fn download_url(self) -> &'static str {
        match self {
            Self::Docker => "https://www.docker.com/products/docker-desktop/",
            Self::Podman => "https://podman.io/",
        }
    }
}

/// Where the effective choice came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChoiceSource {
    Config,
    Registry,
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineChoice {
    pub engine: Engine,
    pub source: ChoiceSource,
}

pub const CONFIG_FILE: &str = "engine.json";
pub const REGISTRY_KEY: &str = r"Software\Code-Pruefer";
pub const REGISTRY_VALUE: &str = "Engine";

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// How long to wait for the API after launching the engine.
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// `podman machine init` downloads a VM image on first use.
const PODMAN_INIT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const PODMAN_CMD_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const PODMAN_QUERY_TIMEOUT: Duration = Duration::from_secs(15);

static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();
/// Podman's API endpoint as reported by `podman machine inspect`
/// (Windows: named pipe, macOS: unix socket of the machine).
static PODMAN_PIPE: Mutex<Option<String>> = Mutex::new(None);
static PODMAN_PIPE_QUERIED: AtomicBool = AtomicBool::new(false);
static STARTING: AtomicBool = AtomicBool::new(false);

/// Sets the app config directory (called once at app start).
pub fn init(config_dir: PathBuf) {
    let _ = CONFIG_DIR.set(config_dir);
}

/// Precedence: config file > registry > default (Docker).
pub fn resolve(config: Option<Engine>, registry: Option<Engine>) -> EngineChoice {
    match (config, registry) {
        (Some(engine), _) => EngineChoice {
            engine,
            source: ChoiceSource::Config,
        },
        (None, Some(engine)) => EngineChoice {
            engine,
            source: ChoiceSource::Registry,
        },
        (None, None) => EngineChoice {
            engine: Engine::Docker,
            source: ChoiceSource::Default,
        },
    }
}

/// The effective engine choice.
pub fn current() -> EngineChoice {
    resolve(
        CONFIG_DIR.get().and_then(|d| read_config(d)),
        read_registry(),
    )
}

/// Stores the choice in the app config file (takes precedence over the registry).
pub fn save(engine: Engine) -> Result<EngineChoice, String> {
    let dir = CONFIG_DIR
        .get()
        .ok_or_else(|| "Konfigurationsordner ist nicht bekannt.".to_string())?;
    write_config(dir, engine)?;
    Ok(current())
}

#[derive(Serialize, Deserialize)]
struct ConfigFile {
    engine: String,
}

fn parse_config(json: &str) -> Option<Engine> {
    serde_json::from_str::<ConfigFile>(json)
        .ok()
        .and_then(|c| Engine::parse(&c.engine))
}

fn read_config(dir: &Path) -> Option<Engine> {
    parse_config(&std::fs::read_to_string(dir.join(CONFIG_FILE)).ok()?)
}

fn write_config(dir: &Path, engine: Engine) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&ConfigFile {
        engine: serde_json::to_value(engine)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default(),
    })
    .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir)
        .and_then(|_| std::fs::write(dir.join(CONFIG_FILE), json))
        .map_err(|e| format!("Einstellung konnte nicht gespeichert werden: {e}"))
}

#[cfg(windows)]
fn read_registry() -> Option<Engine> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(REGISTRY_KEY)
        .ok()?;
    let value: String = key.get_value(REGISTRY_VALUE).ok()?;
    Engine::parse(&value)
}

#[cfg(not(windows))]
fn read_registry() -> Option<Engine> {
    None
}

// ---------------------------------------------------------------------------
// Connection endpoints

pub fn is_podman_host(host: &str) -> bool {
    host.to_ascii_lowercase().contains("podman")
}

/// Orders `hosts` so the chosen engine's endpoints come first (stable),
/// dropping duplicates.
pub fn prefer(hosts: Vec<String>, engine: Engine) -> Vec<String> {
    let (podman, other): (Vec<String>, Vec<String>) =
        hosts.into_iter().partition(|h| is_podman_host(h));
    let ordered = match engine {
        Engine::Podman => podman.into_iter().chain(other),
        Engine::Docker => other.into_iter().chain(podman),
    };
    let mut out: Vec<String> = Vec::new();
    for h in ordered {
        if !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

/// Converts a Windows pipe path (`\\.\pipe\podman-machine-default`) into a
/// bollard host (`npipe:////./pipe/podman-machine-default`).
pub fn pipe_path_to_host(path: &str) -> Option<String> {
    const PREFIX: &str = "//./pipe/";
    let p = path.trim().replace('\\', "/");
    if p.len() <= PREFIX.len() || !p.to_ascii_lowercase().starts_with(PREFIX) {
        return None;
    }
    Some(format!("npipe://{p}"))
}

/// Converts the machine's API socket path on macOS (Podman 5:
/// `$TMPDIR/podman/podman-machine-default-api.sock`) into a bollard host.
pub fn socket_path_to_host(path: &str) -> Option<String> {
    let p = path.trim();
    (p.starts_with('/') && p.len() > 1).then(|| format!("unix://{p}"))
}

/// Podman endpoint from one line of `podman machine inspect` output.
fn podman_endpoint_to_host(line: &str) -> Option<String> {
    if cfg!(windows) {
        pipe_path_to_host(line)
    } else {
        socket_path_to_host(line)
    }
}

/// The Podman endpoint found by the last `podman machine inspect`, if any.
pub fn discovered_podman_host() -> Option<String> {
    PODMAN_PIPE.lock().ok().and_then(|g| g.clone())
}

/// On Windows and macOS with Podman chosen, asks Podman once for its
/// endpoint (pipe or socket) so `candidate_hosts` can use it. No-op elsewhere.
pub async fn ensure_podman_pipe() {
    if !cfg!(any(windows, target_os = "macos")) || current().engine != Engine::Podman {
        return;
    }
    if PODMAN_PIPE_QUERIED.swap(true, Ordering::SeqCst) {
        return;
    }
    refresh_podman_pipe(None).await;
}

async fn refresh_podman_pipe(machine: Option<&str>) {
    if !cfg!(any(windows, target_os = "macos")) {
        return;
    }
    let Some(podman) = podman_path() else {
        return;
    };
    let query = if cfg!(windows) {
        PODMAN_PIPE_QUERY
    } else {
        PODMAN_SOCKET_QUERY
    };
    let mut args = strings(query);
    args.extend(machine.map(String::from));
    if let Ok(out) = run_cmd(&podman, &args, PODMAN_QUERY_TIMEOUT).await {
        if let Some(host) = String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(podman_endpoint_to_host)
        {
            if let Ok(mut g) = PODMAN_PIPE.lock() {
                *g = Some(host);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Locating the engines

/// Finds `name` (without `.exe`) on PATH.
/// macOS `open` (starts Docker Desktop), always by absolute path.
pub const MACOS_OPEN: &str = "/usr/bin/open";

/// First `name` in an absolute `PATH` entry; relative entries (`.`, empty)
/// are skipped, so a program in the working directory is never started.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    find_in(name, &std::env::var_os("PATH")?)
}

fn find_in(name: &str, paths: &std::ffi::OsStr) -> Option<PathBuf> {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    std::env::split_paths(paths)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(format!("{name}{exe}")))
        .find(|p| p.is_file())
}

fn program_files() -> Vec<PathBuf> {
    ["ProgramW6432", "ProgramFiles"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect()
}

/// Docker Desktop application (Windows: `Docker Desktop.exe`, macOS: app bundle).
pub fn docker_desktop_path() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        candidates.extend(
            program_files()
                .into_iter()
                .map(|pf| pf.join("Docker").join("Docker").join("Docker Desktop.exe")),
        );
    } else if cfg!(target_os = "macos") {
        candidates.push("/Applications/Docker.app".into());
    }
    candidates.into_iter().find(|p| p.exists())
}

/// The `podman` CLI: PATH first, then the installers' default locations
/// (GUI apps on macOS do not inherit the shell's PATH).
pub fn podman_path() -> Option<PathBuf> {
    if let Some(p) = find_on_path("podman") {
        return Some(p);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        candidates.extend(
            program_files()
                .into_iter()
                .map(|pf| pf.join("RedHat").join("Podman").join("podman.exe")),
        );
    } else if cfg!(target_os = "macos") {
        for p in [
            "/opt/podman/bin/podman",
            "/opt/homebrew/bin/podman",
            "/usr/local/bin/podman",
        ] {
            candidates.push(p.into());
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

// ---------------------------------------------------------------------------
// Podman machine handling

const PODMAN_LIST: &[&str] = &["machine", "list", "--format", "{{.Name}}"];
const PODMAN_PIPE_QUERY: &[&str] = &[
    "machine",
    "inspect",
    "--format",
    "{{.ConnectionInfo.PodmanPipe.Path}}",
];
const PODMAN_SOCKET_QUERY: &[&str] = &[
    "machine",
    "inspect",
    "--format",
    "{{.ConnectionInfo.PodmanSocket.Path}}",
];

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// Picks the machine to start from `podman machine list --format {{.Name}}`
/// output: the default (marked with `*`) or else the first one.
pub fn pick_machine(list_output: &str) -> Option<String> {
    let names: Vec<&str> = list_output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let chosen = names
        .iter()
        .find(|n| n.ends_with('*'))
        .or_else(|| names.first())?;
    Some(chosen.trim_end_matches('*').trim().to_string())
}

/// Podman CLI calls needed to get a running machine.
pub fn podman_start_steps(machine: Option<&str>) -> Vec<Vec<String>> {
    match machine {
        None => vec![
            strings(&["machine", "init"]),
            strings(&["machine", "start"]),
        ],
        Some(name) => vec![strings(&["machine", "start", name])],
    }
}

// ---------------------------------------------------------------------------
// Starting the engine

/// What [`ensure_running_with`] needs from the outside world; the real
/// implementation is [`RealOps`], tests simulate Docker and Podman.
pub trait EngineOps: Sync {
    type Client: Send;
    /// Connects with retries ([`docker::connect`]).
    fn connect(&self) -> impl Future<Output = Result<Self::Client, String>> + Send;
    /// One quick connection attempt while the engine starts.
    fn poll(&self) -> impl Future<Output = Result<Self::Client, String>> + Send;
    /// Launches the engine; `Ok(Some(note))` is a non-fatal message.
    fn launch(
        &self,
        engine: Engine,
        on_message: &mut (dyn FnMut(String) + Send),
    ) -> impl Future<Output = Result<Option<String>, String>> + Send;
    /// Asks Podman for its endpoint again (it may change on start).
    fn refresh_podman(&self) -> impl Future<Output = ()> + Send;
}

/// The real engine: bollard connections and the launch code below.
pub struct RealOps;

impl EngineOps for RealOps {
    type Client = bollard::Docker;

    async fn connect(&self) -> Result<bollard::Docker, String> {
        docker::connect().await.map_err(|e| e.to_string())
    }

    async fn poll(&self) -> Result<bollard::Docker, String> {
        docker::connect_quick().await.map_err(|e| e.to_string())
    }

    async fn launch(
        &self,
        engine: Engine,
        on_message: &mut (dyn FnMut(String) + Send),
    ) -> Result<Option<String>, String> {
        launch(engine, on_message).await
    }

    async fn refresh_podman(&self) {
        refresh_podman_pipe(None).await;
    }
}

/// Timing of [`ensure_running_with`].
#[derive(Debug, Clone, Copy)]
pub struct StartTiming {
    pub poll_interval: Duration,
    pub timeout: Duration,
}

pub const START_TIMING: StartTiming = StartTiming {
    poll_interval: POLL_INTERVAL,
    timeout: START_TIMEOUT,
};

/// Makes sure the engine answers: connects (with retries) and otherwise
/// starts the chosen engine and waits for its API, reporting German status
/// messages ("Docker Desktop wird gestartet …"). `starting` keeps a second
/// caller (double click, run and AI at once) from launching it twice; that
/// caller only waits. Returns a connected client or a German error message.
pub async fn ensure_running_with<O: EngineOps>(
    ops: &O,
    engine: Engine,
    timing: StartTiming,
    starting: &AtomicBool,
    on_message: &mut (dyn FnMut(String) + Send),
) -> Result<O::Client, String> {
    let first_error = match ops.connect().await {
        Ok(c) => return Ok(c),
        Err(e) => e,
    };
    let name = engine.display_name();
    on_message(format!("{name} wird gestartet …"));
    let launch_note = if starting.swap(true, Ordering::SeqCst) {
        None
    } else {
        let r = ops.launch(engine, on_message).await;
        starting.store(false, Ordering::SeqCst);
        match r {
            Ok(note) => note,
            Err(e) => return Err(format!("{first_error} {e}")),
        }
    };

    let started = Instant::now();
    loop {
        if engine == Engine::Podman {
            ops.refresh_podman().await;
        }
        let last_error = match ops.poll().await {
            Ok(c) => return Ok(c),
            Err(e) => e,
        };
        let waited = started.elapsed();
        if waited >= timing.timeout {
            let mut msg = format!(
                "{name} wurde gestartet, antwortet aber nach {} s noch nicht. Bitte {name} prüfen \
                 (beim ersten Start ggf. Nutzungsbedingungen bestätigen) und danach „Erneut prüfen“ \
                 klicken.",
                waited.as_secs()
            );
            if let Some(note) = launch_note {
                msg.push_str(&format!(" Meldung: {note}"));
            }
            // What the last connection attempt said (per endpoint), so the
            // cause is visible and can be reported.
            msg.push_str(&format!(" Letzter Verbindungsversuch: {last_error}"));
            return Err(msg);
        }
        on_message(format!("{name} wird gestartet … ({} s)", waited.as_secs()));
        tokio::time::sleep(timing.poll_interval).await;
    }
}

/// The chosen engine answers (started if needed); returns a client for it.
/// Used by runs and test generation, so they do not depend on the
/// frontend's (possibly stale) engine status.
pub async fn ensure_running(
    mut on_message: impl FnMut(String) + Send,
) -> Result<bollard::Docker, String> {
    ensure_running_with(
        &RealOps,
        current().engine,
        START_TIMING,
        &STARTING,
        &mut on_message,
    )
    .await
}

/// Starts `engine` and waits until its API answers, reporting German status
/// messages. Returns a German error message on failure.
pub async fn start(
    engine: Engine,
    mut on_message: impl FnMut(String) + Send,
) -> Result<(), String> {
    ensure_running_with(&RealOps, engine, START_TIMING, &STARTING, &mut on_message)
        .await
        .map(|_| ())
}

fn not_installed(engine: Engine) -> String {
    format!(
        "{} wurde nicht gefunden. Bitte installieren ({}) oder oben die andere Engine wählen.",
        engine.display_name(),
        engine.download_url()
    )
}

/// Launches the engine. `Ok(Some(note))` carries a non-fatal message
/// (e.g. "already running") to show if the API never comes up.
async fn launch(
    engine: Engine,
    on_message: &mut (dyn FnMut(String) + Send),
) -> Result<Option<String>, String> {
    if !cfg!(any(windows, target_os = "macos")) {
        // Linux: the engine is a system/user service; starting it needs root
        // or systemd specifics, so we leave that to the user.
        return Err(format!(
            "Automatischer Start ist unter Linux nicht vorgesehen. Bitte {} starten \
             (z. B. „sudo systemctl start docker“ bzw. „systemctl --user start podman.socket“).",
            engine.display_name()
        ));
    }
    match engine {
        Engine::Docker => launch_docker().await.map(|_| None),
        Engine::Podman => launch_podman(on_message).await,
    }
}

async fn launch_docker() -> Result<(), String> {
    let app = docker_desktop_path().ok_or_else(|| not_installed(Engine::Docker))?;
    if cfg!(target_os = "macos") {
        // Absolute path: never an `open` found via PATH.
        let out = run_cmd(
            Path::new(MACOS_OPEN),
            &["-a".to_string(), app.display().to_string()],
            Duration::from_secs(30),
        )
        .await?;
        if !out.status.success() {
            return Err(format!(
                "Docker Desktop konnte nicht gestartet werden: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        return Ok(());
    }
    spawn_detached(&app).map_err(|e| format!("Docker Desktop konnte nicht gestartet werden: {e}"))
}

async fn launch_podman(
    on_message: &mut (dyn FnMut(String) + Send),
) -> Result<Option<String>, String> {
    let podman = podman_path().ok_or_else(|| not_installed(Engine::Podman))?;
    let list = run_cmd(&podman, &strings(PODMAN_LIST), PODMAN_QUERY_TIMEOUT).await?;
    let machine = pick_machine(&String::from_utf8_lossy(&list.stdout));
    let mut note = None;
    for step in podman_start_steps(machine.as_deref()) {
        let is_init = step.get(1).map(String::as_str) == Some("init");
        let timeout = if is_init {
            on_message(
                "Podman-Maschine wird eingerichtet (einmalig, kann einige Minuten dauern) …".into(),
            );
            PODMAN_INIT_TIMEOUT
        } else {
            on_message("Podman wird gestartet …".into());
            PODMAN_CMD_TIMEOUT
        };
        let out = run_cmd(&podman, &step, timeout).await?;
        if !out.status.success() {
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if is_init {
                return Err(format!(
                    "Die Podman-Maschine konnte nicht eingerichtet werden: {detail} \
                     Hinweis: Podman benötigt unter Windows WSL2 (PowerShell als Administrator: \
                     „wsl --install“, danach Neustart)."
                ));
            }
            // e.g. "already running" – the poll decides.
            note = Some(detail);
        }
    }
    refresh_podman_pipe(machine.as_deref()).await;
    Ok(note)
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs a CLI command without a console window and captures its output.
async fn run_cmd(
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let shown = format!("{} {}", program.display(), args.join(" "));
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(format!("„{shown}“ konnte nicht ausgeführt werden: {e}")),
        Err(_) => Err(format!(
            "Zeitüberschreitung nach {} s bei „{shown}“.",
            timeout.as_secs()
        )),
    }
}

/// Starts a GUI application without waiting for it and without tying it to
/// our process (it keeps running when the Code-Prüfer is closed).
fn spawn_detached(app: &Path) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(app);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn path_lookup_skips_relative_entries() {
        let dir = std::env::temp_dir().join(format!("cp-path-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("podman"), "").unwrap();
        let cwd = std::env::current_dir().unwrap();
        // A relative entry that would resolve to the planted file.
        let rel = pathdiff(&dir.join("bin"), &cwd);
        let paths = std::env::join_paths([rel.clone(), PathBuf::from("/nonexistent")]).unwrap();
        assert_eq!(find_in("podman", &paths), None, "{rel:?}");
        let paths = std::env::join_paths([dir.join("bin")]).unwrap();
        assert_eq!(
            find_in("podman", &paths),
            Some(dir.join("bin").join("podman"))
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(Path::new(MACOS_OPEN).is_absolute());
    }

    /// `target` relative to `base` (both absolute), via "..".
    #[cfg(unix)]
    fn pathdiff(target: &Path, base: &Path) -> PathBuf {
        let mut rel = PathBuf::new();
        for _ in base.components().skip(1) {
            rel.push("..");
        }
        rel.join(target.strip_prefix("/").unwrap())
    }

    #[test]
    fn parse_is_lenient() {
        assert_eq!(Engine::parse(" Podman\r\n"), Some(Engine::Podman));
        assert_eq!(Engine::parse("DOCKER"), Some(Engine::Docker));
        assert_eq!(Engine::parse("lxc"), None);
        assert_eq!(Engine::parse(""), None);
    }

    #[test]
    fn config_wins_over_registry_over_default() {
        let c = resolve(Some(Engine::Docker), Some(Engine::Podman));
        assert_eq!((c.engine, c.source), (Engine::Docker, ChoiceSource::Config));
        let c = resolve(None, Some(Engine::Podman));
        assert_eq!(
            (c.engine, c.source),
            (Engine::Podman, ChoiceSource::Registry)
        );
        let c = resolve(None, None);
        assert_eq!(
            (c.engine, c.source),
            (Engine::Docker, ChoiceSource::Default)
        );
    }

    #[test]
    fn config_file_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("code-pruefer-engine-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_config(&dir), None);
        write_config(&dir, Engine::Podman).unwrap();
        assert_eq!(read_config(&dir), Some(Engine::Podman));
        let raw = std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap();
        assert!(raw.contains("\"podman\""), "{raw}");
        write_config(&dir, Engine::Docker).unwrap();
        assert_eq!(read_config(&dir), Some(Engine::Docker));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_config_is_ignored() {
        assert_eq!(parse_config("{\"engine\":\"rkt\"}"), None);
        assert_eq!(parse_config("kaputt"), None);
        assert_eq!(
            parse_config("{\"engine\":\"podman\"}"),
            Some(Engine::Podman)
        );
    }

    #[test]
    fn choice_serializes_for_frontend() {
        let v = serde_json::to_value(resolve(None, Some(Engine::Podman))).unwrap();
        assert_eq!(v["engine"], "podman");
        assert_eq!(v["source"], "registry");
        let e: Engine = serde_json::from_str("\"docker\"").unwrap();
        assert_eq!(e, Engine::Docker);
    }

    #[test]
    fn prefer_orders_chosen_engine_first() {
        let hosts = vec![
            "npipe:////./pipe/docker_engine".to_string(),
            "npipe:////./pipe/podman-machine-default".to_string(),
            "npipe:////./pipe/podman-machine-default".to_string(),
        ];
        assert_eq!(
            prefer(hosts.clone(), Engine::Podman),
            vec![
                "npipe:////./pipe/podman-machine-default",
                "npipe:////./pipe/docker_engine"
            ]
        );
        assert_eq!(
            prefer(hosts, Engine::Docker),
            vec![
                "npipe:////./pipe/docker_engine",
                "npipe:////./pipe/podman-machine-default"
            ]
        );
    }

    #[test]
    fn pipe_paths_become_npipe_hosts() {
        assert_eq!(
            pipe_path_to_host("\\\\.\\pipe\\podman-machine-default\r\n").as_deref(),
            Some("npipe:////./pipe/podman-machine-default")
        );
        assert_eq!(
            pipe_path_to_host("//./pipe/my-machine").as_deref(),
            Some("npipe:////./pipe/my-machine")
        );
        assert_eq!(pipe_path_to_host("<no value>"), None);
        assert_eq!(pipe_path_to_host(""), None);
        assert_eq!(pipe_path_to_host("\\\\.\\pipe\\"), None);
    }

    #[test]
    fn socket_paths_become_unix_hosts() {
        assert_eq!(
            socket_path_to_host("/var/folders/x/T/podman/podman-machine-default-api.sock\n")
                .as_deref(),
            Some("unix:///var/folders/x/T/podman/podman-machine-default-api.sock")
        );
        assert_eq!(socket_path_to_host("<no value>"), None);
        assert_eq!(socket_path_to_host("/"), None);
        assert_eq!(socket_path_to_host(""), None);
    }

    #[test]
    fn machine_selection_prefers_default() {
        assert_eq!(pick_machine(""), None);
        assert_eq!(pick_machine("\n  \n"), None);
        assert_eq!(
            pick_machine("other\npodman-machine-default*\n").as_deref(),
            Some("podman-machine-default")
        );
        assert_eq!(pick_machine("a\r\nb\r\n").as_deref(), Some("a"));
    }

    #[test]
    fn podman_steps_init_only_without_machine() {
        assert_eq!(
            podman_start_steps(None),
            vec![vec!["machine", "init"], vec!["machine", "start"]]
        );
        assert_eq!(
            podman_start_steps(Some("dev")),
            vec![vec!["machine", "start", "dev"]]
        );
    }

    /// Simulated engine: answers after `up_after` connection attempts once
    /// launched (or at once with `running`).
    struct SimEngine {
        running: bool,
        up_after: usize,
        launch_error: Option<String>,
        log: Mutex<Vec<String>>,
    }

    impl SimEngine {
        fn new(running: bool, up_after: usize) -> Self {
            Self {
                running,
                up_after,
                launch_error: None,
                log: Mutex::new(Vec::new()),
            }
        }
        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
        fn count(&self, what: &str) -> usize {
            self.log().iter().filter(|l| *l == what).count()
        }
    }

    impl EngineOps for SimEngine {
        type Client = &'static str;

        async fn connect(&self) -> Result<&'static str, String> {
            self.log.lock().unwrap().push("connect".into());
            if self.running {
                Ok("client")
            } else {
                Err("Keine laufende Container-Engine gefunden (x).".into())
            }
        }

        async fn poll(&self) -> Result<&'static str, String> {
            let mut log = self.log.lock().unwrap();
            log.push("poll".into());
            let polls = log.iter().filter(|l| *l == "poll").count();
            if polls >= self.up_after {
                Ok("client")
            } else {
                Err("noch nicht".into())
            }
        }

        async fn launch(
            &self,
            engine: Engine,
            on_message: &mut (dyn FnMut(String) + Send),
        ) -> Result<Option<String>, String> {
            self.log.lock().unwrap().push(format!("launch {engine:?}"));
            if engine == Engine::Podman {
                on_message("Podman wird gestartet …".into());
            }
            match &self.launch_error {
                Some(e) => Err(e.clone()),
                None => Ok(None),
            }
        }

        async fn refresh_podman(&self) {
            self.log.lock().unwrap().push("refresh".into());
        }
    }

    const FAST: StartTiming = StartTiming {
        poll_interval: Duration::from_millis(1),
        timeout: Duration::from_millis(200),
    };

    fn ensure(sim: &SimEngine, engine: Engine) -> (Result<&'static str, String>, Vec<String>) {
        let starting = AtomicBool::new(false);
        let mut messages = Vec::new();
        let r = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(ensure_running_with(
                sim,
                engine,
                FAST,
                &starting,
                &mut |m| messages.push(m),
            ));
        assert!(!starting.load(Ordering::SeqCst));
        (r, messages)
    }

    #[test]
    fn ensure_running_is_a_no_op_when_engine_answers() {
        let sim = SimEngine::new(true, 0);
        let (r, messages) = ensure(&sim, Engine::Docker);
        assert_eq!(r.unwrap(), "client");
        assert!(messages.is_empty());
        assert_eq!(sim.log(), vec!["connect"]);
    }

    #[test]
    fn ensure_running_starts_docker_and_waits() {
        let sim = SimEngine::new(false, 3);
        let (r, messages) = ensure(&sim, Engine::Docker);
        assert_eq!(r.unwrap(), "client");
        assert_eq!(messages[0], "Docker Desktop wird gestartet …");
        assert!(messages
            .iter()
            .any(|m| m.starts_with("Docker Desktop wird gestartet … (")));
        assert_eq!(sim.count("launch Docker"), 1);
        assert_eq!(sim.count("poll"), 3);
        assert_eq!(sim.count("refresh"), 0);
    }

    #[test]
    fn ensure_running_starts_podman_and_refreshes_its_endpoint() {
        let sim = SimEngine::new(false, 2);
        let (r, messages) = ensure(&sim, Engine::Podman);
        assert_eq!(r.unwrap(), "client");
        assert_eq!(messages[0], "Podman wird gestartet …");
        assert_eq!(sim.count("launch Podman"), 1);
        // The endpoint is asked for before every poll.
        assert_eq!(sim.count("refresh"), sim.count("poll"));
    }

    #[test]
    fn ensure_running_reports_launch_errors_and_timeouts() {
        let mut sim = SimEngine::new(false, usize::MAX);
        sim.launch_error = Some(not_installed(Engine::Docker));
        let (r, _) = ensure(&sim, Engine::Docker);
        let e = r.unwrap_err();
        assert!(
            e.starts_with("Keine laufende Container-Engine gefunden (x)."),
            "{e}"
        );
        assert!(e.contains("Docker Desktop wurde nicht gefunden"), "{e}");
        assert_eq!(sim.count("poll"), 0);

        let sim = SimEngine::new(false, usize::MAX);
        let (r, _) = ensure(&sim, Engine::Podman);
        let e = r.unwrap_err();
        assert!(e.contains("Podman wurde gestartet, antwortet aber"), "{e}");
        assert!(
            e.ends_with(" Letzter Verbindungsversuch: noch nicht"),
            "{e}"
        );
        assert!(sim.count("poll") >= 2);
    }

    #[test]
    fn ensure_running_waits_for_a_start_in_progress() {
        // Another caller is launching the engine: only wait, no 2nd launch.
        let sim = SimEngine::new(false, 2);
        let starting = AtomicBool::new(true);
        let r = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(ensure_running_with(
                &sim,
                Engine::Docker,
                FAST,
                &starting,
                &mut |_| {},
            ));
        assert_eq!(r.unwrap(), "client");
        assert_eq!(sim.count("launch Docker"), 0);
        assert!(
            starting.load(Ordering::SeqCst),
            "the other caller resets it"
        );
    }

    #[test]
    fn messages_are_german_and_actionable() {
        let m = not_installed(Engine::Podman);
        assert!(m.contains("Podman wurde nicht gefunden"));
        assert!(m.contains("https://podman.io/"));
    }
}
