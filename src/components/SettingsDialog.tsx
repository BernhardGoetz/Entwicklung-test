import type { RefObject } from "react";
import type { AiStatus, Engine } from "../api";
import type { ThemePref } from "../theme";
import { EngineSettings, type EngineState } from "./DockerBanner";
import { Modal } from "./Modal";
import { ThemeToggle } from "./ThemeToggle";

interface Props {
  onClose: () => void;
  returnFocus: RefObject<HTMLElement | null>;
  themePref: ThemePref;
  onThemeChange: (p: ThemePref) => void;
  engine: EngineState;
  /** A check or AI run is in progress. */
  busy: boolean;
  onEngineChange: (e: Engine) => void;
  onStart: () => void;
  onRetry: () => void;
  onUpdateImages: () => void;
  ai: AiStatus | null;
  /** AI generation running: the model cannot be changed. */
  aiBusy: boolean;
  onOpenModelSetup: () => void;
}

function modelText(ai: AiStatus | null): string {
  if (ai === null) return "Status wird geladen …";
  const m = ai.modelReady ? ai.models.find((x) => x.id === ai.selected) : undefined;
  return m ? `Ausgewählt: ${m.name}` : "kein Modell eingerichtet";
}

/** „Einstellungen“ behind the gear: appearance, container engine, AI model. */
export function SettingsDialog(p: Props) {
  const modelReady = p.ai?.modelReady ?? false;
  return (
    <Modal title="Einstellungen" onClose={p.onClose} returnFocus={p.returnFocus} className="settings">
      <section className="settings-section" aria-labelledby="set-look">
        <h3 id="set-look">Darstellung</h3>
        <div className="setting-row">
          <span id="set-theme-label">Farbschema</span>
          <ThemeToggle value={p.themePref} onChange={p.onThemeChange} labelledBy="set-theme-label" />
        </div>
        <p className="setting-hint muted">„System“ folgt der Einstellung des Betriebssystems.</p>
      </section>
      <section className="settings-section" aria-labelledby="set-engine">
        <h3 id="set-engine">Container-Engine</h3>
        <EngineSettings
          state={p.engine}
          busy={p.busy}
          onEngineChange={p.onEngineChange}
          onStart={p.onStart}
          onRetry={p.onRetry}
          onUpdateImages={p.onUpdateImages}
        />
      </section>
      <section className="settings-section" aria-labelledby="set-ai">
        <h3 id="set-ai">KI-Modell</h3>
        <div className="setting-status">
          <span className={`dot tone-${modelReady ? "ok" : "neutral"}`} aria-hidden="true" />
          <span>{modelText(p.ai)}</span>
        </div>
        <p className="setting-hint muted">
          Läuft vollständig auf diesem Computer; nur der einmalige Download braucht Internet.
        </p>
        <div className="row">
          <button
            onClick={p.onOpenModelSetup}
            disabled={p.aiBusy}
            className={modelReady ? undefined : "attention"}
            title={p.aiBusy ? "Bitte warten, bis die KI fertig ist" : undefined}
          >
            KI-Modell verwalten …
          </button>
        </div>
      </section>
    </Modal>
  );
}
