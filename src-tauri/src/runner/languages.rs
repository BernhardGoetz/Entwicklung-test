//! Data-driven language table. Adding a language = adding one entry here
//! plus a Dockerfile under `images/`.

use serde::Serialize;
use std::time::Duration;

/// Registry namespace of the published runner images (GHCR names must be
/// lowercase). `.github/workflows/release.yml` checks that it matches the
/// repository owner it pushes to.
pub const IMAGE_REGISTRY: &str = "ghcr.io/bernhardgoetz";

/// Tag of the runner images = app version (`Cargo.toml`, kept equal to
/// `tauri.conf.json` and `package.json`). The release workflow pushes the
/// images as `<IMAGE_REGISTRY>/code-pruefer-<id>:<version>` for tag `v<version>`.
pub const IMAGE_TAG: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct LanguageConfig {
    /// Stable identifier used by the frontend (`"python"`, `"java"`).
    pub id: &'static str,
    /// Display name for the UI.
    pub name: &'static str,
    /// Image reference used when no override env var is set
    /// (`<IMAGE_REGISTRY>/code-pruefer-<id>:<IMAGE_TAG>`).
    pub default_image: &'static str,
    /// Environment variable that overrides the image reference (local dev builds).
    pub image_env: &'static str,
    /// Tag of a locally built image (`docker build -t <local_image> images/<id>`),
    /// used when the default image is neither present nor pullable.
    pub local_image: &'static str,
    /// File name the source code is stored under inside `/work`.
    pub source_file: &'static str,
    /// File extensions accepted by the upload dialog.
    pub extensions: &'static [&'static str],
    /// Optional compile/syntax-check command (run once before the tests).
    pub compile_cmd: Option<&'static [&'static str]>,
    /// Command executed once per test case, stdin = test input.
    pub run_cmd: &'static [&'static str],
    pub compile_timeout: Duration,
    pub run_timeout: Duration,
    /// Starter code shown in the editor.
    pub template: &'static str,
}

impl LanguageConfig {
    /// Image set via env var (e.g. `CODE_PRUEFER_PYTHON_IMAGE=code-pruefer-python:dev`),
    /// if it differs from the default image.
    pub fn image_override(&self) -> Option<String> {
        std::env::var(self.image_env)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != self.default_image)
    }

    /// Image reference, overridable via env var.
    pub fn image(&self) -> String {
        self.image_override()
            .unwrap_or_else(|| self.default_image.to_string())
    }

    /// File name of the bundled image archive for an OCI architecture
    /// (`amd64` / `arm64`), see `runner::images`.
    pub fn archive_name(&self, arch: &str) -> String {
        format!("code-pruefer-{}-{arch}.tar.gz", self.id)
    }
}

pub static LANGUAGES: &[LanguageConfig] = &[
    LanguageConfig {
        id: "python",
        name: "Python 3.12",
        default_image: concat!(
            "ghcr.io/bernhardgoetz/code-pruefer-python:",
            env!("CARGO_PKG_VERSION")
        ),
        image_env: "CODE_PRUEFER_PYTHON_IMAGE",
        local_image: "code-pruefer-python:dev",
        source_file: "main.py",
        extensions: &["py"],
        compile_cmd: Some(&["python", "-m", "py_compile", "main.py"]),
        run_cmd: &["python", "-u", "main.py"],
        compile_timeout: Duration::from_secs(30),
        run_timeout: Duration::from_secs(5),
        template: "# Liest eine Zeile von der Standardeingabe und gibt sie aus\nname = input()\nprint(f\"Hallo, {name}!\")\n",
    },
    LanguageConfig {
        id: "java",
        name: "Java 21",
        default_image: concat!(
            "ghcr.io/bernhardgoetz/code-pruefer-java:",
            env!("CARGO_PKG_VERSION")
        ),
        image_env: "CODE_PRUEFER_JAVA_IMAGE",
        local_image: "code-pruefer-java:dev",
        source_file: "Main.java",
        extensions: &["java"],
        compile_cmd: Some(&[
            "javac",
            "-J-Xmx256m",
            "-J-XX:+UseSerialGC",
            "-J-XX:TieredStopAtLevel=1",
            "-J-XX:-UsePerfData",
            "-encoding",
            "UTF-8",
            "Main.java",
        ]),
        run_cmd: &[
            "java",
            "-Xmx256m",
            "-Xss8m",
            "-XX:+UseSerialGC",
            "-XX:TieredStopAtLevel=1",
            "-XX:-UsePerfData",
            "-Dfile.encoding=UTF-8",
            "-Dstdout.encoding=UTF-8",
            "Main",
        ],
        compile_timeout: Duration::from_secs(30),
        run_timeout: Duration::from_secs(8),
        template: "import java.util.Scanner;\n\n// Die Klasse muss \"Main\" heißen (Datei Main.java).\npublic class Main {\n    public static void main(String[] args) {\n        Scanner sc = new Scanner(System.in);\n        String name = sc.nextLine();\n        System.out.println(\"Hallo, \" + name + \"!\");\n    }\n}\n",
    },
];

