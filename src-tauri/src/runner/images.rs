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
//! Builds with embedded images also ship `<resource_dir>/images/image-digests.json`
//! (written by `scripts/export-image-archives.sh`). With it, the pinned image
//! only counts if its ID (or registry digest) is one listed there, archives
//! are checked against their SHA-256 before loading and against the expected
//! image ID after loading, and pulls use `<repo>@sha256:…` instead of the tag.
//! A rejected image is removed again. Without the file (dev and CI builds)
//! images are trusted by tag as before.
//!
//! Docker and Podman use the same image format but separate stores, so the
//! archive is always loaded into the engine the app is connected to.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use bollard::Docker;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::docker::{self, ImageIdentity};
use super::languages::{self, LanguageConfig};
use super::{Progress, RunnerError};

/// Folder below the Tauri resource dir that holds the image archives.
pub const BUNDLE_SUBDIR: &str = "images";
/// File in the bundle folder with the expected digests.
pub const DIGESTS_FILE: &str = "image-digests.json";

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

/// Contents of `image-digests.json`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Digests {
    /// App version the file was written for.
    pub version: String,
    /// Per language id.
    #[serde(default)]
    pub images: HashMap<String, ImageDigests>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageDigests {
    /// Digest of the pushed manifest list (`sha256:…`); missing in local
    /// builds whose images were never pushed.
    #[serde(default)]
    pub repo_digest: Option<String>,
    /// Per OCI architecture (`amd64`, `arm64`).
    #[serde(default)]
    pub archives: HashMap<String, ArchiveDigests>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveDigests {
    /// SHA-256 of the `.tar.gz` file (hex).
    pub sha256: String,
    /// IDs the engine may report for the loaded image (manifest and config
    /// digest, see [`ImageIdentity`]).
    pub image_ids: Vec<String>,
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_digest(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(is_hex64)
}

/// `sha256:<hex>` and `<hex>` (Podman) compare equal.
fn normalize_id(id: &str) -> String {
    let id = id.trim().to_ascii_lowercase();
    id.strip_prefix("sha256:").unwrap_or(&id).to_string()
}

impl Digests {
    /// Parses and validates the file; the error text is German.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let d: Digests = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        if d.version != languages::IMAGE_TAG {
            return Err(format!(
                "für Version {} statt {}",
                d.version,
                languages::IMAGE_TAG
            ));
        }
        for (id, img) in &d.images {
            let bad_repo = img.repo_digest.as_deref().is_some_and(|r| !is_digest(r));
            let bad_archive = img.archives.values().any(|a| {
                !is_hex64(&a.sha256)
                    || a.image_ids.is_empty()
                    || !a.image_ids.iter().all(|i| is_digest(i))
            });
            if bad_repo || bad_archive {
                return Err(format!("ungültiger Eintrag für {id}"));
            }
        }
        Ok(d)
    }

    /// What the pinned image of `lang` must look like; `arch` selects the
    /// archive whose file hash is expected.
    pub fn verify_for(&self, lang: &LanguageConfig, arch: Option<&str>) -> Verify {
        let entry = self.images.get(lang.id);
        let repo = docker::split_ref(lang.default_image).0;
        let mut ids: Vec<String> = entry
            .into_iter()
            .flat_map(|e| e.archives.values())
            .flat_map(|a| a.image_ids.iter().cloned())
            .chain(entry.and_then(|e| e.repo_digest.clone()))
            .collect();
        ids.sort();
        ids.dedup();
        Verify {
            ids,
            repo_digest: entry
                .and_then(|e| e.repo_digest.as_ref())
                .map(|d| format!("{repo}@{d}")),
            archive_sha256: entry
                .zip(arch)
                .and_then(|(e, a)| e.archives.get(a))
                .map(|a| a.sha256.clone()),
        }
    }
}

/// Reads `<dir>/image-digests.json`; `Ok(None)` if there is none.
pub fn load_digests(dir: Option<&Path>) -> Result<Option<Digests>, RunnerError> {
    let Some(path) = dir.map(|d| d.join(DIGESTS_FILE)) else {
        return Ok(None);
    };
    let invalid = |detail: String| {
        RunnerError::Other(format!(
            "Die Prüfsummen-Datei {} ist ungültig ({detail}). Bitte den Code-Prüfer neu installieren.",
            path.display()
        ))
    };
    match std::fs::read_to_string(&path) {
        Ok(raw) => Digests::parse(&raw).map(Some).map_err(invalid),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(invalid(e.to_string())),
    }
}

