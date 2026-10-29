//! Thin layer over the Docker Engine API (bollard): engine discovery,
//! image pulls/loads and a hardened, short-lived sandbox container.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use bollard::container::LogOutput;
use bollard::exec::StartExecResults;
use bollard::models::{ContainerCreateBody, ExecConfig, HostConfig, ResourcesUlimits};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ImportImageOptionsBuilder,
    ListContainersOptionsBuilder, RemoveContainerOptionsBuilder, TagImageOptionsBuilder,
};
use bollard::Docker;
use bytes::Bytes;
use futures_util::StreamExt;
use serde::Serialize;

use super::compare::{CappedBuffer, OUTPUT_LIMIT};
use super::RunnerError;
use crate::engine;

/// User the code runs as inside the container (non-root).
pub const SANDBOX_USER: &str = "1000:1000";
pub const WORK_DIR: &str = "/work";
/// File that receives the stdin of the current test case.
pub const STDIN_FILE: &str = "/tmp/.stdin";
pub const MEMORY_BYTES: i64 = 512 * 1024 * 1024;
pub const NANO_CPUS: i64 = 1_000_000_000;
pub const PIDS_LIMIT: i64 = 64;
/// Maximum size of a single file written by the program (RLIMIT_FSIZE).
pub const FSIZE_LIMIT: i64 = 16 * 1024 * 1024;
pub const LABEL: &str = "de.code-pruefer.sandbox";
/// Label with the app instance that created the sandbox ([`instance_id`]).
pub const INSTANCE_LABEL: &str = "de.code-pruefer.instance";
/// Sandboxes of other app instances count as orphaned only after this age
/// (longer than any run: 50 tests × 8 s plus compiling), so a second app
/// instance never removes the container of a running check.
pub const ORPHAN_AGE: Duration = Duration::from_secs(15 * 60);
const ENV: &[&str] = &["LANG=C.UTF-8", "LC_ALL=C.UTF-8", "HOME=/tmp"];
/// Raw bytes per `write_file` exec; base64 inflates by 4/3 and must stay
/// below the kernel's 128 KiB limit for a single argument.
const WRITE_CHUNK: usize = 48 * 1024;

/// `DOCKER_HOST`, if set. It stays supported in release builds because some
/// Podman and remote setups need it.
pub fn docker_host() -> Option<String> {
    std::env::var("DOCKER_HOST")
        .ok()
        .filter(|h| !h.trim().is_empty())
}

/// German warning for a `DOCKER_HOST` that talks plain HTTP over the
/// network (`tcp://`, `http://`; the app has no TLS support). Pure,
/// unit-tested.
pub fn insecure_host_warning(host: Option<&str>) -> Option<String> {
    let host = host?.trim();
    let lower = host.to_ascii_lowercase();
    (lower.starts_with("tcp://") || lower.starts_with("http://")).then(|| {
        format!(
            "Achtung: DOCKER_HOST={host} verbindet unverschlüsselt und ohne Anmeldung über das \
             Netzwerk. Wer diesen Port erreicht, kann Container starten und den Rechner \
             übernehmen. Bitte nur auf dem eigenen Rechner verwenden oder auf den Socket bzw. \
             die Named Pipe der Engine umstellen."
        )
    })
}

/// Engine endpoints to try, in order. `DOCKER_HOST` (if set) wins exclusively;
/// otherwise the endpoints of the chosen engine (see `crate::engine`) come first.
pub fn candidate_hosts() -> Vec<String> {
    if let Some(h) = docker_host() {
        return vec![h];
    }
    // Podman's pipe (Windows) or machine socket (macOS) as reported by
    // `podman machine inspect`.
    let mut hosts: Vec<String> = engine::discovered_podman_host().into_iter().collect();
    #[cfg(unix)]
    {
        let mut paths = vec!["/var/run/docker.sock".to_string()];
        if let Some(home) = std::env::var_os("HOME") {
            let home = home.to_string_lossy();
            paths.push(format!("{home}/.docker/run/docker.sock")); // Docker Desktop (macOS)
            paths.push(format!("{home}/.docker/desktop/docker.sock")); // Docker Desktop (Linux)
            paths.push(format!(
                "{home}/.local/share/containers/podman/machine/podman.sock"
            )); // Podman machine (macOS)
        }
        if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
            paths.push(format!("{xdg}/podman/podman.sock"));
        }
        paths.push("/run/podman/podman.sock".to_string());
        hosts.extend(
            paths
                .into_iter()
                .filter(|p| std::path::Path::new(p).exists())
                .map(|p| format!("unix://{p}")),
        );
    }
    #[cfg(windows)]
    {
        hosts.push("npipe:////./pipe/docker_engine".to_string());
        hosts.push("npipe:////./pipe/podman-machine-default".to_string());
    }
    engine::prefer(hosts, engine::current().engine)
}

