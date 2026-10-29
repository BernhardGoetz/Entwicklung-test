import { useState } from "react";
import type { CompareMode, TestCase } from "../api";
import { CompareModeSelect } from "./CompareModeSelect";
import { endingStatus, endsWithNewline } from "./VisibleWhitespace";

interface Props {
  tests: TestCase[];
  onChange: (tests: TestCase[]) => void;
  /** null while the settings are invalid (e.g. tolerance). */
  compareMode: CompareMode | null;
  onCompareModeChange: (mode: CompareMode | null) => void;
  /** Changes when the mode is set from outside (AI suggestion). */
  compareModeKey?: number;
  /** „Zeilenumbruch am Ende verbieten“ (independent of the compare mode). */
  forbidTrailingNewline: boolean;
  onForbidTrailingNewlineChange: (forbid: boolean) => void;
}

/** Same limit as MAX_TESTS in src-tauri/src/runner/mod.rs. */
export const MAX_TESTS = 50;

let nextId = 1000;
export const newTestCase = (n: number): TestCase => ({
  id: nextId++,
  name: `Test ${n}`,
  input: "",
  expected: "",
});

export function TestCaseEditor({
  tests,
  onChange,
  compareMode,
  onCompareModeChange,
  compareModeKey,
  forbidTrailingNewline,
  onForbidTrailingNewlineChange,
}: Props) {
  const update = (id: number, patch: Partial<TestCase>) =>
    onChange(tests.map((t) => (t.id === id ? { ...t, ...patch } : t)));
  // AI expected outputs are the executed reference solution; if that ends
  // with a line break, the forbid option can make these tests fail. The
  // hint is shown once (again only when more such tests appear).
  const aiWithNewline = forbidTrailingNewline
    ? tests.filter((t) => t.origin === "ai" && endsWithNewline(t.expected)).length
    : 0;
  const [hintDismissedAt, setHintDismissedAt] = useState(0);

  return (
    <section className="card">
      <div className="card-header">
        <h2>Testfälle</h2>
        <div className="compare-mode small">
          <CompareModeSelect key={compareModeKey} value={compareMode} onChange={onCompareModeChange} />
          <label
            className="inline"
            title="Jede Ausgabe, die mit einem Zeilenumbruch endet (z. B. durch println oder print()), gilt als nicht bestanden – unabhängig vom Vergleich und von der erwarteten Ausgabe."
          >
            <input
              type="checkbox"
              checked={forbidTrailingNewline}
              onChange={(e) => onForbidTrailingNewlineChange(e.target.checked)}
            />{" "}
            Zeilenumbruch am Ende verbieten
          </label>
        </div>
      </div>
      {aiWithNewline > hintDismissedAt && (
        <div className="hint small">
          Die erwarteten Ausgaben der KI-Testfälle stammen aus der ausgeführten Musterlösung – und die
          endet selbst mit einem Zeilenumbruch ({aiWithNewline}{" "}
          {aiWithNewline === 1 ? "Testfall" : "Testfälle"}). Mit „Zeilenumbruch am Ende verbieten“
          können diese Tests deshalb fehlschlagen, bei „Zeilen (exakt)“ ohne Ignorieren des
          Zeilenendes sogar immer. Bitte die erwartete Ausgabe prüfen.{" "}
          <button onClick={() => setHintDismissedAt(aiWithNewline)}>Verstanden</button>
        </div>
      )}
      {tests.length === 0 && <p className="muted">Noch keine Testfälle angelegt.</p>}
      {tests.map((t) => (
        <div key={t.id} className="testcase">
          <div className="row">
            {t.origin === "ai" && (
              <span className="badge ai" title="Von der KI erzeugt – bitte prüfen">
                KI
              </span>
            )}
            <input
              className="test-name"
              value={t.name}
              onChange={(e) => update(t.id, { name: e.target.value })}
              aria-label="Name des Testfalls"
            />
            <button
              className="danger"
              onClick={() => onChange(tests.filter((x) => x.id !== t.id))}
              title="Testfall entfernen"
            >
              Entfernen
            </button>
          </div>
          <div className="io">
            <label>
              Eingabe (stdin)
              <textarea
                rows={4}
                value={t.input}
                spellCheck={false}
                onChange={(e) => update(t.id, { input: e.target.value })}
              />
            </label>
            <label>
              Erwartete Ausgabe (stdout)
              <textarea
                rows={4}
                value={t.expected}
                spellCheck={false}
                onChange={(e) => update(t.id, { expected: e.target.value })}
              />
              <span
                className={
                  forbidTrailingNewline && endsWithNewline(t.expected)
                    ? "field-status warn-text"
                    : "field-status"
                }
              >
                {endingStatus(t.expected)}
              </span>
            </label>
          </div>
        </div>
      ))}
      <button
        onClick={() => onChange([...tests, newTestCase(tests.length + 1)])}
        disabled={tests.length >= MAX_TESTS}
        title={tests.length >= MAX_TESTS ? `Höchstens ${MAX_TESTS} Testfälle pro Prüfung` : undefined}
      >
        + Testfall hinzufügen
      </button>
    </section>
  );
}
