//! Model management: fixed catalog (Qwen2.5-Coder 1.5B / 7B, pinned to a
//! Hugging Face revision with SHA-256 and size), hardware-based suggestion,
//! download with resume and hash check, import of local `.gguf` files,
//! selection (persisted in `ai.json` in the app config directory).
//!
//! The download is the only network access of the AI feature. It accepts
//! nothing but https URLs on huggingface.co and Hugging Face's own CDN hosts
//! (`*.huggingface.co`, `hf.co`, `*.hf.co`), also for redirects.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::{hex, llama, AiError};

pub const CONFIG_FILE: &str = "ai.json";
pub const MODELS_SUBDIR: &str = "models";
/// Below this much RAM the small model is suggested (OS-reported RAM of a
/// "16 GB" machine is ~15.5 GiB, of an "8 GB" one ~7.7 GiB).
pub const SMALL_MODEL_RAM_BYTES: u64 = 12 * GIB;
pub const SMALL_MODEL: &str = "qwen2.5-coder-1.5b";
pub const LARGE_MODEL: &str = "qwen2.5-coder-7b";
/// Upper bound for imported files.
pub const MAX_IMPORT_BYTES: u64 = 32 * GIB;
/// Solutions per task for imported (unknown) models.
const IMPORTED_SOLUTIONS_SMALL: u32 = 5;
const IMPORTED_SOLUTIONS_LARGE: u32 = 3;
const GIB: u64 = 1024 * 1024 * 1024;
const CHUNK: usize = 8 * 1024 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const MAX_REDIRECTS: usize = 10;

static CATALOG_JSON: &str = include_str!("catalog.json");
static CATALOG: OnceLock<Vec<CatalogModel>> = OnceLock::new();
static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
/// Serializes read-modify-write of `ai.json`.
static CONFIG_LOCK: Mutex<()> = Mutex::new(());
static TRANSFER_RUNNING: AtomicBool = AtomicBool::new(false);

/// Sets the app config and data directories (called once at app start).
pub fn init(config_dir: PathBuf, data_dir: PathBuf) {
    let _ = CONFIG_DIR.set(config_dir);
    let _ = DATA_DIR.set(data_dir);
}

// ---------------------------------------------------------------------------
// Catalog

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogFile {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub quant: String,
    pub license: String,
    /// Candidate solutions generated per task.
    pub solutions: u32,
    /// Commit SHA of the Hugging Face repo.
    pub revision: String,
    /// One file, or the parts of a split GGUF (first part is loaded).
    pub files: Vec<CatalogFile>,
}

#[derive(Deserialize)]
struct CatalogRoot {
    models: Vec<CatalogModel>,
}

pub fn parse_catalog(json: &str) -> Result<Vec<CatalogModel>, String> {
    serde_json::from_str::<CatalogRoot>(json)
        .map(|c| c.models)
        .map_err(|e| e.to_string())
}

pub fn catalog() -> &'static [CatalogModel] {
    CATALOG.get_or_init(|| parse_catalog(CATALOG_JSON).expect("catalog.json is valid"))
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl CatalogModel {
    /// Revision, sizes and hashes are filled in (`scripts/pin-models.sh`).
    pub fn is_pinned(&self) -> bool {
        is_hex(&self.revision, 40)
            && !self.files.is_empty()
            && self
                .files
                .iter()
                .all(|f| f.size > 0 && is_hex(&f.sha256, 64) && safe_file_name(&f.name))
    }

    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn url(&self, file: &CatalogFile) -> String {
        download_url(&self.repo, &self.revision, &file.name)
    }
}

pub fn download_url(repo: &str, revision: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/{revision}/{file}")
}

/// A plain file name (no path separators, no dot files), ending in `.gguf`.
pub fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':'])
        && name.to_ascii_lowercase().ends_with(".gguf")
}

/// Hosts the model download may talk to (also after redirects).
pub fn allowed_url(url: &reqwest::Url) -> bool {
    if url.scheme() != "https" || url.port().is_some_and(|p| p != 443) {
        return false;
    }
    match url.host_str() {
        Some(h) => {
            let h = h.to_ascii_lowercase();
            h == "huggingface.co"
                || h.ends_with(".huggingface.co")
                || h == "hf.co"
                || h.ends_with(".hf.co")
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Hardware

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hardware {
    pub total_ram_bytes: u64,
    pub physical_cores: usize,
    pub apple_silicon: bool,
    /// `None` on non-x86 CPUs.
    pub avx2: Option<bool>,
    pub suggested_model: String,
    /// German notes (slow CPU, little RAM).
    pub warnings: Vec<String>,
}

/// Small model below 12 GiB RAM, else the 7B model.
pub fn suggest(total_ram_bytes: u64) -> &'static str {
    if total_ram_bytes < SMALL_MODEL_RAM_BYTES {
        SMALL_MODEL
    } else {
        LARGE_MODEL
    }
}

pub fn gib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / GIB as f64).replace('.', ",")
}

