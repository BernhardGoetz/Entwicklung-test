//! Provisioning of the runner images in the active engine (Docker or Podman).
//!
//! Resolution order when a language is used:
//! 1. env override (`CODE_PRUEFER_<LANG>_IMAGE`): used exclusively
//!    (present locally, otherwise pulled);
//! 2. the pinned image (`<IMAGE_REGISTRY>/code-pruefer-<id>:<version>`)
//!    already present in the engine's image store;
//! 3. the bundled archive `<resource_dir>/images/code-pruefer-<id>-<arch>.tar.gz`
//!    (shipped with release builds), loaded via `POST /images/load`;
//! 4. pull of the pinned image from GHCR;
//! 5. a locally built `code-pruefer-<id>:dev`;
//! 6. otherwise an actionable German error.
//!
//! Docker and Podman use the same image format but separate stores, so the
//! archive is always loaded into the engine the app is connected to.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use bollard::Docker;
use serde::Serialize;
use tokio::sync::Mutex;

use super::docker;
use super::languages::{self, LanguageConfig};
use super::{Progress, RunnerError};

/// Folder below the Tauri resource dir that holds the image archives.
pub const BUNDLE_SUBDIR: &str = "images";

static BUNDLE_DIR: OnceLock<PathBuf> = OnceLock::new();
/// Serialises image resolution so a run and the background preparation
/// never load or pull the same image twice.
static LOCK: Mutex<()> = Mutex::const_new(());

/// Sets the folder with the bundled archives (called once at app start with
/// `<resource_dir>/images`).
pub fn init_bundle_dir(dir: PathBuf) {
    let _ = BUNDLE_DIR.set(dir);
}

pub fn bundle_dir() -> Option<&'static Path> {
    BUNDLE_DIR.get().map(PathBuf::as_path)
}

/// Maps Rust (`x86_64`, `aarch64`) and engine (`amd64`, `arm64`)
/// architecture names to the OCI names used in the archive file names.
pub fn oci_arch(arch: &str) -> Option<&'static str> {
    match arch.trim().to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" | "x64" => Some("amd64"),
        "aarch64" | "arm64" | "arm64/v8" => Some("arm64"),
        _ => None,
    }
}

/// Architecture of the archive to load: the engine's (its Linux VM) if
/// known, otherwise the host's.
pub fn select_arch(engine_arch: Option<&str>) -> Option<&'static str> {
    engine_arch
        .and_then(oci_arch)
        .or_else(|| oci_arch(std::env::consts::ARCH))
}

pub fn archive_path(dir: &Path, lang: &LanguageConfig, arch: &str) -> PathBuf {
    dir.join(lang.archive_name(arch))
}

/// The bundled archive for `lang`, if it exists.
pub fn bundled_archive(
    dir: Option<&Path>,
    lang: &LanguageConfig,
    arch: Option<&str>,
) -> Option<PathBuf> {
    let path = archive_path(dir?, lang, arch?);
    path.is_file().then_some(path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Use the image if it is in the engine's store.
    Present(String),
    /// Load the archive and make sure it provides `image`.
    Load { archive: PathBuf, image: String },
    /// Pull from the registry.
    Pull(String),
}

/// The resolution order (see module docs). Pure, unit-tested.
pub fn plan(
    lang: &LanguageConfig,
    override_image: Option<String>,
    archive: Option<PathBuf>,
) -> Vec<Step> {
    if let Some(image) = override_image {
        return vec![Step::Present(image.clone()), Step::Pull(image)];
    }
    let primary = lang.default_image.to_string();
    let mut steps = vec![Step::Present(primary.clone())];
    if let Some(archive) = archive {
        steps.push(Step::Load {
            archive,
            image: primary.clone(),
        });
    }
    steps.push(Step::Pull(primary));
    steps.push(Step::Present(lang.local_image.to_string()));
    steps
}

/// Image store operations used by [`execute`]; abstracted for unit tests.
trait Store {
    async fn exists(&self, image: &str) -> Result<bool, RunnerError>;
    async fn load(
        &self,
        archive: &Path,
        image: &str,
        on_progress: &mut impl FnMut(Progress),
    ) -> Result<(), RunnerError>;
    async fn pull(
        &self,
        image: &str,
        on_progress: &mut impl FnMut(Progress),
    ) -> Result<(), RunnerError>;
}

struct EngineStore<'a>(&'a Docker);

