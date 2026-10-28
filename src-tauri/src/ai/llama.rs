//! llama.cpp `llama-server` as a sidecar process.
//!
//! Release builds ship the official llama.cpp binaries (version and SHA-256
//! pinned in `scripts/fetch-llama-server.sh`) in `<resource_dir>/llama/`.
//! The server listens on 127.0.0.1 on a free port and requires a random API
//! key, so other local programs cannot use it. One server runs at a time
//! (for one model); it is restarted when the model changes and killed on
//! cancel and when the app exits.

use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

use super::AiError;
use crate::runner::languages::{override_from, DEV_OVERRIDES};

/// Pinned llama.cpp release (same as `version` in
/// `scripts/fetch-llama-server.sh`, which also holds the SHA-256 per asset).
pub const LLAMA_CPP_VERSION: &str = "b11700";
/// Subdirectory of the resource dir with llama-server and its libraries.
pub const RUNTIME_SUBDIR: &str = "llama";
/// Path to a llama-server binary (debug builds only, e.g. on Linux).
pub const SERVER_ENV: &str = "CODE_PRUEFER_LLAMA_SERVER";
/// Context size in tokens (prompt + answer).
pub const CTX_SIZE: u32 = 8192;
/// Loading a 7B model from a slow disk can take a while.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(120);
const HEALTH_INTERVAL: Duration = Duration::from_millis(300);
/// Upper bound for one completion (CPU, 7B, ~1500 tokens).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Lines of server log kept for error messages.
const STDERR_LINES: usize = 40;
/// Windows `STATUS_DLL_NOT_FOUND` (0xC0000135) as exit code.
const STATUS_DLL_NOT_FOUND: i32 = 0xC000_0135_u32 as i32;

static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Sets the app's resource directory (called once at app start).
pub fn init_resource_dir(dir: PathBuf) {
    let _ = RESOURCE_DIR.set(dir);
}

pub fn server_binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// Finds llama-server: the dev override (if allowed) or
/// `<resource_dir>/llama/llama-server[.exe]`. Pure apart from `is_file`.
pub fn locate_server(
    resource_dir: Option<&Path>,
    env_override: Option<String>,
    allow_override: bool,
) -> Result<PathBuf, AiError> {
    if let Some(p) = override_from(env_override, "", allow_override) {
        let p = PathBuf::from(p);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(AiError::RuntimeMissing(format!(
                "{SERVER_ENV}={} existiert nicht",
                p.display()
            )))
        };
    }
    let Some(dir) = resource_dir else {
        return Err(AiError::RuntimeMissing("Ressourcenordner unbekannt".into()));
    };
    let p = dir.join(RUNTIME_SUBDIR).join(server_binary_name());
    if p.is_file() {
        Ok(p)
    } else {
        Err(AiError::RuntimeMissing(format!("gesucht: {}", p.display())))
    }
}

pub fn server_path() -> Result<PathBuf, AiError> {
    locate_server(
        RESOURCE_DIR.get().map(PathBuf::as_path),
        std::env::var(SERVER_ENV).ok(),
        DEV_OVERRIDES,
    )
}

/// Threads for inference: the physical cores (hyper-threads do not help).
pub fn thread_count() -> usize {
    sysinfo::System::physical_core_count()
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(4)
        .clamp(1, 32)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    pub model: PathBuf,
    pub port: u16,
    pub api_key: String,
    pub threads: usize,
}

/// Command line of llama-server (flags checked against the pinned build).
pub fn server_args(c: &ServerConfig) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-m".into(),
        c.model.display().to_string(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        c.port.to_string(),
        "--api-key".into(),
        c.api_key.clone(),
        "--ctx-size".into(),
        CTX_SIZE.to_string(),
        "--threads".into(),
        c.threads.to_string(),
        // One request at a time; the whole context belongs to it.
        "--parallel".into(),
        "1".into(),
        // Chat template from the GGUF file (Qwen: ChatML).
        "--jinja".into(),
        "--no-webui".into(),
        // Never download anything (e.g. via -hf); models come from us.
        "--offline".into(),
    ];
    // No GPU offload on Windows/Linux (CPU build); Metal on macOS is used
    // automatically by the official build.
    if !cfg!(target_os = "macos") {
        args.extend(["--n-gpu-layers".into(), "0".into()]);
    }
    args
}

