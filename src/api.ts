import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type DockerStatus =
  | { state: "notInstalled" }
  | { state: "notRunning" }
  | { state: "ok"; version: string; product: string };

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
  | { kind: "phase"; message: string };

export interface RunRequest {
  language: string;
  code: string;
  tests: { name: string; input: string; expected: string }[];
  trimWhitespace: boolean;
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
