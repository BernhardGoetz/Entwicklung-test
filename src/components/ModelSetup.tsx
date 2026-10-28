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
  pickModelFiles,
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
  if (m.installed) return m.known && !m.verified ? "eingerichtet (importiert, nicht geprüft)" : "eingerichtet";
  if (m.partialBytes > 0) return `teilweise geladen (${formatGb(m.partialBytes)})`;
  if (m.importedParts > 0) return `${m.importedParts} Teil(e) importiert – es fehlen noch Teile`;
  if (m.known && !m.pinned) return "Download in dieser Version nicht möglich – bitte die Datei importieren";
  return "nicht geladen";
}

/** Import waiting for "Trotzdem importieren". */
interface PendingImport {
  paths: string[];
  /** Catalog entry whose button was used (null: "Anderes Modell importieren"). */
  catalogId: string | null;
  unverified: boolean;
  warnings: string[];
  missingParts: string[];
}

/** Explanation and "Datei importieren …" for a catalog entry that is not installed. */
function ImportHint({ m, idle, onImport }: { m: ModelInfo; idle: boolean; onImport: () => void }) {
  return (
    <div className="import-hint small">
      <div className="row">
        <button onClick={onImport} disabled={!idle}>
          Datei importieren …
        </button>
        <span>
          Erwartet: <code>*{m.quant}*.gguf</code> (eine .gguf-Datei mit „{m.quant}“ im Namen). Große Modelle
          sind auf Hugging Face oft in mehrere Teile aufgeteilt (<code>…-00001-of-0000N.gguf</code>) – dann
          alle Teile herunterladen und zusammen auswählen.
        </span>
      </div>
      {m.repo && (
        <span className="muted">
          Wo finde ich die Datei? Auf huggingface.co im Repository <code>{m.repo}</code>, Reiter „Files and
          versions“ – auch auf einem anderen Rechner, dann per USB-Stick hierher bringen.
        </span>
      )}
      {m.missingParts.length > 0 && (
        <span>
          Es fehlen noch: {m.missingParts.join(", ")}
        </span>
      )}
    </div>
  );
}

/** "KI-Modell": hardware suggestion, download (with progress), import, delete, select. */
export function ModelSetup({ status, onStatus, onClose }: Props) {
  const [busy, setBusy] = useState<string | null>(null);
  const [progress, setProgress] = useState<ModelsProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  // Unknown or unchecked .gguf waiting for "Trotzdem importieren".
  const [pendingImport, setPendingImport] = useState<PendingImport | null>(null);

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

  const runImport = (paths: string[], catalogId: string | null, confirm: boolean) =>
    act(paths.length > 1 ? `${paths.length} Dateien werden geprüft und kopiert …` : "Datei wird geprüft und kopiert …", async () => {
      setPendingImport(null);
      const r = await importModel(paths, catalogId, confirm);
      onStatus(r.status);
      if (r.needsConfirmation) {
        setPendingImport({
          paths,
          catalogId,
          unverified: r.unverified,
          warnings: r.warnings,
          missingParts: r.missingParts,
        });
        return;
      }
      if (r.incomplete)
        setNote(
          "Teil importiert – weitere Teile dieses Modells fehlen noch" +
            (r.missingParts.length > 0 ? `: ${r.missingParts.join(", ")}` : ".")
        );
      else if (r.unverified)
        setNote("Importiert und eingerichtet – nicht geprüft (Prüfsumme fehlt in dieser Version).");
      else if (r.known) setNote("Modell erkannt (Prüfsumme stimmt mit dem Katalog überein) und eingerichtet.");
      else setNote("Unbekanntes Modell importiert. Ergebnisse bitte besonders kritisch prüfen.");
    });

  const pickImport = async () => {
    setError(null);
    try {
      const path = await pickModelFile();
      if (path) await runImport([path], null, false);
    } catch (e) {
      setError(String(e));
    }
  };

  const pickImportFor = async (m: ModelInfo) => {
    setError(null);
    try {
      const picked = await pickModelFiles(`${m.name} importieren (alle Teile auswählen)`);
      const paths = picked === null ? [] : Array.isArray(picked) ? picked : [picked];
      if (paths.length > 0) await runImport(paths, m.id, false);
    } catch (e) {
      setError(String(e));
    }
  };

  const nameOf = (id: string | null) => status?.models.find((m) => m.id === id)?.name ?? "dieses Modell";

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
                    {m.known && m.installed && !m.verified && (
                      <span className="badge timeout" title="Die Datei wurde importiert, konnte aber nicht geprüft werden">
                        nicht geprüft (Prüfsumme fehlt in dieser Version)
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
                        Auswählen
                      </button>
                    )}
                    {!m.installed && (
                      <span className="muted">
                        Auswählen erst möglich, wenn das Modell vollständig da ist: erst importieren
                        {m.pinned ? " oder herunterladen" : ""}.
                      </span>
                    )}
                    {(m.installed || m.partialBytes > 0 || m.importedParts > 0) &&
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
                  {m.known && !m.installed && (
                    <ImportHint m={m} idle={idle} onImport={() => void pickImportFor(m)} />
                  )}
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
            Anderes Modell importieren (.gguf) …
          </button>
        </div>
        {pendingImport && idle && (
          <div className="banner warn small">
            {pendingImport.unverified ? (
              <p>
                Für „{nameOf(pendingImport.catalogId)}“ ist in dieser Version keine Prüfsumme hinterlegt –
                {pendingImport.paths.length > 1 ? " die Dateien können" : " die Datei kann"} deshalb nicht
                geprüft werden. Sie werden diesem Modell zugeordnet und als „nicht geprüft“ markiert. Nur
                Dateien aus dem genannten Hugging-Face-Repository verwenden.
              </p>
            ) : (
              <p>
                Diese Datei ist kein Modell aus dem Katalog (Prüfsumme unbekannt). Ein unbekanntes Modell
                kann schlechte oder unsinnige Testfälle liefern und ist nicht geprüft.
              </p>
            )}
            {pendingImport.warnings.length > 0 && (
              <ul>
                {pendingImport.warnings.map((w) => (
                  <li key={w}>{w}</li>
                ))}
              </ul>
            )}
            {pendingImport.missingParts.length > 0 && (
              <p>
                Danach fehlen noch: {pendingImport.missingParts.join(", ")} (können auch später importiert
                werden).
              </p>
            )}
            <div className="row">
              <button
                className="danger"
                onClick={() => void runImport(pendingImport.paths, pendingImport.catalogId, true)}
              >
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
