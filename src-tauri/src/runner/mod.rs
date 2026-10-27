//! Runs a submission against its test cases inside a sandbox container.

pub mod compare;
pub mod docker;
pub mod languages;

use serde::{Deserialize, Serialize};

use compare::{line_diff, normalize_newlines, normalize_output, outputs_match, DiffLine};
use docker::{PullProgress, Sandbox, STDIN_FILE};

/// Maximum accepted size of source code and of a single test input/expectation.
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("Keine laufende Container-Engine gefunden. Bitte Docker Desktop oder Podman starten.")]
    NoEngine,
    #[error("Unbekannte Sprache: {0}")]
    UnknownLanguage(String),
    #[error("Image {0} konnte nicht geladen werden: {1}")]
    ImagePull(String, String),
    #[error(
        "Das Runner-Image für {language} ist nicht vorhanden (gesucht: {tried}) und konnte nicht \
         heruntergeladen werden ({detail}). Bitte das Image einmalig lokal bauen – in PowerShell \
         bzw. im Terminal im Projektordner, der den Ordner \"images\" enthält: \
         docker build -t {local_image} images/{id} – und danach erneut auf „Prüfen“ klicken."
    )]
    ImageMissing {
        language: String,
        id: String,
        tried: String,
        local_image: String,
        detail: String,
    },
    #[error("Fehler der Container-Engine: {0}")]
    Docker(#[from] bollard::errors::Error),
    #[error("{0}")]
    Other(String),
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
    #[serde(default = "default_true")]
    pub trim_whitespace: bool,
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
}

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
    Phase { message: String },
}

pub async fn run_submission(
    req: RunRequest,
    mut on_progress: impl FnMut(Progress),
) -> Result<RunReport, RunnerError> {
    let lang = languages::get(&req.language)
        .ok_or_else(|| RunnerError::UnknownLanguage(req.language.clone()))?;
    if req.code.len() > MAX_INPUT_BYTES
        || req
            .tests
            .iter()
            .any(|t| t.input.len() > MAX_INPUT_BYTES || t.expected.len() > MAX_INPUT_BYTES)
    {
        return Err(RunnerError::Other(
            "Code oder Testdaten sind zu groß (max. 1 MB je Feld).".into(),
        ));
    }
    let hint = languages::precheck(lang, &req.code);

    let docker = docker::connect().await?;
    let image = resolve_image(&docker, lang, &mut on_progress).await?;

    on_progress(Progress::Phase {
        message: "Starte Container …".into(),
    });
    let sandbox = Sandbox::create(&docker, &image).await?;
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

/// Picks the first locally available image candidate; if none is present,
/// pulls the primary one. A failed pull becomes an actionable German error.
async fn resolve_image(
    docker: &bollard::Docker,
    lang: &languages::LanguageConfig,
    on_progress: &mut impl FnMut(Progress),
) -> Result<String, RunnerError> {
    let candidates = lang.image_candidates();
    on_progress(Progress::Phase {
        message: format!("Prüfe Image {} …", candidates[0]),
    });
    for image in &candidates {
        if docker::image_exists(docker, image).await? {
            return Ok(image.clone());
        }
    }
    let primary = &candidates[0];
    match docker::pull_image(docker, primary, |p| on_progress(Progress::Pull(p))).await {
        Ok(()) => Ok(primary.clone()),
        Err(RunnerError::ImagePull(_, detail)) => Err(RunnerError::ImageMissing {
            language: lang.name.to_string(),
            id: lang.id.to_string(),
            tried: candidates.join(", "),
            local_image: lang.local_image.to_string(),
            detail: detail.trim().replace('\n', " "),
        }),
        Err(e) => Err(e),
    }
}

type SandboxResult = (Option<StepOutput>, bool, Vec<TestResult>);

async fn run_in_sandbox(
    sandbox: &Sandbox,
    lang: &languages::LanguageConfig,
    req: &RunRequest,
    on_progress: &mut impl FnMut(Progress),
) -> Result<SandboxResult, RunnerError> {
    let source_path = format!("{}/{}", docker::WORK_DIR, lang.source_file);
    sandbox
        .write_file(&source_path, normalize_newlines(&req.code).as_bytes())
        .await?;

    let mut compile = None;
    if let Some(cmd) = lang.compile_cmd {
        on_progress(Progress::Phase {
            message: "Kompiliere …".into(),
        });
        let out = sandbox
            .exec_with_stdin_file(cmd, None, lang.compile_timeout)
            .await?;
        let ok = !out.timed_out && out.exit_code == Some(0);
        compile = Some(StepOutput::from(out));
        if !ok {
            return Ok((compile, false, Vec::new()));
        }
    }

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
        let run = async {
            sandbox
                .write_file(STDIN_FILE, normalize_newlines(&test.input).as_bytes())
                .await?;
            sandbox
                .exec_with_stdin_file(lang.run_cmd, Some(STDIN_FILE), lang.run_timeout)
                .await
        };
        let result = match run.await {
            Ok(out) => evaluate(index, name, out, &test.expected, req.trim_whitespace, lang),
            Err(e) => TestResult {
                index,
                name,
                status: TestStatus::Error,
                output: None,
                diff: Vec::new(),
                message: Some(e.to_string()),
            },
        };
        results.push(result);
    }
    Ok((compile, true, results))
}

/// Classifies a finished test run. Pure, unit-tested.
pub fn evaluate(
    index: usize,
    name: String,
    out: docker::ExecOutput,
    expected: &str,
    trim: bool,
    lang: &languages::LanguageConfig,
) -> TestResult {
    let matches = outputs_match(expected, &out.stdout, trim);
    let diff = if matches {
        Vec::new()
    } else {
        line_diff(
            &normalize_output(expected, trim),
            &normalize_output(&out.stdout, trim),
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
    } else if matches {
        (TestStatus::Passed, None)
    } else {
        (TestStatus::Failed, None)
    };
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
            true,
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
            true,
            py(),
        );
        assert_eq!(r.status, TestStatus::Failed);
        assert_eq!(r.diff.len(), 2);
    }

    #[test]
    fn evaluate_timeout_and_runtime_error() {
        let r = evaluate(0, "t".into(), out("", None, true), "x", true, py());
        assert_eq!(r.status, TestStatus::Timeout);
        assert!(r.message.unwrap().contains("5 s"));
        let r = evaluate(0, "t".into(), out("x\n", Some(1), false), "x", true, py());
        assert_eq!(r.status, TestStatus::RuntimeError);
    }

    #[test]
    fn evaluate_marks_truncation() {
        let mut o = out("x", Some(0), false);
        o.stdout_truncated = true;
        let r = evaluate(0, "t".into(), o, "y", true, py());
        assert!(r.message.unwrap().contains("64 KB"));
    }

    #[test]
    fn image_missing_message_is_actionable() {
        let e = RunnerError::ImageMissing {
            language: "Python 3.12".into(),
            id: "python".into(),
            tried: "a, b".into(),
            local_image: "code-pruefer-python:dev".into(),
            detail: "denied".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("docker build -t code-pruefer-python:dev images/python"));
        assert!(msg.contains("gesucht: a, b"));
        assert!(msg.contains("denied"));
    }

    #[test]
    fn run_request_defaults_trim_on() {
        let req: RunRequest = serde_json::from_str(
            r#"{"language":"python","code":"print(1)","tests":[{"input":"","expected":"1"}]}"#,
        )
        .unwrap();
        assert!(req.trim_whitespace);
    }
}
