// DEV ONLY: mocked Tauri backend for the browser preview
// (`VITE_MOCK=1 npm run dev`). Imported from main.tsx behind
// import.meta.env.DEV, so it is not part of production builds.
import type { Channel } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { AiStatus, ImportResult, InstallProgress, LanguageInfo, RunReport, RunRequest, UpdateInfo } from "../api";

const PY = `def gruss(name: str) -> str:
    return f"Hallo, {name}!"


def main() -> None:
    n = int(input())
    for _ in range(n):
        name = input().strip()
        print(gruss(name))


if __name__ == "__main__":
    main()
`;

const JAVA = `import java.util.Scanner;

public class Main {
    public static void main(String[] args) {
        Scanner in = new Scanner(System.in);
        System.out.println("Hallo, " + in.nextLine() + "!");
    }
}
`;

const LANGUAGES: LanguageInfo[] = [
  { id: "python", name: "Python 3.13", sourceFile: "main.py", extensions: ["py"], template: PY, image: "ghcr.io/bernhardgoetz/code-pruefer-python:1", runTimeoutSecs: 5 },
  { id: "java", name: "Java 21", sourceFile: "Main.java", extensions: ["java"], template: JAVA, image: "ghcr.io/bernhardgoetz/code-pruefer-java:1", runTimeoutSecs: 10 },
];

const AI: AiStatus = {
  hardware: { totalRamBytes: 16e9, physicalCores: 8, appleSilicon: false, avx2: true, suggestedModel: "qwen-7b", warnings: [] },
  models: [
    { id: "qwen-1.5b", name: "Qwen2.5-Coder 1.5B", sizeBytes: 1.1e9, license: "Apache-2.0", known: true, pinned: true, installed: false, verified: false, repo: null, quant: "Q4_K_M", importedParts: 0, missingParts: [], partialBytes: 0, suggested: false, selected: false },
    { id: "qwen-7b", name: "Qwen2.5-Coder 7B", sizeBytes: 4.7e9, license: "Apache-2.0", known: true, pinned: true, installed: true, verified: true, repo: null, quant: "Q4_K_M", importedParts: 0, missingParts: [], partialBytes: 0, suggested: true, selected: true },
  ],
  selected: "qwen-7b",
  modelReady: true,
  runtimeAvailable: true,
  runtimeError: null,
  llamaVersion: "b6000",
  modelsDir: "C:\\Users\\Schüler\\AppData\\Local\\Code-Prüfer\\models",
};

const SHEET = `<svg xmlns="http://www.w3.org/2000/svg" width="600" height="800" viewBox="0 0 600 800"><rect width="600" height="800" fill="#fdfdfb"/><g font-family="Georgia,serif" fill="#222"><text x="48" y="80" font-size="28" font-weight="bold">Aufgabe 3: Begrüßung</text><text x="48" y="130" font-size="18">Schreibe ein Programm, das zuerst eine Zahl n</text><text x="48" y="158" font-size="18">und danach n Namen einliest. Für jeden Namen</text><text x="48" y="186" font-size="18">soll „Hallo, &lt;Name&gt;!“ ausgegeben werden.</text><text x="48" y="240" font-size="18" font-weight="bold">Beispiel</text><text x="48" y="272" font-size="16" font-family="monospace">Eingabe: 2 / Anna / Ben</text><text x="48" y="300" font-size="16" font-family="monospace">Ausgabe: Hallo, Anna! / Hallo, Ben!</text></g><g stroke="#ccc">${Array.from({ length: 14 }, (_, i) => `<line x1="48" x2="552" y1="${360 + i * 30}" y2="${360 + i * 30}"/>`).join("")}</g></svg>`;

const IMPORTED: ImportResult = {
  fileName: "Aufgabe-3.png",
  text:
    "Aufgabe 3: Begrüßung\n\nSchreibe ein Programm, das zuerst eine Zahl n und danach n Namen einliest.\nFür jeden Namen soll „Hallo, <Name>!“ ausgegeben werden.\n\nBeispiel\nEingabe:\n2\nAnna\nBen\nAusgabe:\nHallo, Anna!\nHallo, Ben!\n",
  source: "image",
  pages: 1,
  ocrUsed: true,
  warnings: [],
  preview: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(SHEET)}`,
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function report(req: RunRequest): RunReport {
  return {
    language: req.language,
    image: LANGUAGES.find((l) => l.id === req.language)?.image ?? "",
    compile: null,
    compileOk: true,
    hint: null,
    passed: req.tests.length - 1,
    total: req.tests.length,
    tests: req.tests.map((t, i) => {
      const failed = i === 1;
      const stdout = failed ? "Hallo, Anna! \nHallo,Ben!\n" : t.expected;
      return {
        index: i,
        name: t.name,
        status: failed ? "failed" : "passed",
        output: { stdout, stderr: "", stdoutTruncated: false, stderrTruncated: false, exitCode: 0, timedOut: false, durationMs: 41 + i * 7 },
        diff: failed
          ? [
              { kind: "expected", text: "Hallo, Anna!", finalNewline: false },
              { kind: "actual", text: "Hallo, Anna! ", finalNewline: false },
              { kind: "expected", text: "Hallo, Ben!", finalNewline: true },
              { kind: "actual", text: "Hallo,Ben!", finalNewline: true },
            ]
          : [],
        message: null,
        onlyTrailingWhitespaceDiffers: false,
      };
    }),
  };
}

// ?update=available (default) | none | error | progress | notConfigured | portable
const UPDATE_NOTES = `Neu in dieser Version:
- Updates: Der Code-Prüfer sucht beim Start nach neuen Versionen (Einstellungen → Updates).
- Testfälle mit KI: schneller auf Rechnern ohne Grafikkarte.
- Texterkennung: bessere Einrückung bei Screenshots.