pub fn hardware_warnings(total_ram_bytes: u64, avx2: Option<bool>) -> Vec<String> {
    let mut w = Vec::new();
    if avx2 == Some(false) {
        w.push(
            "Der Prozessor unterstützt kein AVX2 – die KI läuft damit sehr langsam.".to_string(),
        );
    }
    if total_ram_bytes > 0 && total_ram_bytes < 6 * GIB {
        w.push(format!(
            "Wenig Arbeitsspeicher ({} GB) – schon das kleine Modell kann knapp werden; \
             andere Programme vorher schließen.",
            gib(total_ram_bytes)
        ));
    }
    w
}

fn detect_avx2() -> Option<bool> {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        Some(std::arch::is_x86_feature_detected!("avx2"))
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        None
    }
}

pub fn hardware() -> Hardware {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};
    let sys = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
    );
    let ram = sys.total_memory();
    let avx2 = detect_avx2();
    Hardware {
        total_ram_bytes: ram,
        physical_cores: llama::thread_count(),
        apple_silicon: cfg!(all(target_os = "macos", target_arch = "aarch64")),
        avx2,
        suggested_model: suggest(ram).to_string(),
        warnings: hardware_warnings(ram, avx2),
    }
}

// ---------------------------------------------------------------------------
// Config (selection, imported models)

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportedModel {
    /// `import-<first 12 hex of sha256>`.
    pub id: String,
    /// Original file name (display).
    pub name: String,
    /// File name in the models directory.
    pub file_name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    #[serde(default)]
    pub selected: Option<String>,
    #[serde(default)]
    pub imported: Vec<ImportedModel>,
}

pub fn parse_config(json: &str) -> AiConfig {
    serde_json::from_str(json).unwrap_or_default()
}

fn read_config_from(dir: &Path) -> AiConfig {
    std::fs::read_to_string(dir.join(CONFIG_FILE))
        .map(|s| parse_config(&s))
        .unwrap_or_default()
}

fn write_config_to(dir: &Path, c: &AiConfig) -> Result<(), AiError> {
    let json = serde_json::to_string_pretty(c).map_err(|e| AiError::Other(e.to_string()))?;
    std::fs::create_dir_all(dir)
        .and_then(|_| std::fs::write(dir.join(CONFIG_FILE), json))
        .map_err(|e| AiError::Other(format!("Einstellung konnte nicht gespeichert werden: {e}")))
}

fn config_dir() -> Result<&'static Path, AiError> {
    CONFIG_DIR
        .get()
        .map(PathBuf::as_path)
        .ok_or_else(|| AiError::Other("Konfigurationsordner ist nicht bekannt.".into()))
}

fn read_config() -> AiConfig {
    CONFIG_DIR
        .get()
        .map(|d| read_config_from(d))
        .unwrap_or_default()
}

fn update_config(f: impl FnOnce(&mut AiConfig)) -> Result<AiConfig, AiError> {
    let dir = config_dir()?;
    let _lock = CONFIG_LOCK.lock();
    let mut c = read_config_from(dir);
    f(&mut c);
    write_config_to(dir, &c)?;
    Ok(c)
}

pub fn models_dir() -> Result<PathBuf, AiError> {
    DATA_DIR
        .get()
        .map(|d| d.join(MODELS_SUBDIR))
        .ok_or_else(|| AiError::Other("Datenordner ist nicht bekannt.".into()))
}

// ---------------------------------------------------------------------------
// Status

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub size_bytes: u64,
    pub license: Option<String>,
    /// From the catalog (hash-verified); `false` = imported unknown file.
    pub known: bool,
    /// Catalog entry has revision and hashes (download possible).
    pub pinned: bool,
    pub installed: bool,
    /// Bytes of an interrupted download (resumable).
    pub partial_bytes: u64,
    pub suggested: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    pub hardware: Hardware,
    pub models: Vec<ModelInfo>,
    pub selected: Option<String>,
    /// A model is selected and installed.
    pub model_ready: bool,
    pub runtime_available: bool,
    pub runtime_error: Option<String>,
    pub llama_version: String,
    pub models_dir: Option<String>,
}

