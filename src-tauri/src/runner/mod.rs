//! Runs a submission against its test cases inside a sandbox container.

pub mod compare;
pub mod docker;
pub mod images;
pub mod languages;

use serde::{Deserialize, Serialize};

use compare::{
    ends_with_newline, line_diff, normalize_newlines, only_trailing_whitespace_differs,
    CompareMode, DiffLine,
};
use docker::{PullProgress, Sandbox, STDIN_FILE};

/// Maximum accepted size of source code and of a single test input/expectation.
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;
/// Maximum number of test cases per run (bounds the run time, see
/// `docker::ORPHAN_AGE`).
pub const MAX_TESTS: usize = 50;

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// Carries the German reason (timeout or no connection).
    #[error(
        "Keine laufende Container-Engine gefunden ({0}). Bitte Docker Desktop oder Podman starten."
    )]
    NoEngine(String),
    #[error("Unbekannte Sprache: {0}")]
    UnknownLanguage(String),
    #[error("Image {0} konnte nicht geladen werden: {1}")]
    ImagePull(String, String),
    #[error("Image-Archiv {0} konnte nicht geladen werden: {1}")]
    ImageLoad(String, String),
    #[error(
        "Das Runner-Image für {language} ist nicht vorhanden (gesucht: {tried}). Es konnte weder \
         aus dem Installationspaket geladen noch heruntergeladen werden ({detail}). Bitte die \
         Internetverbindung prüfen und „Images aktualisieren“ klicken oder den Code-Prüfer neu \
         installieren.{dev_hint}"
    )]
    ImageMissing {
        language: String,
        tried: String,
        /// Build instructions for developers (debug builds), else empty.
        dev_hint: String,
        detail: String,
    },
    /// Shown with [`docker::describe_error`] („HTTP 500: …“, „Endpunkt
    /// nicht vorhanden …“) instead of bollard's English text.
    #[error("Fehler der Container-Engine: {}", docker::describe_error(.0))]
    Docker(#[from] bollard::errors::Error),
    #[error("{0}")]
    Other(String),
}

impl RunnerError {
    /// The engine itself failed (no connection, broken connection, engine
    /// error answer), as opposed to a problem of the program or the input.
    /// After such an error the engine may need a (re)start.
    pub fn is_engine_error(&self) -> bool {
        matches!(self, Self::NoEngine(_) | Self::Docker(_))
    }
}

impl Serialize for RunnerError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestCase {
    #[serde(default)]
    pub name: String,
    pub input: String,
    pub expected: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    pub language: String,
    pub code: String,
    pub tests: Vec<TestCase>,
    /// Ignore trailing whitespace per line and trailing newlines (default on).
    /// Only used without `compare_mode`.
    #[serde(default = "default_true")]
    pub trim_whitespace: bool,
    /// How outputs are compared; `None` = lines, trimmed per `trim_whitespace`.
    #[serde(default)]
    pub compare_mode: Option<CompareMode>,
    /// Fail every test whose stdout ends with a line break, independent of
    /// the comparison mode and the expected output (default off).
    #[serde(default)]
    pub forbid_trailing_newline: bool,
}

impl RunRequest {
    /// The effective comparison mode.
    pub fn mode(&self) -> CompareMode {
        self.compare_mode
            .unwrap_or(CompareMode::lines(self.trim_whitespace))
    }
}

/// Outputs of a program for a list of inputs (see [`collect_outputs`]).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectedOutputs {
    pub image: String,
    pub compile: Option<StepOutput>,
    pub compile_ok: bool,
    /// One entry per input, in order; empty if compiling failed.
    pub outputs: Vec<StepOutput>,
}

