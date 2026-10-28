import { useState } from "react";
import {
  importTaskDocument,
  onDocsProgress,
  pickTaskDocument,
  progressText,
  type ImportResult,
} from "../api";

interface Props {
  value: string;
  onChange: (v: string) => void;
  /** "Text geprüft und korrigiert". */
  confirmed: boolean;
  onConfirmedChange: (v: boolean) => void;
  /** Last imported document, for source info and preview. */
  imported: ImportResult | null;
  onImported: (r: ImportResult) => void;
}

/** Same limits as MAX_FILE_BYTES and MAX_PAGES in src-tauri/src/docs/mod.rs. */
const LIMITS_TEXT = "höchstens 20 MB und 20 Seiten";

const SOURCE_NAMES: Record<ImportResult["source"], string> = {
  docx: "Word-Dokument",
  pdf: "PDF",
  image: "Bild",
};

function sourceText(r: ImportResult): string {
  const pages = r.pages !== null && r.source === "pdf" ? `, ${r.pages} ${r.pages === 1 ? "Seite" : "Seiten"}` : "";
  return `${SOURCE_NAMES[r.source]}${pages}`;
}

export function TaskStatement({
  value,
  onChange,
  confirmed,
  onConfirmedChange,
  imported,
  onImported,
}: Props) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [previewLarge, setPreviewLarge] = useState(false);

  const upload = async () => {
    setError(null);
    let path: string | null;
    try {
      path = await pickTaskDocument();
    } catch (e) {
      setError(String(e));
      return;
    }
    if (!path) return;
    setBusy("Datei wird gelesen …");
    const unlisten = onDocsProgress((p) => setBusy(progressText(p)));
    try {
      const r = await importTaskDocument(path);
      setPreviewLarge(false);
      onImported(r);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
      void unlisten.then((f) => f());
    }
  };

  return (
    <section className="card">
      <div className="card-header">
        <h2>Aufgabenstellung</h2>
        <button
          onClick={() => void upload()}
          disabled={busy !== null}
          title={`Bild (PNG, JPEG), PDF oder Word-Datei (DOCX), ${LIMITS_TEXT}`}
        >
          Aufgabe hochladen …
        </button>
      </div>
      {busy !== null && <p className="muted small">{busy}</p>}
      {error && <div className="hint small">{error}</div>}
      {imported && (
        <div className="import-info small">
          <span>
            „{imported.fileName}“ · {sourceText(imported)}
            {imported.ocrUsed && " · Texterkennung"}
          </span>
          {imported.ocrUsed && (
            <span className="warn-text">
              Der Text wurde automatisch erkannt und kann Fehler enthalten (z. B. 0/O, 1/l,
              Einrückung). Bitte mit dem Original vergleichen.
            </span>
          )}
          {imported.warnings.map((w) => (
            <span key={w} className="warn-text">
              {w}
            </span>
          ))}
        </div>
      )}
      <div className={imported?.preview ? "task-body with-preview" : "task-body"}>
        {imported?.preview && (
          <img
            className={previewLarge ? "task-preview large" : "task-preview"}
            src={imported.preview}
            alt={`Vorschau von „${imported.fileName}“`}
            title={previewLarge ? "Klicken zum Verkleinern" : "Klicken zum Vergrößern"}
            onClick={() => setPreviewLarge((v) => !v)}
          />
        )}
        <textarea
          className="task"
          placeholder="Aufgabenstellung hier einfügen oder ein Aufgabenblatt hochladen (Bild, PDF oder Word) …"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          rows={imported ? 12 : 5}
          spellCheck={false}
        />
      </div>
      {value.trim() !== "" && (
        <label className="inline small confirm">
          <input
            type="checkbox"
            checked={confirmed}
            onChange={(e) => onConfirmedChange(e.target.checked)}
          />{" "}
          Text geprüft und korrigiert
        </label>
      )}
    </section>
  );
}
