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