impl Store for EngineStore<'_> {
    async fn exists(&self, image: &str) -> Result<bool, RunnerError> {
        docker::image_exists(self.0, image).await
    }

    /// Loads the archive; if it carries a different name, the loaded image
    /// is tagged as `image`.
    async fn load(
        &self,
        archive: &Path,
        image: &str,
        on_progress: &mut impl FnMut(Progress),
    ) -> Result<(), RunnerError> {
        let loaded = docker::load_image_archive(self.0, archive, image, |percent| {
            on_progress(Progress::Load {
                image: image.to_string(),
                percent,
            })
        })
        .await?;
        if self.exists(image).await? {
            return Ok(());
        }
        let mismatch = || {
            RunnerError::ImageLoad(
                archive.display().to_string(),
                format!(
                    "Das Archiv enthält nicht das erwartete Image {image} (gefunden: {}).",
                    loaded.join(", ")
                ),
            )
        };
        let source = loaded.first().ok_or_else(mismatch)?;
        docker::tag_image(self.0, source, image).await?;
        if self.exists(image).await? {
            Ok(())
        } else {
            Err(mismatch())
        }
    }

    async fn pull(
        &self,
        image: &str,
        on_progress: &mut impl FnMut(Progress),
    ) -> Result<(), RunnerError> {
        docker::pull_image(self.0, image, |p| on_progress(Progress::Pull(p))).await
    }
}

/// Walks `steps` and returns the first usable image.
async fn execute(
    store: &impl Store,
    lang: &LanguageConfig,
    steps: &[Step],
    on_progress: &mut impl FnMut(Progress),
) -> Result<String, RunnerError> {
    let mut failures: Vec<String> = Vec::new();
    for step in steps {
        match step {
            Step::Present(image) => {
                if store.exists(image).await? {
                    return Ok(image.clone());
                }
            }
            Step::Load { archive, image } => {
                on_progress(Progress::Phase {
                    message: format!(
                        "Lade Image für {} aus dem Installationspaket (einmalig) …",
                        lang.name
                    ),
                });
                match store.load(archive, image, on_progress).await {
                    Ok(()) => return Ok(image.clone()),
                    Err(e) => failures.push(e.to_string()),
                }
            }
            Step::Pull(image) => match store.pull(image, on_progress).await {
                Ok(()) => return Ok(image.clone()),
                Err(RunnerError::ImagePull(_, detail)) => {
                    failures.push(format!("Download: {}", detail.trim().replace('\n', " ")))
                }
                Err(e) => failures.push(format!("Download: {e}")),
            },
        }
    }
    let mut tried: Vec<&str> = Vec::new();
    for step in steps {
        let (Step::Present(i) | Step::Pull(i) | Step::Load { image: i, .. }) = step;
        if !tried.contains(&i.as_str()) {
            tried.push(i);
        }
    }
    Err(RunnerError::ImageMissing {
        language: lang.name.to_string(),
        id: lang.id.to_string(),
        tried: tried.join(", "),
        local_image: lang.local_image.to_string(),
        detail: if failures.is_empty() {
            "keine Quelle verfügbar".into()
        } else {
            failures.join("; ")
        },
    })
}

async fn archive_for(docker: &Docker, lang: &LanguageConfig) -> Option<PathBuf> {
    let dir = bundle_dir()?;
    let arch = docker::engine_arch(docker).await;
    bundled_archive(Some(dir), lang, select_arch(arch.as_deref()))
}

/// Makes the image for `lang` available in the connected engine and
/// returns its reference.
pub async fn ensure_image(
    docker: &Docker,
    lang: &LanguageConfig,
    on_progress: &mut impl FnMut(Progress),
) -> Result<String, RunnerError> {
    let _guard = LOCK.lock().await;
    let override_image = lang.image_override();
    let archive = if override_image.is_none() {
        archive_for(docker, lang).await
    } else {
        None
    };
    let steps = plan(lang, override_image, archive);
    if let Some(Step::Present(first)) = steps.first() {
        on_progress(Progress::Phase {
            message: format!("Prüfe Image {first} …"),
        });
    }
    execute(&EngineStore(docker), lang, &steps, on_progress).await
}

