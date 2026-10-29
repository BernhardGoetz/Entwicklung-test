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
//! from the archive instead of pulling it, with and without
//! `image-digests.json`, and that a manipulated archive is rejected. Runs in
//! its own process, so the bundle directory and env vars do not affect the
//! other integration tests.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use code_pruefer_lib::runner::images::DIGESTS_FILE;
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
            compare_mode: None,
            forbid_trailing_newline: false,
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

fn image_id(image: &str) -> String {
    let out = Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", image])
        .output()
        .expect("docker CLI");
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// `image-digests.json` as written by `scripts/export-image-archives.sh`.
fn write_digests(dir: &Path, arch: &str, sha256: &str, image_id: &str) {
    let json = format!(
        r#"{{"version": "{}", "images": {{"python": {{"archives": {{"{arch}":
            {{"sha256": "{sha256}", "imageIds": ["{image_id}"]}}}}}}}}}}"#,
        languages::IMAGE_TAG
    );
    std::fs::write(dir.join(DIGESTS_FILE), json).unwrap();
}

/// Release-like bundle (archive + digests): a correct archive is loaded, a
/// manipulated one (wrong file hash, or wrong image ID) is rejected and the
/// image is removed again.
#[tokio::test]
#[ignore = "benötigt Docker und code-pruefer-python:dev"]
async fn verifies_archive_against_digests() {
    std::env::remove_var("CODE_PRUEFER_PYTHON_IMAGE");
    let lang = languages::get("python").unwrap();
    let pinned = lang.default_image;
    let client = docker::connect().await.expect("engine");
    let arch = images::select_arch(docker::engine_arch(&client).await.as_deref()).unwrap();
    let dir = bundle_dir();
    images::init_bundle_dir(dir.clone());

    sh(&["tag", DEV_IMAGE, pinned]);
    let archive = dir.join(lang.archive_name(arch));
    save_gz(pinned, &archive);
    let expected_id = image_id(pinned);
    let sha = docker::sha256_file(&archive).await.unwrap();
    let python_errors = |r: &images::ImagesReport| -> String {
        r.errors
            .iter()
            .filter(|e| e.contains("python"))
            .cloned()
            .collect::<Vec<_>>()
            .join("; ")
    };

    // Correct digests: loaded and accepted.
    rmi(pinned);
    write_digests(&dir, arch, &sha, &expected_id);
    let report = images::prepare_all(&client, |_| {}).await;
    assert!(report.done.contains(&lang.name.to_string()), "{report:?}");
    assert_eq!(image_id(pinned), expected_id);

    // An unexpected image under the pinned tag is not used: the archive
    // replaces it.
    sh(&["tag", "code-pruefer-java:dev", pinned]);
    let report = images::prepare_all(&client, |_| {}).await;
    assert!(report.done.contains(&lang.name.to_string()), "{report:?}");
    assert_eq!(image_id(pinned), expected_id);

    // Wrong file hash: not even loaded.
    rmi(pinned);
    write_digests(&dir, arch, &"0".repeat(64), &expected_id);
    let report = images::prepare_all(&client, |_| {}).await;
    let errors = python_errors(&report);
    assert!(
        errors.contains("Prüfsumme des Archivs stimmt nicht"),
        "{report:?}"
    );
    assert!(!docker::image_exists(&client, pinned).await.unwrap());

    // Correct file, wrong image ID: loaded, rejected and removed.
    write_digests(&dir, arch, &sha, &format!("sha256:{}", "1".repeat(64)));
    let report = images::prepare_all(&client, |_| {}).await;
    let errors = python_errors(&report);
    assert!(
        errors.contains("nicht die erwartete Image-ID"),
        "{report:?}"
    );
    assert!(!docker::image_exists(&client, pinned).await.unwrap());

    // A run falls back to the local :dev image (debug build), never to the
    // manipulated one.
    let mut events = Vec::new();
    let image = images::ensure_image(&client, lang, &mut |p| events.push(p))
        .await
        .expect("dev fallback");
    assert_eq!(image, DEV_IMAGE);
    assert!(!docker::image_exists(&client, pinned).await.unwrap());

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
