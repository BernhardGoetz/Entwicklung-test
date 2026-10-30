import { useCallback, useEffect, useRef, useState } from "react";
import {
  ENGINE_NAMES,
  aiStatus,
  checkForUpdate,
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
  setUpdateInterval,
  updateInfo,
  warmUpAi,
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
  type UpdateInfo,
  type UpdateInterval,
  type UpdateOffer,
} from "./api";
import { CodeEditor } from "./components/CodeEditor";
import { EngineNotices, EngineStatus, type EngineState } from "./components/DockerBanner";
import { LanguageSelect } from "./components/LanguageSelect";
import { ResultsPanel } from "./components/ResultsPanel";
import { TaskStatement } from "./components/TaskStatement";
import { AiPanel } from "./components/AiPanel";
import { ModelSetup } from "./components/ModelSetup";
import { MAX_TESTS, TestCaseEditor, newTestCase } from "./components/TestCaseEditor";
import { TabBar, TabPanel, type TabDef } from "./components/Tabs";
import { SettingsDialog } from "./components/SettingsDialog";
import { UpdateDialog } from "./components/UpdateDialog";
import type { CheckNote } from "./components/UpdateSettings";
import { readStored, useTheme, writeStored } from "./theme";

function reportText(r: ImagesReport, what: string): string | null {
  const parts: string[] = [];
  if (r.done.length > 0) parts.push(`${what}: ${r.done.join(", ")}.`);
  if (r.errors.length > 0) parts.push(r.errors.join(" "));
  return parts.length > 0 ? parts.join(" ") : null;
}

type TabKey = "task" | "tests" | "result";
const TAB_KEY = "code-pruefer.tab";
const isTab = (v: string | null): v is TabKey => v === "task" || v === "tests" || v === "result";

// Automatic update check: shortly after the start, then hourly while the app
// runs; the backend only asks GitHub when the chosen interval is due.
const UPDATE_FIRST_CHECK_MS = 5_000;
const UPDATE_RECHECK_MS = 60 * 60 * 1000;