Behoben:
- „Images aktualisieren“ meldete Fehler doppelt.`;

function updateMock(param: string) {
  let info: UpdateInfo = {
    currentVersion: "0.1.0",
    availability: param === "notConfigured" ? "notConfigured" : param === "portable" ? "portable" : "available",
    interval: "daily",
    lastCheck: Date.parse("2026-10-29T08:15:00Z"),
    releasesUrl: "https://github.com/bernhardgoetz/entwicklung-test/releases",
  };
  const offer = { version: "0.2.0", currentVersion: "0.1.0", notes: UPDATE_NOTES, date: "2026-10-28T12:00:00Z" };
  return async (cmd: string, a: Record<string, unknown>) => {
    switch (cmd) {
      case "update_info":
        return info;
      case "set_update_interval":
        info = { ...info, interval: a.interval as UpdateInfo["interval"] };
        return info;
      case "check_for_update":
        if (info.availability !== "available") return null;
        await sleep(a.manual ? 700 : 100);
        if (param === "error" && a.manual)
          throw "Keine Verbindung zum Update-Server (GitHub). Bitte die Internetverbindung prüfen und es später erneut versuchen.";
        info = { ...info, lastCheck: Date.now() };
        return param === "none" || param === "error" ? null : offer;
      case "install_update": {
        const ch = a.onProgress as Channel<InstallProgress>;
        const total = 27_400_000;
        const stop = param === "progress" ? 0.45 : 1;
        for (let f = 0.05; f <= stop + 1e-9; f += 0.05) {
          await sleep(120);
          ch.onmessage({ kind: "download", downloaded: Math.round(total * f), total });
        }
        if (param === "progress") return new Promise(() => undefined);
        ch.onmessage({ kind: "installing" });
        await sleep(800);
        throw "Vorschau: Installation und Neustart werden im Browser nicht ausgeführt.";
      }
      default:
        return undefined;
    }
  };
}

export function installMock(): void {
  const params = new URLSearchParams(location.search);
  const state = params.get("engine") ?? "ok";
  const update = updateMock(params.get("update") ?? "available");
  mockIPC(
    async (cmd, args) => {
      const a = args as Record<string, unknown>;
      switch (cmd) {
        case "docker_status":
          if (state === "missing") return { state: "notInstalled" };
          if (state === "stopped" || state === "stopfail") return { state: "notRunning" };
          if (state === "tcp")
            return {
              state: "ok",
              product: "Docker Desktop",
              version: "28.3.2",
              warning: 
                "Achtung: DOCKER_HOST=tcp://192.168.1.20:2375 verbindet unverschlüsselt und ohne Anmeldung über das Netzwerk. Wer diesen Port erreicht, kann Container starten und den Rechner übernehmen. Bitte nur auf dem eigenen Rechner verwenden oder auf den Socket bzw. die Named Pipe der Engine umstellen.",
            };
          return { state: "ok", product: "Docker Desktop", version: "28.3.2" };
        case "start_engine":
          await sleep(800);
          // ?engine=stopfail: the automatic start fails (header shows „… starten“).
          if (state === "stopfail") throw "Docker Desktop hat nicht innerhalb von 2 Minuten geantwortet.";
          return { state: "ok", product: "Docker Desktop", version: "28.3.2" };
        case "get_engine":
          return { engine: "docker", source: "config" };
        case "set_engine":
          return { engine: a.engine, source: "config" };
        case "list_languages":
          return LANGUAGES;
        case "prepare_images":
        case "update_images":
          await sleep(300);
          return { done: [], errors: [] };
        case "ai_status":
        case "select_model":
          return AI;
        case "warm_up_ai":
          return true;
        case "pick_files":
          return a.kind === "taskDocument"
            ? { token: "mock-token", fileNames: ["Aufgabe-3.png"] }
            : { token: "mock-token", fileNames: ["modell-q4_k_m.gguf"] };
        case "import_task_document":
          await sleep(200);
          return IMPORTED;
        case "run_tests":
          await sleep(600);
          return report(a.request as RunRequest);
        case "suggest_ocr_fixes":
          await sleep(500);
          return { fixes: [], rejected: 0 };
        case "generate_tests":
          await sleep(1500);
          return {
            tests: [{ name: "KI: drei Namen", input: "3\nA\nB\nC\n", expected: "Hallo, A!\nHallo, B!\nHallo, C!\n", source: "generated", agreement: 1 }],
            referenceSolution: PY,
            confidence: { level: "green", reasons: ["Beide Beispiele des Blatts bestanden.", "3 von 3 Lösungen liefern dieselben Ausgaben."] },
            suggestedCompareMode: null,
            compareMode: { kind: "lines", trim: true },
            stats: { solutionsRequested: 3, solutionsCompiled: 3, solutionsRepaired: 0, solutionsPassed: 3, examplesFound: 1, examplesDiscarded: 0, inputsGenerated: 4, testsDropped: 0, llmCalls: 4, completionTokens: 900, tokensPerSecond: 14.2, durationMs: 72000 },
          };
        case "cancel_ai":
          return null;
        case "update_info":
        case "set_update_interval":
        case "check_for_update":
        case "install_update":
          return update(cmd, a);
        default:
          console.warn("mock: unhandled", cmd, args);
          return null;
      }
    },
    { shouldMockEvents: true },
  );
}