/// " Für Entwickler: …" part of [`RunnerError::ImageMissing`] (debug builds).
pub fn dev_build_hint(lang: &languages::LanguageConfig) -> String {
    format!(
        " Für Entwickler: das Image im Projektordner lokal bauen mit docker build -t {} \
         images/{} (bei Podman „podman build …“) und danach erneut auf „Prüfen“ klicken.",
        lang.local_image, lang.id
    )
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TestStatus {
    Passed,
    Failed,
    Timeout,
    RuntimeError,
    /// The sandbox failed while running this test.
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepOutput {
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub exit_code: Option<i64>,
    pub timed_out: bool,
    pub duration_ms: u64,
}

impl From<docker::ExecOutput> for StepOutput {
    fn from(o: docker::ExecOutput) -> Self {
        StepOutput {
            stdout: o.stdout,
            stderr: o.stderr,
            stdout_truncated: o.stdout_truncated,
            stderr_truncated: o.stderr_truncated,
            exit_code: o.exit_code,
            timed_out: o.timed_out,
            duration_ms: o.duration_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub index: usize,
    pub name: String,
    pub status: TestStatus,
    pub output: Option<StepOutput>,
    pub diff: Vec<DiffLine>,
    /// Additional German explanation (e.g. timeout, memory limit).
    pub message: Option<String>,
    /// Failed, and expected and actual output differ only in trailing
    /// whitespace per line or line breaks/empty lines at the end.
    pub only_trailing_whitespace_differs: bool,
}

/// German message when [`RunRequest::forbid_trailing_newline`] rejects an output.
pub const TRAILING_NEWLINE_FORBIDDEN: &str =
    "Die Ausgabe endet mit einem Zeilenumbruch – laut Einstellung nicht erlaubt.";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    pub language: String,
    pub image: String,
    pub compile: Option<StepOutput>,
    pub compile_ok: bool,
    /// Hint shown before/instead of the compile output.
    pub hint: Option<String>,
    pub tests: Vec<TestResult>,
    pub passed: usize,
    pub total: usize,
}

/// Progress events for the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Progress {
    Pull(PullProgress),
    /// Loading a bundled image archive; `percent` of the archive read.
    Load {
        image: String,
        percent: Option<u8>,
    },
    Phase {
        message: String,
    },
}

pub fn language(id: &str) -> Result<&'static languages::LanguageConfig, RunnerError> {
    languages::get(id).ok_or_else(|| RunnerError::UnknownLanguage(id.to_string()))
}

/// Checks the size limits shared by runs and output collection: the code
/// and every field at most [`MAX_INPUT_BYTES`], at most [`MAX_TESTS`] runs.
fn check_limits<'a>(
    code: &str,
    fields: impl IntoIterator<Item = &'a str>,
    runs: usize,
) -> Result<(), RunnerError> {
    if code.len() > MAX_INPUT_BYTES || fields.into_iter().any(|f| f.len() > MAX_INPUT_BYTES) {
        return Err(RunnerError::Other(
            "Code oder Testdaten sind zu groß (max. 1 MB je Feld).".into(),
        ));
    }
    if runs > MAX_TESTS {
        return Err(RunnerError::Other(format!(
            "Zu viele Testfälle ({runs}) – höchstens {MAX_TESTS} pro Prüfung."
        )));
    }
    Ok(())
}

/// Once per run (or per test generation): removes sandboxes left over by
/// crashed runs and makes sure the runner image of `lang` exists. Returns
/// the image to start sandboxes from.
pub async fn prepare(
    docker: &bollard::Docker,
    lang: &languages::LanguageConfig,
    on_progress: &mut impl FnMut(Progress),
) -> Result<String, RunnerError> {
    // Left-overs of crashed runs; failures do not block the run.
    let _ = docker::remove_orphans(docker, docker::ORPHAN_AGE).await;
    images::ensure_image(docker, lang, on_progress).await
}

/// Makes sure the image exists and starts a fresh sandbox container. The
/// caller must `remove()` it.
async fn start_sandbox(
    docker: &bollard::Docker,
    lang: &languages::LanguageConfig,
    on_progress: &mut impl FnMut(Progress),
) -> Result<(Sandbox, String), RunnerError> {
    let image = prepare(docker, lang, on_progress).await?;
    on_progress(Progress::Phase {
        message: "Starte Container …".into(),
    });
    let sandbox = Sandbox::create(docker, &image).await?;
    Ok((sandbox, image))
}

/// Connects (with retries) and runs the submission, see [`run_submission_with`].
pub async fn run_submission(
    req: RunRequest,
    on_progress: impl FnMut(Progress),
) -> Result<RunReport, RunnerError> {
    // Limits first: rejected requests never touch the engine.
    validate_request(&req)?;
    let docker = docker::connect().await?;
    run_submission_with(&docker, req, on_progress).await
}