/// 32 random bytes as hex.
pub fn random_api_key() -> Result<String, AiError> {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf)
        .map_err(|e| AiError::Start(format!("Zufallszahlen nicht verfügbar: {e}")))?;
    Ok(super::hex(&buf))
}

/// A free TCP port on 127.0.0.1 (released again before llama-server binds
/// it; the tiny race is acceptable, a failed bind shows up as a start error).
pub fn free_port() -> Result<u16, AiError> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| AiError::Start(format!("Kein freier Port: {e}")))
}

/// German explanation for an early exit of llama-server.
pub fn explain_exit(code: Option<i32>, log_tail: &str) -> String {
    if code == Some(STATUS_DLL_NOT_FOUND) {
        return "Eine benötigte Systembibliothek fehlt. Bitte die „Microsoft Visual C++ \
                Redistributable (x64)“ von Microsoft installieren \
                (https://aka.ms/vs/17/release/vc_redist.x64.exe) und es erneut versuchen."
            .into();
    }
    let lower = log_tail.to_ascii_lowercase();
    let hint = if lower.contains("failed to load model")
        || lower.contains("error loading model")
        || lower.contains("invalid magic")
    {
        " Die Modelldatei ist beschädigt oder passt nicht zu dieser KI-Laufzeit."
    } else if lower.contains("bind") || lower.contains("address already in use") {
        " Der Port war belegt – bitte erneut versuchen."
    } else if lower.contains("illegal instruction") {
        " Der Prozessor unterstützt die nötigen Befehle (z. B. AVX2) nicht."
    } else {
        ""
    };
    let code = code.map_or("unbekannt".to_string(), |c| c.to_string());
    let tail: String = log_tail
        .lines()
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    format!("llama-server wurde beendet (Exit-Code {code}).{hint}\n{tail}")
        .trim_end()
        .to_string()
}

/// Address and key of the running server.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: String,
}

struct Running {
    child: tokio::process::Child,
    model: PathBuf,
    endpoint: Endpoint,
}

static SERVER: tokio::sync::Mutex<Option<Running>> = tokio::sync::Mutex::const_new(None);

fn local_http(timeout: Duration) -> Result<reqwest::Client, AiError> {
    reqwest::Client::builder()
        // Never route 127.0.0.1 through a configured proxy.
        .no_proxy()
        .timeout(timeout)
        .build()
        .map_err(|e| AiError::Http(e.to_string()))
}

