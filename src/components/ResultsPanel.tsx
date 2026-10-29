import type { RunReport, StepOutput, TestResult, TestStatus } from "../api";
import { DiffView } from "./DiffView";
import { VisibleWhitespace } from "./VisibleWhitespace";

const LABEL: Record<TestStatus, string> = {
  passed: "Bestanden",
  failed: "Falsche Ausgabe",
  timeout: "Zeitüberschreitung",
  runtimeError: "Laufzeitfehler",
  error: "Fehler",
};

function Output({ title, text, truncated }: { title: string; text: string; truncated: boolean }) {
  if (!text) return null;
  return (
    <details open={title === "stderr"}>
      <summary>
        {title}
        {truncated && " (nach 64 KB abgeschnitten)"}
      </summary>
      {/* stdout: trailing spaces and the final line break made visible. */}
      <pre className="output">{title === "stdout" ? <VisibleWhitespace text={text} /> : text}</pre>
    </details>
  );
}

function StepOutputs({ out }: { out: StepOutput }) {
  return (
    <>
      <Output title="stdout" text={out.stdout} truncated={out.stdoutTruncated} />
      <Output title="stderr" text={out.stderr} truncated={out.stderrTruncated} />
    </>
  );
}

function TestRow({ t }: { t: TestResult }) {
  return (
    <div className={`result ${t.status}`}>
      <div className="row">
        <span className={`badge ${t.status}`}>{LABEL[t.status]}</span>
        <strong>{t.name}</strong>
        {t.output && <span className="muted small">{t.output.durationMs} ms</span>}
      </div>
      {t.message && <p className="small">{t.message}</p>}
      {t.onlyTrailingWhitespaceDiffers && (
        <p className="hint small">Unterschied nur am Zeilenende (Leerzeichen/Zeilenumbruch)</p>
      )}
      <DiffView diff={t.diff} />
      {t.output && <StepOutputs out={t.output} />}
    </div>
  );
}

export function ResultsPanel({ report }: { report: RunReport }) {
  const allPassed = report.compileOk && report.passed === report.total && report.total > 0;
  return (
    <section className="card">
      <h2>Ergebnis</h2>
      {!report.compileOk ? (
        <div className="result failed">
          <div className="row">
            <span className="badge failed">Kompilierfehler</span>
            {report.compile?.timedOut && <span>Zeitlimit beim Kompilieren überschritten.</span>}
          </div>
          {report.hint && <p className="hint">{report.hint}</p>}
          {report.compile && <StepOutputs out={report.compile} />}
        </div>
      ) : (
        <>
          <p className={allPassed ? "summary ok" : "summary"}>
            {report.passed} von {report.total} Tests bestanden
          </p>
          {report.tests.map((t) => (
            <TestRow key={t.index} t={t} />
          ))}
        </>
      )}
      <p className="muted small">Image: {report.image}</p>
    </section>
  );
}