export default function App() {
  const { pref: themePref, setPref: setThemePref, theme } = useTheme();
  const [tab, setTab] = useState<TabKey>(() => {
    const v = readStored(TAB_KEY);
    return isTab(v) ? v : "task";
  });
  const selectTab = useCallback((t: TabKey) => {
    setTab(t);
    writeStored(TAB_KEY, t);
  }, []);
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
  // „Zeilenumbruch am Ende verbieten“: independent of the compare mode.
  const [forbidTrailingNewline, setForbidTrailingNewline] = useState(false);
  // Remounts the mode select when the mode is set from the AI suggestion.
  const [compareModeKey, setCompareModeKey] = useState(0);
  // Local AI: model status, setup dialog, generation running.
  const [ai, setAi] = useState<AiStatus | null>(null);
  const [setupOpen, setSetupOpen] = useState(false);
  // „Einstellungen“ (gear): theme, container engine, AI model.
  const [settingsOpen, setSettingsOpen] = useState(false);
  const gearRef = useRef<HTMLButtonElement>(null);
  const [generating, setGenerating] = useState(false);
  // "Erkennungsfehler mit KI korrigieren" is running (same model).
  const [fixing, setFixing] = useState(false);
  const [aiNote, setAiNote] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [report, setReport] = useState<RunReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Runner images: background preparation and "Images aktualisieren".
  const [imagesBusy, setImagesBusy] = useState<string | null>(null);
  const [imagesNote, setImagesNote] = useState<{ text: string; error: boolean } | null>(null);
  const imagesPrepared = useRef(false);
  // App updates: settings info, an offer waiting to be shown, the dialog.
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [updateOffer, setUpdateOffer] = useState<UpdateOffer | null>(null);
  const [updateOpen, setUpdateOpen] = useState(false);
  const [updateChecking, setUpdateChecking] = useState(false);
  const [updateNote, setUpdateNote] = useState<CheckNote | null>(null);

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

  useEffect(() => {
    updateInfo()
      .then(setUpdate)
      .catch(() => undefined);
    const auto = () =>
      checkForUpdate(false)
        .then((offer) => {
          if (offer) setUpdateOffer(offer);
          return updateInfo().then(setUpdate);
        })
        .catch(() => undefined);
    const first = window.setTimeout(auto, UPDATE_FIRST_CHECK_MS);
    const timer = window.setInterval(auto, UPDATE_RECHECK_MS);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(timer);
    };
  }, []);

  const onUpdateIntervalChange = (interval: UpdateInterval) => {
    setUpdateNote(null);
    setUpdateInterval(interval)
      .then(setUpdate)
      .catch((e) => setUpdateNote({ text: String(e), tone: "error" }));
  };

  // „Jetzt nach Updates suchen“: a found update replaces the settings dialog.
  const onCheckUpdate = async () => {
    setUpdateChecking(true);
    setUpdateNote(null);
    try {
      const offer = await checkForUpdate(true);
      if (offer) {
        setUpdateOffer(offer);
        setSettingsOpen(false);
        setUpdateOpen(true);
      } else {
        setUpdateNote({ text: "Keine neue Version verfügbar – diese Version ist aktuell.", tone: "ok" });
      }
    } catch (e) {
      setUpdateNote({ text: String(e), tone: "error" });
    } finally {
      setUpdateChecking(false);
      updateInfo()
        .then(setUpdate)
        .catch(() => undefined);
    }
  };

  const language = languages.find((l) => l.id === languageId);
  const code = codeByLang[languageId] ?? "";
  // "Prüfen" starts the engine itself if it is installed but not running.
  const canRun =
    docker !== null &&
    docker.state !== "notInstalled" &&
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
    // The backend starts the engine itself; only a start in progress blocks.
    if (starting !== null) return "Docker oder Podman wird gerade gestartet …";
    if (imagesBusy !== null) return "Die Runner-Images werden noch vorbereitet …";
    if (ai === null) return "KI-Status wird geladen …";
    if (!ai.runtimeAvailable) return ai.runtimeError ?? "KI-Laufzeit nicht gefunden.";
    if (!ai.modelReady) return "Zuerst ein KI-Modell einrichten („KI-Modell einrichten …“).";
    if (running) return "Bitte warten, bis die Prüfung fertig ist.";
    if (fixing) return "Bitte warten, bis die KI-Korrektur fertig ist.";
    if (compareMode === null) return "Bitte eine gültige Toleranz beim Vergleich eingeben.";
    return null;
  })();

  // Warm-up: once the task text is confirmed and a model is ready, load the
  // model in the background (cancellable with "Abbrechen", no error popups).
  const aiReady = ai !== null && ai.modelReady && ai.runtimeAvailable;
  useEffect(() => {
    if (!taskConfirmed || !aiReady) return;
    warmUpAi().catch(() => undefined);
  }, [taskConfirmed, aiReady, ai?.selected]);

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
    selectTab("result");
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
        forbidTrailingNewline,
      });
      setReport(r);
      // The backend may have started the engine.
      if (docker?.state !== "ok") refreshDocker();
    } catch (e) {
      setError(String(e));
      refreshDocker();
    } finally {
      setRunning(false);
    }
  };

  // Tab badges: small counts/status next to the labels.
  const resultTab: Pick<TabDef<TabKey>, "badge" | "badgeText" | "tone"> = (() => {
    if (running) return { badge: "…", badgeText: "Prüfung läuft", tone: "busy" };
    if (error) return { badge: "!", badgeText: "Fehler", tone: "fail" };
    if (!report) return {};
    if (!report.compileOk) return { badge: "✗", badgeText: "Kompilierfehler", tone: "fail" };
    const ok = report.passed === report.total && report.total > 0;
    return {
      badge: `${ok ? "✓" : "✗"} ${report.passed}/${report.total}`,
      badgeText: `${report.passed} von ${report.total} bestanden`,
      tone: ok ? "ok" : "fail",
    };
  })();
  const tabs: TabDef<TabKey>[] = [
    {
      id: "task",
      label: "Aufgabe",
      ...(fixing
        ? { badge: "…", badgeText: "KI-Korrektur läuft", tone: "busy" as const }
        : task.trim() === ""
          ? {}
          : taskConfirmed
            ? { badge: "✓", badgeText: "Text geprüft", tone: "ok" as const }
            : { badge: "●", badgeText: "Text noch nicht als geprüft markiert", tone: "warn" as const }),
    },
    {
      id: "tests",
      label: "Testfälle",
      badge: generating ? `${tests.length} · KI …` : String(tests.length),
      badgeText: `${tests.length} ${tests.length === 1 ? "Testfall" : "Testfälle"}${generating ? ", KI arbeitet" : ""}`,
      tone: generating ? "busy" : "neutral",
    },
    { id: "result", label: "Ergebnis", ...resultTab },
  ];

  // Short status next to "Prüfen" (the full text is in the title).
  const runStatus = running
    ? progressText(progress)
    : imagesBusy !== null
      ? imagesBusy
      : docker?.state === "notInstalled"
        ? "Zum Prüfen wird Docker oder Podman benötigt."
        : docker?.state === "notRunning"
          ? "Die Container-Engine wird beim Prüfen automatisch gestartet."
          : null;

  // Updates never interrupt a check or an AI job: the offer waits until
  // nothing runs and no other dialog is open; installing waits as well.
  const updateBusy = running
    ? "Eine Prüfung läuft gerade."
    : generating
      ? "Die KI erzeugt gerade Testfälle."
      : fixing
        ? "Die KI korrigiert gerade den Aufgabentext."
        : imagesBusy !== null
          ? "Die Runner-Images werden gerade vorbereitet."
          : starting !== null
            ? "Die Container-Engine wird gerade gestartet."
            : null;
  useEffect(() => {
    if (updateOffer !== null && !updateOpen && updateBusy === null && !settingsOpen && !setupOpen) setUpdateOpen(true);
  }, [updateOffer, updateOpen, updateBusy, settingsOpen, setupOpen]);

  const engineState: EngineState = {
    status: docker,
    engine,
    starting,
    startError,
    images: { busy: imagesBusy, note: imagesNote },
  };

  return (
    <div className="app">
      <header className="appbar">
        <div className="brand">
          <span className="brand-name">Code-Prüfer</span>
          <span className="brand-tag muted">Programmieraufgaben lokal und sicher testen</span>
        </div>
        <EngineStatus state={engineState} onStart={() => void doStart()} />
        <button
          ref={gearRef}
          className="icon-button"
          aria-label="Einstellungen"
          aria-haspopup="dialog"
          title="Einstellungen (Farbschema, Container-Engine, KI-Modell)"
          onClick={() => setSettingsOpen(true)}
        >
          <GearIcon />
        </button>
      </header>
      <EngineNotices state={engineState} onRetry={refreshDocker} onOpenSettings={() => setSettingsOpen(true)} />
      <main>
        <CodeEditor
          language={language}
          code={code}
          theme={theme}
          onChange={(c) => setCodeByLang((m) => ({ ...m, [languageId]: c }))}
          toolbarStart={
            <LanguageSelect
              languages={languages}
              value={languageId}
              onChange={setLanguageId}
              disabled={running || generating}
            />
          }
          toolbarEnd={
            <>
              {runStatus !== null && (
                <span className={running ? "run-status busy" : "run-status"} title={runStatus} aria-live="polite">
                  {runStatus}
                </span>
              )}
              <button className="primary" onClick={() => void onRun()} disabled={!canRun}>
                {running ? "Prüfe …" : "Prüfen"}
              </button>
            </>
          }
        />
        <TabBar tabs={tabs} active={tab} onChange={selectTab} label="Bereiche" />
        <TabPanel id="task" active={tab === "task"}>
          <TaskStatement
            value={task}
            onChange={setTask}
            confirmed={taskConfirmed}
            onConfirmedChange={setTaskConfirmed}
            imported={imported}
            onImported={onImported}
            ai={ai}
            aiBusy={generating}
            onOpenSetup={() => setSetupOpen(true)}
            onFixingChange={setFixing}
          />
        </TabPanel>
        <TabPanel id="tests" active={tab === "tests"}>
          <div className="tests-layout">
            <TestCaseEditor
              tests={tests}
              onChange={setTests}
              compareMode={compareMode}
              onCompareModeChange={setCompareMode}
              compareModeKey={compareModeKey}
              forbidTrailingNewline={forbidTrailingNewline}
              onForbidTrailingNewlineChange={setForbidTrailingNewline}
            />
            <aside className="tests-aside">
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
                onFinished={(failed) => {
                  selectTab("tests");
                  // Engine errors, or the backend started the engine: show the real state.
                  if (failed || docker?.state !== "ok") refreshDocker();
                }}
              />
              {aiNote && <p className="note warn small">{aiNote}</p>}
            </aside>
          </div>
        </TabPanel>
        <TabPanel id="result" active={tab === "result"}>
          <section className="card" aria-labelledby="result-heading">
            <h2 id="result-heading" className="sr-only">
              Ergebnis
            </h2>
            {running && <p className="note busy">{progressText(progress)}</p>}
            {error && <div className="banner error">{error}</div>}
            {report && !running && <ResultsPanel report={report} />}
            {!report && !running && !error && (
              <p className="empty muted">
                Noch keine Prüfung. Code eingeben, Testfälle anlegen und „Prüfen“ drücken – das Ergebnis
                erscheint hier.
              </p>
            )}
          </section>
        </TabPanel>
      </main>
      {settingsOpen && (
        <SettingsDialog
          onClose={() => setSettingsOpen(false)}
          returnFocus={gearRef}
          themePref={themePref}
          onThemeChange={setThemePref}
          engine={engineState}
          busy={running || generating}
          onEngineChange={(e) => void onEngineChange(e)}
          onStart={() => void doStart()}
          onRetry={refreshDocker}
          onUpdateImages={onUpdateImages}
          ai={ai}
          aiBusy={generating}
          onOpenModelSetup={() => {
            // Replace the settings (no stacked dialogs); focus returns to the gear.
            setSettingsOpen(false);
            setSetupOpen(true);
          }}
          update={update}
          updateChecking={updateChecking}
          updateNote={updateNote}
          onUpdateIntervalChange={onUpdateIntervalChange}
          onCheckUpdate={() => void onCheckUpdate()}
        />
      )}
      {updateOpen && updateOffer && (
        <UpdateDialog
          offer={updateOffer}
          busyReason={updateBusy}
          returnFocus={gearRef}
          onLater={() => {
            // „Später“: asked again at the next due check.
            setUpdateOpen(false);
            setUpdateOffer(null);
          }}
        />
      )}
      {setupOpen && (
        <ModelSetup status={ai} onStatus={setAi} onClose={() => setSetupOpen(false)} returnFocus={gearRef} />
      )}
    </div>
  );
}

/** Gear icon (outline, 18 px, currentColor). */
function GearIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
      <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  );
}