fn validate_request(req: &RunRequest) -> Result<&'static languages::LanguageConfig, RunnerError> {
    let lang = language(&req.language)?;
    check_limits(
        &req.code,
        req.tests
            .iter()
            .flat_map(|t| [t.input.as_str(), t.expected.as_str()]),
        req.tests.len(),
    )?;
    req.mode().validate().map_err(RunnerError::Other)?;
    Ok(lang)
}

/// Runs a submission on an engine that is already connected.
pub async fn run_submission_with(
    docker: &bollard::Docker,
    req: RunRequest,
    mut on_progress: impl FnMut(Progress),
) -> Result<RunReport, RunnerError> {
    let lang = validate_request(&req)?;
    let hint = languages::precheck(lang, &req.code);

    let (sandbox, image) = start_sandbox(docker, lang, &mut on_progress).await?;
    let result = run_in_sandbox(&sandbox, lang, &req, &mut on_progress).await;
    sandbox.remove().await;

    let (compile, compile_ok, tests) = result?;
    let passed = tests
        .iter()
        .filter(|t| t.status == TestStatus::Passed)
        .count();
    Ok(RunReport {
        language: lang.id.to_string(),
        image,
        compile,
        compile_ok,
        hint: if compile_ok { None } else { hint },
        total: req.tests.len(),
        passed,
        tests,
    })
}

/// Runs `code` once per input and returns the raw outputs (no expected
/// output, no evaluation) – e.g. to compute expected outputs from a
/// reference solution. Same sandbox, limits and flow as [`run_submission`]:
/// one container, compiled once, every input in a fresh process. Uses
/// `docker` if given, else connects.
pub async fn collect_outputs(
    language_id: &str,
    code: &str,
    inputs: &[String],
    docker: Option<&bollard::Docker>,
    mut on_progress: impl FnMut(Progress),
) -> Result<CollectedOutputs, RunnerError> {
    let lang = language(language_id)?;
    check_limits(code, inputs.iter().map(String::as_str), inputs.len())?;
    let connected;
    let docker = match docker {
        Some(d) => d,
        None => {
            connected = docker::connect().await?;
            &connected
        }
    };
    let image = prepare(docker, lang, &mut on_progress).await?;
    run_program(docker, lang, &image, code, inputs, on_progress).await
}

/// [`collect_outputs`] without the preparation: `image` comes from
/// [`prepare`], which callers with many programs run only once.
pub async fn run_program(
    docker: &bollard::Docker,
    lang: &'static languages::LanguageConfig,
    image: &str,
    code: &str,
    inputs: &[String],
    mut on_progress: impl FnMut(Progress),
) -> Result<CollectedOutputs, RunnerError> {
    check_limits(code, inputs.iter().map(String::as_str), inputs.len())?;
    on_progress(Progress::Phase {
        message: "Starte Container …".into(),
    });
    let session = SandboxSession::open(docker, lang, image).await?;
    let result = async {
        let (compile, compile_ok) = session.compile(code, &mut on_progress).await?;
        let outputs = if compile_ok {
            session.run(inputs, &mut on_progress).await?
        } else {
            Vec::new()
        };
        Ok(CollectedOutputs {
            image: image.to_string(),
            compile,
            compile_ok,
            outputs,
        })
    }
    .await;
    session.close().await;
    result
}

/// One sandbox container for one program: compiled once, then run on any
/// number of inputs (each in a fresh process). Test generation keeps one
/// session per candidate solution for the example check and the majority
/// vote. Call [`SandboxSession::close`] when done.
pub struct SandboxSession {
    sandbox: Sandbox,
    lang: &'static languages::LanguageConfig,
}

impl SandboxSession {
    /// Starts a container from `image` (see [`prepare`]).
    pub async fn open(
        docker: &bollard::Docker,
        lang: &'static languages::LanguageConfig,
        image: &str,
    ) -> Result<Self, RunnerError> {
        Ok(Self {
            sandbox: Sandbox::create(docker, image).await?,
            lang,
        })
    }

    /// Writes `code` (replacing an earlier program) and compiles it.
    pub async fn compile(
        &self,
        code: &str,
        on_progress: &mut impl FnMut(Progress),
    ) -> Result<(Option<StepOutput>, bool), RunnerError> {
        check_limits(code, [], 0)?;
        compile_in_sandbox(&self.sandbox, self.lang, code, on_progress).await
    }

