//! Detects whether a Docker-compatible engine is installed and running.

use serde::Serialize;

use crate::runner::docker;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum DockerStatus {
    /// No engine CLI/app/socket found.
    NotInstalled,
    /// Something is installed but the engine does not answer.
    NotRunning,
    Ok {
        version: String,
        /// e.g. "Docker Desktop", "Podman Engine"
        product: String,
    },
}

pub async fn detect() -> DockerStatus {
    if let Ok(client) = docker::connect().await {
        if let Ok(v) = client.version().await {
            let product = v
                .platform
                .map(|p| p.name)
                .filter(|n| !n.is_empty())
                .or_else(|| {
                    v.components
                        .and_then(|c| c.into_iter().next())
                        .map(|c| c.name)
                })
                .unwrap_or_else(|| "Docker Engine".to_string());
            return DockerStatus::Ok {
                version: v.version.unwrap_or_else(|| "unbekannt".into()),
                product,
            };
        }
    }
    if looks_installed() {
        DockerStatus::NotRunning
    } else {
        DockerStatus::NotInstalled
    }
}

/// Heuristic: CLI on PATH, a known app bundle/install dir, or a socket file.
fn looks_installed() -> bool {
    if std::env::var_os("DOCKER_HOST").is_some() {
        return true;
    }
    let exe = if cfg!(windows) { ".exe" } else { "" };
    let on_path = |name: &str| {
        std::env::var_os("PATH")
            .map(|paths| {
                std::env::split_paths(&paths).any(|dir| dir.join(format!("{name}{exe}")).is_file())
            })
            .unwrap_or(false)
    };
    if on_path("docker") || on_path("podman") {
        return true;
    }
    let mut known: Vec<String> = vec![
        "/Applications/Docker.app".into(),
        "/Applications/Podman Desktop.app".into(),
        "/usr/local/bin/docker".into(),
        "/opt/homebrew/bin/podman".into(),
        "/var/run/docker.sock".into(),
    ];
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        let pf = pf.to_string_lossy();
        known.push(format!("{pf}\\Docker\\Docker"));
        known.push(format!("{pf}\\RedHat\\Podman"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        known.push(format!(
            "{}\\Programs\\Podman Desktop",
            local.to_string_lossy()
        ));
    }
    known.iter().any(|p| std::path::Path::new(p).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serializes_with_state_tag() {
        let s = serde_json::to_value(DockerStatus::NotRunning).unwrap();
        assert_eq!(s["state"], "notRunning");
        let s = serde_json::to_value(DockerStatus::Ok {
            version: "27.0".into(),
            product: "Docker Engine".into(),
        })
        .unwrap();
        assert_eq!(s["state"], "ok");
        assert_eq!(s["version"], "27.0");
    }
}
