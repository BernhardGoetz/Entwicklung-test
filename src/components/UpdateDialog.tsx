import { useState, type RefObject } from "react";
import { formatMb, installUpdate, type InstallProgress, type UpdateOffer } from "../api";
import { Modal } from "./Modal";

interface Props {
  offer: UpdateOffer;
  /** Why installing must wait (a check or AI job is running), or null. */
  busyReason: string | null;
  /** „Später“ / „Schließen“ / Esc: asks again at the next due check. */
  onLater: () => void;
  returnFocus?: RefObject<HTMLElement | null>;
}

type Phase = { kind: "ask" } | { kind: "working"; progress: InstallProgress | null } | { kind: "error"; message: string };

const isWindows = typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);

function progressLine(p: InstallProgress | null): { text: string; percent: number | null } {
  if (p === null) return { text: "Download wird gestartet …", percent: null };
  if (p.kind === "installing")
    return {
      text: isWindows
        ? "Signatur geprüft. Der Installer startet, danach startet der Code-Prüfer neu …"
        : "Signatur geprüft. Die neue Version wird installiert, danach startet der Code-Prüfer neu …",
      percent: 100,
    };
  if (p.total !== null && p.total > 0) {
    const percent = Math.min(100, Math.round((p.downloaded / p.total) * 100));
    return { text: `Wird heruntergeladen … ${percent} % (${formatMb(p.downloaded)} von ${formatMb(p.total)})`, percent };
  }
  return { text: `Wird heruntergeladen … ${formatMb(p.downloaded)}`, percent: null };
}

/**
 * „Update verfügbar“: version, release notes (plain text), „Jetzt
 * aktualisieren“ (download with progress → signature check → install →
 * restart) or „Später“. Cannot be closed while the update is installed.
 */
export function UpdateDialog({ offer, busyReason, onLater, returnFocus }: Props) {
  const [phase, setPhase] = useState<Phase>({ kind: "ask" });
  const working = phase.kind === "working";

  const install = async () => {
    setPhase({ kind: "working", progress: null });
    try {
      await installUpdate((p) => setPhase({ kind: "working", progress: p }));
      // Normally not reached: the app restarts.
    } catch (e) {
      setPhase({ kind: "error", message: String(e) });
    }
  };

  const line = working ? progressLine(phase.progress) : null;
  return (
    <Modal
      title={`Update verfügbar: Version ${offer.version} (installiert: ${offer.currentVersion})`}
      onClose={onLater}
      canClose={!working}
      closeTitle="Bitte warten, bis das Update installiert ist"
      returnFocus={returnFocus}
      className="update"
    >
      <section aria-labelledby="update-notes-heading" className="update-section">
        <h3 id="update-notes-heading">Neu in Version {offer.version}</h3>
        {/* Plain text only: React escapes it, no HTML/Markdown is interpreted. */}
        {offer.notes ? (
          <div className="update-notes" tabIndex={0} aria-label="Versionshinweise">
            {offer.notes}
          </div>
        ) : (
          <p className="muted">Für diese Version gibt es keine Versionshinweise.</p>
        )}
      </section>
      <p className="setting-hint muted">
        Das Update wird von GitHub geladen und vor der Installation auf seine Signatur geprüft. Danach startet der
        Code-Prüfer neu{isWindows ? "; der Installer läuft ohne Rückfragen und zeigt nur den Fortschritt" : ""}.
        Nicht gespeicherte Eingaben (Code, Testfälle) gehen dabei verloren.
      </p>
      {busyReason !== null && !working && (
        <p className="note warn">{busyReason} Bitte warten, bis sie fertig ist – danach kann das Update installiert werden.</p>
      )}
      {line !== null && (
        <div className="ai-progress" role="status" aria-live="polite">
          <span>{line.text}</span>
          {line.percent !== null && <progress max={100} value={line.percent} aria-label="Fortschritt des Updates" />}
        </div>
      )}
      {phase.kind === "error" && <p className="note error">{phase.message}</p>}
      <div className="row update-actions">
        <button onClick={onLater} disabled={working}>
          Später
        </button>
        <button
          className="primary"
          onClick={() => void install()}
          disabled={working || busyReason !== null}
          title={busyReason ?? undefined}
        >
          {phase.kind === "error" ? "Erneut versuchen" : working ? "Wird aktualisiert …" : "Jetzt aktualisieren"}
        </button>
      </div>
    </Modal>
  );
}