/// How often and how patiently [`connect`] pings the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Rounds over all candidate hosts.
    pub attempts: u32,
    /// Timeout of one ping. Docker Desktop under memory pressure or waking
    /// up from "Resource Saver" can take several seconds to answer.
    pub ping_timeout: Duration,
    /// Pause after the first failed round; doubled after each further one.
    pub backoff: Duration,
}

/// Runs and test generation: 3 rounds, 10 s per ping, 1 s and 2 s pause.
pub const CONNECT_POLICY: RetryPolicy = RetryPolicy {
    attempts: 3,
    ping_timeout: Duration::from_secs(10),
    backoff: Duration::from_secs(1),
};

/// Status checks and polling while the engine starts: one round.
pub const QUICK_POLICY: RetryPolicy = RetryPolicy {
    attempts: 1,
    ping_timeout: Duration::from_secs(10),
    backoff: Duration::ZERO,
};

/// Why a single connection attempt failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptError {
    /// The endpoint exists but did not answer within the ping timeout.
    Timeout(Duration),
    /// No connection (endpoint missing, refused, error answer).
    Unreachable(String),
    /// The endpoint is served by a process that is not the engine (Windows
    /// named pipe of another user, see [`super::pipe_owner`]); German text.
    Untrusted(String),
}

/// German reason for [`RunnerError::NoEngine`]. A timeout wins over other
/// failures: it shows that an engine exists but is too slow. Pure,
/// unit-tested.
pub fn failure_reason(errors: &[(String, AttemptError)]) -> String {
    // A refused (possibly hijacked) endpoint is the most important news.
    if let Some((_, AttemptError::Untrusted(why))) = errors
        .iter()
        .find(|(_, e)| matches!(e, AttemptError::Untrusted(_)))
    {
        return why.clone();
    }
    if let Some((host, AttemptError::Timeout(t))) = errors
        .iter()
        .find(|(_, e)| matches!(e, AttemptError::Timeout(_)))
    {
        return format!(
            "Zeitüberschreitung: {host} hat nicht innerhalb von {} s geantwortet",
            t.as_secs()
        );
    }
    match errors.last() {
        Some((host, AttemptError::Unreachable(e))) => {
            format!("keine Verbindung zu {host}: {e}")
        }
        _ => "kein Endpunkt gefunden".to_string(),
    }
}

/// Tries every host per round and pauses with exponential backoff between
/// rounds; returns the first success or the German reason of the failure.
/// Generic over the attempt, so the retry logic is unit-tested.
pub async fn connect_with<T, F, Fut>(
    hosts: &[String],
    policy: RetryPolicy,
    mut attempt: F,
) -> Result<T, String>
where
    F: FnMut(String, Duration) -> Fut,
    Fut: std::future::Future<Output = Result<T, AttemptError>>,
{
    let mut errors: Vec<(String, AttemptError)> = Vec::new();
    let mut pause = policy.backoff;
    for round in 0..policy.attempts.max(1) {
        if round > 0 {
            tokio::time::sleep(pause).await;
            pause = pause.saturating_mul(2);
        }
        for host in hosts {
            match attempt(host.clone(), policy.ping_timeout).await {
                Ok(v) => return Ok(v),
                Err(e) => errors.push((host.clone(), e)),
            }
        }
    }
    Err(failure_reason(&errors))
}

/// One connection attempt: client for `host` plus a ping within `timeout`.
/// On Windows, a named pipe is only used if its server process belongs to
/// this user or to an installed program ([`super::pipe_owner`]).
async fn try_host(host: String, timeout: Duration) -> Result<Docker, AttemptError> {
    #[cfg(windows)]
    {
        let h = host.clone();
        tokio::task::spawn_blocking(move || super::pipe_owner::verify(&h))
            .await
            .map_err(|e| AttemptError::Unreachable(e.to_string()))?
            .map_err(AttemptError::Untrusted)?;
    }
    let docker =
        Docker::connect_with_host(&host).map_err(|e| AttemptError::Unreachable(e.to_string()))?;
    match tokio::time::timeout(timeout, docker.ping()).await {
        Ok(Ok(_)) => Ok(docker),
        Ok(Err(e)) => Err(AttemptError::Unreachable(e.to_string())),
        Err(_) => Err(AttemptError::Timeout(timeout)),
    }
}

/// Connects to the first engine that answers a ping, with retries
/// ([`CONNECT_POLICY`]).
pub async fn connect() -> Result<Docker, RunnerError> {
    connect_policy(CONNECT_POLICY).await
}