    /// Runs the compiled program once per input.
    pub async fn run(
        &self,
        inputs: &[String],
        on_progress: &mut impl FnMut(Progress),
    ) -> Result<Vec<StepOutput>, RunnerError> {
        check_limits("", inputs.iter().map(String::as_str), inputs.len())?;
        let mut outputs = Vec::with_capacity(inputs.len());
        for (index, input) in inputs.iter().enumerate() {
            on_progress(Progress::Phase {
                message: format!("Eingabe {} von {} …", index + 1, inputs.len()),
            });
            outputs.push(run_input(&self.sandbox, self.lang, input).await?.into());
        }
        Ok(outputs)
    }

    /// Removes the container (best effort).
    pub async fn close(self) {
        self.sandbox.remove().await;
    }
}

/// Writes the source file and compiles it (if the language has a compile
/// step). Returns the compiler output and whether compiling succeeded.
async fn compile_in_sandbox(
    sandbox: &Sandbox,
    lang: &languages::LanguageConfig,
    code: &str,
    on_progress: &mut impl FnMut(Progress),
) -> Result<(Option<StepOutput>, bool), RunnerError> {
    let source_path = format!("{}/{}", docker::WORK_DIR, lang.source_file);
    sandbox
        .write_file(&source_path, normalize_newlines(code).as_bytes())
        .await?;

    let Some(cmd) = lang.compile_cmd else {
        return Ok((None, true));
    };
    on_progress(Progress::Phase {
        message: "Kompiliere …".into(),
    });
    let out = sandbox
        .exec_with_stdin_file(cmd, None, lang.compile_timeout)
        .await?;
    let ok = !out.timed_out && out.exit_code == Some(0);
    Ok((Some(StepOutput::from(out)), ok))
}

/// Runs the compiled program once with `input` as stdin.
async fn run_input(
    sandbox: &Sandbox,
    lang: &languages::LanguageConfig,
    input: &str,
) -> Result<docker::ExecOutput, RunnerError> {
    sandbox
        .write_file(STDIN_FILE, normalize_newlines(input).as_bytes())
        .await?;
    sandbox
        .exec_with_stdin_file(lang.run_cmd, Some(STDIN_FILE), lang.run_timeout)
        .await
}

type SandboxResult = (Option<StepOutput>, bool, Vec<TestResult>);

async fn run_in_sandbox(
    sandbox: &Sandbox,
    lang: &languages::LanguageConfig,
    req: &RunRequest,
    on_progress: &mut impl FnMut(Progress),
) -> Result<SandboxResult, RunnerError> {
    let (compile, compile_ok) = compile_in_sandbox(sandbox, lang, &req.code, on_progress).await?;
    if !compile_ok {
        return Ok((compile, false, Vec::new()));
    }

    let mode = req.mode();
    let mut results = Vec::with_capacity(req.tests.len());
    for (index, test) in req.tests.iter().enumerate() {
        on_progress(Progress::Phase {
            message: format!("Test {} von {} …", index + 1, req.tests.len()),
        });
        let name = if test.name.trim().is_empty() {
            format!("Test {}", index + 1)
        } else {
            test.name.clone()
        };
        let result = match run_input(sandbox, lang, &test.input).await {
            Ok(out) => evaluate(
                index,
                name,
                out,
                &test.expected,
                &mode,
                req.forbid_trailing_newline,
                lang,
            ),
            Err(e) => TestResult {
                index,
                name,
                status: TestStatus::Error,
                output: None,
                diff: Vec::new(),
                message: Some(e.to_string()),
                only_trailing_whitespace_differs: false,
            },
        };
        results.push(result);
    }
    Ok((compile, true, results))
}