/// Result of [`prepare_all`] / [`update_all`] for the UI.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagesReport {
    /// Display names of the languages whose image was loaded/updated.
    pub done: Vec<String>,
    /// German error messages (non-fatal).
    pub errors: Vec<String>,
}

/// Background preparation after the engine is up: loads every bundled
/// archive whose image is missing. Never pulls (that happens on demand).
pub async fn prepare_all(docker: &Docker, mut on_progress: impl FnMut(Progress)) -> ImagesReport {
    let mut report = ImagesReport::default();
    for lang in languages::LANGUAGES {
        if lang.image_override().is_some() {
            continue;
        }
        let _guard = LOCK.lock().await;
        let image = lang.default_image;
        match docker::image_exists(docker, image).await {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                report.errors.push(e.to_string());
                break;
            }
        }
        let Some(archive) = archive_for(docker, lang).await else {
            continue;
        };
        on_progress(Progress::Phase {
            message: format!("Lade Image für {} aus dem Installationspaket …", lang.name),
        });
        match EngineStore(docker)
            .load(&archive, image, &mut on_progress)
            .await
        {
            Ok(()) => report.done.push(lang.name.to_string()),
            Err(e) => report.errors.push(e.to_string()),
        }
    }
    report
}

/// "Images aktualisieren": pulls the pinned image of every language.
pub async fn update_all(docker: &Docker, mut on_progress: impl FnMut(Progress)) -> ImagesReport {
    let mut report = ImagesReport::default();
    for lang in languages::LANGUAGES {
        let _guard = LOCK.lock().await;
        let image = lang.image();
        match EngineStore(docker).pull(&image, &mut on_progress).await {
            Ok(()) => report.done.push(lang.name.to_string()),
            Err(RunnerError::ImagePull(_, detail)) => report.errors.push(format!(
                "{}: {image} konnte nicht heruntergeladen werden ({}).",
                lang.name,
                detail.trim().replace('\n', " ")
            )),
            Err(e) => report.errors.push(format!("{}: {e}", lang.name)),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn py() -> &'static LanguageConfig {
        languages::get("python").unwrap()
    }

    #[test]
    fn arch_mapping() {
        assert_eq!(oci_arch("x86_64"), Some("amd64"));
        assert_eq!(oci_arch("aarch64"), Some("arm64"));
        assert_eq!(oci_arch("AMD64"), Some("amd64"));
        assert_eq!(oci_arch("arm64"), Some("arm64"));
        assert_eq!(oci_arch("riscv64"), None);
        // The engine's architecture wins over the host's.
        assert_eq!(select_arch(Some("arm64")), Some("arm64"));
        assert_eq!(select_arch(Some("amd64")), Some("amd64"));
        assert_eq!(select_arch(None), oci_arch(std::env::consts::ARCH));
        assert_eq!(select_arch(Some("s390x")), oci_arch(std::env::consts::ARCH));
    }

    #[test]
    fn archive_path_selection() {
        let dir = std::env::temp_dir().join(format!("code-pruefer-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            archive_path(&dir, py(), "arm64"),
            dir.join("code-pruefer-python-arm64.tar.gz")
        );
        assert_eq!(bundled_archive(Some(&dir), py(), Some("amd64")), None);
        std::fs::write(dir.join("code-pruefer-python-amd64.tar.gz"), b"x").unwrap();
        assert_eq!(
            bundled_archive(Some(&dir), py(), Some("amd64")),
            Some(dir.join("code-pruefer-python-amd64.tar.gz"))
        );
        assert_eq!(bundled_archive(Some(&dir), py(), Some("arm64")), None);
        assert_eq!(bundled_archive(Some(&dir), py(), None), None);
        assert_eq!(bundled_archive(None, py(), Some("amd64")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn plan_order() {
        let primary = py().default_image.to_string();
        let archive = PathBuf::from("/res/images/code-pruefer-python-amd64.tar.gz");
        assert_eq!(
            plan(py(), None, Some(archive.clone())),
            vec![
                Step::Present(primary.clone()),
                Step::Load {
                    archive,
                    image: primary.clone()
                },
                Step::Pull(primary.clone()),
                Step::Present("code-pruefer-python:dev".into()),
            ]
        );
        assert_eq!(
            plan(py(), None, None),
            vec![
                Step::Present(primary.clone()),
                Step::Pull(primary),
                Step::Present("code-pruefer-python:dev".into()),
            ]
        );
        assert_eq!(
            plan(py(), Some("mein:x".into()), None),
            vec![Step::Present("mein:x".into()), Step::Pull("mein:x".into())]
        );
    }

    /// In-memory store recording the calls.
    #[derive(Default)]
    struct Fake {
        present: RefCell<Vec<String>>,
        load_ok: bool,
        pull_ok: bool,
        calls: RefCell<Vec<String>>,
    }

    impl Store for Fake {
        async fn exists(&self, image: &str) -> Result<bool, RunnerError> {
            self.calls.borrow_mut().push(format!("exists {image}"));
            Ok(self.present.borrow().iter().any(|p| p == image))
        }
        async fn load(
            &self,
            _archive: &Path,
            image: &str,
            _on: &mut impl FnMut(Progress),
        ) -> Result<(), RunnerError> {
            self.calls.borrow_mut().push(format!("load {image}"));
            if self.load_ok {
                self.present.borrow_mut().push(image.into());
                Ok(())
            } else {
                Err(RunnerError::ImageLoad("a.tar.gz".into(), "kaputt".into()))
            }
        }
        async fn pull(
            &self,
            image: &str,
            _on: &mut impl FnMut(Progress),
        ) -> Result<(), RunnerError> {
            self.calls.borrow_mut().push(format!("pull {image}"));
            if self.pull_ok {
                Ok(())
            } else {
                Err(RunnerError::ImagePull(image.into(), "denied".into()))
            }
        }
    }

    fn run(store: &Fake, archive: bool) -> Result<String, RunnerError> {
        let archive = archive.then(|| PathBuf::from("a.tar.gz"));
        let steps = plan(py(), None, archive);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(execute(store, py(), &steps, &mut |_| {}))
    }

    #[test]
    fn present_image_is_used_without_loading() {
        let store = Fake::default();
        store.present.borrow_mut().push(py().default_image.into());
        assert_eq!(run(&store, true).unwrap(), py().default_image);
        assert_eq!(store.calls.borrow().len(), 1);
    }

    #[test]
    fn bundled_archive_comes_before_pull() {
        let store = Fake {
            load_ok: true,
            pull_ok: true,
            ..Default::default()
        };
        assert_eq!(run(&store, true).unwrap(), py().default_image);
        let calls = store.calls.borrow();
        assert!(calls[1].starts_with("load "), "{calls:?}");
        assert!(!calls.iter().any(|c| c.starts_with("pull ")));
    }

    #[test]
    fn failed_load_falls_back_to_pull_then_dev() {
        let store = Fake {
            pull_ok: true,
            ..Default::default()
        };
        assert_eq!(run(&store, true).unwrap(), py().default_image);
        let store = Fake::default();
        store
            .present
            .borrow_mut()
            .push("code-pruefer-python:dev".into());
        assert_eq!(run(&store, true).unwrap(), "code-pruefer-python:dev");
        let calls = store.calls.borrow();
        let kinds: Vec<&str> = calls.iter().map(|c| c.split(' ').next().unwrap()).collect();
        assert_eq!(kinds, vec!["exists", "load", "pull", "exists"]);
    }

    #[test]
    fn nothing_available_gives_german_error() {
        let store = Fake::default();
        let msg = run(&store, true).unwrap_err().to_string();
        assert!(msg.contains("Installationspaket"), "{msg}");
        assert!(msg.contains("kaputt"), "{msg}");
        assert!(msg.contains("Download: denied"), "{msg}");
        assert!(msg.contains("code-pruefer-python:dev"), "{msg}");
    }
}
