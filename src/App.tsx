import { useCallback, useEffect, useRef, useState } from "react";
import {
  ENGINE_NAMES,
  dockerStatus,
  getEngine,
  listLanguages,
  onEngineProgress,
  onProgress,
  runTests,
  setEngine,
  startEngine,
  type DockerStatus,
  type Engine,
  type EngineChoice,
  type LanguageInfo,
  type Progress,
  type RunReport,
  type TestCase,
} from "./api";
import { CodeEditor } from "./components/CodeEditor";
import { DockerBanner } from "./components/DockerBanner";
import { LanguageSelect } from "./components/LanguageSelect";
import { ResultsPanel } from "./components/ResultsPanel";
import { TaskStatement } from "./components/TaskStatement";
import { TestCaseEditor, newTestCase } from "./components/TestCaseEditor";

function progressText(p: Progress | null): string {
  if (!p) return "Läuft …";
  if (p.kind === "phase") return p.message;
  const pct = p.percent !== null ? ` ${p.percent} %` : "";
  return `Lade Image ${p.image} herunter (einmalig):${pct} ${p.status}`;
}

export default function App() {
  const [docker, setDocker] = useState<DockerStatus | null>(null);
  const [engine, setEngineChoice] = useState<EngineChoice | null>(null);
  const [starting, setStarting] = useState<string | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
  // The engine is started automatically once per app start (and after a switch).
  const autoStartTried = useRef(false);
  const engineRef = useRef<EngineChoice | null>(null);
  const [languages, setLanguages] = useState<LanguageInfo[]>([]);
  const [languageId, setLanguageId] = useState("python");
  const [codeByLang, setCodeByLang] = useState<Record<string, string>>({});
  const [task, setTask] = useState("");
  const [tests, setTests] = useState<TestCase[]>([
    { ...newTestCase(1), input: "Welt\n", expected: "Hallo, Welt!\n" },
  ]);
  const [trimWhitespace, setTrimWhitespace] = useState(true);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [report, setReport] = useState<RunReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  const doStart = useCallback(async () => {
    const name = engineRef.current ? ENGINE_NAMES[engineRef.current.engine] : "Container-Engine";
    setStartError(null);
    setStarting(`${name} wird gestartet …`);
    try {
      setDocker(await startEngine());
    } catch (e) {
      setStartError(String(e));
      setDocker(await dockerStatus().catch((): DockerStatus => ({ state: "notInstalled" })));
    } finally {
      setStarting(null);
    }
  }, []);

  const refreshDocker = useCallback(() => {
    setDocker(null);
    dockerStatus()
      .then((s) => {
        setDocker(s);
        if (s.state === "notRunning" && !autoStartTried.current) {
          autoStartTried.current = true;
          void doStart();
        }
      })
      .catch(() => setDocker({ state: "notInstalled" }));
  }, [doStart]);

  const onEngineChange = async (e: Engine) => {
    try {
      const choice = await setEngine(e);
      engineRef.current = choice;
      setEngineChoice(choice);
      setStartError(null);
      autoStartTried.current = false;
      refreshDocker();
    } catch (err) {
      setStartError(String(err));
    }
  };

  useEffect(() => {
    getEngine()
      .then((choice) => {
        engineRef.current = choice;
        setEngineChoice(choice);
      })
      .catch(() => undefined)
      .finally(refreshDocker);
    listLanguages()
      .then((langs) => {
        setLanguages(langs);
        setCodeByLang(Object.fromEntries(langs.map((l) => [l.id, l.template])));
      })
      .catch((e) => setError(String(e)));
    const unlisten = onProgress(setProgress);
    // Only show engine messages while a start is in progress.
    const unlistenEngine = onEngineProgress((m) => setStarting((prev) => (prev === null ? null : m)));
    return () => {
      void unlisten.then((f) => f());
      void unlistenEngine.then((f) => f());
    };
  }, [refreshDocker]);

  const language = languages.find((l) => l.id === languageId);
  const code = codeByLang[languageId] ?? "";
  const canRun =
    docker?.state === "ok" && starting === null && !running && tests.length > 0 && code.trim() !== "";

  const onRun = async () => {
    setRunning(true);
    setError(null);
    setReport(null);
    setProgress(null);
    try {
      const r = await runTests({
        language: languageId,
        code,
        tests: tests.map(({ name, input, expected }) => ({ name, input, expected })),
        trimWhitespace,
      });
      setReport(r);
    } catch (e) {
      setError(String(e));
      refreshDocker();
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="app">
      <header>
        <h1>Code-Prüfer</h1>
        <span className="muted">Programmieraufgaben lokal und sicher testen</span>
      </header>
      <DockerBanner
        status={docker}
        engine={engine}
        starting={starting}
        startError={startError}
        busy={running}
        onRetry={refreshDocker}
        onStart={() => void doStart()}
        onEngineChange={(e) => void onEngineChange(e)}
      />
      <main>
        <div className="col">
          <TaskStatement value={task} onChange={setTask} />
          <div className="toolbar">
            <LanguageSelect
              languages={languages}
              value={languageId}
              onChange={setLanguageId}
              disabled={running}
            />
          </div>
          <CodeEditor
            language={language}
            code={code}
            onChange={(c) => setCodeByLang((m) => ({ ...m, [languageId]: c }))}
          />
        </div>
        <div className="col">
          <TestCaseEditor
            tests={tests}
            onChange={setTests}
            trimWhitespace={trimWhitespace}
            onTrimChange={setTrimWhitespace}
          />
          <div className="toolbar">
            <button className="primary" onClick={onRun} disabled={!canRun}>
              {running ? "Prüfe …" : "Prüfen"}
            </button>
            {running && <span className="muted">{progressText(progress)}</span>}
            {!running && docker?.state !== "ok" && (
              <span className="muted small">Zum Prüfen wird Docker oder Podman benötigt.</span>
            )}
          </div>
          {error && <div className="banner warn">{error}</div>}
          {report && <ResultsPanel report={report} />}
        </div>
      </main>
    </div>
  );
}
