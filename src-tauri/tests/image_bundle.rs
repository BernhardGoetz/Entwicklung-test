//! Integration test for bundled image archives (needs Docker and the
//! locally built `code-pruefer-python:dev` image):
//!
//! ```sh
//! docker build -t code-pruefer-python:dev images/python
//! cd src-tauri && cargo test --test image_bundle -- --ignored --test-threads=1
//! ```
//!
//! The test saves the image as `code-pruefer-python-<arch>.tar.gz`, removes
//! the pinned reference from the engine and checks that the app loads it
//! from the archive instead of pulling it. Runs in its own process, so the
//! bundle directory and env vars do not affect the other integration tests.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use code_pruefer_lib::runner::{self, docker, images, languages, Progress};

const DEV_IMAGE: &str = "code-pruefer-python:dev";

fn sh(args: &[&str]) {
    let status = Command::new("docker")
        .args(args)
        .status()
        .expect("docker CLI");
    assert!(status.success(), "docker {args:?} failed");
}

fn rmi(image: &str) {
    let _ = Command::new("docker")
        .args(["rmi", image])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// `docker save <image> | gzip > <dest>`
fn save_gz(image: &str, dest: &Path) {
    let mut save = Command::new("docker")
        .args(["save", image])
        .stdout(Stdio::piped())
        .spawn()
        .expect("docker save");
    let out = std::fs::File::create(dest).unwrap();
    let mut gz = flate2::write::GzEncoder::new(out, flate2::Compression::fast());
    std::io::copy(save.stdout.as_mut().unwrap(), &mut gz).unwrap();
    gz.finish().unwrap();
    assert!(save.wait().unwrap().success());
}

fn bundle_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("code-pruefer-bundle-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Saves `archived_as` into the bundle dir, removes the pinned ref and
/// checks that the app resolves the pinned image from the archive.
async fn load_via_app(archived_as: &str) {
    std::env::remove_var("CODE_PRUEFER_PYTHON_IMAGE");
    let lang = languages::get("python").unwrap();
    let pinned = lang.default_image;
    let client = docker::connect().await.expect("engine");
    let arch = images::select_arch(docker::engine_arch(&client).await.as_deref()).unwrap();

    let dir = bundle_dir();
    images::init_bundle_dir(dir.clone());
    if archived_as != DEV_IMAGE {
        sh(&["tag", DEV_IMAGE, archived_as]);
    }
    save_gz(archived_as, &dir.join(lang.archive_name(arch)));
    if archived_as != DEV_IMAGE {
        rmi(archived_as);
    }
    rmi(pinned);
    assert!(!docker::image_exists(&client, pinned).await.unwrap());

    let mut events = Vec::new();
    let image = images::ensure_image(&client, lang, &mut |p| events.push(p))
        .await
        .expect("image from archive");
    assert_eq!(image, pinned);
    assert!(docker::image_exists(&client, pinned).await.unwrap());
    assert!(
        events.iter().any(|e| matches!(e, Progress::Load { .. })),
        "not loaded from the archive"
    );
    assert!(
        !events.iter().any(|e| matches!(e, Progress::Pull(_))),
        "must not pull"
    );

    // The loaded image is usable for a run.
    let report = runner::run_submission(
        runner::RunRequest {
            language: "python".into(),
            code: "print(input())".into(),
            tests: vec![runner::TestCase {
                name: String::new(),
                input: "x\n".into(),
                expected: "x\n".into(),
            }],
            trim_whitespace: true,
        },
        |_| {},
    )
    .await
    .expect("run");
    assert_eq!(report.image, pinned);
    assert_eq!(report.passed, 1);

    rmi(pinned);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
#[ignore = "benötigt Docker und code-pruefer-python:dev"]
async fn loads_image_from_bundled_archive() {
    let pinned = languages::get("python").unwrap().default_image;
    // Archive carries exactly the pinned reference (as in release builds).
    load_via_app(pinned).await;
    // Archive with a different name: the app tags the loaded image.
    load_via_app(DEV_IMAGE).await;
}