fn file_len(p: &Path) -> Option<u64> {
    std::fs::metadata(p)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// All files present with the catalog size (files only get there by an
/// atomic rename after the hash check).
pub fn catalog_installed(dir: &Path, m: &CatalogModel) -> bool {
    !m.files.is_empty()
        && m.files
            .iter()
            .all(|f| file_len(&dir.join(&f.name)) == Some(f.size))
}

pub fn status() -> AiStatus {
    let hw = hardware();
    let config = read_config();
    let dir = models_dir().ok();
    let selected = config.selected.clone();
    let mut models: Vec<ModelInfo> = catalog()
        .iter()
        .map(|m| {
            let (installed, partial) = match &dir {
                Some(d) => (
                    catalog_installed(d, m),
                    m.files
                        .iter()
                        .filter_map(|f| file_len(&part_path(&d.join(&f.name))))
                        .sum(),
                ),
                None => (false, 0),
            };
            ModelInfo {
                id: m.id.clone(),
                name: m.name.clone(),
                size_bytes: m.size(),
                license: Some(m.license.clone()),
                known: true,
                pinned: m.is_pinned(),
                installed,
                partial_bytes: partial,
                suggested: m.id == hw.suggested_model,
                selected: selected.as_deref() == Some(m.id.as_str()),
            }
        })
        .collect();
    for imp in &config.imported {
        let installed = dir
            .as_ref()
            .is_some_and(|d| file_len(&d.join(&imp.file_name)) == Some(imp.size));
        models.push(ModelInfo {
            id: imp.id.clone(),
            name: imp.name.clone(),
            size_bytes: imp.size,
            license: None,
            known: false,
            pinned: false,
            installed,
            partial_bytes: 0,
            suggested: false,
            selected: selected.as_deref() == Some(imp.id.as_str()),
        });
    }
    let model_ready = models.iter().any(|m| m.selected && m.installed);
    let (runtime_available, runtime_error) = match llama::server_path() {
        Ok(_) => (true, None),
        Err(e) => (false, Some(e.to_string())),
    };
    AiStatus {
        hardware: hw,
        models,
        selected,
        model_ready,
        runtime_available,
        runtime_error,
        llama_version: llama::LLAMA_CPP_VERSION.to_string(),
        models_dir: dir.map(|d| d.display().to_string()),
    }
}

/// The selected, installed model: path of the (first) GGUF file and the
/// number of solutions to generate.
pub struct SelectedModel {
    pub id: String,
    pub path: PathBuf,
    pub solutions: u32,
}

pub fn selected_model() -> Result<SelectedModel, AiError> {
    let not_ready = || {
        AiError::Other(
            "Kein KI-Modell eingerichtet. Bitte unter „KI-Modell“ ein Modell herunterladen \
             oder importieren und auswählen."
                .into(),
        )
    };
    let config = read_config();
    let id = config.selected.clone().ok_or_else(not_ready)?;
    let dir = models_dir()?;
    if let Some(m) = catalog().iter().find(|m| m.id == id) {
        if !catalog_installed(&dir, m) {
            return Err(not_ready());
        }
        return Ok(SelectedModel {
            id,
            path: dir.join(&m.files[0].name),
            solutions: m.solutions,
        });
    }
    let imp = config
        .imported
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(not_ready)?;
    let path = dir.join(&imp.file_name);
    if file_len(&path) != Some(imp.size) {
        return Err(not_ready());
    }
    Ok(SelectedModel {
        id,
        path,
        solutions: if imp.size > 3 * GIB {
            IMPORTED_SOLUTIONS_LARGE
        } else {
            IMPORTED_SOLUTIONS_SMALL
        },
    })
}

pub fn select(id: &str) -> Result<AiStatus, AiError> {
    let st = status();
    match st.models.iter().find(|m| m.id == id) {
        Some(m) if m.installed => {}
        Some(_) => {
            return Err(AiError::Other(
                "Das Modell ist noch nicht vollständig vorhanden.".into(),
            ))
        }
        None => return Err(AiError::Other(format!("Unbekanntes Modell: {id}"))),
    }
    update_config(|c| c.selected = Some(id.to_string()))?;
    Ok(status())
}

/// Selects `id` if no model is selected yet.
fn select_if_none(id: &str) {
    let _ = update_config(|c| {
        if c.selected.is_none() {
            c.selected = Some(id.to_string());
        }
    });
}

pub async fn delete(id: &str) -> Result<AiStatus, AiError> {
    let dir = models_dir()?;
    let config = read_config();
    let files: Vec<PathBuf> = if let Some(m) = catalog().iter().find(|m| m.id == id) {
        m.files.iter().map(|f| dir.join(&f.name)).collect()
    } else if let Some(imp) = config.imported.iter().find(|m| m.id == id) {
        vec![dir.join(&imp.file_name)]
    } else {
        return Err(AiError::Other(format!("Unbekanntes Modell: {id}")));
    };
    if TRANSFER_RUNNING.load(Ordering::SeqCst) {
        return Err(AiError::Other(
            "Bitte warten, bis Download oder Import beendet ist.".into(),
        ));
    }
    // The server keeps the file open (Windows cannot delete it then).
    llama::shutdown().await;
    for f in &files {
        for p in [f.clone(), part_path(f)] {
            match std::fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(AiError::Other(format!(
                        "{} konnte nicht gelöscht werden: {e}",
                        p.display()
                    )))
                }
            }
        }
    }
    update_config(|c| {
        c.imported.retain(|m| m.id != id);
        if c.selected.as_deref() == Some(id) {
            c.selected = None;
        }
    })?;
    Ok(status())
}

// ---------------------------------------------------------------------------
// Download and import

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsProgress {
    pub model_id: String,
    /// German status line.
    pub message: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub percent: Option<u8>,
}

pub fn percent(done: u64, total: u64) -> Option<u8> {
    (total > 0).then(|| ((done.min(total) as u128 * 100) / total as u128) as u8)
}

/// Only one download/import at a time.
struct TransferGuard;

impl TransferGuard {
    fn acquire() -> Result<Self, AiError> {
        if TRANSFER_RUNNING.swap(true, Ordering::SeqCst) {
            return Err(AiError::Other(
                "Es läuft bereits ein Download oder Import.".into(),
            ));
        }
        Ok(TransferGuard)
    }
}

impl Drop for TransferGuard {
    fn drop(&mut self) {
        TRANSFER_RUNNING.store(false, Ordering::SeqCst);
    }
}

fn io_err(what: &str, e: std::io::Error) -> AiError {
    AiError::Other(format!("{what}: {e} (Speicherplatz voll?)"))
}

