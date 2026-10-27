//! Thin layer over the Docker Engine API (bollard): engine discovery,
//! image pulls and a hardened, short-lived sandbox container.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine as _;
use bollard::container::LogOutput;
use bollard::exec::StartExecResults;
use bollard::models::{ContainerCreateBody, ExecConfig, HostConfig, ResourcesUlimits};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, RemoveContainerOptionsBuilder,
};
use bollard::Docker;
use futures_util::StreamExt;
use serde::Serialize;

use super::compare::{CappedBuffer, OUTPUT_LIMIT};
use super::RunnerError;

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
const ENV: &[&str] = &["LANG=C.UTF-8", "LC_ALL=C.UTF-8", "HOME=/tmp"];
/// Raw bytes per `write_file` exec; base64 inflates by 4/3 and must stay
/// below the kernel's 128 KiB limit for a single argument.
const WRITE_CHUNK: usize = 48 * 1024;

/// Engine endpoints to try, in order. `DOCKER_HOST` (if set) wins exclusively.
pub fn candidate_hosts() -> Vec<String> {
    if let Some(h) = std::env::var("DOCKER_HOST")
        .ok()
        .filter(|h| !h.trim().is_empty())
    {
        return vec![h];
    }
    #[allow(unused_mut)]
    let mut hosts: Vec<String> = Vec::new();
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
    hosts
}

/// Connects to the first engine that answers a ping.
pub async fn connect() -> Result<Docker, RunnerError> {
    for host in candidate_hosts() {
        let Ok(docker) = Docker::connect_with_host(&host) else {
            continue;
        };
        if let Ok(Ok(_)) = tokio::time::timeout(Duration::from_secs(3), docker.ping()).await {
            return Ok(docker);
        }
    }
    Err(RunnerError::NoEngine)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullProgress {
    pub image: String,
    pub status: String,
    /// 0–100 if known.
    pub percent: Option<u8>,
}

/// Whether `image` is available in the local image store.
pub async fn image_exists(docker: &Docker, image: &str) -> Result<bool, RunnerError> {
    match docker.inspect_image(image).await {
        Ok(_) => Ok(true),
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(false),
        Err(e) => Err(e.into()),
    }
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
        labels: Some(HashMap::from([(LABEL.to_string(), "1".to_string())])),
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
    /// `None` if the process was killed after a timeout.
    pub exit_code: Option<i64>,
    pub timed_out: bool,
    pub duration_ms: u64,
}

/// A running sandbox container. Call [`Sandbox::remove`] when done.
pub struct Sandbox {
    docker: Docker,
    id: String,
}

impl Sandbox {
    pub async fn create(docker: &Docker, image: &str) -> Result<Self, RunnerError> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let name = format!("code-pruefer-{}-{nanos}", std::process::id());
        let opts = CreateContainerOptionsBuilder::default().name(&name).build();
        let id = docker
            .create_container(Some(opts), sandbox_config(image))
            .await?
            .id;
        let sandbox = Sandbox {
            docker: docker.clone(),
            id,
        };
        if let Err(e) = sandbox.docker.start_container(&sandbox.id, None).await {
            sandbox.remove().await;
            return Err(e.into());
        }
        Ok(sandbox)
    }

    /// Force-removes the container; errors are ignored (best effort).
    pub async fn remove(self) {
        let opts = RemoveContainerOptionsBuilder::default().force(true).build();
        let _ = self.docker.remove_container(&self.id, Some(opts)).await;
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

    /// Runs `cmd` with stdin redirected from `stdin_file` (if given).
    pub async fn exec_with_stdin_file(
        &self,
        cmd: &[&str],
        stdin_file: Option<&str>,
        timeout: Duration,
    ) -> Result<ExecOutput, RunnerError> {
        let script = format!("exec \"$@\" < {}", stdin_file.unwrap_or("/dev/null"));
        let mut full = vec!["sh", "-c", &script, "sh"];
        full.extend_from_slice(cmd);
        self.exec(&full, timeout).await
    }

    /// Runs a command, collecting capped stdout/stderr. On timeout all
    /// sandbox user processes (except PID 1) are killed.
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
        let timed_out = match tokio::time::timeout(timeout, read_all).await {
            Ok(res) => {
                res?;
                false
            }
            Err(_) => true,
        };
        drop(output);
        let duration_ms = started.elapsed().as_millis() as u64;

        let exit_code = if timed_out {
            self.kill_user_processes().await;
            None
        } else {
            self.wait_exit_code(&exec_id).await?
        };
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

    async fn wait_exit_code(&self, exec_id: &str) -> Result<Option<i64>, RunnerError> {
        for _ in 0..50 {
            let info = self.docker.inspect_exec(exec_id).await?;
            if info.running != Some(true) {
                return Ok(info.exit_code);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(None)
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