/// Starts llama-server for `model` (or reuses the running one) and waits
/// until the model is loaded.
pub async fn ensure_running(
    model: &Path,
    cancel: &CancellationToken,
    mut on_status: impl FnMut(String),
) -> Result<Endpoint, AiError> {
    let mut guard = SERVER.lock().await;
    if let Some(r) = guard.as_mut() {
        let alive = matches!(r.child.try_wait(), Ok(None));
        if alive && r.model == model {
            return Ok(r.endpoint.clone());
        }
    }
    if let Some(old) = guard.take() {
        stop(old).await;
    }

    let binary = server_path()?;
    let config = ServerConfig {
        model: model.to_path_buf(),
        port: free_port()?,
        api_key: random_api_key()?,
        threads: thread_count(),
    };
    on_status("KI-Modell wird geladen …".into());
    let mut cmd = tokio::process::Command::new(&binary);
    cmd.args(server_args(&config))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Shared libraries (ggml-*.dll / .dylib) live next to the binary.
    if let Some(dir) = binary.parent() {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd
        .spawn()
        .map_err(|e| AiError::Start(format!("{}: {e}", binary.display())))?;

    let log = Arc::new(Mutex::new(VecDeque::<String>::new()));
    if let Some(stderr) = child.stderr.take() {
        let log = log.clone();
        // Must keep draining stderr, or the server blocks on a full pipe.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut l) = log.lock() {
                    if l.len() >= STDERR_LINES {
                        l.pop_front();
                    }
                    l.push_back(line);
                }
            }
        });
    }
    let log_tail = || {
        log.lock()
            .map(|l| l.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    };

    let endpoint = Endpoint {
        base_url: format!("http://127.0.0.1:{}", config.port),
        api_key: config.api_key.clone(),
    };
    let http = local_http(Duration::from_secs(2))?;
    let started = Instant::now();
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill().await;
            return Err(AiError::Cancelled);
        }
        if let Ok(Some(status)) = child.try_wait() {
            // Give the log reader a moment to collect the last lines.
            tokio::time::sleep(Duration::from_millis(100)).await;
            return Err(AiError::Start(explain_exit(status.code(), &log_tail())));
        }
        if let Ok(resp) = http
            .get(format!("{}/health", endpoint.base_url))
            .send()
            .await
        {
            if resp.status().is_success() {
                break;
            }
        }
        let waited = started.elapsed();
        if waited >= HEALTH_TIMEOUT {
            let _ = child.kill().await;
            return Err(AiError::Start(format!(
                "Das Modell wurde nach {} s noch nicht geladen.\n{}",
                waited.as_secs(),
                log_tail()
            )));
        }
        on_status(format!("KI-Modell wird geladen … ({} s)", waited.as_secs()));
        tokio::time::sleep(HEALTH_INTERVAL).await;
    }

    *guard = Some(Running {
        child,
        model: model.to_path_buf(),
        endpoint: endpoint.clone(),
    });
    Ok(endpoint)
}

async fn stop(mut r: Running) {
    let _ = r.child.kill().await;
}

/// Stops the server, if any (cancel, model change, app exit).
pub async fn shutdown() {
    if let Some(r) = SERVER.lock().await.take() {
        stop(r).await;
    }
}

