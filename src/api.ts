import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

export type DockerStatus =
  | { state: "notInstalled" }
  | { state: "notRunning" }
  | {
      state: "ok";
      version: string;
      product: string;
      /** German security warning, e.g. for an unencrypted DOCKER_HOST. */
      warning?: string;
    };

export interface LanguageInfo {
  id: string;
  name: string;
  sourceFile: string;
  extensions: string[];
  template: string;
  image: string;
  runTimeoutSecs: number;
}

export interface TestCase {
  id: number;
  name: string;
  input: string;
  expected: string;
}

export interface StepOutput {
  stdout: string;
  stderr: string;
  stdoutTruncated: boolean;
  stderrTruncated: boolean;
  exitCode: number | null;
  timedOut: boolean;
  durationMs: number;
}

export type DiffKind = "equal" | "expected" | "actual";
export interface DiffLine {
  kind: DiffKind;
  text: string;
}

export type TestStatus = "passed" | "failed" | "timeout" | "runtimeError" | "error";

export interface TestResult {
  index: number;
  name: string;
  status: TestStatus;
  output: StepOutput | null;
  diff: DiffLine[];
  message: string | null;
}

export interface RunReport {
  language: string;
  image: string;
  compile: StepOutput | null;
  compileOk: boolean;
  hint: string | null;
  tests: TestResult[];
  passed: number;
  total: number;
}

export type Progress =
  | { kind: "pull"; image: string; status: string; percent: number | null }
  | { kind: "load"; image: string; percent: number | null }
  | { kind: "phase"; message: string };

/** German text for a progress event (runs, image preparation and updates). */
export function progressText(p: Progress | null): string {
  if (!p) return "Läuft …";
  if (p.kind === "phase") return p.message;
  const pct = p.percent !== null ? ` ${p.percent} %` : "";
  if (p.kind === "load") return `Lade Image ${p.image} aus dem Installationspaket (einmalig):${pct}`;
  return `Lade Image ${p.image} herunter:${pct} ${p.status}`;
}

/** Output comparison (CompareMode in src-tauri/src/runner/compare.rs). */
export type CompareMode =
  | { kind: "lines"; trim: boolean }
  | { kind: "tokens"; caseSensitive: boolean }
  | { kind: "float"; absTol: number; relTol: number };

/** Same limit as MAX_TOLERANCE in src-tauri/src/runner/compare.rs. */
export const MAX_TOLERANCE = 1e6;

export interface RunRequest {
  language: string;
  code: string;
  tests: { name: string; input: string; expected: string }[];
  trimWhitespace: boolean;
  compareMode?: CompareMode;
}

export const dockerStatus = () => invoke<DockerStatus>("docker_status");
export const listLanguages = () => invoke<LanguageInfo[]>("list_languages");
export const runTests = (request: RunRequest) => invoke<RunReport>("run_tests", { request });
export const onProgress = (cb: (p: Progress) => void): Promise<UnlistenFn> =>
  listen<Progress>("run-progress", (e) => cb(e.payload));

export type Engine = "docker" | "podman";
export interface EngineChoice {
  engine: Engine;
  /** Where the choice came from: app setting, installer (registry) or default. */
  source: "config" | "registry" | "default";
}

export const ENGINE_NAMES: Record<Engine, string> = {
  docker: "Docker Desktop",
  podman: "Podman",
};

export const getEngine = () => invoke<EngineChoice>("get_engine");
export const setEngine = (engine: Engine) => invoke<EngineChoice>("set_engine", { engine });
/** Starts the chosen engine and resolves with the status once its API answers. */
export const startEngine = () => invoke<DockerStatus>("start_engine");
export const onEngineProgress = (cb: (message: string) => void): Promise<UnlistenFn> =>
  listen<string>("engine-progress", (e) => cb(e.payload));

export interface ImagesReport {
  /** Languages whose image was loaded (prepare) or updated (update). */
  done: string[];
  /** German, non-fatal error messages. */
  errors: string[];
}

/** Loads bundled image archives into the active engine if images are missing. */
export const prepareImages = () => invoke<ImagesReport>("prepare_images");
/** Pulls the pinned runner images from GHCR ("Images aktualisieren"). */
export const updateImages = () => invoke<ImagesReport>("update_images");
export const onImagesProgress = (cb: (p: Progress) => void): Promise<UnlistenFn> =>
  listen<Progress>("images-progress", (e) => cb(e.payload));

/** Result of importing a task sheet (ImportResult in src-tauri/src/docs/mod.rs). */
export interface ImportResult {
  fileName: string;
  text: string;
  source: "docx" | "pdf" | "image";
  /** Pages (PDF), 1 for images, null for Word documents. */
  pages: number | null;
  /** Text (partly) from OCR: check with extra care. */
  ocrUsed: boolean;
  warnings: string[];
  /** Preview of the original as data URL (image or first PDF page). */
  preview: string | null;
}

/** Same list as EXTENSIONS in src-tauri/src/docs/mod.rs. */
export const DOC_EXTENSIONS = ["png", "jpg", "jpeg", "pdf", "docx"];

/** Opens the file dialog; resolves with the chosen path or null. */
export const pickTaskDocument = () =>
  open({
    title: "Aufgabenblatt öffnen",
    multiple: false,
    directory: false,
    filters: [{ name: "Aufgabenblatt (Bild, PDF, Word)", extensions: DOC_EXTENSIONS }],
  });
export const importTaskDocument = (path: string) =>
  invoke<ImportResult>("import_task_document", { path });
export const onDocsProgress = (cb: (p: Progress) => void): Promise<UnlistenFn> =>
  listen<Progress>("docs-progress", (e) => cb(e.payload));
