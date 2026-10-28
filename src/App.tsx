import { useCallback, useEffect, useRef, useState } from "react";
import {
  ENGINE_NAMES,
  aiStatus,
  dockerStatus,
  getEngine,
  listLanguages,
  onEngineProgress,
  onImagesProgress,
  onProgress,
  prepareImages,
  progressText,
  runTests,
  setEngine,
  startEngine,
  updateImages,
  type AiStatus,
  type DockerStatus,
  type GenerateResult,
  type Engine,
  type CompareMode,
  type EngineChoice,
  type ImagesReport,
  type ImportResult,
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
import { AiPanel } from "./components/AiPanel";
import { ModelSetup } from "./components/ModelSetup";
import { MAX_TESTS, TestCaseEditor, newTestCase } from "./components/TestCaseEditor";

function reportText(r: ImagesReport, what: string): string | null {
  const parts: string[] = [];
  if (r.done.length > 0) parts.push(`${what}: ${r.done.join(", ")}.`);
  if (r.errors.length > 0) parts.push(r.errors.join(" "));
  return parts.length > 0 ? parts.join(" ") : null;
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
  // "Text geprüft und korrigiert": reset by an import, kept on manual edits.
  // Required before test cases are generated from the task text.
  const [taskConfirmed, setTaskConfirmed] = useState(false);
  const [imported, setImported] = useState<ImportResult | null>(null);
  const [tests, setTests] = useState<TestCase[]>([
    { ...newTestCase(1), input: "Welt\n", expected: "Hallo, Welt!\n" },
  ]);
  const [compareMode, setCompareMode] = useState<CompareMode | null>({ kind: "lines", trim: true });
  // Remounts the mode select when the mode is set from the AI suggestion.
  const [compareModeKey, setCompareModeKey] = useState(0);
  // Local AI: model status, setup dialog, generation running.
  const [ai, setAi] = useState<AiStatus | null>(null);
  const [setupOpen, setSetupOpen] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [aiNote, setAiNote] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [report, setReport] = useState<RunReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Runner images: background preparation and "Images aktualisieren".
  const [imagesBusy, setImagesBusy] = useState<string | null>(null);
  const [imagesNote, setImagesNote] = useState<{ text: string; error: boolean } | null>(null);
  const imagesPrepared = useRef(false);

  const runImageTask = useCallback(
    async (task: () => Promise<ImagesReport>, busyText: string, doneText: string, failText: string) => {
      setImagesBusy(busyText);
      setImagesNote(null);
      try {
        const r = await task();
        const text = reportText(r, doneText);
        if (text) setImagesNote({ text, error: r.errors.length > 0 });
      } catch (e) {
        setImagesNote({ text: `${failText} ${String(e)}`, error: true });
      } finally {
        setImagesBusy(null);
      }
    },
    [],
  );

  // Once the engine runs: load bundled images that are missing (once per engine).
  useEffect(() => {
    if (docker?.state !== "ok" || imagesPrepared.current) return;
    imagesPrepared.current = true;
    void runImageTask(
      prepareImages,
      "Prüfe Runner-Images …",
      "Images aus dem Installationspaket geladen",
      "Images konnten nicht vorbereitet werden:",
    );
  }, [docker, runImageTask]);

  const onUpdateImages = () =>
    void runImageTask(
      updateImages,
      "Images werden aktualisiert …",
      "Images aktualisiert",
      "Aktualisierung fehlgeschlagen (nicht schlimm – die vorhandenen Images bleiben nutzbar):",
    );

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
      // Each engine has its own image store.
      imagesPrepared.current = false;
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
    aiStatus()
      .then(setAi)
      .catch(() => undefined);
    const unlisten = onProgress(setProgress);
    const unlistenImages = onImagesProgress((p) =>
      setImagesBusy((prev) => (prev === null ? null : progressText(p))),
    );
    // Only show engine messages while a start is in progress.
    const unlistenEngine = onEngineProgress((m) => setStarting((prev) => (prev === null ? null : m)));
    return () => {
      void unlisten.then((f) => f());
      void unlistenEngine.then((f) => f());
      void unlistenImages.then((f) => f());
    };
  }, [refreshDocker]);

  const language = languages.find((l) => l.id === languageId);
  const code = codeByLang[languageId] ?? "";
  const canRun =
    docker?.state === "ok" &&
    starting === null &&
    imagesBusy === null &&
    !running &&
    !generating &&
    tests.length > 0 &&
    compareMode !== null &&
    code.trim() !== "";

  const onImported = (r: ImportResult) => {
    setImported(r);
    setTask(r.text);
    setTaskConfirmed(false);
  };

  // Why "Testfälle mit KI erzeugen" is not available (German), or null.
  const aiBlocked = (() => {
    if (task.trim() === "" || !taskConfirmed)
      return "Zuerst die Aufgabenstellung einfügen, prüfen und „Text geprüft und korrigiert“ anhaken.";
    if (docker?.state !== "ok" || starting !== null) return "Docker oder Podman muss laufen.";
    if (imagesBusy !== null) return "Die Runner-Images werden noch vorbereitet …";
    if (ai === null) return "KI-Status wird geladen …";
    if (!ai.runtimeAvailable) return ai.runtimeError ?? "KI-Laufzeit nicht gefunden.";
    if (!ai.modelReady) return "Zuerst ein KI-Modell einrichten („KI-Modell einrichten …“).";
    if (running) return "Bitte warten, bis die Prüfung fertig ist.";
    if (compareMode === null) return "Bitte eine gültige Toleranz beim Vergleich eingeben.";
    return null;
  })();

  // Generated tests are appended (never replace existing ones), up to
  // MAX_TESTS. The ref holds the current list (edits during generation).
  const testsRef = useRef(tests);
  testsRef.current = tests;
  const onAiResult = (r: GenerateResult) => {
    setAiNote(null);
    if (r.tests.length === 0) return;
    const prev = testsRef.current;
    const room = Math.max(0, MAX_TESTS - prev.length);
    const taken = r.tests.slice(0, room);
    if (taken.length < r.tests.length) {
      setAiNote(
        `${r.tests.length - taken.length} KI-Testfälle nicht übernommen – höchstens ${MAX_TESTS} Testfälle pro Prüfung.`,
      );
    }
    setTests([
      ...prev,
      ...taken.map((t, i) => ({
        ...newTestCase(prev.length + i + 1),
        name: t.name,
        input: t.input,
        expected: t.expected,
        origin: "ai" as const,
      })),
    ]);
  };

  const onRun = async () => {
    if (compareMode === null) return;
    setRunning(true);
    setError(null);
    setReport(null);
    setProgress(null);
    try {
      const r = await runTests({
        language: languageId,
        code,
        tests: tests.map(({ name, input, expected }) => ({ name, input, expected })),
        // Old field kept for the default (lines) comparison.
        trimWhitespace: compareMode.kind === "lines" ? compareMode.trim : true,
        compareMode,
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
        busy={running || generating}
        images={{ busy: imagesBusy, note: imagesNote }}
        onUpdateImages={onUpdateImages}
        onRetry={refreshDocker}
        onStart={() => void doStart()}
        onEngineChange={(e) => void onEngineChange(e)}
      />
      <main>
        <div className="col">
          <TaskStatement
            value={task}
            onChange={setTask}
            confirmed={taskConfirmed}
            onConfirmedChange={setTaskConfirmed}
            imported={imported}
            onImported={onImported}
          />
          <div className="toolbar">
            <LanguageSelect
              languages={languages}
              value={languageId}
              onChange={setLanguageId}
              disabled={running || generating}
            />
          </div>
          <CodeEditor
            language={language}
            code={code}
            onChange={(c) => setCodeByLang((m) => ({ ...m, [languageId]: c }))}
          />
        </div>
        <div className="col">
          <AiPanel
            status={ai}
            blockedReason={aiBlocked}
            taskText={task}
            language={languageId}
            compareMode={compareMode}
            onResult={onAiResult}
            onApplyMode={(m) => {
              setCompareMode(m);
              setCompareModeKey((k) => k + 1);
            }}
            onOpenSetup={() => setSetupOpen(true)}
            onRunningChange={setGenerating}
          />
          {aiNote && <div className="hint small">{aiNote}</div>}
          <TestCaseEditor
            tests={tests}
            onChange={setTests}
            compareMode={compareMode}
            onCompareModeChange={setCompareMode}
            compareModeKey={compareModeKey}
          />
          <div className="toolbar">
            <button className="primary" onClick={onRun} disabled={!canRun}>
              {running ? "Prüfe …" : "Prüfen"}
            </button>
            {running && <span className="muted">{progressText(progress)}</span>}
            {!running && imagesBusy !== null && <span className="muted">{imagesBusy}</span>}
            {!running && docker?.state !== "ok" && (
              <span className="muted small">Zum Prüfen wird Docker oder Podman benötigt.</span>
            )}
          </div>
          {error && <div className="banner warn">{error}</div>}
          {report && <ResultsPanel report={report} />}
        </div>
      </main>
      {setupOpen && <ModelSetup status={ai} onStatus={setAi} onClose={() => setSetupOpen(false)} />}
    </div>
  );
}