// ---------------------------------------------------------------------------
// Chat client

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn system(s: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: s.into(),
        }
    }
    pub fn user(s: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: s.into(),
        }
    }
    pub fn assistant(s: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: s.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_tokens: u32,
    /// JSON schema the answer must follow (grammar-constrained by llama-server).
    pub json_schema: Option<Value>,
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatResponse {
    pub content: String,
    pub completion_tokens: Option<u64>,
    pub tokens_per_second: Option<f64>,
    /// "stop" or "length" (answer cut off at `max_tokens`).
    pub finish_reason: Option<String>,
}

/// A chat model. Implemented by [`LlamaClient`]; mocked in tests.
pub trait Llm {
    fn chat(&self, req: &ChatRequest)
        -> impl Future<Output = Result<ChatResponse, AiError>> + Send;
}

// The client uses llama-server's native endpoints: `/apply-template`
// renders the messages with the model's chat template, `/completion`
// generates with an optional `json_schema`, from which the server builds a
// grammar that constrains sampling. (The OpenAI-compatible chat endpoint
// of the pinned build wraps that grammar into its chat-format parser,
// which failed with a ChatML test model; the native path does not.)

/// Body for `/apply-template`.
pub fn template_body(req: &ChatRequest) -> Value {
    json!({ "messages": req.messages })
}

/// Body for `/completion` with the rendered prompt.
pub fn completion_body(prompt: &str, req: &ChatRequest) -> Value {
    let mut body = json!({
        "prompt": prompt,
        "temperature": req.temperature,
        "n_predict": req.max_tokens,
        "stream": false,
        "cache_prompt": true,
    });
    if let Some(schema) = &req.json_schema {
        body["json_schema"] = schema.clone();
    }
    if let Some(seed) = req.seed {
        body["seed"] = json!(seed);
    }
    body
}

#[derive(Deserialize)]
struct RawTemplate {
    prompt: String,
}

#[derive(Deserialize)]
struct RawCompletion {
    #[serde(default)]
    content: String,
    tokens_predicted: Option<u64>,
    stop_type: Option<String>,
    timings: Option<RawTimings>,
}
#[derive(Deserialize)]
struct RawTimings {
    predicted_per_second: Option<f64>,
}

pub fn parse_template(raw: &str) -> Result<String, AiError> {
    serde_json::from_str::<RawTemplate>(raw)
        .map(|t| t.prompt)
        .map_err(|e| AiError::Http(format!("Unerwartete Antwort ({e})")))
}

/// Parses a `/completion` answer (incl. `timings` for tokens/s).
pub fn parse_completion(raw: &str) -> Result<ChatResponse, AiError> {
    let r: RawCompletion = serde_json::from_str(raw)
        .map_err(|e| AiError::Http(format!("Unerwartete Antwort ({e})")))?;
    Ok(ChatResponse {
        content: r.content,
        completion_tokens: r.tokens_predicted,
        tokens_per_second: r
            .timings
            .and_then(|t| t.predicted_per_second)
            .filter(|v| v.is_finite() && *v > 0.0),
        // "limit": cut off at n_predict.
        finish_reason: r.stop_type.map(|t| {
            if t == "limit" {
                "length".to_string()
            } else {
                "stop".to_string()
            }
        }),
    })
}

/// HTTP client for the running llama-server.
pub struct LlamaClient {
    http: reqwest::Client,
    endpoint: Endpoint,
    cancel: CancellationToken,
}

impl LlamaClient {
    pub fn new(endpoint: Endpoint, cancel: CancellationToken) -> Result<Self, AiError> {
        Ok(Self {
            http: local_http(REQUEST_TIMEOUT)?,
            endpoint,
            cancel,
        })
    }

    async fn post(&self, path: &str, body: &Value) -> Result<String, AiError> {
        let resp = self
            .http
            .post(format!("{}{path}", self.endpoint.base_url))
            .bearer_auth(&self.endpoint.api_key)
            .json(body)
            .send()
            .await
            .map_err(|e| AiError::Http(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| AiError::Http(e.to_string()))?;
        if !status.is_success() {
            let snippet: String = text.chars().take(300).collect();
            return Err(AiError::Http(format!("HTTP {status}: {snippet}")));
        }
        Ok(text)
    }
}

impl Llm for LlamaClient {
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, AiError> {
        let send = async {
            let prompt = parse_template(&self.post("/apply-template", &template_body(req)).await?)?;
            parse_completion(
                &self
                    .post("/completion", &completion_body(&prompt, req))
                    .await?,
            )
        };
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(AiError::Cancelled),
            r = send => r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_version_matches_fetch_script() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let script =
            std::fs::read_to_string(root.join("../scripts/fetch-llama-server.sh")).unwrap();
        assert!(
            script.contains(&format!("version=\"{LLAMA_CPP_VERSION}\"")),
            "fetch-llama-server.sh pins another llama.cpp version"
        );
        let notices = std::fs::read_to_string(root.join("../THIRD_PARTY_NOTICES.txt")).unwrap();
        assert!(notices.contains(&format!("Release {LLAMA_CPP_VERSION}")));
    }

    #[test]
    fn locate_server_prefers_allowed_override() {
        let dir = std::env::temp_dir().join(format!("cp-llama-{}", std::process::id()));
        let bin_dir = dir.join(RUNTIME_SUBDIR);
        std::fs::create_dir_all(&bin_dir).unwrap();
        // Missing binary: clear German message with the searched path.
        let e = locate_server(Some(&dir), None, true)
            .unwrap_err()
            .to_string();
        assert!(e.starts_with("KI-Laufzeit nicht gefunden"), "{e}");
        assert!(e.contains(server_binary_name()), "{e}");
        std::fs::write(bin_dir.join(server_binary_name()), b"x").unwrap();
        assert_eq!(
            locate_server(Some(&dir), None, true).unwrap(),
            bin_dir.join(server_binary_name())
        );
        // Override only when allowed (debug builds).
        let other = dir.join("other-server");
        std::fs::write(&other, b"x").unwrap();
        let ov = Some(other.display().to_string());
        assert_eq!(locate_server(Some(&dir), ov.clone(), true).unwrap(), other);
        assert_eq!(
            locate_server(Some(&dir), ov, false).unwrap(),
            bin_dir.join(server_binary_name())
        );
        let e = locate_server(Some(&dir), Some("/nicht/da".into()), true).unwrap_err();
        assert!(e.to_string().contains(SERVER_ENV));
        assert!(locate_server(None, None, true).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn server_args_bind_localhost_with_key() {
        let args = server_args(&ServerConfig {
            model: "/m/model.gguf".into(),
            port: 4242,
            api_key: "k".repeat(64),
            threads: 6,
        });
        let pair = |flag: &str| {
            let i = args.iter().position(|a| a == flag).expect(flag);
            args[i + 1].clone()
        };
        assert_eq!(pair("-m"), "/m/model.gguf");
        assert_eq!(pair("--host"), "127.0.0.1");
        assert_eq!(pair("--port"), "4242");
        assert_eq!(pair("--api-key").len(), 64);
        assert_eq!(pair("--ctx-size"), "8192");
        assert_eq!(pair("--threads"), "6");
        for flag in ["--no-webui", "--offline", "--jinja"] {
            assert!(args.iter().any(|a| a == flag), "{flag}");
        }
    }

    #[test]
    fn api_keys_are_random_hex() {
        let a = random_api_key().unwrap();
        let b = random_api_key().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn free_port_is_nonzero() {
        assert!(free_port().unwrap() > 0);
    }

    #[test]
    fn exit_explanations() {
        let m = explain_exit(Some(-1073741515), "");
        assert!(m.contains("Visual C++"), "{m}");
        let m = explain_exit(Some(1), "a\nllama_model_load: error loading model: x\n");
        assert!(m.contains("beschädigt"), "{m}");
        assert!(m.contains("Exit-Code 1"), "{m}");
        let m = explain_exit(None, "");
        assert!(m.contains("unbekannt"), "{m}");
    }

    #[test]
    fn request_bodies() {
        let req = ChatRequest {
            messages: vec![ChatMessage::system("s"), ChatMessage::user("u")],
            temperature: 0.7,
            max_tokens: 100,
            json_schema: Some(json!({"type": "object"})),
            seed: Some(7),
        };
        let t = template_body(&req);
        assert_eq!(t["messages"][0]["role"], "system");
        assert_eq!(t["messages"][1]["content"], "u");
        let b = completion_body("<p>", &req);
        assert_eq!(b["prompt"], "<p>");
        assert_eq!(b["n_predict"], 100);
        assert_eq!(b["stream"], false);
        assert_eq!(b["seed"], 7);
        assert_eq!(b["json_schema"]["type"], "object");
        let plain = completion_body(
            "x",
            &ChatRequest {
                json_schema: None,
                seed: None,
                ..req
            },
        );
        assert!(plain.get("json_schema").is_none());
        assert!(plain.get("seed").is_none());
    }

    #[test]
    fn parse_answers() {
        assert_eq!(
            parse_template(r#"{"prompt":"<|im_start|>user"}"#).unwrap(),
            "<|im_start|>user"
        );
        assert!(parse_template("{}").is_err());
        let raw = r#"{"index":0,"content":"hallo","tokens_predicted":12,"stop":true,
            "stop_type":"eos","timings":{"predicted_per_second":21.5,"prompt_n":3}}"#;
        let r = parse_completion(raw).unwrap();
        assert_eq!(r.content, "hallo");
        assert_eq!(r.completion_tokens, Some(12));
        assert_eq!(r.tokens_per_second, Some(21.5));
        assert_eq!(r.finish_reason.as_deref(), Some("stop"));
        let r = parse_completion(r#"{"content":"x","stop_type":"limit"}"#).unwrap();
        assert_eq!(r.finish_reason.as_deref(), Some("length"));
        assert_eq!(r.tokens_per_second, None);
        assert!(parse_completion("kaputt").is_err());
    }
}
