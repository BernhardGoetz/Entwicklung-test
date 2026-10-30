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
    <li className={`result ${t.status}`}>
      <div className="result-head">
        <span className={`status ${t.status}`}>{LABEL[t.status]}</span>
        <strong className="result-name">{t.name}</strong>
        {t.output && <span className="muted small">{t.output.durationMs} ms</span>}
      </div>
      {t.message && <p className="small">{t.message}</p>}
      {t.onlyTrailingWhitespaceDiffers && (
        <p className="note warn small">Unterschied nur am Zeilenende (Leerzeichen/Zeilenumbruch)</p>
      )}
      <DiffView diff={t.diff} />
      {t.output && <StepOutputs out={t.output} />}
    </li>
  );
}

export function ResultsPanel({ report }: { report: RunReport }) {
  const allPassed = report.compileOk && report.passed === report.total && report.total > 0;
  return (
    <div className="results">
      {!report.compileOk ? (
        <div className="result failed">
          <div className="result-head">
            <span className="status failed">Kompilierfehler</span>
            {report.compile?.timedOut && <span>Zeitlimit beim Kompilieren überschritten.</span>}
          </div>
          {report.hint && <p className="note warn">{report.hint}</p>}
          {report.compile && <StepOutputs out={report.compile} />}
        </div>
      ) : (
        <>
          <p className={allPassed ? "summary ok" : "summary fail"}>
            {report.passed} von {report.total} Tests bestanden
          </p>
          <ol className="result-list">
            {report.tests.map((t) => (
              <TestRow key={t.index} t={t} />
            ))}
          </ol>
        </>
      )}
      <p className="muted small">Image: {report.image}</p>
    </div>
  );
}
