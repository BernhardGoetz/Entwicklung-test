import { openUrl } from "@tauri-apps/plugin-opener";
import type { DockerStatus } from "../api";

const DOCKER_DESKTOP = "https://www.docker.com/products/docker-desktop/";
const PODMAN_DESKTOP = "https://podman-desktop.io/";

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

interface Props {
  status: DockerStatus | null;
  onRetry: () => void;
}

export function DockerBanner({ status, onRetry }: Props) {
  if (status === null) {
    return <div className="banner info">Suche Container-Engine …</div>;
  }
  if (status.state === "ok") {
    return (
      <div className="banner ok">
        ✓ {status.product} {status.version} läuft – Code wird lokal in isolierten Containern ausgeführt.
      </div>
    );
  }
  return (
    <div className="banner warn">
      {status.state === "notRunning" ? (
        <p>
          <strong>Docker/Podman ist installiert, aber nicht gestartet.</strong> Bitte Docker Desktop
          bzw. Podman Desktop (oder <code>podman machine start</code>) starten und dann erneut prüfen.
        </p>
      ) : (
        <p>
          <strong>Keine Container-Engine gefunden.</strong> Der Code-Prüfer benötigt{" "}
          <ExtLink href={DOCKER_DESKTOP}>Docker Desktop</ExtLink> oder{" "}
          <ExtLink href={PODMAN_DESKTOP}>Podman Desktop</ExtLink>. Docker Desktop ist kostenlos für
          private Nutzung/Bildung – bitte Lizenzbedingungen selbst prüfen.
        </p>
      )}
      <button onClick={onRetry}>Erneut prüfen</button>
    </div>
  );
}
