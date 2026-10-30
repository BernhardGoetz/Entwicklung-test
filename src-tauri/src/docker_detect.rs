//! Detects whether a Docker-compatible engine is installed and running.

use serde::Serialize;

use crate::engine;
use crate::runner::{docker, RunnerError};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum DockerStatus {
    /// No engine CLI/app/socket found.
    NotInstalled,
    /// Something is installed but the engine does not answer. `reason`:
    /// what the connection attempt said (per endpoint, German), shown as
    /// detail so a report names the cause.
    NotRunning {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Ok {
        version: String,
        /// e.g. "Docker Desktop", "Podman Engine"
        product: String,
        /// German security warning (e.g. unencrypted `DOCKER_HOST`).
        #[serde(skip_serializing_if = "Option::is_none")]
        warning: Option<String>,
    },
}

pub async fn detect() -> DockerStatus {
    let reason = match docker::connect_quick().await {
        Ok(client) => match client.version().await {
            Ok(v) => return ok_status(v),
            // The ping worked, but the engine answers with an error.
            Err(e) => format!(
                "Die Engine antwortet mit einem Fehler: {}",
                docker::describe_error(&e)
            ),
        },
        Err(RunnerError::NoEngine(reason)) => reason,
        Err(e) => e.to_string(),
    };
    if looks_installed() {
        DockerStatus::NotRunning {
            reason: Some(reason),
        }
    } else {
        DockerStatus::NotInstalled
    }
}

fn ok_status(v: bollard::models::SystemVersion) -> DockerStatus {
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
    DockerStatus::Ok {
        version: v.version.unwrap_or_else(|| "unbekannt".into()),
        product,
        warning: docker::insecure_host_warning(docker::docker_host().as_deref()),
    }
}

/// Heuristic: CLI on PATH, a known app bundle/install dir, or a socket file.
fn looks_installed() -> bool {
    if std::env::var_os("DOCKER_HOST").is_some() {
        return true;
    }
    if engine::find_on_path("docker").is_some() || engine::podman_path().is_some() {
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
        let s = serde_json::to_value(DockerStatus::NotRunning { reason: None }).unwrap();
        assert_eq!(s, serde_json::json!({"state": "notRunning"}));
        let s = serde_json::to_value(DockerStatus::NotRunning {
            reason: Some("npipe:////./pipe/docker_engine: HTTP 500: x".into()),
        })
        .unwrap();
        assert_eq!(s["state"], "notRunning");
        assert_eq!(s["reason"], "npipe:////./pipe/docker_engine: HTTP 500: x");
        let s = serde_json::to_value(DockerStatus::Ok {
            version: "27.0".into(),
            product: "Docker Engine".into(),
            warning: None,
        })
        .unwrap();
        assert_eq!(s["state"], "ok");
        assert_eq!(s["version"], "27.0");
        assert!(s.get("warning").is_none());
    }
}