/// Classifies a finished test run. With `forbid_trailing_newline`, an
/// output ending with a line break fails even if it matches. Pure,
/// unit-tested.
pub fn evaluate(
    index: usize,
    name: String,
    out: docker::ExecOutput,
    expected: &str,
    mode: &CompareMode,
    forbid_trailing_newline: bool,
    lang: &languages::LanguageConfig,
) -> TestResult {
    let matches = mode.matches(expected, &out.stdout);
    let newline_forbidden = forbid_trailing_newline && ends_with_newline(&out.stdout);
    let diff = if matches {
        Vec::new()
    } else {
        line_diff(
            &mode.display_text(expected),
            &mode.display_text(&out.stdout),
        )
    };
    let (status, message) = if out.timed_out {
        (
            TestStatus::Timeout,
            Some(format!(
                "Zeitlimit von {} s überschritten – Prozess wurde beendet (Endlosschleife? Wartet das Programm auf mehr Eingaben?).",
                lang.run_timeout.as_secs()
            )),
        )
    } else if out.exit_code != Some(0) {
        let msg = match out.exit_code {
            Some(137) => "Prozess wurde vom System beendet (Exit-Code 137) – evtl. Speicherlimit von 512 MB überschritten.".to_string(),
            Some(c) => format!("Programm wurde mit Exit-Code {c} beendet."),
            None => "Exit-Code unbekannt.".to_string(),
        };
        (TestStatus::RuntimeError, Some(msg))
    } else if newline_forbidden {
        (
            TestStatus::Failed,
            Some(TRAILING_NEWLINE_FORBIDDEN.to_string()),
        )
    } else if matches {
        (TestStatus::Passed, None)
    } else {
        (TestStatus::Failed, None)
    };
    let only_trailing_whitespace_differs = status == TestStatus::Failed
        && !matches
        && only_trailing_whitespace_differs(expected, &out.stdout);
    let message = match (message, out.stdout_truncated) {
        (m, true) => Some(
            [
                m.unwrap_or_default(),
                "Ausgabe wurde nach 64 KB abgeschnitten.".into(),
            ]
            .join(" ")
            .trim()
            .to_string(),
        ),
        (m, false) => m,
    };
    TestResult {
        index,
        name,
        status,
        output: Some(out.into()),
        diff,
        message,
        only_trailing_whitespace_differs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docker::ExecOutput;

    fn out(stdout: &str, exit: Option<i64>, timed_out: bool) -> ExecOutput {
        ExecOutput {
            stdout: stdout.into(),
            exit_code: exit,
            timed_out,
            ..Default::default()
        }
    }

    fn py() -> &'static languages::LanguageConfig {
        languages::get("python").unwrap()
    }

    #[test]
    fn evaluate_passed_with_crlf_expected() {
        let r = evaluate(
            0,
            "t".into(),
            out("42\n", Some(0), false),
            "42\r\n",
            &CompareMode::lines(true),
            false,
            py(),
        );
        assert_eq!(r.status, TestStatus::Passed);
        assert!(r.diff.is_empty());
    }

    #[test]
    fn evaluate_failed_has_diff() {
        let r = evaluate(
            0,
            "t".into(),
            out("41\n", Some(0), false),
            "42\n",
            &CompareMode::lines(true),
            false,
            py(),
        );
        assert_eq!(r.status, TestStatus::Failed);
        assert_eq!(r.diff.len(), 2);
    }

    #[test]
    fn evaluate_timeout_and_runtime_error() {
        let r = evaluate(
            0,
            "t".into(),
            out("", None, true),
            "x",
            &CompareMode::lines(true),
            false,
            py(),
        );
        assert_eq!(r.status, TestStatus::Timeout);
        assert!(r.message.unwrap().contains("5 s"));
        let r = evaluate(
            0,
            "t".into(),
            out("x\n", Some(1), false),
            "x",
            &CompareMode::lines(true),
            false,
            py(),
        );
        assert_eq!(r.status, TestStatus::RuntimeError);
    }

    #[test]
    fn evaluate_marks_truncation() {
        let mut o = out("x", Some(0), false);
        o.stdout_truncated = true;
        let r = evaluate(
            0,
            "t".into(),
            o,
            "y",
            &CompareMode::lines(true),
            false,
            py(),
        );
        assert!(r.message.unwrap().contains("64 KB"));
    }

    #[test]
    fn image_missing_message_is_actionable() {
        let e = |dev_hint: String| RunnerError::ImageMissing {
            language: "Python 3.12".into(),
            tried: "a, b".into(),
            dev_hint,
            detail: "denied".into(),
        };
        let msg = e(dev_build_hint(py())).to_string();
        assert!(msg.contains("docker build -t code-pruefer-python:dev images/python"));
        assert!(msg.contains("Images aktualisieren"));
        assert!(msg.contains("gesucht: a, b"));
        assert!(msg.contains("denied"));
        // Release builds: no developer instructions.
        let msg = e(String::new()).to_string();
        assert!(!msg.contains("docker build"), "{msg}");
        assert!(msg.ends_with("neu installieren."), "{msg}");
    }

    #[test]
    fn too_many_tests_are_rejected() {
        let req = |n: usize| RunRequest {
            language: "python".into(),
            code: "print(1)".into(),
            tests: (0..n)
                .map(|_| TestCase {
                    name: String::new(),
                    input: String::new(),
                    expected: "1".into(),
                })
                .collect(),
            trim_whitespace: true,
            compare_mode: None,
            forbid_trailing_newline: false,
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // Rejected before any engine access.
        let msg = rt
            .block_on(run_submission(req(MAX_TESTS + 1), |_| {}))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("Zu viele Testfälle (51)"), "{msg}");
        assert!(msg.contains("höchstens 50"), "{msg}");
    }

    #[test]
    fn run_request_defaults_trim_on() {
        let req: RunRequest = serde_json::from_str(
            r#"{"language":"python","code":"print(1)","tests":[{"input":"","expected":"1"}]}"#,
        )
        .unwrap();
        assert!(req.trim_whitespace);
        assert_eq!(req.mode(), CompareMode::lines(true));
        assert!(!req.forbid_trailing_newline);
        let req: RunRequest = serde_json::from_str(
            r#"{"language":"python","code":"","tests":[],"forbidTrailingNewline":true}"#,
        )
        .unwrap();
        assert!(req.forbid_trailing_newline);
    }

    fn eval_forbid(stdout: &str, expected: &str, mode: CompareMode) -> TestResult {
        evaluate(
            0,
            "t".into(),
            out(stdout, Some(0), false),
            expected,
            &mode,
            true,
            py(),
        )
    }

    #[test]
    fn forbidden_trailing_newline_fails_in_every_mode() {
        let modes = [
            CompareMode::lines(true),
            CompareMode::lines(false),
            CompareMode::Tokens {
                case_sensitive: true,
            },
            CompareMode::Float {
                abs_tol: 1e-6,
                rel_tol: 1e-6,
            },
        ];
        for mode in modes {
            // Fails although the output equals the expected output.
            for stdout in ["42\n", "42\r\n", "42\r", "\n"] {
                let expected = if stdout == "\n" { "" } else { "42\n" };
                let r = eval_forbid(stdout, expected, mode);
                assert_eq!(r.status, TestStatus::Failed, "{mode:?} {stdout:?}");
                assert_eq!(
                    r.message.as_deref(),
                    Some(TRAILING_NEWLINE_FORBIDDEN),
                    "{mode:?}"
                );
            }
            // Without a final line break: compared as usual.
            let r = eval_forbid("42", "42", mode);
            assert_eq!(r.status, TestStatus::Passed, "{mode:?}");
            assert!(r.message.is_none());
            let r = eval_forbid("41", "42", mode);
            assert_eq!(r.status, TestStatus::Failed, "{mode:?}");
            assert!(r.message.is_none());
            // Empty output has no line break.
            let r = eval_forbid("", "", mode);
            assert_eq!(r.status, TestStatus::Passed, "{mode:?}");
        }
        // Expected output with a line break: trimmed lines still pass,
        // exact lines fail (expected and actual differ at the end).
        assert_eq!(
            eval_forbid("42", "42\n", CompareMode::lines(true)).status,
            TestStatus::Passed
        );
        let r = eval_forbid("42", "42\n", CompareMode::lines(false));
        assert_eq!(r.status, TestStatus::Failed);
        assert!(r.only_trailing_whitespace_differs);
        // Off: line breaks at the end are fine.
        let r = evaluate(
            0,
            "t".into(),
            out("42\n", Some(0), false),
            "42\n",
            &CompareMode::lines(false),
            false,
            py(),
        );
        assert_eq!(r.status, TestStatus::Passed);
    }

    #[test]
    fn forbidden_newline_does_not_hide_runtime_errors() {
        let r = evaluate(
            0,
            "t".into(),
            out("x\n", Some(1), false),
            "x\n",
            &CompareMode::lines(true),
            true,
            py(),
        );
        assert_eq!(r.status, TestStatus::RuntimeError);
        assert!(!r.message.unwrap().contains("Zeilenumbruch"));
    }

    #[test]
    fn evaluate_flags_trailing_whitespace_only_differences() {
        let exact = CompareMode::lines(false);
        let eval = |stdout: &str, expected: &str, mode: &CompareMode| {
            evaluate(
                0,
                "t".into(),
                out(stdout, Some(0), false),
                expected,
                mode,
                false,
                py(),
            )
        };
        let r = eval("Hallo\n", "Hallo", &exact);
        assert_eq!(r.status, TestStatus::Failed);
        assert!(r.only_trailing_whitespace_differs);
        // The diff shows the difference (final line break).
        assert_eq!(r.diff.len(), 2);
        assert!(r.diff[1].final_newline);
        assert!(eval("a  \nb", "a\nb", &exact).only_trailing_whitespace_differs);
        assert!(!eval("b\n", "a\n", &exact).only_trailing_whitespace_differs);
        // Passed tests never carry the flag.
        let r = eval("Hallo\n", "Hallo", &CompareMode::lines(true));
        assert_eq!(r.status, TestStatus::Passed);
        assert!(!r.only_trailing_whitespace_differs);
        // Runtime errors neither.
        let r = evaluate(
            0,
            "t".into(),
            out("a\n", Some(1), false),
            "a",
            &exact,
            false,
            py(),
        );
        assert!(!r.only_trailing_whitespace_differs);
        let json = serde_json::to_value(eval("a\n", "a", &exact)).unwrap();
        assert_eq!(json["onlyTrailingWhitespaceDiffers"], true);
    }

    #[test]
    fn compare_mode_overrides_trim_flag() {
        let req: RunRequest = serde_json::from_str(
            r#"{"language":"python","code":"","tests":[],"trimWhitespace":false,
                "compareMode":{"kind":"tokens","caseSensitive":false}}"#,
        )
        .unwrap();
        assert_eq!(
            req.mode(),
            CompareMode::Tokens {
                case_sensitive: false
            }
        );
        let req: RunRequest = serde_json::from_str(
            r#"{"language":"python","code":"","tests":[],"trimWhitespace":false}"#,
        )
        .unwrap();
        assert_eq!(req.mode(), CompareMode::lines(false));
    }

    #[test]
    fn evaluate_in_token_and_float_mode() {
        let tokens = CompareMode::Tokens {
            case_sensitive: true,
        };
        let r = evaluate(
            0,
            "t".into(),
            out("1  2\n3\n", Some(0), false),
            "1 2 3",
            &tokens,
            false,
            py(),
        );
        assert_eq!(r.status, TestStatus::Passed);
        let float = CompareMode::Float {
            abs_tol: 1e-3,
            rel_tol: 0.0,
        };
        let r = evaluate(
            0,
            "t".into(),
            out("0.3334\n", Some(0), false),
            "0.3333",
            &float,
            false,
            py(),
        );
        assert_eq!(r.status, TestStatus::Passed);
        let r = evaluate(
            0,
            "t".into(),
            out("0.34\n", Some(0), false),
            "0.3333",
            &float,
            false,
            py(),
        );
        assert_eq!(r.status, TestStatus::Failed);
        assert!(!r.diff.is_empty());
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn invalid_tolerance_is_rejected_before_engine_access() {
        let req = RunRequest {
            language: "python".into(),
            code: "print(1)".into(),
            tests: Vec::new(),
            trim_whitespace: true,
            compare_mode: Some(CompareMode::Float {
                abs_tol: -1.0,
                rel_tol: 0.0,
            }),
            forbid_trailing_newline: false,
        };
        let msg = block_on(run_submission(req, |_| {}))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("Ungültige Toleranz"), "{msg}");
    }

    #[test]
    fn collect_outputs_checks_limits_before_engine_access() {
        let inputs = vec![String::new(); MAX_TESTS + 1];
        let msg = block_on(collect_outputs("python", "print(1)", &inputs, None, |_| {}))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("höchstens 50"), "{msg}");
        let big = vec!["x".repeat(MAX_INPUT_BYTES + 1)];
        let msg = block_on(collect_outputs("python", "print(1)", &big, None, |_| {}))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("zu groß"), "{msg}");
        let msg = block_on(collect_outputs("cobol", "", &[], None, |_| {}))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("Unbekannte Sprache"), "{msg}");
    }
}
