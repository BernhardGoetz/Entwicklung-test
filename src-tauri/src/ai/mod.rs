//! Local AI: llama.cpp sidecar (`llama`), model management (`models`) and
//! the test generation pipeline (`pipeline`, `prompts`).
//!
//! Everything runs on this machine; the only network access is the model
//! download from huggingface.co (`models`).

pub mod llama;
pub mod models;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error(
        "KI-Laufzeit nicht gefunden ({0}). Bitte den Code-Prüfer neu installieren; in \
         Entwickler-Builds llama-server mit „scripts/fetch-llama-server.sh“ holen oder \
         CODE_PRUEFER_LLAMA_SERVER setzen."
    )]
    RuntimeMissing(String),
    #[error("Die KI-Laufzeit konnte nicht gestartet werden: {0}")]
    Start(String),
    #[error("Fehler bei der Anfrage an die KI-Laufzeit: {0}")]
    Http(String),
    #[error("Abgebrochen.")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

impl Serialize for AiError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

/// Cancellation tokens of the AI operations in flight (test generation,
/// model download); `cancel_ai` cancels all of them.
static OPERATIONS: Mutex<Vec<(u64, CancellationToken)>> = Mutex::new(Vec::new());
static NEXT_OP: AtomicU64 = AtomicU64::new(1);

/// A registered operation; deregisters itself when dropped.
pub struct Operation {
    id: u64,
    pub token: CancellationToken,
}

impl Operation {
    pub fn start() -> Self {
        let id = NEXT_OP.fetch_add(1, Ordering::Relaxed);
        let token = CancellationToken::new();
        if let Ok(mut ops) = OPERATIONS.lock() {
            ops.push((id, token.clone()));
        }
        Operation { id, token }
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        if let Ok(mut ops) = OPERATIONS.lock() {
            ops.retain(|(id, _)| *id != self.id);
        }
    }
}

/// Cancels every running AI operation. Returns how many were running.
pub fn cancel_all() -> usize {
    OPERATIONS
        .lock()
        .map(|ops| {
            for (_, t) in ops.iter() {
                t.cancel();
            }
            ops.len()
        })
        .unwrap_or(0)
}

/// Lower-case hex encoding.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_register_and_cancel() {
        let a = Operation::start();
        let b = Operation::start();
        assert!(cancel_all() >= 2);
        assert!(a.token.is_cancelled() && b.token.is_cancelled());
        let id = a.id;
        drop(a);
        assert!(!OPERATIONS.lock().unwrap().iter().any(|(i, _)| *i == id));
    }

    #[test]
    fn hex_is_lowercase() {
        assert_eq!(hex(&[0, 0xab, 0x10]), "00ab10");
    }

    #[test]
    fn errors_are_german_strings() {
        let v = serde_json::to_value(AiError::Cancelled).unwrap();
        assert_eq!(v, "Abgebrochen.");
        assert!(AiError::RuntimeMissing("x".into())
            .to_string()
            .starts_with("KI-Laufzeit nicht gefunden"));
    }
}
