import { useEffect, useState } from "react";
import {
  CANCELLED,
  cancelAi,
  deleteModel,
  downloadModel,
  formatGb,
  importModel,
  onModelsProgress,
  pickModelFile,
  selectModel,
  type AiStatus,
  type ModelInfo,
  type ModelsProgress,
} from "../api";

interface Props {
  status: AiStatus | null;
  onStatus: (s: AiStatus) => void;
  onClose: () => void;
}

function hardwareText(s: AiStatus): string {
  const h = s.hardware;
  const parts = [`Arbeitsspeicher ${formatGb(h.totalRamBytes)}`, `${h.physicalCores} Kerne`];
  if (h.appleSilicon) parts.push("Apple Silicon (Metal)");
  else if (h.avx2 !== null) parts.push(`AVX2 ${h.avx2 ? "ja" : "nein"}`);
  return parts.join(" · ");
}

function modelState(m: ModelInfo): string {
  if (m.installed) return "eingerichtet";
  if (m.partialBytes > 0) return `teilweise geladen (${formatGb(m.partialBytes)})`;
  if (m.known && !m.pinned) return "Download in dieser Version nicht möglich – bitte importieren";
  return "nicht geladen";
}

/** "KI-Modell": hardware suggestion, download (with progress), import, delete, select. */
export function ModelSetup({ status, onStatus, onClose }: Props) {
  const [busy, setBusy] = useState<string | null>(null);
  const [progress, setProgress] = useState<ModelsProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  // Unknown .gguf waiting for "Trotzdem importieren".
  const [pendingImport, setPendingImport] = useState<string | null>(null);

  useEffect(() => {
    const unlisten = onModelsProgress(setProgress);
    return () => void unlisten.then((f) => f());
  }, []);

  const act = async (label: string, task: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    setNote(null);
    setProgress(null);
    try {
      await task();
    } catch (e) {
      const msg = String(e);
      if (msg === CANCELLED) setNote("Abgebrochen. Ein angefangener Download wird beim nächsten Mal fortgesetzt.");
      else setError(msg);
    } finally {
      setBusy(null);
      setProgress(null);
    }
  };

  const download = (m: ModelInfo) =>
    act(`${m.name} wird heruntergeladen …`, async () => {
      onStatus(await downloadModel(m.id));
      setNote(`${m.name} ist eingerichtet. Ab jetzt läuft die KI ohne Internet.`);
    });

  const runImport = (path: string, confirmUnknown: boolean) =>
    act("Datei wird geprüft und kopiert …", async () => {
      const r = await importModel(path, confirmUnknown);
      onStatus(r.status);
      if (r.needsConfirmation) {
        setPendingImport(path);
        return;
      }
      setPendingImport(null);
      if (r.incomplete) setNote("Teil importiert – weitere Teile dieses Modells fehlen noch.");
      else if (r.known) setNote("Modell erkannt (Prüfsumme stimmt mit dem Katalog überein) und eingerichtet.");
      else setNote("Unbekanntes Modell importiert. Ergebnisse bitte besonders kritisch prüfen.");
    });

  const pickImport = async () => {
    setError(null);
    try {
      const path = await pickModelFile();
      if (path) await runImport(path, false);
    } catch (e) {
      setError(String(e));
    }
  };

  const remove = (m: ModelInfo) =>
    act(`${m.name} wird gelöscht …`, async () => {
      setConfirmDelete(null);
      onStatus(await deleteModel(m.id));
    });

  const choose = (m: ModelInfo) =>
    act("Auswahl wird gespeichert …", async () => {
      onStatus(await selectModel(m.id));
    });

  const idle = busy === null;

  return (
    <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label="KI-Modell einrichten">
      <div className="modal card">
        <div className="card-header">
          <h2>KI-Modell</h2>
          <button onClick={onClose} disabled={!idle} title={idle ? undefined : "Bitte warten oder abbrechen"}>
            Schließen
          </button>
        </div>
        <p className="small">
          Die KI läuft vollständig auf diesem Computer. Nur der einmalige Download des Modells von
          huggingface.co braucht Internet; danach funktioniert alles offline. Aufgabentexte und Code
          verlassen den Computer nie. Ohne Internet: die .gguf-Datei auf einem anderen Rechner
          herunterladen und hier importieren.
        </p>
        {status === null ? (
          <p className="muted">Lade Status …</p>
        ) : (
          <>
            <p className="small muted">
              {hardwareText(status)} · KI-Laufzeit llama.cpp {status.llamaVersion}
            </p>
            {status.hardware.warnings.map((w) => (
              <div key={w} className="hint small">
                {w}
              </div>
            ))}
            {!status.runtimeAvailable && status.runtimeError && (
              <div className="banner warn small">{status.runtimeError}</div>
            )}
            <ul className="model-list">
              {status.models.map((m) => (
                <li key={m.id} className={m.selected ? "model selected" : "model"}>
                  <div className="row">
                    <strong>{m.name}</strong>
                    {m.sizeBytes > 0 && <span className="muted small">{formatGb(m.sizeBytes)}</span>}
                    {m.license && <span className="muted small">{m.license}</span>}
                    {m.suggested && <span className="badge info">empfohlen</span>}
                    {m.selected && <span className="badge passed">ausgewählt</span>}
                    {!m.known && (
                      <span className="badge timeout" title="Prüfsumme nicht im Katalog">
                        unbekanntes Modell
                      </span>
                    )}
                  </div>
                  <div className="row small">
                    <span className="muted">{modelState(m)}</span>
                    {m.known && !m.installed && m.pinned && (
                      <button onClick={() => void download(m)} disabled={!idle}>
                        {m.partialBytes > 0 ? "Download fortsetzen" : "Herunterladen"}
                      </button>
                    )}
                    {m.installed && !m.selected && (
                      <button onClick={() => void choose(m)} disabled={!idle}>
                        Verwenden
                      </button>
                    )}
                    {(m.installed || m.partialBytes > 0) &&
                      (confirmDelete === m.id ? (
                        <>
                          <button className="danger" onClick={() => void remove(m)} disabled={!idle}>
                            Wirklich löschen?
                          </button>
                          <button onClick={() => setConfirmDelete(null)}>Nein</button>
                        </>
                      ) : (
                        <button className="danger" onClick={() => setConfirmDelete(m.id)} disabled={!idle}>
                          Löschen
                        </button>
                      ))}
                  </div>
                </li>
              ))}
            </ul>
            <p className="small muted">
              Empfehlung: unter 12 GB Arbeitsspeicher das 1.5B-Modell (schneller, schwächer),
              sonst das 7B-Modell (deutlich besser, langsamer).
              {status.modelsDir && <> Speicherort: {status.modelsDir}</>}
            </p>
          </>
        )}
        <div className="row">
          <button onClick={() => void pickImport()} disabled={!idle}>
            Datei importieren (.gguf) …
          </button>
        </div>
        {pendingImport && idle && (
          <div className="banner warn small">
            <p>
              Diese Datei ist kein Modell aus dem Katalog (Prüfsumme unbekannt). Ein unbekanntes Modell
              kann schlechte oder unsinnige Testfälle liefern und ist nicht geprüft.
            </p>
            <div className="row">
              <button className="danger" onClick={() => void runImport(pendingImport, true)}>
                Trotzdem importieren
              </button>
              <button onClick={() => setPendingImport(null)}>Abbrechen</button>
            </div>
          </div>
        )}
        {busy !== null && (
          <div className="ai-progress small">
            <span>{progress?.message ?? busy}</span>
            {progress?.percent !== null && progress?.percent !== undefined && (
              <progress max={100} value={progress.percent} />
            )}
            <button onClick={() => void cancelAi()}>Abbrechen</button>
          </div>
        )}
        {note && <p className="small ok-text">{note}</p>}
        {error && <div className="banner warn small">{error}</div>}
      </div>
    </div>
  );
}
