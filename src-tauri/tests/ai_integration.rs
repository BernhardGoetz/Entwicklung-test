//! Integration tests for the local AI: a real llama-server and, for the
//! pipeline, the real sandbox (Docker/Podman with the runner images).
//!
//! `#[ignore]`d; they need
//! - `CODE_PRUEFER_LLAMA_SERVER`: path to llama-server (debug builds only),
//!   e.g. `scripts/fetch-llama-server.sh linux-x64 /tmp/llama` and
//!   `/tmp/llama/llama-server`,
//! - `CODE_PRUEFER_TEST_MODEL`: a GGUF model, ideally
//!   Qwen2.5-Coder-1.5B-Instruct Q5_K_M (any GGUF with a chat template works
//!   for the plumbing test),
//! - for `pipeline_on_sample_tasks` additionally the `:dev` runner images
//!   (see `docker_integration.rs`).
//!
//! ```sh
//! cd src-tauri && cargo test --test ai_integration -- --ignored --nocapture --test-threads=1
//! ```

use std::path::PathBuf;
use std::time::Instant;

use code_pruefer_lib::ai::llama::{self, ChatMessage, ChatRequest, LlamaClient, Llm};
use code_pruefer_lib::ai::pipeline::{
    self, ConfidenceLevel, GenerateRequest, SandboxExecutor, Settings,
};
use code_pruefer_lib::ai::AiError;
use tokio_util::sync::CancellationToken;

fn model() -> PathBuf {
    PathBuf::from(std::env::var("CODE_PRUEFER_TEST_MODEL").expect("CODE_PRUEFER_TEST_MODEL setzen"))
}

fn setup_images() {
    for (var, img) in [
        ("CODE_PRUEFER_PYTHON_IMAGE", "code-pruefer-python:dev"),
        ("CODE_PRUEFER_JAVA_IMAGE", "code-pruefer-java:dev"),
    ] {
        if std::env::var(var).is_err() {
            std::env::set_var(var, img);
        }
    }
}

#[tokio::test]
#[ignore = "benötigt llama-server (CODE_PRUEFER_LLAMA_SERVER) und ein GGUF-Modell (CODE_PRUEFER_TEST_MODEL)"]
async fn llama_server_answers_with_json_schema() {
    let cancel = CancellationToken::new();
    let started = Instant::now();
    let endpoint = llama::ensure_running(&model(), &cancel, |m| eprintln!("{m}"))
        .await
        .expect("llama-server starten");
    eprintln!(
        "Server bereit nach {:.1} s",
        started.elapsed().as_secs_f64()
    );

    // Reuse: same model, same endpoint.
    let again = llama::ensure_running(&model(), &cancel, |_| {})
        .await
        .unwrap();
    assert_eq!(again.base_url, endpoint.base_url);

    // Without the API key the server refuses.
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{}/v1/chat/completions", endpoint.base_url))
        .json(&serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);

    let client = LlamaClient::new(endpoint, cancel.clone()).unwrap();
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "inputs": { "type": "array", "maxItems": 3, "items": { "type": "string" } } },
        "required": ["inputs"]
    });
    let r = client
        .chat(&ChatRequest {
            messages: vec![
                ChatMessage::system("Answer with JSON."),
                ChatMessage::user("Give three test inputs for a program that doubles a number."),
            ],
            temperature: 0.7,
            max_tokens: 256,
            json_schema: Some(schema),
            seed: Some(1),
        })
        .await
        .expect("chat");
    eprintln!("Antwort: {}", r.content);
    eprintln!(
        "Token/s: {:?}, Tokens: {:?}, Ende: {:?}",
        r.tokens_per_second, r.completion_tokens, r.finish_reason
    );
    assert!(r.tokens_per_second.is_some());
    if r.finish_reason.as_deref() == Some("stop") {
        // Grammar-constrained: valid JSON of the schema even from a weak model.
        let v: serde_json::Value = serde_json::from_str(&r.content).expect("JSON laut Schema");
        assert!(v["inputs"].is_array());
        assert!(v["inputs"].as_array().unwrap().len() <= 3);
    }

    // Cancelling an ongoing request.
    let cancel2 = CancellationToken::new();
    let endpoint = llama::ensure_running(&model(), &cancel2, |_| {})
        .await
        .unwrap();
    let client = LlamaClient::new(endpoint, cancel2.clone()).unwrap();
    let c = cancel2.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        c.cancel();
    });
    let r = client
        .chat(&ChatRequest {
            messages: vec![ChatMessage::user("Write a very long story.")],
            temperature: 0.7,
            max_tokens: 4000,
            json_schema: None,
            seed: None,
        })
        .await;
    assert!(matches!(r, Err(AiError::Cancelled)), "{r:?}");
    llama::shutdown().await;
}

