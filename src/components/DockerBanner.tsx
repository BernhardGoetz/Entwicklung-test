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
    <label className="engine-switch" title="Wird beim Start der App automatisch gestartet">
      Container-Engine:
      <select
        value={engine.engine}
        disabled={disabled}
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

interface Props {
  status: DockerStatus | null;
  engine: EngineChoice | null;
  /** Status message while the engine is being started, otherwise null. */
  starting: string | null;
  startError: string | null;
  busy: boolean;
  onRetry: () => void;
  onStart: () => void;
  onEngineChange: (e: Engine) => void;
}

export function DockerBanner({
  status,
  engine,
  starting,
  startError,
  busy,
  onRetry,
  onStart,
  onEngineChange,
}: Props) {
  const name = engine ? ENGINE_NAMES[engine.engine] : "Docker/Podman";
  const engineSwitch = (
    <EngineSwitch engine={engine} onChange={onEngineChange} disabled={busy || starting !== null} />
  );

  if (starting !== null) {
    return (
      <div className="banner info banner-row">
        <span>⏳ {starting} Bitte warten – das kann bis zu zwei Minuten dauern.</span>
      </div>
    );
  }
  if (status === null) {
    return <div className="banner info">Suche Container-Engine …</div>;
  }
  if (status.state === "ok") {
    return (
      <div className="banner ok banner-row">
        <span>
          ✓ {status.product} {status.version} läuft – Code wird lokal in isolierten Containern
          ausgeführt.
        </span>
        {engineSwitch}
      </div>
    );
  }
  return (
    <div className="banner warn">
      {startError && (
        <p>
          <strong>Start fehlgeschlagen:</strong> {startError}
        </p>
      )}
      {status.state === "notRunning" ? (
        <p>
          <strong>{name} ist installiert, aber nicht gestartet.</strong> Die App startet die
          gewählte Engine automatisch; alternativ {name} selbst starten und dann erneut prüfen.
        </p>
      ) : (
        <p>
          <strong>Keine Container-Engine gefunden.</strong> Der Code-Prüfer benötigt{" "}
          <ExtLink href={DOWNLOAD.docker}>Docker Desktop</ExtLink> oder{" "}
          <ExtLink href={DOWNLOAD.podman}>Podman</ExtLink>. Docker Desktop ist kostenlos für
          private Nutzung/Bildung – bitte Lizenzbedingungen selbst prüfen.
        </p>
      )}
      <div className="banner-row">
        <span className="banner-actions">
          {status.state === "notRunning" && <button onClick={onStart}>{name} starten</button>}
          <button onClick={onRetry}>Erneut prüfen</button>
        </span>
        {engineSwitch}
      </div>
    </div>
  );
}
