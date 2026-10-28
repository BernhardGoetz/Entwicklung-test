//! Code-Prüfer: Tauri backend.

pub mod docker_detect;
pub mod docs;
pub mod engine;
pub mod runner;

use tauri::{AppHandle, Emitter, Manager, RunEvent};

use runner::{docker, images, languages, RunReport, RunRequest, RunnerError};

/// Event name for progress updates during a run (image pull, phases).
pub const PROGRESS_EVENT: &str = "run-progress";
/// Event name for status messages while the container engine starts.
pub const ENGINE_EVENT: &str = "engine-progress";
/// Event name for progress while images are prepared or updated.
pub const IMAGES_EVENT: &str = "images-progress";
/// Event name for progress while a task document is imported.
pub const DOCS_EVENT: &str = "docs-progress";

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
/// missing (called once the engine is up, i.e. at app start). Also removes
/// sandbox containers left over from a crash.
#[tauri::command]
async fn prepare_images(app: AppHandle) -> Result<images::ImagesReport, RunnerError> {
    let docker = docker::connect().await?;
    let _ = docker::remove_orphans(&docker, docker::ORPHAN_AGE).await;
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

/// Imports a task sheet (image, PDF, DOCX) chosen in the file dialog and
/// returns its text for review. Parsing and OCR block, so they run on a
/// blocking thread.
#[tauri::command]
async fn import_task_document(
    app: AppHandle,
    path: String,
) -> Result<docs::ImportResult, docs::DocsError> {
    tauri::async_runtime::spawn_blocking(move || {
        docs::import_file(std::path::Path::new(&path), |p| {
            let _ = app.emit(DOCS_EVENT, p);
        })
    })
    .await
    .map_err(|e| docs::DocsError::Invalid(format!("Interner Fehler beim Import: {e}")))?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
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
            update_images,
            import_task_document
        ])
        .build(tauri::generate_context!())
        .expect("Fehler beim Starten der Anwendung")
        .run(|_app, event| {
            // Closing the app during a run must not leave its container behind.
            if let RunEvent::Exit = event {
                tauri::async_runtime::block_on(async {
                    let cleanup = async {
                        if let Ok(docker) = docker::connect().await {
                            let _ = docker::remove_own_sandboxes(&docker).await;
                        }
                    };
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), cleanup).await;
                });
            }
        });
}

#[cfg(test)]
mod tests {
    /// The frontend may only open the URLs the capability allows
    /// (`opener:allow-open-url` instead of `opener:default`).
    #[test]
    fn opener_capability_allows_exactly_the_download_pages() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read_to_string(root.join("capabilities/default.json")).unwrap();
        let cap: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let perms = cap["permissions"].as_array().unwrap();
        assert!(!perms.iter().any(|p| p == "opener:default"));
        let opener = perms
            .iter()
            .find(|p| p["identifier"] == "opener:allow-open-url")
            .expect("opener:allow-open-url");
        let mut allowed: Vec<&str> = opener["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["url"].as_str().unwrap())
            .collect();
        allowed.sort();

        let banner =
            std::fs::read_to_string(root.join("../src/components/DockerBanner.tsx")).unwrap();
        let mut used: Vec<&str> = banner
            .split('"')
            .filter(|s| s.starts_with("https://"))
            .collect();
        used.sort();
        assert_eq!(allowed, used);
    }
}
