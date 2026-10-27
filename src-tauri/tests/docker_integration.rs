//! Integration tests against a real Docker/Podman engine.
//!
//! They are `#[ignore]`d because they need a running engine and the runner
//! images. Run them with locally built images:
//!
//! ```sh
//! docker build -t code-pruefer-python:dev images/python
//! docker build -t code-pruefer-java:dev images/java
//! cd src-tauri && cargo test -- --ignored --test-threads=1
//! ```
//!
//! Without `CODE_PRUEFER_*_IMAGE` set, the tests use the `:dev` tags above.

use code_pruefer_lib::runner::{self, RunReport, RunRequest, TestCase, TestStatus};

fn setup() {
    for (var, img) in [
        ("CODE_PRUEFER_PYTHON_IMAGE", "code-pruefer-python:dev"),
        ("CODE_PRUEFER_JAVA_IMAGE", "code-pruefer-java:dev"),
    ] {
        if std::env::var(var).is_err() {
            std::env::set_var(var, img);
        }
    }
}

fn tc(input: &str, expected: &str) -> TestCase {
    TestCase {
        name: String::new(),
        input: input.into(),
        expected: expected.into(),
    }
}

async fn run(language: &str, code: &str, tests: Vec<TestCase>) -> RunReport {
    setup();
    runner::run_submission(
        RunRequest {
            language: language.into(),
            code: code.into(),
            tests,
            trim_whitespace: true,
        },
        |_| {},
    )
    .await
    .expect("run_submission failed")
}

fn statuses(r: &RunReport) -> Vec<TestStatus> {
    r.tests.iter().map(|t| t.status).collect()
}

#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn python_pass_fail_crlf() {
    let code = "a, b = map(int, input().split())\r\nprint(a + b)\r\n";
    let r = run(
        "python",
        code,
        vec![tc("1 2\r\n", "3\r\n"), tc("2 2\n", "5\n")],
    )
    .await;
    assert!(r.compile_ok);
    assert_eq!(statuses(&r), vec![TestStatus::Passed, TestStatus::Failed]);
    assert!(!r.tests[1].diff.is_empty());
}

#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn python_syntax_error_is_compile_error() {
    let r = run("python", "print(\n", vec![tc("", "")]).await;
    assert!(!r.compile_ok);
    assert!(r.compile.unwrap().stderr.contains("SyntaxError"));
}

#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn python_timeout_is_killed_and_next_test_still_runs() {
    let code = "import sys\nif input() == 'loop':\n    while True: pass\nprint('ok')\n";
    let r = run("python", code, vec![tc("loop\n", ""), tc("x\n", "ok\n")]).await;
    assert_eq!(statuses(&r), vec![TestStatus::Timeout, TestStatus::Passed]);
}

#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn python_runtime_error_and_truncation() {
    let r = run(
        "python",
        "import sys\nif input() == 'big':\n    print('x' * 200000)\nelse:\n    raise ValueError('kaputt')\n",
        vec![tc("err\n", ""), tc("big\n", "")],
    )
    .await;
    assert_eq!(r.tests[0].status, TestStatus::RuntimeError);
    assert!(r.tests[0]
        .output
        .as_ref()
        .unwrap()
        .stderr
        .contains("ValueError"));
    assert!(r.tests[1].output.as_ref().unwrap().stdout_truncated);
}

#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn sandbox_is_hardened() {
    let code = r#"
import os, socket
print(os.getuid())
try:
    open('/etc/x', 'w'); print('rootfs-writable')
except OSError:
    print('rootfs-ro')
try:
    socket.create_connection(('1.1.1.1', 53), timeout=2); print('net')
except OSError:
    print('no-net')
open('/work/ok.txt', 'w').write('x'); print('work-ok')
"#;
    let r = run(
        "python",
        code,
        vec![tc("", "1000\nrootfs-ro\nno-net\nwork-ok\n")],
    )
    .await;
    assert_eq!(statuses(&r), vec![TestStatus::Passed], "{:?}", r.tests[0]);
}

#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn java_compile_and_run() {
    let code = "import java.util.*;\npublic class Main { public static void main(String[] a) { Scanner s = new Scanner(System.in); System.out.println(\"Hallo \" + s.nextLine() + \" ä\"); } }\n";
    let r = run("java", code, vec![tc("Welt\n", "Hallo Welt ä\n")]).await;
    assert!(r.compile_ok, "{:?}", r.compile);
    assert_eq!(statuses(&r), vec![TestStatus::Passed], "{:?}", r.tests[0]);
}

#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn java_wrong_class_name_gives_hint() {
    let r = run("java", "public class Aufgabe { }\n", vec![tc("", "")]).await;
    assert!(!r.compile_ok);
    assert!(r.hint.is_some());
}

#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn containers_are_removed() {
    let _ = run("python", "print(1)\n", vec![tc("", "1")]).await;
    let out = std::process::Command::new("docker")
        .args(["ps", "-aq", "--filter", "label=de.code-pruefer.sandbox"])
        .output()
        .expect("docker CLI");
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "sandbox containers left behind"
    );
}

#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn falls_back_to_local_dev_image_without_override() {
    // Simulates a Windows start without env vars: the published default image
    // is not present locally, so the locally built `:dev` tag must be used.
    let old = std::env::var("CODE_PRUEFER_PYTHON_IMAGE").ok();
    std::env::remove_var("CODE_PRUEFER_PYTHON_IMAGE");
    let r = runner::run_submission(
        RunRequest {
            language: "python".into(),
            code: "print(input())".into(),
            tests: vec![tc("x\n", "x\n")],
            trim_whitespace: true,
        },
        |_| {},
    )
    .await;
    if let Some(v) = old {
        std::env::set_var("CODE_PRUEFER_PYTHON_IMAGE", v);
    }
    let r = r.expect("run_submission failed");
    assert_eq!(r.image, "code-pruefer-python:dev");
    assert_eq!(statuses(&r), vec![TestStatus::Passed]);
}

#[tokio::test]
#[ignore = "benötigt Docker"]
async fn missing_override_image_gives_actionable_error() {
    let old = std::env::var("CODE_PRUEFER_PYTHON_IMAGE").ok();
    std::env::set_var(
        "CODE_PRUEFER_PYTHON_IMAGE",
        "code-pruefer-does-not-exist.invalid/nope:0",
    );
    let r = runner::run_submission(
        RunRequest {
            language: "python".into(),
            code: "print(1)".into(),
            tests: vec![tc("", "1")],
            trim_whitespace: true,
        },
        |_| {},
    )
    .await;
    match old {
        Some(v) => std::env::set_var("CODE_PRUEFER_PYTHON_IMAGE", v),
        None => std::env::remove_var("CODE_PRUEFER_PYTHON_IMAGE"),
    }
    let msg = match r {
        Ok(_) => panic!("must fail"),
        Err(e) => e.to_string(),
    };
    assert!(
        msg.contains("docker build -t code-pruefer-python:dev images/python"),
        "{msg}"
    );
}

#[tokio::test]
#[ignore = "benötigt Docker"]
async fn start_is_a_no_op_when_engine_already_answers() {
    let mut messages = Vec::new();
    code_pruefer_lib::engine::start(code_pruefer_lib::engine::Engine::Docker, |m| {
        messages.push(m)
    })
    .await
    .expect("engine should already be reachable");
    assert!(messages.is_empty(), "{messages:?}");
}
