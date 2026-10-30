//! Self-update from the published GitHub releases (docs/UPDATER.md).
//!
//! Uses the official `tauri-plugin-updater`, but only from Rust: the webview
//! gets no updater permission, it calls the commands below. The endpoint is
//! fixed ([`ENDPOINT`], HTTPS); every download is checked against the public
//! key compiled into the app (minisign signature, mandatory) and the
//! signature must name the announced version (`requireSignedVersion`).
//!
//! The public key is not in `tauri.conf.json`: release builds add it through
//! the overlay `src-tauri/tauri.updater.conf.json` (written by
//! `node scripts/updater.mjs config` from the GitHub variable
//! `TAURI_UPDATER_PUBKEY`). Without it the plugin is not registered at all
//! (its configuration requires a key, a missing one would abort the start)
//! and the settings show „Updates in dieser Version nicht verfügbar“.
//!
//! Schedule: automatic checks at most once per interval (daily by default,
//! weekly, or never); the time of the last successful check and the interval
//! are stored in `updater.json` in the app config directory.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::{Error as UpdaterError, Update, UpdaterExt};

/// `latest` = the newest published release; GitHub never resolves it to a
/// draft or pre-release (QAS), see docs/UPDATER.md.
pub const ENDPOINT: &str =
    "https://github.com/bernhardgoetz/entwicklung-test/releases/latest/download/latest.json";
/// Shown to the portable version (no self-update). Synced with the opener
/// capability by a test in lib.rs.
pub const RELEASES_URL: &str = "https://github.com/bernhardgoetz/entwicklung-test/releases";
/// Debug builds only: alternative endpoint for local tests (http allowed).
pub const ENDPOINT_ENV: &str = "CODE_PRUEFER_UPDATE_ENDPOINT";
pub const CONFIG_FILE: &str = "updater.json";

const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DAY_SECS: u64 = 24 * 60 * 60;
/// Release notes are shown as plain text; longer notes are cut.
pub const MAX_NOTES_CHARS: usize = 4000;

static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();
static CONFIGURED: AtomicBool = AtomicBool::new(false);
/// The update found by the last check, installed by [`download_and_install`].
static PENDING: Mutex<Option<Update>> = Mutex::new(None);
/// One check at a time (automatic timer and the button).
static CHECK_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static INSTALLING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Interval {
    #[default]
    Daily,
    Weekly,
    Never,
}

