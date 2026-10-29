import type { ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ENGINE_NAMES, type DockerStatus, type Engine, type EngineChoice } from "../api";

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

function EngineSwitch({
  engine,
  onChange,
  disabled,
}: {
  engine: EngineChoice | null;
  onChange: (e: Engine) => void;
  disabled: boolean;
}) {
  if (!engine) return null;
  return (
    <label className="inline engine-switch" title="Wird beim Start der App automatisch gestartet">
      <span className="hide-narrow muted">Engine</span>
      <select
        value={engine.engine}
        disabled={disabled}
        aria-label="Container-Engine"
        onChange={(e) => onChange(e.target.value as Engine)}
      >
        {(Object.keys(ENGINE_NAMES) as Engine[]).map((id) => (
          <option key={id} value={id}>
            {ENGINE_NAMES[id]}
          </option>
        ))}
      </select>
    </label>
  );
}

export interface ImagesState {
  /** Progress text while images are prepared/updated, otherwise null. */
  busy: string | null;
  note: { text: string; error: boolean } | null;
}

interface Props {
  status: DockerStatus | null;
  images: ImagesState;
  onUpdateImages: () => void;
  engine: EngineChoice | null;
  /** Status message while the engine is being started, otherwise null. */
  starting: string | null;
  startError: string | null;
  busy: boolean;
  onRetry: () => void;
  onStart: () => void;
  onEngineChange: (e: Engine) => void;
  /** Further header actions (theme, AI model), shown after the engine actions. */
  trailing?: ReactNode;
}

type Tone = "ok" | "warn" | "error" | "busy" | "neutral";

/**
 * Engine state for the header: a compact status (dot + text), the engine
 * actions and – full width below – notices that need more room.
 */
export function DockerBanner({
  status,
  images,
  onUpdateImages,
  engine,
  starting,
  startError,
  busy,
  onRetry,
  onStart,
  onEngineChange,
  trailing,
}: Props) {
  const name = engine ? ENGINE_NAMES[engine.engine] : "Docker/Podman";
  const engineSwitch = (
    <EngineSwitch engine={engine} onChange={onEngineChange} disabled={busy || starting !== null} />
  );

  let tone: Tone;
  let text: ReactNode;
  let actions: ReactNode = null;
  const notices: ReactNode[] = [];

  if (starting !== null) {
    tone = "busy";
    text = (
      <>
        {starting} <span className="muted hide-narrow">Bitte warten – das kann bis zu zwei Minuten dauern.</span>
      </>
    );
  } else if (status === null) {
    tone = "neutral";
    text = "Suche Container-Engine …";
  } else if (status.state === "ok") {
    tone = "ok";
    text = (
      <>
        {status.product} {status.version} läuft
        <span className="muted hide-md"> · Code wird lokal in isolierten Containern ausgeführt</span>
      </>
    );
    actions = (
      <button
        onClick={onUpdateImages}
        disabled={busy || images.busy !== null}
        title="Lädt die Runner-Images dieser Version neu von ghcr.io (Internet nötig)"
      >
        Images aktualisieren
      </button>
    );
    if (status.warning) notices.push(<p key="w" className="note warn">{status.warning}</p>);
    if (images.busy !== null) notices.push(<p key="ib" className="note busy">{images.busy}</p>);
    else if (images.note)
      notices.push(
        <p key="in" className={images.note.error ? "note warn" : "note ok"}>
          {images.note.text}
        </p>,
      );
  } else {
    if (startError)
      notices.push(
        <p key="se" className="note error">
          <strong>Start fehlgeschlagen:</strong> {startError}
        </p>,
      );
    if (status.state === "notRunning") {
      tone = "warn";
      text = `${name} ist nicht gestartet`;
      notices.push(
        <p key="nr" className="note warn">
          <strong>{name} ist installiert, aber nicht gestartet.</strong> Die App startet die gewählte
          Engine automatisch; alternativ {name} selbst starten und dann erneut prüfen.
        </p>,
      );
    } else {
      tone = "error";
      text = "Keine Container-Engine gefunden";
      notices.push(
        <p key="ni" className="note error">
          Der Code-Prüfer benötigt <ExtLink href={DOWNLOAD.docker}>Docker Desktop</ExtLink> oder{" "}
          <ExtLink href={DOWNLOAD.podman}>Podman</ExtLink>. Docker Desktop ist kostenlos für private
          Nutzung/Bildung – bitte Lizenzbedingungen selbst prüfen.
        </p>,
      );
    }
    actions = (
      <>
        {status.state === "notRunning" && <button onClick={onStart}>{name} starten</button>}
        <button onClick={onRetry}>Erneut prüfen</button>
      </>
    );
  }

  return (
    <>
      <div
        className="engine-status"
        role="status"
        title={status?.state === "ok" ? "Code wird lokal in isolierten Containern ausgeführt." : undefined}
      >
        <span className={`dot tone-${tone}`} aria-hidden="true" />
        <span className="engine-text">{text}</span>
      </div>
      <div className="appbar-actions">
        {actions}
        {engineSwitch}
        {trailing}
      </div>
      {notices.length > 0 && <div className="appbar-notices">{notices}</div>}
    </>
  );
}
