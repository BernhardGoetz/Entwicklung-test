//! Data-driven language table. Adding a language = adding one entry here
//! plus a Dockerfile under `images/`.

use serde::Serialize;
use std::time::Duration;

/// Tag of the runner images published by `.github/workflows/images.yml`.
pub const IMAGE_TAG: &str = "0.1.0";

#[derive(Debug, Clone)]
pub struct LanguageConfig {
    /// Stable identifier used by the frontend (`"python"`, `"java"`).
    pub id: &'static str,
    /// Display name for the UI.
    pub name: &'static str,
    /// Image reference used when no override env var is set.
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
    /// Image reference, overridable via env var (e.g. `CODE_PRUEFER_PYTHON_IMAGE=code-pruefer-python:dev`).
    pub fn image(&self) -> String {
        std::env::var(self.image_env)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| self.default_image.to_string())
    }

    /// Images to look for locally, in order. An env override is used
    /// exclusively; otherwise the published image comes first, then the
    /// locally built `:dev` tag. Only the first entry is ever pulled.
    pub fn image_candidates(&self) -> Vec<String> {
        let image = self.image();
        if image != self.default_image {
            return vec![image];
        }
        vec![image, self.local_image.to_string()]
    }
}

pub static LANGUAGES: &[LanguageConfig] = &[
    LanguageConfig {
        id: "python",
        name: "Python 3.12",
        default_image: "ghcr.io/bernhardgoetz/code-pruefer-python:0.1.0",
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
        default_image: "ghcr.io/bernhardgoetz/code-pruefer-java:0.1.0",
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
    fn default_images_use_current_tag() {
        for l in LANGUAGES {
            assert!(
                l.default_image.ends_with(&format!(":{IMAGE_TAG}")),
                "{}",
                l.id
            );
        }
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