impl Interval {
    fn period_secs(self) -> Option<u64> {
        match self {
            Self::Daily => Some(DAY_SECS),
            Self::Weekly => Some(7 * DAY_SECS),
            Self::Never => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Availability {
    Available,
    /// Built without updater public key (local/CI/dev builds).
    NotConfigured,
    /// Windows portable zip: there is no installer to run.
    Portable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    #[serde(default)]
    interval: Interval,
    /// Unix seconds of the last successful check.
    #[serde(default)]
    last_check: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_version: String,
    pub availability: Availability,
    pub interval: Interval,
    /// Milliseconds since the epoch (for `new Date()`), or null.
    pub last_check: Option<u64>,
    pub releases_url: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOffer {
    pub version: String,
    pub current_version: String,
    /// Plain text (control characters removed, shortened).
    pub notes: Option<String>,
    /// RFC 3339.
    pub date: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum InstallProgress {
    Download {
        downloaded: u64,
        total: Option<u64>,
    },
    /// Downloaded and signature verified; the installer starts.
    Installing,
}

/// Called once at start with the app config directory and whether the
/// updater plugin was registered (see [`configured`]).
pub fn init(config_dir: PathBuf, configured: bool) {
    let _ = CONFIG_DIR.set(config_dir);
    CONFIGURED.store(configured, Ordering::SeqCst);
}

/// True if the app was built with an updater public key
/// (`plugins.updater.pubkey`); only then may the plugin be registered.
pub fn configured(config: &tauri::Config) -> bool {
    config
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|k| k.as_str())
        .is_some_and(|k| !k.trim().is_empty())
}

pub fn availability() -> Availability {
    if !CONFIGURED.load(Ordering::SeqCst) {
        Availability::NotConfigured
    } else if is_portable() {
        Availability::Portable
    } else {
        Availability::Available
    }
}

/// The NSIS installer puts `uninstall.exe` next to `code-pruefer.exe`; the
/// portable zip has none.
fn is_portable() -> bool {
    if !cfg!(windows) {
        return false;
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| !d.join("uninstall.exe").is_file()))
        .unwrap_or(false)
}

/// Is an automatic check due? A last check in the future (clock set back)
/// counts as due.
pub fn is_due(interval: Interval, last_check: Option<u64>, now: u64) -> bool {
    let Some(period) = interval.period_secs() else {
        return false;
    };
    match last_check {
        None => true,
        Some(last) if last > now => true,
        Some(last) => now - last >= period,
    }
}

/// Plain text for the dialog: no control characters except line breaks and
/// tabs, at most [`MAX_NOTES_CHARS`] characters, `None` if empty.
pub fn clean_notes(notes: Option<&str>) -> Option<String> {
    let text: String = notes?
        .replace("\r\n", "\n")
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect();
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut out: String = text.chars().take(MAX_NOTES_CHARS).collect();
    if text.chars().count() > MAX_NOTES_CHARS {
        out.push_str(" …");
    }
    Some(out)
}

/// German message for a failed check (shown only for the manual check).
pub fn check_error_text(e: &UpdaterError) -> String {
    match e {
        UpdaterError::Reqwest(_) | UpdaterError::Network(_) | UpdaterError::Io(_) => {
            "Keine Verbindung zum Update-Server (GitHub). Bitte die Internetverbindung prüfen und es später erneut versuchen.".into()
        }
        UpdaterError::ReleaseNotFound | UpdaterError::Serialization(_) => {
            "Auf GitHub wurden keine gültigen Update-Informationen gefunden. Bitte später erneut versuchen.".into()
        }
        UpdaterError::TargetNotFound(_) | UpdaterError::TargetsNotFound(_) => {
            "Die neueste Version enthält kein Update-Paket für dieses System. Bitte den Installer von der Release-Seite verwenden.".into()
        }
        other => format!("Die Suche nach Updates ist fehlgeschlagen: {other}"),
    }
}

/// German message for a failed download/installation.
pub fn install_error_text(e: &UpdaterError) -> String {
    match e {
        UpdaterError::Minisign(_)
        | UpdaterError::Base64(_)
        | UpdaterError::SignatureUtf8(_)
        | UpdaterError::SignedVersionMismatch { .. }
        | UpdaterError::MissingSignedVersion => {
            "Die Signatur des Updates ist ungültig. Das Update wurde verworfen und nicht installiert.".into()
        }
        UpdaterError::Reqwest(_) | UpdaterError::Network(_) => {
            "Der Download ist fehlgeschlagen. Bitte die Internetverbindung prüfen und es erneut versuchen.".into()
        }
        UpdaterError::InvalidUpdaterFormat | UpdaterError::BinaryNotFoundInArchive => {
            "Das heruntergeladene Update-Paket passt nicht zu diesem System. Bitte den Installer von der Release-Seite verwenden.".into()
        }
        other => format!("Das Update konnte nicht installiert werden: {other}"),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_stored(dir: &Path) -> Stored {
    std::fs::read_to_string(dir.join(CONFIG_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_stored(dir: &Path, stored: &Stored) -> Result<(), String> {
    let json = serde_json::to_string_pretty(stored).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir)
        .and_then(|_| std::fs::write(dir.join(CONFIG_FILE), json))
        .map_err(|e| format!("Einstellung konnte nicht gespeichert werden: {e}"))
}

fn stored() -> Stored {
    CONFIG_DIR.get().map(|d| read_stored(d)).unwrap_or_default()
}

pub fn info<R: Runtime>(app: &AppHandle<R>) -> UpdateInfo {
    let s = stored();
    UpdateInfo {
        current_version: app.package_info().version.to_string(),
        availability: availability(),
        interval: s.interval,
        last_check: s.last_check.map(|t| t.saturating_mul(1000)),
        releases_url: RELEASES_URL,
    }
}

pub fn set_interval<R: Runtime>(
    app: &AppHandle<R>,
    interval: Interval,
) -> Result<UpdateInfo, String> {
    let dir = CONFIG_DIR
        .get()
        .ok_or_else(|| "Konfigurationsordner ist nicht bekannt.".to_string())?;
    let mut s = read_stored(dir);
    s.interval = interval;
    write_stored(dir, &s)?;
    Ok(info(app))
}

fn endpoint() -> Result<url::Url, String> {
    #[cfg(debug_assertions)]
    if let Ok(e) = std::env::var(ENDPOINT_ENV) {
        if !e.trim().is_empty() {
            return url::Url::parse(e.trim()).map_err(|e| e.to_string());
        }
    }
    url::Url::parse(ENDPOINT).map_err(|e| e.to_string())
}

/// Checks for a newer version. `manual` = the button: always checks and
/// returns German errors; otherwise only if due (no network otherwise).
/// Errors of the automatic check are only logged by the caller.
pub async fn check<R: Runtime>(
    app: &AppHandle<R>,
    manual: bool,
) -> Result<Option<UpdateOffer>, String> {
    if availability() != Availability::Available {
        return Ok(None);
    }
    let _guard = CHECK_LOCK.lock().await;
    let s = stored();
    if !manual && !is_due(s.interval, s.last_check, now_secs()) {
        return Ok(None);
    }
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint()?])
        .map_err(|e| e.to_string())?
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let found = updater.check().await.map_err(|e| check_error_text(&e))?;
    if let Some(dir) = CONFIG_DIR.get() {
        let mut s = read_stored(dir);
        s.last_check = Some(now_secs());
        if let Err(e) = write_stored(dir, &s) {
            eprintln!("updater: {e}");
        }
    }
    let offer = found.as_ref().map(|u| UpdateOffer {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        notes: clean_notes(u.body.as_deref()),
        date: u.date.and_then(|d| {
            d.format(&time::format_description::well_known::Rfc3339)
                .ok()
        }),
    });
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = found;
    Ok(offer)
}

/// Downloads the pending update (progress on `progress`), verifies its
/// signature, runs `before_install` (stops llama-server and containers that
/// would lock files) and installs it. Windows: starts the NSIS installer in
/// passive mode (`/P /UPDATE /R`) and exits; the installer restarts the app.
/// macOS: replaces the app bundle, then restarts.
pub async fn download_and_install<R, F, Fut>(
    app: &AppHandle<R>,
    progress: &Channel<InstallProgress>,
    before_install: F,
) -> Result<(), String>
where
    R: Runtime,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("Das Update wird bereits installiert.".into());
    }
    let result = async {
        let update = PENDING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| {
                "Kein Update vorhanden. Bitte erneut nach Updates suchen.".to_string()
            })?;
        let mut downloaded: u64 = 0;
        let mut last_sent: u64 = 0;
        let bytes = update
            .download(
                |chunk, total| {
                    downloaded += chunk as u64;
                    // At most ~200 messages per download.
                    let step = total.map(|t| (t / 200).max(1)).unwrap_or(256 * 1024);
                    if downloaded - last_sent >= step || Some(downloaded) == total {
                        last_sent = downloaded;
                        let _ = progress.send(InstallProgress::Download { downloaded, total });
                    }
                },
                || {},
            )
            .await
            .map_err(|e| install_error_text(&e))?;
        let _ = progress.send(InstallProgress::Installing);
        before_install().await;
        // Verifies the signature again, then installs (Windows: exits).
        update.install(bytes).map_err(|e| install_error_text(&e))?;
        app.restart();
    }
    .await;
    INSTALLING.store(false, Ordering::SeqCst);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_texts_are_german() {
        let sig = install_error_text(&UpdaterError::SignedVersionMismatch {
            signed: "0.1.0".into(),
            announced: "0.2.0".into(),
        });
        assert!(sig.contains("Signatur") && sig.contains("nicht installiert"));
        assert!(install_error_text(&UpdaterError::MissingSignedVersion).contains("Signatur"));
        assert!(install_error_text(&UpdaterError::InvalidUpdaterFormat).contains("passt nicht"));
        assert!(check_error_text(&UpdaterError::ReleaseNotFound)
            .contains("keine gültigen Update-Informationen"));
        assert!(check_error_text(&UpdaterError::Network("x".into())).contains("Internetverbindung"));
        assert!(
            check_error_text(&UpdaterError::TargetsNotFound(vec![])).contains("kein Update-Paket")
        );
    }

    #[test]
    fn due_rules() {
        let now = 1_000_000_000;
        assert!(is_due(Interval::Daily, None, now));
        assert!(!is_due(Interval::Daily, Some(now - DAY_SECS + 1), now));
        assert!(is_due(Interval::Daily, Some(now - DAY_SECS), now));
        assert!(!is_due(Interval::Weekly, Some(now - 6 * DAY_SECS), now));
        assert!(is_due(Interval::Weekly, Some(now - 7 * DAY_SECS), now));
        assert!(!is_due(Interval::Never, None, now));
        assert!(!is_due(Interval::Never, Some(0), now));
        // Clock set back: check again instead of waiting until then.
        assert!(is_due(Interval::Daily, Some(now + 10), now));
    }

    #[test]
    fn stored_settings_roundtrip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("cp-updater-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_stored(&dir), Stored::default());
        assert_eq!(Stored::default().interval, Interval::Daily);
        let s = Stored {
            interval: Interval::Weekly,
            last_check: Some(42),
        };
        write_stored(&dir, &s).unwrap();
        assert_eq!(read_stored(&dir), s);
        let raw = std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap();
        assert!(raw.contains("\"interval\": \"weekly\"") && raw.contains("\"lastCheck\": 42"));
        // Broken or partial files fall back to the defaults.
        std::fs::write(dir.join(CONFIG_FILE), "{\"interval\":\"never\"}").unwrap();
        assert_eq!(read_stored(&dir).interval, Interval::Never);
        std::fs::write(dir.join(CONFIG_FILE), "kaputt").unwrap();
        assert_eq!(read_stored(&dir), Stored::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn notes_are_plain_and_bounded() {
        assert_eq!(clean_notes(None), None);
        assert_eq!(clean_notes(Some("  \n ")), None);
        assert_eq!(
            clean_notes(Some("Neu:\r\n- A\u{1b}[31m\u{0}\n\t- B <b>x</b>")).as_deref(),
            Some("Neu:\n- A[31m\n\t- B <b>x</b>")
        );
        let long = "x".repeat(MAX_NOTES_CHARS + 10);
        let cut = clean_notes(Some(&long)).unwrap();
        assert_eq!(cut.chars().count(), MAX_NOTES_CHARS + 2);
        assert!(cut.ends_with(" …"));
    }

    #[test]
    fn configured_needs_a_non_empty_pubkey() {
        let mut config = tauri::Config::default();
        assert!(!configured(&config));
        config
            .plugins
            .0
            .insert("updater".into(), serde_json::json!({ "pubkey": " " }));
        assert!(!configured(&config));
        config.plugins.0.insert(
            "updater".into(),
            serde_json::json!({ "pubkey": "dW50cnVzdGVk" }),
        );
        assert!(configured(&config));
    }

    /// The shipped configuration has no key: release builds add it via the
    /// overlay; dev/local builds therefore run without updater.
    #[test]
    fn base_config_has_no_updater_key() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read_to_string(root.join("tauri.conf.json")).unwrap();
        let conf: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(conf["plugins"]["updater"].is_null());
        assert!(conf["bundle"]["createUpdaterArtifacts"].is_null());
    }

    /// The overlay written by `scripts/updater.mjs config` is a valid
    /// plugin configuration: key, signed version required, passive
    /// Windows installer. Skipped without node.
    #[test]
    fn release_overlay_is_a_valid_plugin_config() {
        // Public key of the minisign-verify test vectors (no secret exists here).
        let pubkey = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            "untrusted comment: minisign public key\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n",
        );
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let out =
            std::env::temp_dir().join(format!("cp-updater-overlay-{}.json", std::process::id()));
        let status = std::process::Command::new("node")
            .arg(root.join("../scripts/updater.mjs"))
            .arg("config")
            .arg(&out)
            .env("TAURI_UPDATER_PUBKEY", &pubkey)
            .status();
        let Ok(status) = status else {
            eprintln!("übersprungen: node fehlt");
            return;
        };
        assert!(status.success());
        let overlay: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        let _ = std::fs::remove_file(&out);
        let mut config = tauri::Config::default();
        config.plugins.0 = serde_json::from_value(overlay["plugins"].clone()).unwrap();
        assert!(configured(&config));
        let plugin: tauri_plugin_updater::Config =
            serde_json::from_value(overlay["plugins"]["updater"].clone()).unwrap();
        assert_eq!(plugin.pubkey, pubkey);
        assert!(plugin.require_signed_version);
        assert!(!plugin.allow_downgrades && !plugin.dangerous_insecure_transport_protocol);
        assert!(
            plugin.endpoints.is_empty(),
            "Endpunkt steht fest im Code (ENDPOINT)"
        );
        assert_eq!(plugin.windows.unwrap().install_mode.to_string(), "passive");
    }

    #[test]
    fn endpoint_is_https_latest_release() {
        let url = url::Url::parse(ENDPOINT).unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("github.com"));
        assert!(url
            .path()
            .ends_with("/releases/latest/download/latest.json"));
        assert!(ENDPOINT.starts_with(RELEASES_URL));
    }
}
