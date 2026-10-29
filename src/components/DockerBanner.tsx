import type { ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ENGINE_NAMES, type DockerStatus, type Engine, type EngineChoice } from "../api";

// The only URLs the app may open (synced with capabilities/default.json by a
// test in src-tauri/src/lib.rs – keep the literals in this file).
const DOWNLOAD: Record<Engine, string> = {
  docker: "https://www.docker.com/products/docker-desktop/",
  podman: "https://podman.io/",
};

function ExtLink({ href, children }: { href: string; children: string }) {
  return (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        void openUrl(href);
      }}
    >
      {children}
    </a>
  );
}

/** „Docker Desktop oder Podman“ with links to the download pages. */
function DownloadLinks() {
  return (
    <>
      <ExtLink href={DOWNLOAD.docker}>Docker Desktop</ExtLink> oder <ExtLink href={DOWNLOAD.podman}>Podman</ExtLink>
    </>
  );
}

export interface ImagesState {
  /** Progress text while images are prepared/updated, otherwise null. */
  busy: string | null;
  note: { text: string; error: boolean } | null;
}

/** Engine state shared by the header, the notices and the settings. */
export interface EngineState {
  status: DockerStatus | null;
  engine: EngineChoice | null;
  /** Status message while the engine is being started, otherwise null. */
  starting: string | null;
  startError: string | null;
  images: ImagesState;
}

type Tone = "ok" | "warn" | "error" | "busy" | "neutral";

const engineName = (engine: EngineChoice | null) => (engine ? ENGINE_NAMES[engine.engine] : "Docker/Podman");

function summary({ status, engine, starting }: EngineState): { tone: Tone; text: string } {
  if (starting !== null) return { tone: "busy", text: starting };
  if (status === null) return { tone: "neutral", text: "Suche Container-Engine …" };
  if (status.state === "ok") return { tone: "ok", text: `${status.product} ${status.version} läuft` };
  if (status.state === "notRunning") return { tone: "warn", text: `${engineName(engine)} ist nicht gestartet` };
  return { tone: "error", text: "Keine Container-Engine gefunden" };
}

/** Header: dot + short text, „<Engine> starten“ while the engine is stopped. */
export function EngineStatus({ state, onStart }: { state: EngineState; onStart: () => void }) {
  const { tone, text } = summary(state);
  return (
    <div
      className="engine-status"
      role="status"
      title={state.status?.state === "ok" ? "Code wird lokal in isolierten Containern ausgeführt." : undefined}
    >
      <span className={`dot tone-${tone}`} aria-hidden="true" />
      <span className="engine-text">{text}</span>
      {state.starting === null && state.status?.state === "notRunning" && (
        <button className="compact" onClick={onStart}>
          {engineName(state.engine)} starten
        </button>
      )}
    </div>
  );
}

/**
 * Notices under the header that need attention: engine missing/stopped,
 * start errors, security warnings, failed image preparation.
 */
export function EngineNotices({
  state,
  onRetry,
  onOpenSettings,
}: {
  state: EngineState;
  onRetry: () => void;
  onOpenSettings: () => void;
}) {
  const { status, engine, starting, startError, images } = state;
  const name = engineName(engine);
  const notices: ReactNode[] = [];
  if (starting !== null) {
    notices.push(
      <p key="st" className="note busy">
        {starting} <span className="muted">Bitte warten – das kann bis zu zwei Minuten dauern.</span>
      </p>,
    );
  } else if (status !== null && status.state !== "ok") {
    if (startError)
      notices.push(
        <p key="se" className="note error">
          <strong>Start fehlgeschlagen:</strong> {startError}
        </p>,
      );
    if (status.state === "notRunning") {
      notices.push(
        <p key="nr" className="note warn">
          <strong>{name} ist installiert, aber nicht gestartet.</strong> Die App startet die gewählte Engine
          automatisch; alternativ {name} selbst starten und dann erneut prüfen.
          <button onClick={onRetry}>Erneut prüfen</button>
        </p>,
      );
    } else {
      notices.push(
        <p key="ni" className="note error">
          Der Code-Prüfer benötigt <DownloadLinks />. Docker Desktop ist kostenlos für private Nutzung/Bildung –
          bitte Lizenzbedingungen selbst prüfen.
          <button onClick={onRetry}>Erneut prüfen</button>
          <button onClick={onOpenSettings}>Engine wählen …</button>
        </p>,
      );
    }
  }
  if (status?.state === "ok" && status.warning)
    notices.push(
      <p key="w" className="note warn">
        {status.warning}
      </p>,
    );
  if (images.busy === null && images.note?.error)
    notices.push(
      <p key="in" className="note warn">
        {images.note.text}
      </p>,
    );
  if (notices.length === 0) return null;
  return (
    <div className="engine-notices" aria-live="polite">
      {notices}
    </div>
  );
}

/** „Container-Engine“ section of the settings dialog. */
export function EngineSettings({
  state,
  busy,
  onEngineChange,
  onStart,
  onRetry,
  onUpdateImages,
}: {
  state: EngineState;
  /** A check or AI run is in progress: no engine switch. */
  busy: boolean;
  onEngineChange: (e: Engine) => void;
  onStart: () => void;
  onRetry: () => void;
  onUpdateImages: () => void;
}) {
  const { status, engine, starting, startError, images } = state;
  const { tone, text } = summary(state);
  const name = engineName(engine);
  const ok = status?.state === "ok";
  return (
    <>
      <div className="setting-row">
        <label htmlFor="engine-select">Engine</label>
        <select
          id="engine-select"
          value={engine?.engine ?? "docker"}
          disabled={engine === null || busy || starting !== null}
          title={busy ? "Bitte warten, bis Prüfung bzw. KI fertig sind" : undefined}
          onChange={(e) => onEngineChange(e.target.value as Engine)}
        >
          {(Object.keys(ENGINE_NAMES) as Engine[]).map((id) => (
            <option key={id} value={id}>
              {ENGINE_NAMES[id]}
            </option>
          ))}
        </select>
      </div>
      <p className="setting-hint muted">
        Die gewählte Engine wird beim Start der App automatisch gestartet. Code wird lokal in isolierten
        Containern ausgeführt.
      </p>
      <div className="setting-status" role="status">
        <span className={`dot tone-${tone}`} aria-hidden="true" />
        <span>{text}</span>
      </div>
      {startError && status?.state !== "ok" && (
        <p className="note error">
          <strong>Start fehlgeschlagen:</strong> {startError}
        </p>
      )}
      {ok && status.warning && <p className="note warn">{status.warning}</p>}
      <div className="row">
        {starting === null && status?.state === "notRunning" && <button onClick={onStart}>{name} starten</button>}
        {starting === null && status !== null && !ok && <button onClick={onRetry}>Erneut prüfen</button>}
        <button
          onClick={onUpdateImages}
          disabled={!ok || busy || images.busy !== null}
          title={
            ok
              ? "Lädt die Runner-Images dieser Version neu von ghcr.io (Internet nötig)"
              : "Erst verfügbar, wenn die Container-Engine läuft"
          }
        >
          Images aktualisieren
        </button>
      </div>
      {images.busy !== null && <p className="note busy">{images.busy}</p>}
      {images.busy === null && images.note && (
        <p className={images.note.error ? "note warn" : "note ok"}>{images.note.text}</p>
      )}
      <p className="setting-hint muted">
        Noch nicht installiert? Download: <DownloadLinks />. Docker Desktop ist kostenlos für private
        Nutzung/Bildung – bitte Lizenzbedingungen selbst prüfen.
      </p>
    </>
  );
}