struct Sample {
    name: &'static str,
    language: &'static str,
    text: &'static str,
}

const SAMPLES: &[Sample] = &[
    Sample {
        name: "Python, mit Beispielen: Summe",
        language: "python",
        text: "Summe zweier Zahlen\n\nLies zwei ganze Zahlen a und b (durch ein Leerzeichen getrennt, \
               in einer Zeile) und gib ihre Summe aus.\n\nBeispiel 1\nEingabe:\n3 4\nAusgabe:\n7\n\n\
               Beispiel 2\nEingabe:\n-5 12\nAusgabe:\n7\n",
    },
    Sample {
        name: "Java, mit Beispiel: Fakultät",
        language: "java",
        text: "Fakultät\n\nLies eine ganze Zahl n (0 <= n <= 20) und gib n! aus.\n\nBeispiel\nEingabe:\n5\n\
               Ausgabe:\n120\n",
    },
    Sample {
        name: "Python, ohne Beispiele: Vokale zählen",
        language: "python",
        text: "Lies eine Zeile Text und gib aus, wie viele Vokale (a, e, i, o, u, Groß- und \
               Kleinschreibung egal) sie enthält.",
    },
    Sample {
        name: "Java, mehrdeutig/schwer",
        language: "java",
        text: "Schreibe ein Programm, das die Eingabe sinnvoll verarbeitet und das richtige Ergebnis \
               ausgibt. Beispiel: Eingabe 3, Ausgabe 11.",
    },
];

