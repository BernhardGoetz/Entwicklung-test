import { useEffect, useRef, useState } from "react";
import {
  CANCELLED,
  applyOcrFixes,
  cancelAi,
  onOcrFixProgress,
  suggestOcrFixes,
  type AiProgress,
  type AiStatus,
  type OcrFix,
} from "../api";

interface Props {
  /** Current task text. */
  text: string;
  /** Text as recognized by the OCR (kept for "wiederherstellen"). */
  original: string;
  /** Sets a new text and resets "Text geprüft und korrigiert". */
  onTextChange: (text: string) => void;
  ai: AiStatus | null;
  /** Test generation is running (shares the model). */
  aiBusy: boolean;
  onOpenSetup: () => void;
  onRunningChange: (running: boolean) => void;
}

type FixState = "open" | "accepted" | "discarded";

interface Suggestions {
  /** Text the fixes refer to. */
  base: string;
  fixes: OcrFix[];
  states: FixState[];
  rejected: number;
}

/** Splits old/new into common prefix, changed middle and common suffix. */
function splitChange(a: string, b: string): [string, string, string, string] {
  let p = 0;
  while (p < a.length && p < b.length && a[p] === b[p]) p++;
  let s = 0;
  while (s < a.length - p && s < b.length - p && a[a.length - 1 - s] === b[b.length - 1 - s]) s++;
  return [a.slice(0, p), a.slice(p, a.length - s), b.slice(p, b.length - s), a.slice(a.length - s)];
}

function FixDiff({ fix }: { fix: OcrFix }) {
  const [prefix, removed, added, suffix] = splitChange(fix.old, fix.new);
  return (
    <code className="fix-diff">
      <span className="fix-old">
        {prefix}
        <mark>{removed}</mark>
        {suffix}
      </span>
      {" → "}
      <span className="fix-new">
        {prefix}
        <mark>{added}</mark>
        {suffix}
      </span>
    </code>
  );
}