/// Expected identity of a pinned image (from [`Digests::verify_for`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Verify {
    /// Accepted image IDs (archive manifest/config digests, registry digest).
    pub ids: Vec<String>,
    /// `<repo>@sha256:…` of the published image, also used for pulls.
    pub repo_digest: Option<String>,
    /// SHA-256 of the archive for the engine's architecture.
    pub archive_sha256: Option<String>,
}

impl Verify {
    /// Whether an image in the store is the expected one.
    pub fn accepts(&self, image: &ImageIdentity) -> bool {
        let id = normalize_id(&image.id);
        self.ids.iter().any(|i| normalize_id(i) == id)
            || self
                .repo_digest
                .as_ref()
                .is_some_and(|rd| image.repo_digests.iter().any(|r| r == rd))
    }

    fn accepts_id(&self, id: &str) -> bool {
        let id = normalize_id(id);
        self.ids.iter().any(|i| normalize_id(i) == id)
    }
}

/// Repository name without registry, namespace and tag
/// (`ghcr.io/x/code-pruefer-python:1` → `code-pruefer-python`).
fn repo_basename(image: &str) -> &str {
    let repo = docker::split_ref(image).0;
    repo.rsplit('/').next().unwrap_or(repo)
}

/// The loaded image to tag as `image`: one with an expected ID, otherwise
/// one with the same repository name (e.g. `code-pruefer-python:dev`).
/// Pure, unit-tested.
pub fn pick_source<'a>(
    loaded: &'a [String],
    image: &str,
    verify: Option<&Verify>,
) -> Option<&'a String> {
    let is_id = |l: &str| is_digest(l) || is_hex64(l);
    loaded
        .iter()
        .find(|l| is_id(l) && verify.is_some_and(|v| v.accepts_id(l)))
        .or_else(|| {
            loaded
                .iter()
                .find(|l| !is_id(l) && repo_basename(l) == repo_basename(image))
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Use the image if it is in the engine's store.
    Present(String),
    /// Load the archive and make sure it provides `image`.
    Load { archive: PathBuf, image: String },
    /// Pull `source` (the tag or `<repo>@sha256:…`) and provide it as `image`.
    Pull { source: String, image: String },
}

/// The resolution order (see module docs). `pull` is the reference to pull
/// the pinned image from (`None`: no download). Pure, unit-tested.
pub fn plan(
    lang: &LanguageConfig,
    override_image: Option<String>,
    archive: Option<PathBuf>,
    pull: Option<String>,
) -> Vec<Step> {
    if let Some(image) = override_image {
        return vec![
            Step::Present(image.clone()),
            Step::Pull {
                source: image.clone(),
                image,
            },
        ];
    }
    let primary = lang.default_image.to_string();
    let mut steps = vec![Step::Present(primary.clone())];
    if let Some(archive) = archive {
        steps.push(Step::Load {
            archive,
            image: primary.clone(),
        });
    }
    if let Some(source) = pull {
        steps.push(Step::Pull {
            source,
            image: primary,
        });
    }
    steps.push(Step::Present(lang.local_image.to_string()));
    steps
}

/// Image store operations used by [`execute`]; abstracted for unit tests.
trait Store {
    async fn inspect(&self, image: &str) -> Result<Option<ImageIdentity>, RunnerError>;
    async fn load(
        &self,
        archive: &Path,
        image: &str,
        verify: Option<&Verify>,
        on_progress: &mut impl FnMut(Progress),
    ) -> Result<(), RunnerError>;
    async fn pull(
        &self,
        source: &str,
        image: &str,
        on_progress: &mut impl FnMut(Progress),
    ) -> Result<(), RunnerError>;
    async fn remove(&self, image: &str);
}

struct EngineStore<'a>(&'a Docker);

