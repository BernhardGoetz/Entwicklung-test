//! File selection in the backend (audit 2, I5).
//!
//! The webview never passes file paths: the backend opens the system file
//! dialog itself (`tauri-plugin-dialog` from Rust; the webview has no
//! dialog permission) and returns only an opaque token plus the chosen file
//! names (for display). The import commands take the token, which maps to
//! the paths and to what they were picked for. A script in the webview
//! (e.g. after an XSS) can therefore only import files the user chose in
//! the dialog for exactly that purpose.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// What the files are picked for (determines dialog filter and which
/// import command accepts the token).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PickKind {
    /// One task sheet (image, PDF, DOCX).
    TaskDocument,
    /// One `.gguf` file ("Anderes Modell importieren").
    Model,
    /// One or more `.gguf` files for a catalog entry (all parts).
    ModelParts,
}

/// Answer of the dialog command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Picked {
    pub token: String,
    pub file_names: Vec<String>,
}

/// A selection: paths, purpose, catalog entry (for [`PickKind::ModelParts`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub kind: PickKind,
    pub catalog_id: Option<String>,
    pub paths: Vec<PathBuf>,
}

struct Entry {
    token: String,
    created: Instant,
    selection: Selection,
}

/// Selections are kept for a while (an unknown model needs a second call
/// after "Trotzdem importieren"), at most [`MAX_ENTRIES`].
pub const TTL: Duration = Duration::from_secs(60 * 60);
pub const MAX_ENTRIES: usize = 8;

/// Token → selection; pure apart from the clock, unit-tested.
#[derive(Default)]
pub struct Store {
    entries: VecDeque<Entry>,
}

impl Store {
    pub const fn new() -> Self {
        Store {
            entries: VecDeque::new(),
        }
    }

    pub fn insert(&mut self, token: String, selection: Selection, now: Instant) -> Picked {
        self.entries
            .retain(|e| now.saturating_duration_since(e.created) < TTL);
        while self.entries.len() >= MAX_ENTRIES {
            self.entries.pop_front();
        }
        let file_names = selection
            .paths
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        self.entries.push_back(Entry {
            token: token.clone(),
            created: now,
            selection,
        });
        Picked { token, file_names }
    }

    /// The selection behind `token` if it was made for one of `kinds`.
    pub fn get(&self, token: &str, kinds: &[PickKind], now: Instant) -> Result<Selection, String> {
        let entry = self
            .entries
            .iter()
            .find(|e| e.token == token && now.saturating_duration_since(e.created) < TTL)
            .ok_or_else(|| {
                "Die Dateiauswahl ist abgelaufen. Bitte die Datei erneut auswählen.".to_string()
            })?;
        if !kinds.contains(&entry.selection.kind) {
            return Err("Die Dateiauswahl passt nicht zu dieser Aktion.".into());
        }
        Ok(entry.selection.clone())
    }
}

static STORE: Mutex<Store> = Mutex::new(Store::new());

fn token() -> Result<String, String> {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).map_err(|e| format!("Zufallszahlen nicht verfügbar: {e}"))?;
    Ok(crate::ai::hex(&buf))
}

/// Remembers a selection and returns its token.
pub fn remember(selection: Selection) -> Result<Picked, String> {
    let token = token()?;
    let mut store = STORE.lock().map_err(|e| e.to_string())?;
    Ok(store.insert(token, selection, Instant::now()))
}

/// The selection behind `token` (made for one of `kinds`).
pub fn selection(token: &str, kinds: &[PickKind]) -> Result<Selection, String> {
    STORE
        .lock()
        .map_err(|e| e.to_string())?
        .get(token, kinds, Instant::now())
}

/// Dialog title and filter for `kind`; `model_name` for model parts.
pub fn dialog_spec(
    kind: PickKind,
    model_name: Option<&str>,
) -> (String, &'static str, Vec<&'static str>) {
    match kind {
        PickKind::TaskDocument => (
            "Aufgabenblatt öffnen".into(),
            "Aufgabenblatt (Bild, PDF, Word)",
            crate::docs::EXTENSIONS.to_vec(),
        ),
        PickKind::Model => (
            "KI-Modell importieren".into(),
            "KI-Modell (GGUF)",
            vec!["gguf"],
        ),
        PickKind::ModelParts => (
            format!(
                "{} importieren (alle Teile auswählen)",
                model_name.unwrap_or("KI-Modell")
            ),
            "KI-Modell (GGUF)",
            vec!["gguf"],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(kind: PickKind, paths: &[&str]) -> Selection {
        Selection {
            kind,
            catalog_id: None,
            paths: paths.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn tokens_map_to_their_selection_and_purpose() {
        let mut store = Store::new();
        let t0 = Instant::now();
        let picked = store.insert(
            "a".into(),
            sel(PickKind::TaskDocument, &["/x/Blatt 3.pdf"]),
            t0,
        );
        assert_eq!(picked.file_names, vec!["Blatt 3.pdf"]);
        let json = serde_json::to_value(&picked).unwrap();
        assert_eq!(json["fileNames"][0], "Blatt 3.pdf");
        assert_eq!(
            store.get("a", &[PickKind::TaskDocument], t0).unwrap().paths,
            vec![PathBuf::from("/x/Blatt 3.pdf")]
        );
        // Wrong purpose, unknown token, expired.
        let e = store.get("a", &[PickKind::Model], t0).unwrap_err();
        assert!(e.contains("passt nicht"), "{e}");
        assert!(store.get("b", &[PickKind::TaskDocument], t0).is_err());
        let e = store
            .get("a", &[PickKind::TaskDocument], t0 + TTL)
            .unwrap_err();
        assert!(e.contains("abgelaufen"), "{e}");
        // Reusable within the TTL (second call after "Trotzdem importieren").
        assert!(store.get("a", &[PickKind::TaskDocument], t0).is_ok());
    }

    #[test]
    fn store_is_bounded() {
        let mut store = Store::new();
        let t0 = Instant::now();
        for i in 0..MAX_ENTRIES + 3 {
            store.insert(format!("t{i}"), sel(PickKind::Model, &["/m.gguf"]), t0);
        }
        assert_eq!(store.entries.len(), MAX_ENTRIES);
        assert!(store.get("t0", &[PickKind::Model], t0).is_err());
        assert!(store
            .get(&format!("t{}", MAX_ENTRIES + 2), &[PickKind::Model], t0)
            .is_ok());
    }

    #[test]
    fn dialog_filters() {
        let (title, _, ext) = dialog_spec(PickKind::TaskDocument, None);
        assert_eq!(title, "Aufgabenblatt öffnen");
        assert_eq!(ext, crate::docs::EXTENSIONS);
        let (title, _, ext) = dialog_spec(PickKind::ModelParts, Some("Qwen 7B"));
        assert_eq!(title, "Qwen 7B importieren (alle Teile auswählen)");
        assert_eq!(ext, ["gguf"]);
        let json = serde_json::to_string(&PickKind::ModelParts).unwrap();
        assert_eq!(json, "\"modelParts\"");
    }
}