/** "Erkennungsfehler mit KI korrigieren": suggestions from the local model, each to accept or discard. */
export function OcrFixPanel({ text, original, onTextChange, ai, aiBusy, onOpenSetup, onRunningChange }: Props) {
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<AiProgress | null>(null);
  const [seconds, setSeconds] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [cancelled, setCancelled] = useState(false);
  const [suggestions, setSuggestions] = useState<Suggestions | null>(null);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    const unlisten = onOcrFixProgress(setProgress);
    return () => {
      void unlisten.then((f) => f());
      if (timer.current !== null) window.clearInterval(timer.current);
    };
  }, []);

  const textFor = (s: Suggestions, states: FixState[]) =>
    applyOcrFixes(
      s.base,
      s.fixes.filter((_, i) => states[i] === "accepted"),
    );

  // Edited by hand (or newly imported): the suggestions no longer fit.
  useEffect(() => {
    if (suggestions && text !== textFor(suggestions, suggestions.states)) setSuggestions(null);
  }, [text, suggestions]);

  const run = async () => {
    setRunning(true);
    onRunningChange(true);
    setError(null);
    setCancelled(false);
    setSuggestions(null);
    setProgress(null);
    setSeconds(0);
    const started = Date.now();
    timer.current = window.setInterval(() => setSeconds(Math.round((Date.now() - started) / 1000)), 1000);
    const base = text;
    try {
      const r = await suggestOcrFixes(base);
      setSuggestions({ base, fixes: r.fixes, states: r.fixes.map(() => "open"), rejected: r.rejected });
    } catch (e) {
      if (String(e) === CANCELLED) setCancelled(true);
      else setError(String(e));
    } finally {
      if (timer.current !== null) window.clearInterval(timer.current);
      timer.current = null;
      setRunning(false);
      onRunningChange(false);
    }
  };

  const setStates = (update: (states: FixState[]) => FixState[]) => {
    if (!suggestions) return;
    const states = update(suggestions.states);
    const next = { ...suggestions, states };
    const newText = textFor(next, states);
    setSuggestions(next);
    if (newText !== text) onTextChange(newText);
  };
  const setOne = (i: number, state: FixState) => setStates((s) => s.map((v, j) => (j === i ? state : v)));

  const restore = () => {
    setSuggestions(null);
    onTextChange(original);
  };

  const ready = ai !== null && ai.runtimeAvailable && ai.modelReady;
  const blocked = (() => {
    if (ai === null) return "KI-Status wird geladen …";
    if (!ai.runtimeAvailable) return ai.runtimeError ?? "KI-Laufzeit nicht gefunden.";
    if (aiBusy) return "Bitte warten, bis die KI-Testfälle fertig sind.";
    if (text.trim() === "") return "Es gibt keinen Text zum Korrigieren.";
    return null;
  })();
  const open = suggestions?.states.filter((s) => s === "open").length ?? 0;

  return (
    <div className="ocr-fix small">
      <div className="row">
        {ai !== null && ai.runtimeAvailable && !ai.modelReady ? (
          <>
            <span className="muted">Für „Erkennungsfehler mit KI korrigieren“ wird ein KI-Modell benötigt.</span>
            <button onClick={onOpenSetup} disabled={aiBusy}>
              KI-Modell einrichten …
            </button>
          </>
        ) : (
          <button
            onClick={() => void run()}
            disabled={running || blocked !== null || !ready}
            title={
              blocked ??
              "Die lokale KI schlägt Korrekturen typischer Erkennungsfehler vor (z. B. 1/l, 0/O, fehlende Anführungszeichen). Nichts wird automatisch geändert."
            }
          >
            {running ? "KI sucht Erkennungsfehler …" : "Erkennungsfehler mit KI korrigieren"}
          </button>
        )}
        {running && <button onClick={() => void cancelAi()}>Abbrechen</button>}
        {!running && text !== original && (
          <button onClick={restore} title="Setzt den Text auf das Ergebnis der Texterkennung zurück">
            Ursprünglichen Text wiederherstellen
          </button>
        )}
        {!running && blocked && ready && <span className="muted">{blocked}</span>}
      </div>
      {running && (
        <div className="ai-progress">
          <span>
            {progress ? progress.message : "KI wird gestartet …"} · {seconds} s
          </span>
        </div>
      )}
      {cancelled && <p className="muted">Abgebrochen.</p>}
      {error && <div className="banner error">{error}</div>}
      {suggestions && (
        <div className="fix-list">
          {suggestions.fixes.length === 0 ? (
            <p className="muted">Die KI hat keine plausiblen Erkennungsfehler gefunden.</p>
          ) : (
            <>
              <div className="row">
                <span>
                  {suggestions.fixes.length} {suggestions.fixes.length === 1 ? "Vorschlag" : "Vorschläge"} – bitte
                  mit dem Original vergleichen:
                </span>
                {open > 0 && (
                  <button onClick={() => setStates((s) => s.map((v) => (v === "open" ? "accepted" : v)))}>
                    Alle übernehmen
                  </button>
                )}
              </div>
              <ul className="fixes">
                {suggestions.fixes.map((f, i) => {
                  const state = suggestions.states[i];
                  return (
                    <li key={`${f.line}:${f.start}`} className={`fix ${state}`}>
                      <span className="muted">Zeile {f.line}:</span> <FixDiff fix={f} />
                      {f.reason && <span className="muted"> ({f.reason})</span>}
                      <span className="fix-actions">
                        {state === "open" ? (
                          <>
                            <button onClick={() => setOne(i, "accepted")}>Übernehmen</button>
                            <button onClick={() => setOne(i, "discarded")}>Verwerfen</button>
                          </>
                        ) : (
                          <>
                            <span className={state === "accepted" ? "ok-text" : "muted"}>
                              {state === "accepted" ? "übernommen" : "verworfen"}
                            </span>
                            <button onClick={() => setOne(i, "open")}>Rückgängig</button>
                          </>
                        )}
                      </span>
                    </li>
                  );
                })}
              </ul>
            </>
          )}
          {suggestions.rejected > 0 && (
            <p className="muted">
              {suggestions.rejected}{" "}
              {suggestions.rejected === 1 ? "weiterer Vorschlag wurde" : "weitere Vorschläge wurden"} als unplausibel
              verworfen (z. B. Umformulierungen oder neue Wörter).
            </p>
          )}
        </div>
      )}
      {text !== original && (
        <details>
          <summary>Ursprünglich erkannten Text anzeigen</summary>
          <pre className="output">{original}</pre>
        </details>
      )}
    </div>
  );
}
