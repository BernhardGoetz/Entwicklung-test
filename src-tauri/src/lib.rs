//! Code-Prüfer: Tauri backend.

pub mod docker_detect;
pub mod engine;
pub mod runner;

use tauri::{AppHandle, Emitter, Manager};

use runner::{docker, images, languages, RunReport, RunRequest, RunnerError};

/// Event name for progress updates during a run (image pull, phases).
pub const PROGRESS_EVENT: &str = "run-progress";
/// Event name for status messages while the container engine starts.
pub const ENGINE_EVENT: &str = "engine-progress";
/// Event name for progress while images are prepared or updated.
pub const IMAGES_EVENT: &str = "images-progress";

#[tauri::command]
async fn docker_status() -> docker_detect::DockerStatus {
    docker_detect::detect().await
}

#[tauri::command]
fn get_engine() -> engine::EngineChoice {
    engine::current()
}

#[tauri::command]
fn set_engine(engine: engine::Engine) -> Result<engine::EngineChoice, String> {
    engine::save(engine)
}

/// Starts the chosen engine (if it is not reachable yet), waits for its API
/// and returns the resulting status.
#[tauri::command]
async fn start_engine(app: AppHandle) -> Result<docker_detect::DockerStatus, String> {
    engine::start(engine::current().engine, |message| {
        let _ = app.emit(ENGINE_EVENT, message);
    })
    .await?;
    Ok(docker_detect::detect().await)
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

/// Loads bundled image archives into the active engine if the images are
/// missing (called once the engine is up).
#[tauri::command]
async fn prepare_images(app: AppHandle) -> Result<images::ImagesReport, RunnerError> {
    let docker = docker::connect().await?;
    Ok(images::prepare_all(&docker, |p| {
        let _ = app.emit(IMAGES_EVENT, p);
    })
    .await)
}

/// "Images aktualisieren": pulls the pinned images from GHCR.
#[tauri::command]
async fn update_images(app: AppHandle) -> Result<images::ImagesReport, RunnerError> {
    let docker = docker::connect().await?;
    Ok(images::update_all(&docker, |p| {
        let _ = app.emit(IMAGES_EVENT, p);
    })
    .await)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            if let Ok(dir) = app.path().app_config_dir() {
                engine::init(dir);
            }
            // Release builds ship the runner images as archives in
            // <resource_dir>/images (next to the .exe on Windows,
            // Contents/Resources on macOS).
            if let Ok(dir) = app.path().resource_dir() {
                images::init_bundle_dir(dir.join(images::BUNDLE_SUBDIR));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            docker_status,
            get_engine,
            set_engine,
            start_engine,
            list_languages,
            run_tests,
            prepare_images,
            update_images
        ])
        .run(tauri::generate_context!())
        .expect("Fehler beim Starten der Anwendung");
}