impl Store for EngineStore<'_> {
    async fn inspect(&self, image: &str) -> Result<Option<ImageIdentity>, RunnerError> {
        docker::inspect_identity(self.0, image).await
    }

    /// Checks the archive's SHA-256 (if digests are known), loads it and,
    /// if it carries a different name, tags the matching loaded image as
    /// `image`.
    async fn load(
        &self,
        archive: &Path,
        image: &str,
        verify: Option<&Verify>,
        on_progress: &mut impl FnMut(Progress),
    ) -> Result<(), RunnerError> {
        let fail = |detail: String| RunnerError::ImageLoad(archive.display().to_string(), detail);
        if let Some(verify) = verify {
            let Some(expected) = verify.archive_sha256.as_deref() else {
                return Err(fail(
                    "Für dieses Archiv ist keine Prüfsumme hinterlegt.".into(),
                ));
            };
            let actual = docker::sha256_file(archive)
                .await
                .map_err(|e| fail(e.to_string()))?;
            if actual != expected {
                return Err(fail(
                    "Die Prüfsumme des Archivs stimmt nicht – die Datei ist beschädigt oder \
                     wurde verändert. Bitte den Code-Prüfer neu installieren."
                        .into(),
                ));
            }
        }
        let loaded = docker::load_image_archive(self.0, archive, image, |percent| {
            on_progress(Progress::Load {
                image: image.to_string(),
                percent,
            })
        })
        .await?;
        if loaded.is_empty() || loaded.iter().any(|l| l == image) {
            return Ok(());
        }
        let source = pick_source(&loaded, image, verify).ok_or_else(|| {
            fail(format!(
                "Das Archiv enthält nicht das erwartete Image {image} (gefunden: {}).",
                loaded.join(", ")
            ))
        })?;
        docker::tag_image(self.0, source, image).await
    }

    async fn pull(
        &self,
        source: &str,
        image: &str,
        on_progress: &mut impl FnMut(Progress),
    ) -> Result<(), RunnerError> {
        docker::pull_image(self.0, source, |p| on_progress(Progress::Pull(p))).await?;
        if source != image {
            docker::tag_image(self.0, source, image).await?;
        }
        Ok(())
    }

    async fn remove(&self, image: &str) {
        docker::remove_image(self.0, image).await
    }
}

/// After a load or pull: `image` must exist and, if digests are known, be
/// the expected one; otherwise it is removed again. `Err` carries the
/// German reason.
async fn check(store: &impl Store, image: &str, verify: Option<&Verify>) -> Result<(), String> {
    match store.inspect(image).await {
        Ok(Some(found)) if verify.is_none_or(|v| v.accepts(&found)) => Ok(()),
        Ok(Some(found)) => {
            store.remove(image).await;
            Err(format!(
                "Das Image hat nicht die erwartete Image-ID (gefunden: {}) und wurde wieder entfernt.",
                found.id
            ))
        }
        Ok(None) => Err(format!("Image {image} ist danach nicht vorhanden.")),
        Err(e) => Err(e.to_string()),
    }
}

/// Loads `archive` as `image` and checks the result.
async fn load_checked(
    store: &impl Store,
    archive: &Path,
    image: &str,
    verify: Option<&Verify>,
    on_progress: &mut impl FnMut(Progress),
) -> Result<(), RunnerError> {
    store.load(archive, image, verify, on_progress).await?;
    check(store, image, verify)
        .await
        .map_err(|e| RunnerError::ImageLoad(archive.display().to_string(), e))
}

/// Pulls `source` as `image` and checks the result.
async fn pull_checked(
    store: &impl Store,
    source: &str,
    image: &str,
    verify: Option<&Verify>,
    on_progress: &mut impl FnMut(Progress),
) -> Result<(), RunnerError> {
    store.pull(source, image, on_progress).await?;
    check(store, image, verify)
        .await
        .map_err(|e| RunnerError::ImagePull(source.to_string(), e))
}