#[tokio::test]
#[ignore = "benötigt llama-server, ein GGUF-Modell und Docker mit den Runner-Images"]
async fn pipeline_on_sample_tasks() {
    setup_images();
    let cancel = CancellationToken::new();
    let endpoint = llama::ensure_running(&model(), &cancel, |m| eprintln!("{m}"))
        .await
        .expect("llama-server starten");
    let solutions: usize = std::env::var("CODE_PRUEFER_TEST_SOLUTIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let llm = LlamaClient::new(endpoint, cancel.clone()).unwrap();
    let exec = SandboxExecutor::connect(cancel.clone(), |m| eprintln!("{m}"))
        .await
        .expect("Engine");
    let only = std::env::var("CODE_PRUEFER_TEST_SAMPLE").ok();
    for s in SAMPLES
        .iter()
        .filter(|s| only.as_deref().is_none_or(|o| s.name.contains(o)))
    {
        let started = Instant::now();
        let req = GenerateRequest {
            task_text: s.text.into(),
            language: s.language.into(),
            compare_mode: None,
        };
        let r = pipeline::generate(
            &llm,
            &exec,
            &req,
            Settings {
                solutions,
                repair_rounds: 1,
            },
            &cancel,
            |p| eprintln!("  [{:?}] {}", p.phase, p.message),
        )
        .await
        .expect("Pipeline");
        eprintln!(
            "== {}: {:?}, {} Tests, {:.0} s, {:.1} Token/s\n   Gründe: {:?}\n   Statistik: {:?}",
            s.name,
            r.confidence.level,
            r.tests.len(),
            started.elapsed().as_secs_f64(),
            r.stats.tokens_per_second.unwrap_or(0.0),
            r.confidence.reasons,
            r.stats
        );
        for t in &r.tests {
            eprintln!(
                "   {}: {:?} -> {:?} ({:.0} %)",
                t.name,
                t.input,
                t.expected,
                t.agreement * 100.0
            );
        }
        // Invariants that hold for every model.
        if r.confidence.level == ConfidenceLevel::Red {
            assert!(r.tests.is_empty());
            assert!(r.reference_solution.is_none());
        } else {
            assert!(!r.tests.is_empty());
            assert!(r.reference_solution.is_some());
        }
        assert!(r.tests.len() <= 15);
    }
    llama::shutdown().await;
}

/// Scripted model with real programs: checks the pipeline against the real
/// sandbox (expected outputs come from executing the solutions).
struct ScriptedLlm {
    calls: std::sync::Mutex<usize>,
    solutions: Vec<&'static str>,
    structure: &'static str,
    inputs: &'static str,
}

impl Llm for ScriptedLlm {
    async fn chat(&self, req: &ChatRequest) -> Result<llama::ChatResponse, AiError> {
        let system = &req.messages[0].content;
        let content = if system.contains("analyse programming exercises") {
            self.structure.to_string()
        } else if system.contains("write test inputs") {
            self.inputs.to_string()
        } else if req.messages.len() > 2 {
            // Repair round.
            self.solutions[0].to_string()
        } else {
            let mut c = self.calls.lock().unwrap();
            *c += 1;
            self.solutions[*c % self.solutions.len()].to_string()
        };
        Ok(llama::ChatResponse {
            content,
            ..Default::default()
        })
    }
}

#[tokio::test]
#[ignore = "benötigt Docker und die Runner-Images"]
async fn pipeline_with_real_sandbox() {
    setup_images();
    let cancel = CancellationToken::new();
    let exec = SandboxExecutor::connect(cancel.clone(), |m| eprintln!("{m}"))
        .await
        .expect("Engine");
    let task = "Lies zwei ganze Zahlen a und b und gib a + b aus.\n\nBeispiel\nEingabe:\n3 4\nAusgabe:\n7\n";
    let req = |language: &str| GenerateRequest {
        task_text: task.into(),
        language: language.into(),
        compare_mode: None,
    };
    let structure = r#"{"inputFormat":"a b","outputFormat":"a+b","examples":[{"input":"3 4","output":"7"},{"input":"1 1","output":"2"}],"compareMode":"tokens"}"#;
    let inputs = r#"{"inputs":["0 0","-5 12","2000000000 2000000000"]}"#;

    let python = ScriptedLlm {
        calls: Default::default(),
        solutions: vec![
            "```python\na, b = map(int, input().split())\nprint(a + b)\n```",
            "```python\nimport sys\nprint(sum(map(int, sys.stdin.read().split())))\n```",
            // Wrong: fails the sheet example.
            "```python\na, b = map(int, input().split())\nprint(a * b)\n```",
            // Syntax error -> repaired with solution 0.
            "```python\nprint(\n```",
        ],
        structure,
        inputs,
    };
    let r = pipeline::generate(
        &python,
        &exec,
        &req("python"),
        Settings {
            solutions: 4,
            repair_rounds: 1,
        },
        &cancel,
        |_| {},
    )
    .await
    .expect("Pipeline Python");
    eprintln!("Python: {:?} {:?}\n{:?}", r.confidence, r.stats, r.tests);
    assert_eq!(r.stats.examples_found, 1, "invented example dropped");
    assert_eq!(r.stats.solutions_repaired, 1);
    assert_eq!(r.stats.solutions_passed, 3);
    assert_eq!(r.confidence.level, ConfidenceLevel::Green);
    let expected: Vec<&str> = r.tests.iter().map(|t| t.expected.as_str()).collect();
    assert_eq!(expected, vec!["7\n", "0\n", "7\n", "4000000000\n"]);
    assert_eq!(
        r.compare_mode,
        code_pruefer_lib::runner::compare::CompareMode::Tokens {
            case_sensitive: true
        }
    );

    let java = ScriptedLlm {
        calls: Default::default(),
        solutions: vec![
            "```java\nimport java.util.Scanner;\npublic class Main { public static void main(String[] x) { Scanner s = new Scanner(System.in); long a = s.nextLong(), b = s.nextLong(); System.out.println(a + b); } }\n```",
            "```java\nimport java.util.Scanner;\npublic class Main { public static void main(String[] x) { Scanner s = new Scanner(System.in); long a = s.nextLong(), b = s.nextLong(); System.out.println(a + b); } }\n```",
            // int overflow on the last input: outvoted.
            "```java\nimport java.util.Scanner;\npublic class Main { public static void main(String[] x) { Scanner s = new Scanner(System.in); int a = s.nextInt(), b = s.nextInt(); System.out.println(a + b); } }\n```",
        ],
        structure,
        inputs,
    };
    let r = pipeline::generate(
        &java,
        &exec,
        &req("java"),
        Settings {
            solutions: 3,
            repair_rounds: 1,
        },
        &cancel,
        |_| {},
    )
    .await
    .expect("Pipeline Java");
    eprintln!("Java: {:?} {:?}\n{:?}", r.confidence, r.stats, r.tests);
    assert_eq!(r.stats.solutions_passed, 3);
    // Two solutions (long) agree, one (int) overflows: 2 of 3 = 67 % -> yellow.
    assert_eq!(r.confidence.level, ConfidenceLevel::Yellow);
    assert_eq!(r.tests.last().unwrap().expected, "4000000000\n");
    assert!(r.reference_solution.unwrap().contains("long a"));
}

/// Sandbox containers of this test process (label `de.code-pruefer.sandbox`).
fn sandbox_containers() -> Vec<String> {
    let out = std::process::Command::new("docker")
        .args(["ps", "-aq", "--filter", "label=de.code-pruefer.sandbox"])
        .output()
        .expect("docker ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(String::from)
        .collect()
}

fn dockerd_running() -> bool {
    std::process::Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Stops dockerd (all containers die with it) and starts it again in the
/// background, as happens when Docker Desktop restarts.
fn restart_dockerd() {
    let _ = std::process::Command::new("pkill")
        .args(["-x", "dockerd"])
        .status();
    // Wait until the process has exited (it holds its pid file until then).
    let alive = || {
        std::process::Command::new("pgrep")
            .args(["-x", "dockerd"])
            .stdout(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    for _ in 0..300 {
        if !alive() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(!alive(), "dockerd läuft noch");
    let log = std::env::temp_dir().join("code-pruefer-dockerd-restart.log");
    std::process::Command::new("sh")
        .args([
            "-c",
            &format!("setsid dockerd > {} 2>&1 < /dev/null &", log.display()),
        ])
        .status()
        .expect("dockerd starten");
}

/// Model that restarts dockerd while it "writes" the second solution.
struct RestartingLlm {
    inner: ScriptedLlm,
    solution_calls: std::sync::Mutex<usize>,
}

impl Llm for RestartingLlm {
    async fn chat(&self, req: &ChatRequest) -> Result<llama::ChatResponse, AiError> {
        let solution = !req.messages[0]
            .content
            .contains("analyse programming exercises")
            && !req.messages[0].content.contains("write test inputs");
        if solution {
            let n = {
                let mut c = self.solution_calls.lock().unwrap();
                *c += 1;
                *c
            };
            if n == 2 {
                eprintln!("-- dockerd wird neu gestartet --");
                tokio::task::spawn_blocking(restart_dockerd).await.unwrap();
            }
        }
        self.inner.chat(req).await
    }
}

/// The engine fails in the middle of a test generation (dockerd is stopped,
/// its containers die, and it comes back a few seconds later): the run must
/// reconnect and finish with the same result. Needs root and a dockerd that
/// may be restarted (Linux CI or dev container), so it is opt-in:
/// `CODE_PRUEFER_TEST_DOCKERD_RESTART=1`.
#[tokio::test]
#[ignore = "startet dockerd neu (root); CODE_PRUEFER_TEST_DOCKERD_RESTART=1"]
async fn pipeline_survives_engine_restart() {
    if std::env::var("CODE_PRUEFER_TEST_DOCKERD_RESTART").as_deref() != Ok("1") {
        eprintln!("übersprungen: CODE_PRUEFER_TEST_DOCKERD_RESTART=1 setzen");
        return;
    }
    setup_images();
    assert!(dockerd_running());
    let before = sandbox_containers();
    let cancel = CancellationToken::new();
    let messages = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let m = messages.clone();
    let exec = SandboxExecutor::connect(cancel.clone(), move |msg| {
        eprintln!("  [Engine] {msg}");
        m.lock().unwrap().push(msg);
    })
    .await
    .expect("Engine");
    let llm = RestartingLlm {
        inner: ScriptedLlm {
            calls: Default::default(),
            solutions: vec![
                "```python\na, b = map(int, input().split())\nprint(a + b)\n```",
                "```python\nimport sys\nprint(sum(map(int, sys.stdin.read().split())))\n```",
                "```python\na, b = map(int, input().split())\nprint(b + a)\n```",
            ],
            structure: r#"{"inputFormat":"a b","outputFormat":"a+b","examples":[{"input":"3 4","output":"7"}],"compareMode":"tokens"}"#,
            inputs: r#"{"inputs":["0 0","-5 12"]}"#,
        },
        solution_calls: Default::default(),
    };
    let req = GenerateRequest {
        task_text: "Lies a und b und gib a + b aus.\n\nBeispiel\nEingabe:\n3 4\nAusgabe:\n7\n"
            .into(),
        language: "python".into(),
        compare_mode: None,
    };
    let started = Instant::now();
    let r = pipeline::generate(
        &llm,
        &exec,
        &req,
        Settings {
            solutions: 3,
            repair_rounds: 1,
        },
        &cancel,
        |p| eprintln!("  [{:?}] {}", p.phase, p.message),
    )
    .await
    .expect("Pipeline übersteht den Neustart der Engine");
    eprintln!(
        "Nach Neustart: {:?}, {:.1} s, {} Wiederherstellung(en)\n{:?}",
        r.confidence.level,
        started.elapsed().as_secs_f64(),
        exec.recoveries(),
        r.tests
    );
    assert!(dockerd_running());
    assert!(exec.recoveries() >= 1, "kein Engine-Fehler beobachtet");
    assert_eq!(r.stats.solutions_passed, 3);
    assert_eq!(r.confidence.level, ConfidenceLevel::Green);
    let expected: Vec<&str> = r.tests.iter().map(|t| t.expected.as_str()).collect();
    assert_eq!(expected, vec!["7\n", "0\n", "7\n"]);
    // Containers that died with the old dockerd are cleaned up as orphans.
    let docker = code_pruefer_lib::runner::docker::connect().await.unwrap();
    code_pruefer_lib::runner::docker::remove_orphans(&docker, std::time::Duration::ZERO)
        .await
        .unwrap();
    let after = sandbox_containers();
    assert!(after.iter().all(|c| before.contains(c)), "{after:?}");
}