fn http_client() -> Result<reqwest::Client, AiError> {
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("zu viele Weiterleitungen")
        } else if allowed_url(attempt.url()) {
            attempt.follow()
        } else {
            let host = attempt.url().host_str().unwrap_or("?").to_string();
            attempt.error(format!("Weiterleitung zu nicht erlaubtem Server {host}"))
        }
    });
    reqwest::Client::builder()
        .redirect(policy)
        .https_only(true)
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("Code-Pruefer/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AiError::Http(e.to_string()))
}

/// Hashes an existing file (resume) in chunks.
async fn hash_file(
    path: &Path,
    hasher: &mut Sha256,
    cancel: &CancellationToken,
) -> Result<u64, AiError> {
    let mut f = tokio::fs::File::open(path)
        .await
        .map_err(|e| io_err("Datei konnte nicht gelesen werden", e))?;
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        let n = f
            .read(&mut buf)
            .await
            .map_err(|e| io_err("Datei konnte nicht gelesen werden", e))?;
        if n == 0 {
            return Ok(total);
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
}

/// Parses the start of `Content-Range: bytes <start>-<end>/<total>`.
pub fn content_range_start(header: &str) -> Option<u64> {
    header
        .trim()
        .strip_prefix("bytes ")?
        .split('-')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Downloads one file to `dest` (via `dest.part`, resumable), verifying
/// size and SHA-256 before the atomic rename.
#[allow(clippy::too_many_arguments)]
async fn download_file(
    client: &reqwest::Client,
    url: reqwest::Url,
    dest: &Path,
    expected_size: u64,
    expected_sha256: &str,
    done_before: u64,
    total_all: u64,
    cancel: &CancellationToken,
    on_progress: &mut impl FnMut(u64, u64),
) -> Result<(), AiError> {
    let part = part_path(dest);
    let mut hasher = Sha256::new();
    let mut have = match file_len(&part) {
        Some(n) if n > 0 && n <= expected_size => hash_file(&part, &mut hasher, cancel).await?,
        Some(_) => {
            let _ = tokio::fs::remove_file(&part).await;
            0
        }
        None => 0,
    };

    if have < expected_size {
        let mut req = client.get(url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AiError::Cancelled),
            r = req.send() => r.map_err(|e| AiError::Http(format!("Download fehlgeschlagen: {e}")))?,
        };
        let status = resp.status();
        let append = if status == reqwest::StatusCode::PARTIAL_CONTENT {
            let start = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(content_range_start);
            start == Some(have)
        } else if status.is_success() {
            false
        } else {
            return Err(AiError::Http(format!(
                "Download fehlgeschlagen: HTTP {status}"
            )));
        };
        if !append {
            // Server ignored the range: start over.
            have = 0;
            hasher = Sha256::new();
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(&part)
            .await
            .map_err(|e| io_err("Datei konnte nicht angelegt werden", e))?;
        let mut stream = resp.bytes_stream();
        let mut last = Instant::now();
        loop {
            let chunk = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    let _ = file.flush().await;
                    return Err(AiError::Cancelled);
                }
                c = stream.next() => c,
            };
            let Some(chunk) = chunk else { break };
            let chunk = chunk.map_err(|e| AiError::Http(format!("Download unterbrochen: {e}")))?;
            have += chunk.len() as u64;
            if have > expected_size {
                drop(file);
                let _ = tokio::fs::remove_file(&part).await;
                return Err(AiError::Other(
                    "Die Datei ist größer als erwartet und wurde verworfen.".into(),
                ));
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|e| io_err("Schreiben fehlgeschlagen", e))?;
            if last.elapsed() >= PROGRESS_INTERVAL {
                last = Instant::now();
                on_progress(done_before + have, total_all);
            }
        }
        file.flush()
            .await
            .map_err(|e| io_err("Schreiben fehlgeschlagen", e))?;
    }

    let actual = hex(&hasher.finalize());
    if have != expected_size || actual != expected_sha256 {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(AiError::Other(format!(
            "Die heruntergeladene Datei ist unvollständig oder beschädigt (Prüfsumme stimmt \
             nicht) und wurde verworfen. Bitte erneut herunterladen. ({} von {expected_size} Bytes)",
            have
        )));
    }
    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| io_err("Datei konnte nicht umbenannt werden", e))?;
    on_progress(done_before + have, total_all);
    Ok(())
}