/// Walks `steps` and returns the first usable image. `verify` applies to
/// the pinned image (`lang.default_image`) only.
async fn execute(
    store: &impl Store,
    lang: &LanguageConfig,
    steps: &[Step],
    verify: Option<&Verify>,
    on_progress: &mut impl FnMut(Progress),
) -> Result<String, RunnerError> {
    let verify_for = |image: &str| verify.filter(|_| image == lang.default_image);
    let mut failures: Vec<String> = Vec::new();
    for step in steps {
        match step {
            Step::Present(image) => match store.inspect(image).await? {
                Some(found) if verify_for(image).is_none_or(|v| v.accepts(&found)) => {
                    return Ok(image.clone())
                }
                Some(_) => failures.push(format!(
                    "Das vorhandene Image {image} ist nicht das erwartete"
                )),
                None => {}
            },
            Step::Load { archive, image } => {
                on_progress(Progress::Phase {
                    message: format!(
                        "Lade Image für {} aus dem Installationspaket (einmalig) …",
                        lang.name
                    ),
                });
                match load_checked(store, archive, image, verify_for(image), on_progress).await {
                    Ok(()) => return Ok(image.clone()),
                    Err(e) => failures.push(e.to_string()),
                }
            }
            Step::Pull { source, image } => {
                match pull_checked(store, source, image, verify_for(image), on_progress).await {
                    Ok(()) => return Ok(image.clone()),
                    Err(RunnerError::ImagePull(_, detail)) => {
                        failures.push(format!("Download: {}", detail.trim().replace('\n', " ")))
                    }
                    Err(e) => failures.push(format!("Download: {e}")),
                }
            }
        }
    }
    let mut tried: Vec<&str> = Vec::new();
    for step in steps {
        let (Step::Present(i) | Step::Pull { image: i, .. } | Step::Load { image: i, .. }) = step;
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

/// Bundled archive and expected digests for `lang` in the connected engine.
async fn bundle_for(
    docker: &Docker,
    lang: &LanguageConfig,
) -> Result<(Option<PathBuf>, Option<Verify>), RunnerError> {
    let dir = bundle_dir();
    let digests = load_digests(dir)?;
    let arch = select_arch(docker::engine_arch(docker).await.as_deref());
    Ok((
        bundled_archive(dir, lang, arch),
        digests.map(|d| d.verify_for(lang, arch)),
    ))
}

/// Reference to pull the pinned image from: its registry digest if the
/// digests are known (none known: no download), otherwise the tag.
fn pull_source(lang: &LanguageConfig, verify: Option<&Verify>) -> Option<String> {
    match verify {
        Some(v) => v.repo_digest.clone(),
        None => Some(lang.default_image.to_string()),
    }
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
    let (archive, verify) = if override_image.is_none() {
        bundle_for(docker, lang).await?
    } else {
        (None, None)
    };
    let pull = pull_source(lang, verify.as_ref());
    let steps = plan(lang, override_image, archive, pull);
    if let Some(Step::Present(first)) = steps.first() {
        on_progress(Progress::Phase {
            message: format!("Prüfe Image {first} …"),
        });
    }
    execute(
        &EngineStore(docker),
        lang,
        &steps,
        verify.as_ref(),
        on_progress,
    )
    .await
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
/// archive whose image is missing (or not the expected one). Never pulls
/// (that happens on demand).
pub async fn prepare_all(docker: &Docker, mut on_progress: impl FnMut(Progress)) -> ImagesReport {
    let mut report = ImagesReport::default();
    let store = EngineStore(docker);
    for lang in languages::LANGUAGES {
        if lang.image_override().is_some() {
            continue;
        }
        let _guard = LOCK.lock().await;
        let image = lang.default_image;
        let (archive, verify) = match bundle_for(docker, lang).await {
            Ok(found) => found,
            Err(e) => {
                report.errors.push(e.to_string());
                break;
            }
        };
        match store.inspect(image).await {
            Ok(Some(found)) if verify.as_ref().is_none_or(|v| v.accepts(&found)) => continue,
            Ok(_) => {}
            Err(e) => {
                report.errors.push(e.to_string());
                break;
            }
        }
        let Some(archive) = archive else {
            continue;
        };
        on_progress(Progress::Phase {
            message: format!("Lade Image für {} aus dem Installationspaket …", lang.name),
        });
        match load_checked(&store, &archive, image, verify.as_ref(), &mut on_progress).await {
            Ok(()) => report.done.push(lang.name.to_string()),
            Err(e) => report.errors.push(e.to_string()),
        }
    }
    report
}

/// "Images aktualisieren": pulls the pinned image of every language (by
/// registry digest if known).
pub async fn update_all(docker: &Docker, mut on_progress: impl FnMut(Progress)) -> ImagesReport {
    let mut report = ImagesReport::default();
    let store = EngineStore(docker);
    for lang in languages::LANGUAGES {
        let _guard = LOCK.lock().await;
        let (image, verify) = match lang.image_override() {
            Some(image) => (image, None),
            None => match bundle_for(docker, lang).await {
                Ok((_, verify)) => (lang.default_image.to_string(), verify),
                Err(e) => {
                    report.errors.push(e.to_string());
                    break;
                }
            },
        };
        let source = if verify.is_some() {
            pull_source(lang, verify.as_ref())
        } else {
            Some(image.clone())
        };
        let Some(source) = source else {
            report.errors.push(format!(
                "{}: Für diese Version ist kein veröffentlichtes Image hinterlegt (lokaler Build) – \
                 das mitgelieferte Image wird weiter verwendet.",
                lang.name
            ));
            continue;
        };
        match pull_checked(&store, &source, &image, verify.as_ref(), &mut on_progress).await {
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
        let pull = |source: &str| Step::Pull {
            source: source.into(),
            image: primary.clone(),
        };
        assert_eq!(
            plan(py(), None, Some(archive.clone()), Some(primary.clone())),
            vec![
                Step::Present(primary.clone()),
                Step::Load {
                    archive,
                    image: primary.clone()
                },
                pull(&primary),
                Step::Present("code-pruefer-python:dev".into()),
            ]
        );
        assert_eq!(
            plan(py(), None, None, Some(primary.clone())),
            vec![
                Step::Present(primary.clone()),
                pull(&primary),
                Step::Present("code-pruefer-python:dev".into()),
            ]
        );
        assert_eq!(
            plan(py(), Some("mein:x".into()), None, Some(primary.clone())),
            vec![
                Step::Present("mein:x".into()),
                Step::Pull {
                    source: "mein:x".into(),
                    image: "mein:x".into()
                }
            ]
        );
    }

    fn hex(c: char) -> String {
        std::iter::repeat_n(c, 64).collect()
    }

    fn digests_json(repo_digest: Option<&str>) -> String {
        let repo = repo_digest
            .map(|d| format!(r#""repoDigest": "{d}","#))
            .unwrap_or_default();
        format!(
            r#"{{"version": "{}", "registry": "ghcr.io/x",
                "images": {{"python": {{{repo} "archives": {{
                  "amd64": {{"file": "code-pruefer-python-amd64.tar.gz", "sha256": "{}",
                             "imageIds": ["sha256:{}", "sha256:{}"]}},
                  "arm64": {{"file": "code-pruefer-python-arm64.tar.gz", "sha256": "{}",
                             "imageIds": ["sha256:{}"]}}}}}}}}}}"#,
            languages::IMAGE_TAG,
            hex('a'),
            hex('b'),
            hex('c'),
            hex('d'),
            hex('e')
        )
    }

    #[test]
    fn digests_file_is_validated() {
        let rd = format!("sha256:{}", hex('f'));
        let d = Digests::parse(&digests_json(Some(&rd))).unwrap();
        assert_eq!(d.images["python"].archives["arm64"].sha256, hex('d'));
        // Wrong version, malformed digests and garbage are rejected.
        let other = digests_json(None).replace(languages::IMAGE_TAG, "9.9.9");
        assert!(Digests::parse(&other).unwrap_err().contains("9.9.9"));
        assert!(Digests::parse(&digests_json(Some("sha256:abc"))).is_err());
        assert!(Digests::parse(&digests_json(None).replace(&hex('a'), "a")).is_err());
        assert!(Digests::parse(&digests_json(None).replace(&hex('a'), &hex('A'))).is_err());
        assert!(Digests::parse("{").is_err());
    }

    #[test]
    fn digests_file_is_optional() {
        let dir = std::env::temp_dir().join(format!("code-pruefer-digests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load_digests(None).unwrap(), None);
        assert_eq!(load_digests(Some(&dir)).unwrap(), None);
        std::fs::write(dir.join(DIGESTS_FILE), digests_json(None)).unwrap();
        assert!(load_digests(Some(&dir)).unwrap().is_some());
        std::fs::write(dir.join(DIGESTS_FILE), "kaputt").unwrap();
        let msg = load_digests(Some(&dir)).unwrap_err().to_string();
        assert!(msg.contains("Prüfsummen-Datei"), "{msg}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn digests_select_pull_reference_and_ids() {
        let rd = format!("sha256:{}", hex('f'));
        let d = Digests::parse(&digests_json(Some(&rd))).unwrap();
        let v = d.verify_for(py(), Some("amd64"));
        let repo = "ghcr.io/bernhardgoetz/code-pruefer-python";
        assert_eq!(v.repo_digest, Some(format!("{repo}@{rd}")));
        assert_eq!(v.archive_sha256, Some(hex('a')));
        assert_eq!(v.ids.len(), 4, "{:?}", v.ids);
        // Pulls use the registry digest, never the tag.
        assert_eq!(pull_source(py(), Some(&v)), Some(format!("{repo}@{rd}")));
        assert_eq!(
            plan(py(), None, None, pull_source(py(), Some(&v)))[1],
            Step::Pull {
                source: format!("{repo}@{rd}"),
                image: py().default_image.into()
            }
        );
        // Local build (no registry digest): no download; no file: tag.
        let local = Digests::parse(&digests_json(None)).unwrap();
        assert_eq!(pull_source(py(), Some(&local.verify_for(py(), None))), None);
        assert_eq!(pull_source(py(), None), Some(py().default_image.into()));
        // Archive of the other architecture; unknown language.
        assert_eq!(
            d.verify_for(py(), Some("arm64")).archive_sha256,
            Some(hex('d'))
        );
        let java = languages::get("java").unwrap();
        assert_eq!(d.verify_for(java, Some("amd64")), Verify::default());
    }

    #[test]
    fn verify_accepts_expected_ids_only() {
        let rd = format!("sha256:{}", hex('f'));
        let v = Digests::parse(&digests_json(Some(&rd)))
            .unwrap()
            .verify_for(py(), Some("amd64"));
        let ident = |id: String, repo_digests: Vec<String>| ImageIdentity { id, repo_digests };
        // Manifest digest (containerd store), config digest, Podman w/o prefix.
        assert!(v.accepts(&ident(format!("sha256:{}", hex('b')), vec![])));
        assert!(v.accepts(&ident(format!("sha256:{}", hex('c')), vec![])));
        assert!(v.accepts(&ident(hex('e'), vec![])));
        // Pulled by registry digest (classic store: config ID unknown).
        let pulled = format!("ghcr.io/bernhardgoetz/code-pruefer-python@{rd}");
        assert!(v.accepts(&ident(format!("sha256:{}", hex('9')), vec![pulled])));
        // Anything else is rejected, also the same digest in another repo.
        assert!(!v.accepts(&ident(format!("sha256:{}", hex('9')), vec![])));
        let elsewhere = format!("ghcr.io/evil/code-pruefer-python@{rd}");
        assert!(!v.accepts(&ident(format!("sha256:{}", hex('9')), vec![elsewhere])));
    }

    #[test]
    fn only_matching_loaded_image_is_retagged() {
        let image = py().default_image;
        let v = Verify {
            ids: vec![format!("sha256:{}", hex('b'))],
            ..Default::default()
        };
        let loaded = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Same repository name, any registry/tag.
        let l = loaded(&["evil:latest", "localhost/code-pruefer-python:dev"]);
        assert_eq!(
            pick_source(&l, image, None).map(String::as_str),
            Some("localhost/code-pruefer-python:dev")
        );
        // An ID only counts if it is expected.
        let id = format!("sha256:{}", hex('b'));
        let l = loaded(&["evil:latest", &id]);
        assert_eq!(pick_source(&l, image, Some(&v)), Some(&id));
        assert_eq!(pick_source(&l, image, None), None);
        let other = format!("sha256:{}", hex('9'));
        assert_eq!(pick_source(&loaded(&[&other]), image, Some(&v)), None);
        assert_eq!(pick_source(&loaded(&["python:3.12"]), image, None), None);
    }

    /// In-memory store recording the calls. Loaded/pulled images get the
    /// ID `load_id` / `pull_id`.
    #[derive(Default)]
    struct Fake {
        present: RefCell<HashMap<String, ImageIdentity>>,
        load_ok: bool,
        pull_ok: bool,
        load_id: String,
        pull_id: String,
        calls: RefCell<Vec<String>>,
    }

    impl Fake {
        fn with(&self, image: &str, id: &str) {
            self.present.borrow_mut().insert(
                image.into(),
                ImageIdentity {
                    id: id.into(),
                    repo_digests: Vec::new(),
                },
            );
        }
    }

    impl Store for Fake {
        async fn inspect(&self, image: &str) -> Result<Option<ImageIdentity>, RunnerError> {
            self.calls.borrow_mut().push(format!("exists {image}"));
            Ok(self.present.borrow().get(image).cloned())
        }
        async fn load(
            &self,
            _archive: &Path,
            image: &str,
            _verify: Option<&Verify>,
            _on: &mut impl FnMut(Progress),
        ) -> Result<(), RunnerError> {
            self.calls.borrow_mut().push(format!("load {image}"));
            if self.load_ok {
                self.with(image, &self.load_id);
                Ok(())
            } else {
                Err(RunnerError::ImageLoad("a.tar.gz".into(), "kaputt".into()))
            }
        }
        async fn pull(
            &self,
            source: &str,
            image: &str,
            _on: &mut impl FnMut(Progress),
        ) -> Result<(), RunnerError> {
            self.calls.borrow_mut().push(format!("pull {source}"));
            if self.pull_ok {
                self.with(image, &self.pull_id);
                Ok(())
            } else {
                Err(RunnerError::ImagePull(source.into(), "denied".into()))
            }
        }
        async fn remove(&self, image: &str) {
            self.calls.borrow_mut().push(format!("remove {image}"));
            self.present.borrow_mut().remove(image);
        }
    }

    fn run_with(
        store: &Fake,
        archive: bool,
        verify: Option<&Verify>,
    ) -> Result<String, RunnerError> {
        let archive = archive.then(|| PathBuf::from("a.tar.gz"));
        let steps = plan(py(), None, archive, pull_source(py(), verify));
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(execute(store, py(), &steps, verify, &mut |_| {}))
    }

    fn run(store: &Fake, archive: bool) -> Result<String, RunnerError> {
        run_with(store, archive, None)
    }

    fn kinds(store: &Fake) -> Vec<String> {
        store
            .calls
            .borrow()
            .iter()
            .map(|c| c.split(' ').next().unwrap().to_string())
            .collect()
    }

    #[test]
    fn present_image_is_used_without_loading() {
        let store = Fake::default();
        store.with(py().default_image, "x");
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
        store.with("code-pruefer-python:dev", "x");
        assert_eq!(run(&store, true).unwrap(), "code-pruefer-python:dev");
        assert_eq!(kinds(&store), vec!["exists", "load", "pull", "exists"]);
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

    fn release_verify() -> Verify {
        let rd = format!("sha256:{}", hex('f'));
        Digests::parse(&digests_json(Some(&rd)))
            .unwrap()
            .verify_for(py(), Some("amd64"))
    }

    #[test]
    fn unexpected_present_image_is_replaced_from_archive() {
        let v = release_verify();
        let store = Fake {
            load_ok: true,
            load_id: format!("sha256:{}", hex('b')),
            ..Default::default()
        };
        store.with(py().default_image, "sha256:fremd");
        assert_eq!(
            run_with(&store, true, Some(&v)).unwrap(),
            py().default_image
        );
        assert_eq!(kinds(&store), vec!["exists", "load", "exists"]);
    }

    #[test]
    fn loaded_image_with_wrong_id_is_removed() {
        let v = release_verify();
        let store = Fake {
            load_ok: true,
            load_id: format!("sha256:{}", hex('9')),
            pull_ok: true,
            pull_id: format!("sha256:{}", hex('f')),
            ..Default::default()
        };
        // The manipulated archive is rejected and removed; the pull by
        // registry digest provides the image instead.
        assert_eq!(
            run_with(&store, true, Some(&v)).unwrap(),
            py().default_image
        );
        let calls = store.calls.borrow();
        assert_eq!(
            calls
                .iter()
                .map(|c| c.split(' ').next().unwrap())
                .collect::<Vec<_>>(),
            vec!["exists", "load", "exists", "remove", "pull", "exists"]
        );
        assert!(
            calls[4].ends_with(&format!("@sha256:{}", hex('f'))),
            "{calls:?}"
        );
        drop(calls);

        let store = Fake {
            load_ok: true,
            load_id: format!("sha256:{}", hex('9')),
            ..Default::default()
        };
        let msg = run_with(&store, true, Some(&v)).unwrap_err().to_string();
        assert!(msg.contains("nicht die erwartete Image-ID"), "{msg}");
        assert!(!store.present.borrow().contains_key(py().default_image));
    }

    #[test]
    fn dev_image_is_not_checked_against_digests() {
        let v = release_verify();
        let store = Fake::default();
        store.with("code-pruefer-python:dev", "sha256:lokal");
        assert_eq!(
            run_with(&store, false, Some(&v)).unwrap(),
            "code-pruefer-python:dev"
        );
    }
}
