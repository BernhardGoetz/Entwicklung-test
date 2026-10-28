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
//! Without `CODE_PRUEFER_*_IMAGE` set, the tests use the `:dev` tags above
//! (env overrides and the `:dev` fallback only exist in debug builds, which
//! `cargo test` uses).

use code_pruefer_lib::runner::compare::CompareMode;
use code_pruefer_lib::runner::{self, docker, RunReport, RunRequest, TestCase, TestStatus};

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
            compare_mode: None,
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

/// Nothing a test leaves running survives into the next test: neither a
/// detached background process nor one that closed its output.
#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn left_over_processes_are_killed_after_each_test() {
    let code = r#"
import os, time
mode = input()
if mode == 'fork':
    if os.fork() == 0:
        os.setsid()
        for fd in (0, 1, 2):
            os.close(fd)
        time.sleep(100)
        os._exit(0)
    print('ok')
elif mode == 'close':
    print('ok', flush=True)
    os.close(1)
    os.close(2)
    time.sleep(100)
else:
    me = os.getpid()
    # Killed orphans stay as zombies of PID 1 (`sleep`); only live ones count.
    def alive(p):
        return open(f'/proc/{p}/stat').read().rsplit(')', 1)[1].split()[0] != 'Z'
    print(len([p for p in os.listdir('/proc') if p.isdigit() and int(p) not in (1, me) and alive(p)]))
"#;
    let r = run(
        "python",
        code,
        vec![
            tc("fork\n", "ok\n"),
            tc("count\n", "0\n"),
            tc("close\n", "ok\n"),
            tc("count\n", "0\n"),
        ],
    )
    .await;
    assert_eq!(
        statuses(&r),
        vec![
            TestStatus::Passed,
            TestStatus::Passed,
            TestStatus::Timeout,
            TestStatus::Passed
        ],
        "{:?}",
        r.tests
    );
    assert_eq!(r.tests[2].output.as_ref().unwrap().exit_code, None);
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
async fn tokens_mode_ignores_line_structure() {
    setup();
    let code = "n = int(input())\nfor i in range(n):\n    print(i, end='  ')\n";
    let request = |mode| RunRequest {
        language: "python".into(),
        code: code.into(),
        tests: vec![tc("3\n", "0\n1\n2\n")],
        trim_whitespace: true,
        compare_mode: Some(mode),
    };
    let tokens = CompareMode::Tokens {
        case_sensitive: true,
    };
    let r = runner::run_submission(request(tokens), |_| {})
        .await
        .unwrap();
    assert_eq!(statuses(&r), vec![TestStatus::Passed], "{:?}", r.tests[0]);
    let r = runner::run_submission(request(CompareMode::lines(true)), |_| {})
        .await
        .unwrap();
    assert_eq!(statuses(&r), vec![TestStatus::Failed]);
}

#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn collect_outputs_runs_every_input() {
    setup();
    let inputs: Vec<String> = vec!["2\n".into(), "5\n".into(), "x\n".into()];
    let mut phases = Vec::new();
    let c = runner::collect_outputs("python", "print(int(input()) ** 2)\n", &inputs, None, |p| {
        phases.push(p)
    })
    .await
    .unwrap();
    assert!(c.compile_ok);
    let stdout: Vec<&str> = c.outputs.iter().map(|o| o.stdout.as_str()).collect();
    assert_eq!(stdout, vec!["4\n", "25\n", ""]);
    assert_eq!(c.outputs[0].exit_code, Some(0));
    assert_ne!(c.outputs[2].exit_code, Some(0));
    assert!(c.outputs[2].stderr.contains("ValueError"));
    assert!(!phases.is_empty());

    // Java: compiled once; a compile error gives no outputs.
    let java = "public class Main { public static void main(String[] a) { \
                System.out.println(new java.util.Scanner(System.in).nextInt() + 1); } }\n";
    let c = runner::collect_outputs("java", java, &inputs[..2], None, |_| {})
        .await
        .unwrap();
    assert!(c.compile_ok, "{:?}", c.compile);
    let stdout: Vec<&str> = c.outputs.iter().map(|o| o.stdout.as_str()).collect();
    assert_eq!(stdout, vec!["3\n", "6\n"]);
    let c = runner::collect_outputs("java", "class Main {", &inputs, None, |_| {})
        .await
        .unwrap();
    assert!(!c.compile_ok);
    assert!(c.outputs.is_empty());
    assert!(
        sandbox_containers().is_empty(),
        "sandbox containers left behind"
    );
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
    assert!(
        sandbox_containers().is_empty(),
        "sandbox containers left behind"
    );
}

fn sandbox_containers() -> Vec<String> {
    let out = std::process::Command::new("docker")
        .args(["ps", "-aq", "--filter", "label=de.code-pruefer.sandbox"])
        .output()
        .expect("docker CLI");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .map(String::from)
        .collect()
}

/// `docker create` of an idle container with the sandbox labels.
fn create_labeled(instance: &str) -> String {
    let instance_label = format!("{}={instance}", docker::INSTANCE_LABEL);
    let out = std::process::Command::new("docker")
        .args(["create", "--label", "de.code-pruefer.sandbox=1", "--label"])
        .arg(&instance_label)
        .args(["code-pruefer-python:dev", "sleep", "infinity"])
        .output()
        .expect("docker CLI");
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn orphaned_sandboxes_are_removed() {
    let client = docker::connect().await.expect("engine");
    let own = create_labeled(docker::instance_id());
    let foreign = create_labeled("anderer-prozess");
    let present = |id: &str| {
        sandbox_containers()
            .iter()
            .any(|c| id.starts_with(c.as_str()))
    };
    assert!(present(&own) && present(&foreign));

    // Own left-over: removed at once; another instance's: only when old.
    docker::remove_orphans(&client, docker::ORPHAN_AGE)
        .await
        .unwrap();
    assert!(!present(&own));
    assert!(present(&foreign));
    docker::remove_orphans(&client, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(!present(&foreign));
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
            compare_mode: None,
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
            compare_mode: None,
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