/// Like [`connect`], one round only ([`QUICK_POLICY`]): status checks.
pub async fn connect_quick() -> Result<Docker, RunnerError> {
    connect_policy(QUICK_POLICY).await
}

pub async fn connect_policy(policy: RetryPolicy) -> Result<Docker, RunnerError> {
    engine::ensure_podman_pipe().await;
    let hosts = candidate_hosts();
    connect_with(&hosts, policy, try_host)
        .await
        .map_err(RunnerError::NoEngine)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullProgress {
    pub image: String,
    pub status: String,
    /// 0–100 if known.
    pub percent: Option<u8>,
}

/// ID and registry digests of an image in the store. The ID is the config
/// digest (classic Docker store, Podman) or the manifest digest (Docker
/// with the containerd image store).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageIdentity {
    pub id: String,
    /// `<repo>@sha256:…` references the image was pulled by.
    pub repo_digests: Vec<String>,
}

/// Identity of `image`, `None` if it is not in the local image store.
pub async fn inspect_identity(
    docker: &Docker,
    image: &str,
) -> Result<Option<ImageIdentity>, RunnerError> {
    match docker.inspect_image(image).await {
        Ok(info) => Ok(Some(ImageIdentity {
            id: info.id.unwrap_or_default(),
            repo_digests: info.repo_digests.unwrap_or_default(),
        })),
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Whether `image` is available in the local image store.
pub async fn image_exists(docker: &Docker, image: &str) -> Result<bool, RunnerError> {
    Ok(inspect_identity(docker, image).await?.is_some())
}

/// Removes the reference `image` (the image itself goes once no other
/// reference is left); errors are ignored (best effort).
pub async fn remove_image(docker: &Docker, image: &str) {
    let _ = docker
        .remove_image(
            image,
            None::<bollard::query_parameters::RemoveImageOptions>,
            None,
        )
        .await;
}

/// SHA-256 of a file as lowercase hex (read on a blocking thread).
pub async fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Pulls `image` from its registry, reporting progress.
pub async fn pull_image(
    docker: &Docker,
    image: &str,
    mut on_progress: impl FnMut(PullProgress),
) -> Result<(), RunnerError> {
    let opts = CreateImageOptionsBuilder::default()
        .from_image(image)
        .build();
    let mut stream = docker.create_image(Some(opts), None, None);
    let mut layers: HashMap<String, (i64, i64)> = HashMap::new();
    while let Some(item) = stream.next().await {
        let info = item.map_err(|e| RunnerError::ImagePull(image.to_string(), e.to_string()))?;
        if let Some(err) = info.error_detail.and_then(|d| d.message) {
            return Err(RunnerError::ImagePull(image.to_string(), err));
        }
        if let (Some(id), Some(detail)) = (info.id, info.progress_detail) {
            if let (Some(cur), Some(total)) = (detail.current, detail.total) {
                layers.insert(id, (cur, total));
            }
        }
        let (cur, total) = layers
            .values()
            .fold((0i64, 0i64), |(c, t), (lc, lt)| (c + lc, t + lt));
        let percent = (total > 0).then(|| ((cur * 100) / total).clamp(0, 100) as u8);
        on_progress(PullProgress {
            image: image.to_string(),
            status: info.status.unwrap_or_default(),
            percent,
        });
    }
    Ok(())
}

/// Architecture of the engine (its Linux VM on Windows/macOS) as reported
/// by `/version`, e.g. `amd64` or `arm64`.
pub async fn engine_arch(docker: &Docker) -> Option<String> {
    docker
        .version()
        .await
        .ok()
        .and_then(|v| v.arch)
        .filter(|a| !a.is_empty())
}

/// Counts the compressed bytes read from the archive (for progress).
struct CountingReader<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// Image name or ID from a `/images/load` status line
/// (`Loaded image: <ref>` / `Loaded image ID: sha256:…`; Podman may list
/// several names separated by commas).
pub fn parse_loaded(line: &str) -> Vec<String> {
    let line = line.trim();
    let rest = line
        .strip_prefix("Loaded image ID:")
        .or_else(|| line.strip_prefix("Loaded image:"))
        .or_else(|| line.strip_prefix("Loaded image(s):"));
    rest.map(|r| {
        r.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

/// Loads a gzip-compressed `docker save` archive into the engine's image
/// store (`POST /images/load`, also offered by Podman's compat API). The
/// archive is decompressed here and streamed as plain tar, so it works the
/// same for Docker and Podman. Returns the names/IDs the engine reported.
pub async fn load_image_archive(
    docker: &Docker,
    archive: &Path,
    image: &str,
    mut on_progress: impl FnMut(Option<u8>),
) -> Result<Vec<String>, RunnerError> {
    let shown = archive.display().to_string();
    let err = |detail: String| RunnerError::ImageLoad(shown.clone(), detail);
    let file = std::fs::File::open(archive).map_err(|e| err(e.to_string()))?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
    let read = Arc::new(AtomicU64::new(0));

    let (tx, rx) = tokio::sync::mpsc::channel::<std::io::Result<Bytes>>(4);
    let counter = read.clone();
    let reader = tokio::task::spawn_blocking(move || {
        let mut gz = flate2::read::GzDecoder::new(CountingReader {
            inner: std::io::BufReader::new(file),
            count: counter,
        });
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let item = match gz.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => Ok(Bytes::copy_from_slice(&buf[..n])),
                Err(e) => Err(e),
            };
            let failed = item.is_err();
            if tx.blocking_send(item).is_err() || failed {
                break;
            }
        }
    });
    let body = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    let opts = ImportImageOptionsBuilder::default().quiet(true).build();
    let mut stream = docker.import_image_stream(opts, body, None);

    let mut loaded = Vec::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    let result = loop {
        tokio::select! {
            item = stream.next() => match item {
                None => break Ok(()),
                Some(Err(e)) => break Err(err(e.to_string())),
                Some(Ok(info)) => {
                    if let Some(msg) = info.error_detail.and_then(|d| d.message) {
                        break Err(err(msg));
                    }
                    for text in [info.stream, info.status].into_iter().flatten() {
                        loaded.extend(parse_loaded(&text));
                    }
                }
            },
            _ = ticker.tick() => {
                let done = read.load(Ordering::Relaxed);
                on_progress((total > 0).then(|| ((done * 100) / total).min(100) as u8));
            }
        }
    };
    drop(stream);
    let _ = reader.await;
    result?;
    on_progress(Some(100));
    if loaded.is_empty() && !image_exists(docker, image).await? {
        return Err(err("Die Engine hat kein geladenes Image gemeldet.".into()));
    }
    Ok(loaded)
}

/// Splits `registry/repo:tag` into repository and tag (default `latest`).
pub fn split_ref(image: &str) -> (&str, &str) {
    match image.rfind(':') {
        Some(i) if !image[i + 1..].contains('/') => (&image[..i], &image[i + 1..]),
        _ => (image, "latest"),
    }
}

/// Tags `source` (name or ID) as `target`.
pub async fn tag_image(docker: &Docker, source: &str, target: &str) -> Result<(), RunnerError> {
    let (repo, tag) = split_ref(target);
    let opts = TagImageOptionsBuilder::default()
        .repo(repo)
        .tag(tag)
        .build();
    docker.tag_image(source, Some(opts)).await?;
    Ok(())
}

/// ID of this app process, stored in [`INSTANCE_LABEL`].
pub fn instance_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        format!("{}-{nanos}", std::process::id())
    })
}