/// Downloads a catalog model from huggingface.co (resumable).
pub async fn download(
    id: &str,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(ModelsProgress),
) -> Result<AiStatus, AiError> {
    let m = catalog()
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| AiError::Other(format!("Unbekanntes Modell: {id}")))?;
    if !m.is_pinned() {
        return Err(AiError::Other(format!(
            "Für „{}“ sind im Modellkatalog dieser Version noch keine Prüfsummen eingetragen; \
             ein Download ist deshalb nicht möglich. Bitte die .gguf-Datei über „Datei \
             importieren …“ einlesen.",
            m.name
        )));
    }
    let _guard = TransferGuard::acquire()?;
    let dir = models_dir()?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| io_err("Modellordner konnte nicht angelegt werden", e))?;
    let client = http_client()?;
    let total = m.size();
    let mut done = 0u64;
    let count = m.files.len();
    for (i, f) in m.files.iter().enumerate() {
        let dest = dir.join(&f.name);
        if file_len(&dest) == Some(f.size) {
            done += f.size;
            continue;
        }
        let part_note = if count > 1 {
            format!(" (Teil {} von {count})", i + 1)
        } else {
            String::new()
        };
        let mut report = |d: u64, t: u64| {
            on_progress(ModelsProgress {
                model_id: m.id.clone(),
                message: format!("Lade {}{part_note}: {} von {} GB", m.name, gib(d), gib(t)),
                done_bytes: d,
                total_bytes: t,
                percent: percent(d, t),
            })
        };
        report(done, total);
        let url = reqwest::Url::parse(&m.url(f))
            .ok()
            .filter(allowed_url)
            .ok_or_else(|| {
                AiError::Other(format!("Nicht erlaubte Download-Adresse: {}", m.url(f)))
            })?;
        download_file(
            &client,
            url,
            &dest,
            f.size,
            &f.sha256,
            done,
            total,
            cancel,
            &mut report,
        )
        .await?;
        done += f.size;
    }
    select_if_none(&m.id);
    Ok(status())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    /// Id of the imported model (`None` while confirmation is pending).
    pub model_id: Option<String>,
    /// The file matches a catalog file (SHA-256).
    pub known: bool,
    /// Unknown file: the user must confirm (`confirm_unknown`).
    pub needs_confirmation: bool,
    /// For split catalog models: other parts are still missing.
    pub incomplete: bool,
    pub sha256: String,
    pub status: AiStatus,
}

/// What an imported file is, by its SHA-256.
#[derive(Debug, PartialEq)]
pub enum Match<'a> {
    Catalog(&'a CatalogModel, &'a CatalogFile),
    Unknown,
}

pub fn match_hash<'a>(models: &'a [CatalogModel], sha256: &str) -> Match<'a> {
    for m in models.iter().filter(|m| m.is_pinned()) {
        if let Some(f) = m.files.iter().find(|f| f.sha256 == sha256) {
            return Match::Catalog(m, f);
        }
    }
    Match::Unknown
}

pub fn imported_id(sha256: &str) -> String {
    format!("import-{}", &sha256[..12.min(sha256.len())])
}

