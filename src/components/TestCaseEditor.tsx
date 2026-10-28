import type { CompareMode, TestCase } from "../api";
import { CompareModeSelect } from "./CompareModeSelect";

interface Props {
  tests: TestCase[];
  onChange: (tests: TestCase[]) => void;
  /** null while the settings are invalid (e.g. tolerance). */
  compareMode: CompareMode | null;
  onCompareModeChange: (mode: CompareMode | null) => void;
}

/** Same limit as MAX_TESTS in src-tauri/src/runner/mod.rs. */
const MAX_TESTS = 50;

let nextId = 1000;
export const newTestCase = (n: number): TestCase => ({
  id: nextId++,
  name: `Test ${n}`,
  input: "",
  expected: "",
});

export function TestCaseEditor({ tests, onChange, compareMode, onCompareModeChange }: Props) {
  const update = (id: number, patch: Partial<TestCase>) =>
    onChange(tests.map((t) => (t.id === id ? { ...t, ...patch } : t)));

  return (
    <section className="card">
      <div className="card-header">
        <h2>Testfälle</h2>
        <CompareModeSelect value={compareMode} onChange={onCompareModeChange} />
      </div>
      {tests.length === 0 && <p className="muted">Noch keine Testfälle angelegt.</p>}
      {tests.map((t) => (
        <div key={t.id} className="testcase">
          <div className="row">
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
