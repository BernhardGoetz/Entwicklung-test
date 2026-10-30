import { useEffect, useRef, useState } from "react";
import {
  CANCELLED,
  cancelAi,
  generateTests,
  onAiProgress,
  type AiProgress,
  type AiStatus,
  type CompareMode,
  type GenerateResult,
} from "../api";

interface Props {
  status: AiStatus | null;
  /** German reason why generating is not possible now, or null. */
  blockedReason: string | null;
  taskText: string;
  language: string;
  compareMode: CompareMode | null;
  /** Generated tests are appended by the parent. */
  onResult: (r: GenerateResult) => void;
  onApplyMode: (mode: CompareMode) => void;
  onOpenSetup: () => void;
  onRunningChange: (running: boolean) => void;
  /** Called when a generation ended: the backend may have started the engine (or found it gone). */
  onFinished: (failed: boolean) => void;
}

const LEVEL_TEXT = { green: "Grün", yellow: "Gelb", red: "Rot" } as const;
const LEVEL_HINT = {
  green: "Hohe Konfidenz: Beispiele des Blatts bestanden, Lösungen einig.",
  yellow: "Nur ein Hinweis: erwartete Ausgaben bitte prüfen.",
  red: "Keine verlässlichen Testfälle möglich.",
} as const;

export function modeName(m: CompareMode): string {
  if (m.kind === "lines") return "Zeilen (exakt)";
  if (m.kind === "tokens") return "Wörter (Leerraum egal)";
  return "Zahlen mit Toleranz";
}

function sameMode(a: CompareMode | null, b: CompareMode | null): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

function progressLine(p: AiProgress | null, seconds: number): string {
  if (!p) return `KI wird gestartet … (${seconds} s)`;
  const parts = [p.message];
  if (p.tokensPerSecond !== null) parts.push(`${p.tokensPerSecond.toFixed(1).replace(".", ",")} Token/s`);
  if (p.etaSeconds !== null && p.etaSeconds > 0) parts.push(`noch ca. ${formatDuration(p.etaSeconds)}`);
  parts.push(`${formatDuration(seconds)} vergangen`);
  return parts.join(" · ");
}

function formatDuration(s: number): string {
  if (s < 60) return `${s} s`;
  return `${Math.floor(s / 60)} min ${s % 60} s`;
}

/** "Testfälle mit KI erzeugen": progress, cancel, confidence, reference solution. */
export function AiPanel({
  status,
  blockedReason,
  taskText,
  language,
  compareMode,
  onResult,
  onApplyMode,
  onOpenSetup,
  onRunningChange,
  onFinished,
}: Props) {
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<AiProgress | null>(null);
  const [seconds, setSeconds] = useState(0);
  const [result, setResult] = useState<GenerateResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cancelled, setCancelled] = useState(false);
  const [modeApplied, setModeApplied] = useState(false);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    const unlisten = onAiProgress(setProgress);
    return () => {
      void unlisten.then((f) => f());
      if (timer.current !== null) window.clearInterval(timer.current);
    };
  }, []);

  const generate = async () => {
    setRunning(true);
    onRunningChange(true);
    setError(null);
    setCancelled(false);
    setResult(null);
    setModeApplied(false);
    setProgress(null);
    setSeconds(0);
    const started = Date.now();
    timer.current = window.setInterval(() => setSeconds(Math.round((Date.now() - started) / 1000)), 1000);
    let failed = false;
    try {
      const r = await generateTests({
        taskText,
        language,
        compareMode: compareMode ?? undefined,
      });
      setResult(r);
      onResult(r);
    } catch (e) {
      if (String(e) === CANCELLED) setCancelled(true);
      else {
        setError(String(e));
        failed = true;
      }
    } finally {
      if (timer.current !== null) window.clearInterval(timer.current);
      timer.current = null;
      setRunning(false);
      onRunningChange(false);
      onFinished(failed);
    }
  };

  const modelReady = status?.modelReady ?? false;
  const suggestion = result?.suggestedCompareMode ?? null;
  const showSuggestion =
    suggestion !== null && !modeApplied && !sameMode(suggestion, compareMode) && result?.confidence.level !== "red";

  return (
    <section className="card ai-panel" aria-labelledby="ai-heading">
      <div className="card-header">
        <h2 id="ai-heading">Testfälle mit KI</h2>
        {!modelReady && (
          <button onClick={onOpenSetup} disabled={running} className="attention">
            KI-Modell einrichten …
          </button>
        )}
      </div>
      <div className="toolbar wrap">
        <button
          onClick={() => void generate()}
          disabled={running || blockedReason !== null}
          title={blockedReason ?? "Erzeugt Testfälle aus der Aufgabenstellung (lokal, ohne Internet)"}
        >
          {running ? "KI arbeitet …" : "Testfälle mit KI erzeugen"}
        </button>
        {running && <button onClick={() => void cancelAi()}>Abbrechen</button>}
        {!running && blockedReason && <span className="muted small">{blockedReason}</span>}
      </div>
      {running && (
        <div className="ai-progress small">
          <span>{progressLine(progress, seconds)}</span>
          {progress?.current != null && progress.total != null && (
            <progress max={progress.total} value={progress.current} />
          )}
        </div>
      )}
      {!running && !result && !error && !cancelled && (
        <p className="muted small">
          Die KI schreibt mehrere Lösungen und Testeingaben; die erwarteten Ausgaben entstehen durch
          Ausführen der Lösungen, die die Beispiele des Blatts bestehen. Je nach Rechner dauert das
          1–10 Minuten.
        </p>
      )}
      {cancelled && <p className="muted small">Abgebrochen.</p>}
      {error && <div className="banner error small">{error}</div>}
      {result && (
        <div className="ai-result">
          <div className="row">
            <span className={`status conf-${result.confidence.level}`}>
              Konfidenz: {LEVEL_TEXT[result.confidence.level]}
            </span>
            <span className="small muted">{LEVEL_HINT[result.confidence.level]}</span>
          </div>
          <ul className="small reasons">
            {result.confidence.reasons.map((r) => (
              <li key={r}>{r}</li>
            ))}
          </ul>
          <p className="small muted">
            {result.tests.length} Testfälle übernommen (mit „KI“ markiert) ·{" "}
            {result.stats.solutionsPassed} von {result.stats.solutionsRequested} Lösungen bestanden ·{" "}
            {result.stats.examplesFound} Beispiel(e) aus dem Blatt · {Math.round(result.stats.durationMs / 1000)} s
            {result.stats.tokensPerSecond !== null &&
              ` · ${result.stats.tokensPerSecond.toFixed(1).replace(".", ",")} Token/s`}
          </p>
          {showSuggestion && suggestion && (
            <div className="row small">
              <span>
                Die KI schlägt den Vergleich „{modeName(suggestion)}“ vor (aktuell:{" "}
                {compareMode ? `„${modeName(compareMode)}“` : "ungültig"}).
              </span>
              <button
                onClick={() => {
                  onApplyMode(suggestion);
                  setModeApplied(true);
                }}
              >
                Vorschlag übernehmen
              </button>
            </div>
          )}
          {result.referenceSolution && (
            <details>
              <summary>Musterlösung anzeigen</summary>
              <pre className="output">{result.referenceSolution}</pre>
            </details>
          )}
        </div>
      )}
    </section>
  );
}
