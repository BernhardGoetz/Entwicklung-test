//! Code-Prüfer: Tauri backend.

pub mod docker_detect;
pub mod runner;

use tauri::{AppHandle, Emitter};

use runner::{languages, RunReport, RunRequest, RunnerError};

/// Event name for progress updates during a run (image pull, phases).
pub const PROGRESS_EVENT: &str = "run-progress";

#[tauri::command]
async fn docker_status() -> docker_detect::DockerStatus {
    docker_detect::detect().await
}

#[tauri::command]
fn list_languages() -> Vec<languages::LanguageInfo> {
    languages::infos()
}

#[tauri::command]
async fn run_tests(app: AppHandle, request: RunRequest) -> Result<RunReport, RunnerError> {
    runner::run_submission(request, |p| {
        let _ = app.emit(PROGRESS_EVENT, p);
    })
    .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            docker_status,
            list_languages,
            run_tests
        ])
        .run(tauri::generate_context!())
        .expect("Fehler beim Starten der Anwendung");
}