pub fn get(id: &str) -> Option<&'static LanguageConfig> {
    LANGUAGES.iter().find(|l| l.id == id)
}

/// Language-specific sanity check before running; returns a German hint.
pub fn precheck(lang: &LanguageConfig, code: &str) -> Option<String> {
    if lang.id == "java" && !has_public_main_class(code) {
        return Some(
            "Java-Code muss eine Klasse namens \"Main\" enthalten (z. B. `public class Main { public static void main(String[] args) { ... } }`), \
             da die Datei als Main.java gespeichert wird."
                .to_string(),
        );
    }
    None
}

fn has_public_main_class(code: &str) -> bool {
    code.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .collect::<Vec<_>>()
        .windows(2)
        .any(|w| w[0] == "class" && w[1] == "Main")
}

/// Serializable summary for the frontend.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageInfo {
    pub id: String,
    pub name: String,
    pub source_file: String,
    pub extensions: Vec<String>,
    pub template: String,
    pub image: String,
    pub run_timeout_secs: u64,
}

pub fn infos() -> Vec<LanguageInfo> {
    LANGUAGES
        .iter()
        .map(|l| LanguageInfo {
            id: l.id.into(),
            name: l.name.into(),
            source_file: l.source_file.into(),
            extensions: l.extensions.iter().map(|e| e.to_string()).collect(),
            template: l.template.into(),
            image: l.image(),
            run_timeout_secs: l.run_timeout.as_secs(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_and_defaults() {
        let py = get("python").unwrap();
        assert_eq!(py.source_file, "main.py");
        assert_eq!(py.run_timeout, Duration::from_secs(5));
        let java = get("java").unwrap();
        assert_eq!(java.source_file, "Main.java");
        assert_eq!(java.run_timeout, Duration::from_secs(8));
        assert!(get("cobol").is_none());
    }

    #[test]
    fn default_images_follow_release_scheme() {
        assert_eq!(IMAGE_REGISTRY, IMAGE_REGISTRY.to_lowercase());
        for l in LANGUAGES {
            assert_eq!(
                l.default_image,
                format!("{IMAGE_REGISTRY}/code-pruefer-{}:{IMAGE_TAG}", l.id)
            );
            assert_eq!(
                l.archive_name("arm64"),
                format!("code-pruefer-{}-arm64.tar.gz", l.id)
            );
        }
    }

    /// The image tag is the Cargo version; the release workflow tags by the
    /// app version, so all three version fields must agree.
    #[test]
    fn versions_are_in_sync() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let version_of = |file: &str| -> String {
            let raw = std::fs::read_to_string(root.join(file)).unwrap();
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            v["version"].as_str().unwrap().to_string()
        };
        assert_eq!(version_of("tauri.conf.json"), IMAGE_TAG);
        assert_eq!(version_of("../package.json"), IMAGE_TAG);
    }

    #[test]
    fn local_images_match_build_instructions() {
        for l in LANGUAGES {
            assert_eq!(l.local_image, format!("code-pruefer-{}:dev", l.id));
        }
    }

    #[test]
    fn java_requires_class_main() {
        let java = get("java").unwrap();
        assert!(precheck(java, "public class Main { }").is_none());
        assert!(precheck(java, "class Main{}").is_none());
        assert!(precheck(java, "public class Aufgabe1 { }").is_some());
        assert!(precheck(java, "public class MainApp { }").is_some());
        assert!(precheck(get("python").unwrap(), "print(1)").is_none());
    }
}