/// Imports a local `.gguf` file: hashes it, copies it into the models
/// directory and marks it as catalog model (hash match) or, only with
/// `confirm_unknown`, as unknown model.
pub async fn import(
    path: &Path,
    confirm_unknown: bool,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(ModelsProgress),
) -> Result<ImportOutcome, AiError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !name.to_ascii_lowercase().ends_with(".gguf") {
        return Err(AiError::Other("Bitte eine .gguf-Datei wählen.".into()));
    }
    let size =
        file_len(path).ok_or_else(|| AiError::Other(format!("„{name}“ wurde nicht gefunden.")))?;
    if size > MAX_IMPORT_BYTES {
        return Err(AiError::Other(format!(
            "„{name}“ ist zu groß ({} GB, höchstens {} GB).",
            gib(size),
            gib(MAX_IMPORT_BYTES)
        )));
    }
    let mut magic = [0u8; 4];
    tokio::fs::File::open(path)
        .await
        .map_err(|e| io_err("Datei konnte nicht gelesen werden", e))?
        .read_exact(&mut magic)
        .await
        .map_err(|_| AiError::Other(format!("„{name}“ ist keine GGUF-Datei.")))?;
    if &magic != b"GGUF" {
        return Err(AiError::Other(format!("„{name}“ ist keine GGUF-Datei.")));
    }

    let _guard = TransferGuard::acquire()?;
    let progress = |phase: &str, done: u64| ModelsProgress {
        model_id: String::new(),
        message: format!("{phase} „{name}“: {} von {} GB", gib(done), gib(size)),
        done_bytes: done,
        total_bytes: size,
        percent: percent(done, size),
    };

    // Pass 1: hash.
    let mut hasher = Sha256::new();
    {
        let mut f = tokio::fs::File::open(path)
            .await
            .map_err(|e| io_err("Datei konnte nicht gelesen werden", e))?;
        let mut buf = vec![0u8; CHUNK];
        let mut done = 0u64;
        let mut last = Instant::now();
        loop {
            if cancel.is_cancelled() {
                return Err(AiError::Cancelled);
            }
            let n = f
                .read(&mut buf)
                .await
                .map_err(|e| io_err("Datei konnte nicht gelesen werden", e))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            done += n as u64;
            if last.elapsed() >= PROGRESS_INTERVAL {
                last = Instant::now();
                on_progress(progress("Prüfe", done));
            }
        }
    }
    let sha256 = hex(&hasher.finalize());

    let (target_name, model_id, known) = match match_hash(catalog(), &sha256) {
        Match::Catalog(m, f) => (f.name.clone(), m.id.clone(), true),
        Match::Unknown => {
            if !confirm_unknown {
                return Ok(ImportOutcome {
                    model_id: None,
                    known: false,
                    needs_confirmation: true,
                    incomplete: false,
                    sha256,
                    status: status(),
                });
            }
            let id = imported_id(&sha256);
            (format!("{id}.gguf"), id, false)
        }
    };

    // Pass 2: copy (via .part, then atomic rename), hashing again so the
    // copy is verified as well.
    let dir = models_dir()?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| io_err("Modellordner konnte nicht angelegt werden", e))?;
    let dest = dir.join(&target_name);
    let part = part_path(&dest);
    let copy = async {
        let mut src = tokio::fs::File::open(path)
            .await
            .map_err(|e| io_err("Datei konnte nicht gelesen werden", e))?;
        let mut out = tokio::fs::File::create(&part)
            .await
            .map_err(|e| io_err("Datei konnte nicht angelegt werden", e))?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; CHUNK];
        let mut done = 0u64;
        let mut last = Instant::now();
        loop {
            if cancel.is_cancelled() {
                return Err(AiError::Cancelled);
            }
            let n = src
                .read(&mut buf)
                .await
                .map_err(|e| io_err("Datei konnte nicht gelesen werden", e))?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            out.write_all(&buf[..n])
                .await
                .map_err(|e| io_err("Schreiben fehlgeschlagen", e))?;
            done += n as u64;
            if last.elapsed() >= PROGRESS_INTERVAL {
                last = Instant::now();
                on_progress(progress("Kopiere", done));
            }
        }
        out.flush()
            .await
            .map_err(|e| io_err("Schreiben fehlgeschlagen", e))?;
        if hex(&h.finalize()) != sha256 {
            return Err(AiError::Other(
                "Die Datei hat sich beim Kopieren verändert – bitte erneut importieren.".into(),
            ));
        }
        Ok(())
    };
    if let Err(e) = copy.await {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    // A running server may hold the old file open.
    llama::shutdown().await;
    tokio::fs::rename(&part, &dest)
        .await
        .map_err(|e| io_err("Datei konnte nicht umbenannt werden", e))?;

    if !known {
        update_config(|c| {
            c.imported.retain(|m| m.id != model_id);
            c.imported.push(ImportedModel {
                id: model_id.clone(),
                name,
                file_name: target_name.clone(),
                size,
                sha256: sha256.clone(),
            });
        })?;
    }
    let incomplete = catalog()
        .iter()
        .find(|m| m.id == model_id)
        .is_some_and(|m| !catalog_installed(&dir, m));
    if !incomplete {
        select_if_none(&model_id);
    }
    Ok(ImportOutcome {
        model_id: Some(model_id),
        known,
        needs_confirmation: false,
        incomplete,
        sha256,
        status: status(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned(id: &str, files: &[(&str, u64, char)]) -> CatalogModel {
        CatalogModel {
            id: id.into(),
            name: id.into(),
            repo: "Qwen/X-GGUF".into(),
            quant: "q4_k_m".into(),
            license: "Apache-2.0".into(),
            solutions: 3,
            revision: "a".repeat(40),
            files: files
                .iter()
                .map(|(n, s, c)| CatalogFile {
                    name: n.to_string(),
                    size: *s,
                    sha256: c.to_string().repeat(64),
                })
                .collect(),
        }
    }

    #[test]
    fn bundled_catalog_is_valid() {
        let models = parse_catalog(CATALOG_JSON).unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec![SMALL_MODEL, LARGE_MODEL]);
        for m in &models {
            assert!(m.repo.starts_with("Qwen/Qwen2.5-Coder-"), "{}", m.repo);
            assert!(m.repo.ends_with("-Instruct-GGUF"), "{}", m.repo);
            assert_eq!(m.license, "Apache-2.0");
            // 3B is excluded (non-commercial license).
            assert!(!m.repo.contains("-3B-"), "{}", m.repo);
            // Pinned entries must be complete; unpinned ones stay empty.
            if !m.revision.is_empty() || !m.files.is_empty() {
                assert!(m.is_pinned(), "{} ist nur teilweise gepinnt", m.id);
                for f in &m.files {
                    assert!(f.name.contains(&m.quant), "{}", f.name);
                    assert!(allowed_url(&reqwest::Url::parse(&m.url(f)).unwrap()));
                }
            }
        }
        assert_eq!(models[0].solutions, 5);
        assert_eq!(models[1].solutions, 3);
    }

    #[test]
    fn pinning_requires_revision_and_hashes() {
        let m = pinned("m", &[("m-q4_k_m.gguf", 10, 'b')]);
        assert!(m.is_pinned());
        assert_eq!(
            m.url(&m.files[0]),
            format!(
                "https://huggingface.co/Qwen/X-GGUF/resolve/{}/m-q4_k_m.gguf",
                "a".repeat(40)
            )
        );
        let mut bad = m.clone();
        bad.revision = "main".into();
        assert!(!bad.is_pinned());
        let mut bad = m.clone();
        bad.files[0].sha256 = "B".repeat(64);
        assert!(!bad.is_pinned());
        let mut bad = m.clone();
        bad.files[0].size = 0;
        assert!(!bad.is_pinned());
        let mut bad = m.clone();
        bad.files[0].name = "../x.gguf".into();
        assert!(!bad.is_pinned());
        let mut bad = m;
        bad.files.clear();
        assert!(!bad.is_pinned());
    }

    #[test]
    fn only_hugging_face_hosts_are_allowed() {
        let ok = |u: &str| allowed_url(&reqwest::Url::parse(u).unwrap());
        assert!(ok("https://huggingface.co/Qwen/x/resolve/abc/f.gguf"));
        assert!(ok("https://cdn-lfs.huggingface.co/repos/x"));
        assert!(ok("https://cdn-lfs-us-1.hf.co/repos/x"));
        assert!(ok("https://cas-bridge.xethub.hf.co/xet-bridge-us/x"));
        assert!(ok("https://hf.co/x"));
        assert!(!ok("http://huggingface.co/x"));
        assert!(!ok("https://huggingface.co:8443/x"));
        assert!(!ok("https://evilhuggingface.co/x"));
        assert!(!ok("https://huggingface.co.evil.com/x"));
        assert!(!ok("https://hf.co.evil.com/x"));
        assert!(!ok("https://example.com/huggingface.co"));
        assert!(!ok("https://127.0.0.1/x"));
    }

    #[test]
    fn file_names_are_plain() {
        assert!(safe_file_name(
            "qwen2.5-coder-7b-instruct-q4_k_m-00001-of-00002.gguf"
        ));
        assert!(!safe_file_name("../a.gguf"));
        assert!(!safe_file_name("a\\b.gguf"));
        assert!(!safe_file_name("C:a.gguf"));
        assert!(!safe_file_name(".gguf"));
        assert!(!safe_file_name("a.bin"));
    }

    #[test]
    fn suggestion_by_ram() {
        assert_eq!(suggest(8 * GIB), SMALL_MODEL);
        assert_eq!(suggest(12 * GIB - 1), SMALL_MODEL);
        assert_eq!(suggest(12 * GIB), LARGE_MODEL);
        assert_eq!(suggest(32 * GIB), LARGE_MODEL);
        // "16 GB" as reported by the OS.
        assert_eq!(suggest(15_800_000_000), LARGE_MODEL);
    }

    #[test]
    fn hardware_warnings_are_german() {
        assert!(hardware_warnings(16 * GIB, Some(true)).is_empty());
        assert!(hardware_warnings(16 * GIB, None).is_empty());
        let w = hardware_warnings(4 * GIB, Some(false));
        assert_eq!(w.len(), 2);
        assert!(w[0].contains("AVX2"));
        assert!(w[1].contains("4,0 GB"), "{}", w[1]);
    }

    #[test]
    fn detected_hardware_is_plausible() {
        let h = hardware();
        assert!(h.total_ram_bytes > 0);
        assert!(h.physical_cores >= 1);
        assert!(h.suggested_model == SMALL_MODEL || h.suggested_model == LARGE_MODEL);
    }

    #[test]
    fn hash_matching_uses_pinned_catalog_only() {
        let models = vec![
            pinned("a", &[("a-q4_k_m.gguf", 1, 'a')]),
            pinned(
                "b",
                &[
                    ("b-q4_k_m-00001-of-00002.gguf", 1, 'b'),
                    ("b-q4_k_m-00002-of-00002.gguf", 1, 'c'),
                ],
            ),
        ];
        match match_hash(&models, &"c".repeat(64)) {
            Match::Catalog(m, f) => {
                assert_eq!(m.id, "b");
                assert!(f.name.contains("00002"));
            }
            Match::Unknown => panic!("expected match"),
        }
        assert_eq!(match_hash(&models, &"d".repeat(64)), Match::Unknown);
        let mut unpinned = models.clone();
        unpinned[0].revision.clear();
        assert_eq!(match_hash(&unpinned, &"a".repeat(64)), Match::Unknown);
        assert_eq!(imported_id(&"e".repeat(64)), "import-eeeeeeeeeeee");
    }

    #[test]
    fn installed_needs_all_parts_with_size() {
        let dir = std::env::temp_dir().join(format!("cp-models-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = pinned(
            "b",
            &[("p1-q4_k_m.gguf", 3, 'b'), ("p2-q4_k_m.gguf", 2, 'c')],
        );
        assert!(!catalog_installed(&dir, &m));
        std::fs::write(dir.join("p1-q4_k_m.gguf"), b"abc").unwrap();
        assert!(!catalog_installed(&dir, &m));
        std::fs::write(dir.join("p2-q4_k_m.gguf"), b"abc").unwrap();
        assert!(!catalog_installed(&dir, &m), "wrong size");
        std::fs::write(dir.join("p2-q4_k_m.gguf"), b"ab").unwrap();
        assert!(catalog_installed(&dir, &m));
        assert_eq!(part_path(&dir.join("x.gguf")), dir.join("x.gguf.part"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn config_roundtrip_and_tolerance() {
        let dir = std::env::temp_dir().join(format!("cp-aicfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_config_from(&dir), AiConfig::default());
        let c = AiConfig {
            selected: Some("import-abc".into()),
            imported: vec![ImportedModel {
                id: "import-abc".into(),
                name: "x.gguf".into(),
                file_name: "import-abc.gguf".into(),
                size: 5,
                sha256: "f".repeat(64),
            }],
        };
        write_config_to(&dir, &c).unwrap();
        assert_eq!(read_config_from(&dir), c);
        let raw = std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap();
        assert!(raw.contains("\"fileName\""), "{raw}");
        assert_eq!(parse_config("kaputt"), AiConfig::default());
        assert_eq!(parse_config("{}"), AiConfig::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn content_range_and_percent() {
        assert_eq!(content_range_start("bytes 100-199/200"), Some(100));
        assert_eq!(content_range_start("bytes */200"), None);
        assert_eq!(content_range_start("items 1-2/3"), None);
        assert_eq!(percent(50, 200), Some(25));
        assert_eq!(percent(300, 200), Some(100));
        assert_eq!(percent(1, 0), None);
        assert_eq!(gib(GIB + GIB / 2), "1,5");
    }

    #[test]
    fn unpinned_download_is_refused_before_network() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let token = CancellationToken::new();
        for m in catalog().iter().filter(|m| !m.is_pinned()) {
            let e = rt
                .block_on(download(&m.id, &token, |_| {}))
                .unwrap_err()
                .to_string();
            assert!(e.contains("keine Prüfsummen"), "{e}");
        }
        let e = rt
            .block_on(download("gibt-es-nicht", &token, |_| {}))
            .unwrap_err()
            .to_string();
        assert!(e.contains("Unbekanntes Modell"), "{e}");
    }

    #[test]
    fn import_rejects_non_gguf_files() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let dir = std::env::temp_dir().join(format!("cp-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let token = CancellationToken::new();
        let txt = dir.join("a.txt");
        std::fs::write(&txt, b"GGUF").unwrap();
        let e = rt
            .block_on(import(&txt, false, &token, |_| {}))
            .unwrap_err();
        assert!(e.to_string().contains(".gguf"), "{e}");
        let fake = dir.join("fake.gguf");
        std::fs::write(&fake, b"PK\x03\x04 kein gguf").unwrap();
        let e = rt
            .block_on(import(&fake, true, &token, |_| {}))
            .unwrap_err();
        assert!(e.to_string().contains("keine GGUF-Datei"), "{e}");
        let e = rt
            .block_on(import(&dir.join("fehlt.gguf"), true, &token, |_| {}))
            .unwrap_err();
        assert!(e.to_string().contains("nicht gefunden"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Minimal HTTP server for `download_file` (plain http on 127.0.0.1;
    /// the https/host checks are tested in `only_hugging_face_hosts_are_allowed`).
    async fn serve(body: Vec<u8>, honor_range: bool) -> reqwest::Url {
        use tokio::io::AsyncBufReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let body = body.clone();
                tokio::spawn(async move {
                    let (r, mut w) = sock.into_split();
                    let mut lines = tokio::io::BufReader::new(r).lines();
                    let mut start = 0usize;
                    while let Ok(Some(l)) = lines.next_line().await {
                        if l.is_empty() {
                            break;
                        }
                        if let Some(v) = l.to_ascii_lowercase().strip_prefix("range: bytes=") {
                            start = v.trim_end_matches('-').parse().unwrap();
                        }
                    }
                    let head = if honor_range && start > 0 {
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                            body.len() - start, start, body.len() - 1, body.len()
                        )
                    } else {
                        start = 0;
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                    };
                    let _ = w.write_all(head.as_bytes()).await;
                    let _ = w.write_all(&body[start..]).await;
                    let _ = w.shutdown().await;
                });
            }
        });
        reqwest::Url::parse(&format!("http://{addr}/model.gguf")).unwrap()
    }

    #[test]
    fn download_resumes_verifies_and_renames() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let dir = std::env::temp_dir().join(format!("cp-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let sha = hex(&Sha256::digest(&body));
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let token = CancellationToken::new();
        rt.block_on(async {
            for honor_range in [true, false] {
                let url = serve(body.clone(), honor_range).await;
                let dest = dir.join(format!("m-{honor_range}.gguf"));
                // Interrupted download: the first 1000 bytes are already there.
                std::fs::write(part_path(&dest), &body[..1000]).unwrap();
                let mut last = 0;
                download_file(
                    &client,
                    url.clone(),
                    &dest,
                    body.len() as u64,
                    &sha,
                    0,
                    body.len() as u64,
                    &token,
                    &mut |d, _| last = d,
                )
                .await
                .unwrap();
                assert_eq!(std::fs::read(&dest).unwrap(), body);
                assert!(!part_path(&dest).exists());
                assert_eq!(last, body.len() as u64);

                // Wrong hash: the file is discarded.
                let bad = dir.join(format!("bad-{honor_range}.gguf"));
                let e = download_file(
                    &client,
                    url,
                    &bad,
                    body.len() as u64,
                    &"0".repeat(64),
                    0,
                    1,
                    &token,
                    &mut |_, _| {},
                )
                .await
                .unwrap_err();
                assert!(e.to_string().contains("Prüfsumme"), "{e}");
                assert!(!bad.exists() && !part_path(&bad).exists());
            }
            // Cancelled before the request.
            let url = serve(body.clone(), true).await;
            let cancelled = CancellationToken::new();
            cancelled.cancel();
            let e = download_file(
                &client,
                url,
                &dir.join("c.gguf"),
                body.len() as u64,
                &sha,
                0,
                1,
                &cancelled,
                &mut |_, _| {},
            )
            .await
            .unwrap_err();
            assert!(matches!(e, AiError::Cancelled));
        });
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