/// Names of the sandboxes this process is using right now.
fn active() -> std::sync::MutexGuard<'static, HashSet<String>> {
    static ACTIVE: OnceLock<std::sync::Mutex<HashSet<String>>> = OnceLock::new();
    ACTIVE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Whether a sandbox container is left over: not in use by this process
/// and either created by this process (its removal failed) or older than
/// `min_age` (another or a crashed instance). Pure, unit-tested.
pub fn is_orphan(instance: Option<&str>, in_use: bool, age: Duration, min_age: Duration) -> bool {
    !in_use && (instance == Some(instance_id()) || age >= min_age)
}

/// Force-removes the sandbox containers (label [`LABEL`]) for which `pred`
/// (instance label, in use by this process, age) holds; returns how many.
async fn remove_sandboxes(
    docker: &Docker,
    pred: impl Fn(Option<&str>, bool, Duration) -> bool,
) -> Result<usize, RunnerError> {
    let filters = HashMap::from([("label", vec![LABEL])]);
    let opts = ListContainersOptionsBuilder::default()
        .all(true)
        .filters(&filters)
        .build();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let mut removed = 0;
    for c in docker.list_containers(Some(opts)).await? {
        let Some(id) = c.id else { continue };
        let in_use = {
            let active = active();
            c.names
                .iter()
                .flatten()
                .any(|n| active.contains(n.trim_start_matches('/')))
        };
        let age = Duration::from_secs(now.saturating_sub(c.created.unwrap_or(now)).max(0) as u64);
        let instance = c.labels.as_ref().and_then(|l| l.get(INSTANCE_LABEL));
        if pred(instance.map(String::as_str), in_use, age) {
            let opts = RemoveContainerOptionsBuilder::default().force(true).build();
            if docker.remove_container(&id, Some(opts)).await.is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// Removes left-over sandboxes (see [`is_orphan`]), e.g. after a crash.
/// Called when the engine is up and before every run.
pub async fn remove_orphans(docker: &Docker, min_age: Duration) -> Result<usize, RunnerError> {
    remove_sandboxes(docker, |instance, in_use, age| {
        is_orphan(instance, in_use, age, min_age)
    })
    .await
}

/// Removes every sandbox of this process (app exit).
pub async fn remove_own_sandboxes(docker: &Docker) -> Result<usize, RunnerError> {
    remove_sandboxes(docker, |instance, _, _| instance == Some(instance_id())).await
}

/// Hardened container definition. The container itself only idles
/// (`sleep infinity`); all work happens via `exec`.
pub fn sandbox_config(image: &str) -> ContainerCreateBody {
    let tmpfs = HashMap::from([
        (
            WORK_DIR.to_string(),
            "rw,nosuid,nodev,size=64m,mode=1777".to_string(),
        ),
        (
            "/tmp".to_string(),
            "rw,nosuid,nodev,size=64m,mode=1777".to_string(),
        ),
    ]);
    let ulimit = |name: &str, v: i64| ResourcesUlimits {
        name: Some(name.to_string()),
        soft: Some(v),
        hard: Some(v),
    };
    ContainerCreateBody {
        image: Some(image.to_string()),
        cmd: Some(vec!["sleep".into(), "infinity".into()]),
        user: Some(SANDBOX_USER.to_string()),
        working_dir: Some(WORK_DIR.to_string()),
        env: Some(ENV.iter().map(|s| s.to_string()).collect()),
        network_disabled: Some(true),
        labels: Some(HashMap::from([
            (LABEL.to_string(), "1".to_string()),
            (INSTANCE_LABEL.to_string(), instance_id().to_string()),
        ])),
        host_config: Some(HostConfig {
            network_mode: Some("none".to_string()),
            memory: Some(MEMORY_BYTES),
            memory_swap: Some(MEMORY_BYTES),
            nano_cpus: Some(NANO_CPUS),
            pids_limit: Some(PIDS_LIMIT),
            readonly_rootfs: Some(true),
            tmpfs: Some(tmpfs),
            cap_drop: Some(vec!["ALL".to_string()]),
            security_opt: Some(vec!["no-new-privileges".to_string()]),
            ulimits: Some(vec![ulimit("fsize", FSIZE_LIMIT), ulimit("nofile", 256)]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// `None` if the process was killed after a timeout (also when it
    /// closed its output but kept running).
    pub exit_code: Option<i64>,
    pub timed_out: bool,
    pub duration_ms: u64,
}

/// A running sandbox container. Call [`Sandbox::remove`] when done.
pub struct Sandbox {
    docker: Docker,
    id: String,
    name: String,
}

impl Sandbox {
    pub async fn create(docker: &Docker, image: &str) -> Result<Self, RunnerError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        // The counter keeps names unique when sessions start concurrently.
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let name = format!("code-pruefer-{}-{nanos}-{seq}", std::process::id());
        // Registered before creation, so a concurrent orphan cleanup never
        // takes it for a left-over.
        active().insert(name.clone());
        let opts = CreateContainerOptionsBuilder::default().name(&name).build();
        let id = match docker
            .create_container(Some(opts), sandbox_config(image))
            .await
        {
            Ok(created) => created.id,
            Err(e) => {
                active().remove(&name);
                return Err(e.into());
            }
        };
        let sandbox = Sandbox {
            docker: docker.clone(),
            id,
            name,
        };
        if let Err(e) = sandbox.docker.start_container(&sandbox.id, None).await {
            sandbox.remove().await;
            return Err(e.into());
        }
        Ok(sandbox)
    }

    /// Force-removes the container; errors are ignored (best effort, a
    /// left-over is removed by [`remove_orphans`] later).
    pub async fn remove(self) {
        let opts = RemoveContainerOptionsBuilder::default().force(true).build();
        let _ = self.docker.remove_container(&self.id, Some(opts)).await;
        active().remove(&self.name);
    }

    /// Writes `content` to `path` inside the container. The data travels as
    /// base64 command arguments, which avoids relying on stdin half-close
    /// (not available on Windows named pipes) and on `put_archive` (does not
    /// work reliably for tmpfs mounts).
    pub async fn write_file(&self, path: &str, content: &[u8]) -> Result<(), RunnerError> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut chunks: Vec<&[u8]> = content.chunks(WRITE_CHUNK).collect();
        if chunks.is_empty() {
            chunks.push(&[]);
        }
        for (i, chunk) in chunks.iter().enumerate() {
            let redirect = if i == 0 { ">" } else { ">>" };
            let script = format!("printf %s \"$1\" | base64 -d {redirect} \"$2\"");
            let cmd = ["sh", "-c", &script, "sh", &b64.encode(chunk), path];
            let out = self.exec(&cmd, Duration::from_secs(10)).await?;
            if out.exit_code != Some(0) {
                return Err(RunnerError::Other(format!(
                    "Datei {path} konnte nicht in den Container geschrieben werden: {}",
                    out.stderr.trim()
                )));
            }
        }
        Ok(())
    }

    /// Runs user code (`cmd`, compile step or test) with stdin redirected
    /// from `stdin_file` (if given). Afterwards every remaining process of
    /// the sandbox user is killed, so nothing the program left running in
    /// the background survives into the next test.
    pub async fn exec_with_stdin_file(
        &self,
        cmd: &[&str],
        stdin_file: Option<&str>,
        timeout: Duration,
    ) -> Result<ExecOutput, RunnerError> {
        let script = format!("exec \"$@\" < {}", stdin_file.unwrap_or("/dev/null"));
        let mut full = vec!["sh", "-c", &script, "sh"];
        full.extend_from_slice(cmd);
        let out = self.exec(&full, timeout).await;
        // `exec` already killed everything after a timeout.
        if !matches!(&out, Ok(o) if o.timed_out) {
            self.kill_user_processes().await;
        }
        out
    }

    /// Runs a command, collecting capped stdout/stderr. If it does not end
    /// within `timeout` – also when it closed its output but keeps running –
    /// it counts as timed out and all sandbox user processes (except PID 1)
    /// are killed.
    pub async fn exec(&self, cmd: &[&str], timeout: Duration) -> Result<ExecOutput, RunnerError> {
        let config = ExecConfig {
            attach_stdin: Some(false),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
            user: Some(SANDBOX_USER.to_string()),
            working_dir: Some(WORK_DIR.to_string()),
            env: Some(ENV.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        };
        let exec_id = self.docker.create_exec(&self.id, config).await?.id;
        let started = Instant::now();
        let StartExecResults::Attached { mut output, .. } =
            self.docker.start_exec(&exec_id, None).await?
        else {
            return Err(RunnerError::Other(
                "Exec wurde unerwartet detached gestartet".into(),
            ));
        };

        let mut stdout = CappedBuffer::new(OUTPUT_LIMIT);
        let mut stderr = CappedBuffer::new(OUTPUT_LIMIT);
        let read_all = async {
            while let Some(msg) = output.next().await {
                match msg? {
                    LogOutput::StdOut { message } | LogOutput::Console { message } => {
                        stdout.push(&message)
                    }
                    LogOutput::StdErr { message } => stderr.push(&message),
                    LogOutput::StdIn { .. } => {}
                }
            }
            Ok::<_, bollard::errors::Error>(())
        };
        let mut timed_out = match tokio::time::timeout(timeout, read_all).await {
            Ok(res) => {
                res?;
                false
            }
            Err(_) => true,
        };
        drop(output);

        let exit_code = if timed_out {
            None
        } else {
            self.wait_exit_code(&exec_id, started + timeout).await?
        };
        if exit_code.is_none() {
            timed_out = true;
            self.kill_user_processes().await;
        }
        let duration_ms = started.elapsed().as_millis() as u64;
        Ok(ExecOutput {
            stdout_truncated: stdout.truncated(),
            stderr_truncated: stderr.truncated(),
            stdout: stdout.into_string(),
            stderr: stderr.into_string(),
            exit_code,
            timed_out,
            duration_ms,
        })
    }

    /// Exit code once the exec has ended; `None` if it still runs at
    /// `deadline` (with a short grace period for the engine to notice the
    /// exit after the output closed).
    async fn wait_exit_code(
        &self,
        exec_id: &str,
        deadline: Instant,
    ) -> Result<Option<i64>, RunnerError> {
        let deadline = deadline.max(Instant::now() + Duration::from_millis(500));
        loop {
            let info = self.docker.inspect_exec(exec_id).await?;
            if info.running != Some(true) {
                return Ok(info.exit_code);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// `kill -9 -1` signals every process of the sandbox user except PID 1
    /// (the idle `sleep`), so the container stays usable for the next test.
    async fn kill_user_processes(&self) {
        let config = ExecConfig {
            cmd: Some(vec!["sh".into(), "-c".into(), "kill -9 -1".into()]),
            user: Some(SANDBOX_USER.to_string()),
            ..Default::default()
        };
        if let Ok(created) = self.docker.create_exec(&self.id, config).await {
            if let Ok(StartExecResults::Attached { mut output, .. }) =
                self.docker.start_exec(&created.id, None).await
            {
                let _ = tokio::time::timeout(Duration::from_secs(5), async {
                    while output.next().await.is_some() {}
                })
                .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_config_is_hardened() {
        let cfg = sandbox_config("img:1");
        assert_eq!(cfg.image.as_deref(), Some("img:1"));
        assert_eq!(cfg.user.as_deref(), Some("1000:1000"));
        assert_eq!(cfg.network_disabled, Some(true));
        let env = cfg.env.unwrap();
        assert!(env.contains(&"LANG=C.UTF-8".to_string()));

        let hc = cfg.host_config.expect("host config");
        assert_eq!(hc.network_mode.as_deref(), Some("none"));
        assert_eq!(hc.memory, Some(512 * 1024 * 1024));
        assert_eq!(hc.memory_swap, hc.memory, "no swap: memory_swap == memory");
        assert_eq!(hc.nano_cpus, Some(1_000_000_000));
        assert_eq!(hc.pids_limit, Some(64));
        assert_eq!(hc.readonly_rootfs, Some(true));
        assert_eq!(hc.cap_drop, Some(vec!["ALL".to_string()]));
        assert_eq!(hc.security_opt, Some(vec!["no-new-privileges".to_string()]));

        let tmpfs = hc.tmpfs.expect("tmpfs");
        for mount in ["/work", "/tmp"] {
            let opts = tmpfs
                .get(mount)
                .unwrap_or_else(|| panic!("{mount} missing"));
            assert!(opts.contains("size="), "{mount} needs a size limit");
        }

        let ulimits = hc.ulimits.expect("ulimits");
        let fsize = ulimits
            .iter()
            .find(|u| u.name.as_deref() == Some("fsize"))
            .expect("fsize ulimit");
        assert_eq!(fsize.soft, Some(FSIZE_LIMIT));
        assert_eq!(fsize.hard, Some(FSIZE_LIMIT));
    }

    #[test]
    fn sandboxes_carry_instance_label() {
        let labels = sandbox_config("img:1").labels.unwrap();
        assert_eq!(labels.get(LABEL).map(String::as_str), Some("1"));
        assert_eq!(
            labels.get(INSTANCE_LABEL).map(String::as_str),
            Some(instance_id())
        );
    }

    #[test]
    fn orphan_rules() {
        let own = Some(instance_id());
        let young = Duration::from_secs(60);
        let old = ORPHAN_AGE + Duration::from_secs(1);
        // In use by this process: never.
        assert!(!is_orphan(own, true, old, ORPHAN_AGE));
        assert!(!is_orphan(Some("x"), true, old, ORPHAN_AGE));
        // Own, not in use (removal failed): at once.
        assert!(is_orphan(own, false, young, ORPHAN_AGE));
        // Other or crashed instance, or no label (older versions): when old.
        assert!(!is_orphan(Some("x"), false, young, ORPHAN_AGE));
        assert!(is_orphan(Some("x"), false, old, ORPHAN_AGE));
        assert!(is_orphan(None, false, old, ORPHAN_AGE));
        assert!(is_orphan(None, false, young, Duration::ZERO));
    }

    #[test]
    fn loaded_lines_are_parsed() {
        assert_eq!(
            parse_loaded("Loaded image: ghcr.io/x/code-pruefer-python:1.0\n"),
            vec!["ghcr.io/x/code-pruefer-python:1.0"]
        );
        assert_eq!(
            parse_loaded("Loaded image ID: sha256:abc"),
            vec!["sha256:abc"]
        );
        assert_eq!(parse_loaded("Loaded image: a:1, b:2"), vec!["a:1", "b:2"]);
        assert!(parse_loaded("Loading layer 12 MB").is_empty());
    }

    #[tokio::test]
    async fn file_hash_is_sha256_hex() {
        let path = std::env::temp_dir().join(format!("code-pruefer-hash-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).await.unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn refs_split_into_repo_and_tag() {
        assert_eq!(
            split_ref("ghcr.io/x/code-pruefer-java:0.2.0"),
            ("ghcr.io/x/code-pruefer-java", "0.2.0")
        );
        assert_eq!(
            split_ref("localhost:5000/img"),
            ("localhost:5000/img", "latest")
        );
        assert_eq!(split_ref("img"), ("img", "latest"));
    }

    #[test]
    fn plain_tcp_docker_host_is_warned_about() {
        let w = insecure_host_warning(Some("tcp://192.168.1.5:2375")).unwrap();
        assert!(w.contains("unverschlüsselt"), "{w}");
        assert!(w.contains("tcp://192.168.1.5:2375"), "{w}");
        assert!(insecure_host_warning(Some("HTTP://localhost:2375")).is_some());
        assert_eq!(
            insecure_host_warning(Some("unix:///run/podman/podman.sock")),
            None
        );
        assert_eq!(
            insecure_host_warning(Some("npipe:////./pipe/podman-machine-default")),
            None
        );
        assert_eq!(insecure_host_warning(None), None);
    }

    fn hosts(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    const FAST: RetryPolicy = RetryPolicy {
        attempts: 3,
        ping_timeout: Duration::from_millis(50),
        backoff: Duration::from_millis(10),
    };

    #[tokio::test]
    async fn connect_retries_with_backoff_until_engine_answers() {
        let calls = std::sync::Mutex::new(Vec::new());
        let started = Instant::now();
        // The engine answers from the third round on (e.g. after a restart).
        let r = connect_with(&hosts(&["a", "b"]), FAST, |host, timeout| {
            let mut c = calls.lock().unwrap();
            c.push(host.clone());
            let round = (c.len() - 1) / 2;
            async move {
                assert_eq!(timeout, Duration::from_millis(50));
                if round >= 2 && host == "b" {
                    Ok(host)
                } else {
                    Err(AttemptError::Unreachable("refused".into()))
                }
            }
        })
        .await;
        assert_eq!(r.unwrap(), "b");
        assert_eq!(
            *calls.lock().unwrap(),
            hosts(&["a", "b", "a", "b", "a", "b"])
        );
        // Pauses of 10 ms and 20 ms between the rounds.
        assert!(started.elapsed() >= Duration::from_millis(30));
    }

    #[tokio::test]
    async fn connect_gives_up_with_reason() {
        let mut n = 0;
        let r: Result<(), String> =
            connect_with(&hosts(&["npipe:////./pipe/docker_engine"]), FAST, |_, t| {
                n += 1;
                async move { Err(AttemptError::Timeout(t)) }
            })
            .await;
        assert_eq!(n, 3);
        let msg = r.unwrap_err();
        assert!(msg.starts_with("Zeitüberschreitung"), "{msg}");
        assert!(msg.contains("docker_engine"), "{msg}");
        // Only one round for status checks.
        let mut n = 0;
        let r: Result<(), String> = connect_with(&hosts(&["x"]), QUICK_POLICY, |_, _| {
            n += 1;
            async { Err(AttemptError::Unreachable("Verbindung abgelehnt".into())) }
        })
        .await;
        assert_eq!(n, 1);
        assert_eq!(
            r.unwrap_err(),
            "keine Verbindung zu x: Verbindung abgelehnt"
        );
        let r: Result<(), String> =
            connect_with(&[], FAST, |_, _| async { Ok::<(), AttemptError>(()) }).await;
        assert_eq!(r.unwrap_err(), "kein Endpunkt gefunden");
    }

    #[test]
    fn timeout_wins_in_reason_and_error_text() {
        let reason = failure_reason(&[
            ("a".into(), AttemptError::Timeout(Duration::from_secs(10))),
            ("b".into(), AttemptError::Unreachable("x".into())),
        ]);
        assert!(reason.contains("10 s"), "{reason}");
        let msg = RunnerError::NoEngine(reason).to_string();
        assert!(msg.starts_with("Keine laufende Container-Engine gefunden (Zeitüberschreitung"));
        assert!(
            msg.ends_with("Bitte Docker Desktop oder Podman starten."),
            "{msg}"
        );
        assert_eq!(CONNECT_POLICY.ping_timeout, Duration::from_secs(10));
        const { assert!(CONNECT_POLICY.attempts >= 2) };
    }

    #[test]
    fn refused_pipe_wins_in_reason() {
        let reason = failure_reason(&[
            ("a".into(), AttemptError::Timeout(Duration::from_secs(10))),
            (
                "npipe:////./pipe/docker_engine".into(),
                AttemptError::Untrusted("abgelehnt: fremder Prozess".into()),
            ),
            ("b".into(), AttemptError::Unreachable("x".into())),
        ]);
        assert_eq!(reason, "abgelehnt: fremder Prozess");
    }

    #[test]
    fn docker_host_env_overrides_candidates() {
        // Only this test touches DOCKER_HOST.
        let old = std::env::var("DOCKER_HOST").ok();
        std::env::set_var("DOCKER_HOST", "tcp://127.0.0.1:2375");
        assert_eq!(candidate_hosts(), vec!["tcp://127.0.0.1:2375".to_string()]);
        match old {
            Some(v) => std::env::set_var("DOCKER_HOST", v),
            None => std::env::remove_var("DOCKER_HOST"),
        }
    }
}
