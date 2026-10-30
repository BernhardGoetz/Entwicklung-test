import { openUrl } from "@tauri-apps/plugin-opener";
import type { UpdateInfo, UpdateInterval } from "../api";

const INTERVALS: { value: UpdateInterval; label: string; title: string }[] = [
  { value: "daily", label: "Täglich", title: "Beim Start höchstens einmal am Tag nach Updates suchen" },
  { value: "weekly", label: "Wöchentlich", title: "Beim Start höchstens einmal pro Woche nach Updates suchen" },
  { value: "never", label: "Nie", title: "Nicht automatisch suchen (die Schaltfläche funktioniert weiter)" },
];

export interface CheckNote {
  text: string;
  tone: "ok" | "error";
}

function ReleasesLink({ url, children }: { url: string; children: string }) {
  return (
    <a
      href={url}
      onClick={(e) => {
        e.preventDefault();
        void openUrl(url);
      }}
    >
      {children}
    </a>
  );
}

function lastCheckText(ms: number | null): string {
  if (ms === null) return "noch nie";
  return new Date(ms).toLocaleString("de-DE", { dateStyle: "medium", timeStyle: "short" });
}

/** „Updates“ section of the settings dialog. */
export function UpdateSettings({
  info,
  checking,
  note,
  onIntervalChange,
  onCheck,
}: {
  info: UpdateInfo | null;
  checking: boolean;
  note: CheckNote | null;
  onIntervalChange: (i: UpdateInterval) => void;
  onCheck: () => void;
}) {
  if (info === null) return <p className="muted small">Status wird geladen …</p>;
  const version = (
    <div className="setting-row">
      <span>Installierte Version</span>
      <span className="tabular">{info.currentVersion}</span>
    </div>
  );
  if (info.availability === "notConfigured") {
    return (
      <>
        {version}
        <div className="setting-status">
          <span className="dot tone-neutral" aria-hidden="true" />
          <span>Updates in dieser Version nicht verfügbar</span>
        </div>
        <p className="setting-hint muted">
          Diese Fassung wurde ohne Update-Schlüssel gebaut (Test- oder Entwicklerversion). Neue Versionen gibt es
          auf der <ReleasesLink url={info.releasesUrl}>Release-Seite auf GitHub</ReleasesLink>.
        </p>
      </>
    );
  }
  if (info.availability === "portable") {
    return (
      <>
        {version}
        <div className="setting-status">
          <span className="dot tone-neutral" aria-hidden="true" />
          <span>Die portable Version aktualisiert sich nicht selbst.</span>
        </div>
        <p className="setting-hint muted">
          Neue Versionen gibt es auf der <ReleasesLink url={info.releasesUrl}>Release-Seite auf GitHub</ReleasesLink>.
          Mit dem Installer (empfohlen) kommen Updates automatisch.
        </p>
      </>
    );
  }
  return (
    <>
      {version}
      <div className="setting-row">
        <span id="set-update-interval-label">Automatisch suchen</span>
        <fieldset className="segmented" aria-labelledby="set-update-interval-label">
          <legend className="sr-only">Automatisch nach Updates suchen</legend>
          {INTERVALS.map((o) => (
            <label key={o.value} title={o.title} className={info.interval === o.value ? "checked" : undefined}>
              <input
                type="radio"
                name="update-interval"
                value={o.value}
                checked={info.interval === o.value}
                onChange={() => onIntervalChange(o.value)}
              />
              <span>{o.label}</span>
            </label>
          ))}
        </fieldset>
      </div>
      <div className="setting-row">
        <span>Zuletzt gesucht</span>
        <span>{lastCheckText(info.lastCheck)}</span>
      </div>
      <p className="setting-hint muted">
        Gesucht wird beim Start und während die App läuft, höchstens im gewählten Abstand. Angeboten werden nur
        veröffentlichte Versionen von GitHub; jedes Update wird vor der Installation auf seine Signatur geprüft.
      </p>
      <div className="row">
        <button onClick={onCheck} disabled={checking}>
          {checking ? "Suche läuft …" : "Jetzt nach Updates suchen"}
        </button>
      </div>
      {checking && <p className="note busy" role="status">Suche nach Updates …</p>}
      {!checking && note && (
        <p className={note.tone === "ok" ? "note ok" : "note error"} role="status">
          {note.text}
        </p>
      )}
    </>
  );
}
