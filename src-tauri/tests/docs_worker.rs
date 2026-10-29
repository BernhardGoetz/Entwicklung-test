//! The document worker (`docs::worker`) with the real app executable: the
//! audit PoCs (decompression bomb, self-drawing XObject) and every way the
//! worker can misbehave end with a German error while this process – the
//! "app" – keeps running. Uses the `code-pruefer` binary cargo builds for
//! integration tests; no Docker needed. `cargo test --release --test
//! docs_worker` checks the release binary (without the debug-only
//! self-test modes).

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[cfg(debug_assertions)]
use code_pruefer_lib::docs::worker::SELF_TEST_ENV;
use code_pruefer_lib::docs::worker::{self, WorkerConfig};

fn config() -> WorkerConfig {
    let mut c = WorkerConfig::new(PathBuf::from(env!("CARGO_BIN_EXE_code-pruefer")));
    c.memory_limit = 512 * 1024 * 1024;
    c.phase_timeout = Duration::from_secs(8);
    c.total_timeout = Duration::from_secs(20);
    c
}

#[cfg(debug_assertions)]
fn with_self_test(mode: &str) -> WorkerConfig {
    let mut c = config();
    c.env.push((SELF_TEST_ENV.into(), mode.into()));
    c
}

// --- PDF fixtures (generated here instead of committing binaries) ---

fn raw_pdf(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut s = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    s.extend_from_slice(data);
    s.extend_from_slice(b"\nendstream");
    s
}

fn page(resources: &str) -> Vec<u8> {
    format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << {resources} >> /Contents 4 0 R >>"
    )
    .into_bytes()
}

/// `bomb.pdf` of the audit: one content stream of `mib` MiB spaces.
fn bomb_pdf(mib: usize) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    let chunk = vec![b' '; 1024 * 1024];
    for _ in 0..mib {
        e.write_all(&chunk).unwrap();
    }
    raw_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page(""),
        stream("/Filter /FlateDecode", &e.finish().unwrap()),
    ])
}

/// `recursive.pdf` of the audit (623 bytes there): a form that draws itself.
fn recursive_pdf() -> Vec<u8> {
    raw_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page("/XObject << /X 5 0 R >>"),
        stream("", b"/X Do"),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /X 5 0 R >> >>",
            b"/X Do",
        ),
    ])
}

fn text_pdf(text: &str) -> Vec<u8> {
    let content = format!("BT /F1 12 Tf 72 760 Td ({text}) Tj ET");
    raw_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page("/Font << /F1 5 0 R >>"),
        stream("", content.as_bytes()),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    ])
}

fn import_err(config: &WorkerConfig, name: &str, bytes: &[u8]) -> String {
    let started = Instant::now();
    let e = worker::import(config, name, bytes, |_| {})
        .expect_err("import must fail")
        .to_string();
    eprintln!("{name}: {:.1} s: {e}", started.elapsed().as_secs_f64());
    e
}

const UNSAFE_PDF: &str = "Das PDF konnte nicht sicher gelesen werden";

#[test]
fn text_pdf_is_imported_in_the_worker() {
    let text = "Lies zwei Zahlen ein und gib ihre Summe aus. Beispiel: 3 und 4 ergibt 7.";
    let mut phases = Vec::new();
    let r = worker::import(&config(), "blatt.pdf", &text_pdf(text), |p| phases.push(p)).unwrap();
    assert!(r.text.contains("Lies zwei Zahlen ein"), "{}", r.text);
    assert_eq!(r.pages, Some(1));
    assert_eq!(r.file_name, "blatt.pdf");
    assert!(!phases.is_empty());
}

#[test]
fn errors_of_the_import_keep_their_wording() {
    let e = import_err(&config(), "a.pdf", b"%PDF-1.4\nkaputt");
    assert!(e.contains("PDF-Datei konnte nicht gelesen werden"), "{e}");
    let e = import_err(&config(), "bild.png", b"%PDF-1.4");
    assert!(e.contains("„.png“"), "{e}");
}

#[test]
fn bomb_pdf_is_rejected_by_the_checks() {
    // 200 MiB of spaces in 200 KB (the audit PoC had 1 GiB in 1 MB).
    let e = import_err(&config(), "bomb.pdf", &bomb_pdf(200));
    assert!(e.starts_with(UNSAFE_PDF), "{e}");
    assert!(e.contains("extrem stark komprimierte Daten"), "{e}");
}

#[test]
fn recursive_pdf_is_rejected_by_the_checks() {
    let e = import_err(&config(), "recursive.pdf", &recursive_pdf());
    assert!(e.starts_with(UNSAFE_PDF), "{e}");
    assert!(e.contains("Grafikobjekte"), "{e}");
}

/// Without the checks the PoCs reach lopdf/pdf-extract and kill the worker
/// (as they killed the app before): stack overflow, allocation failure.
#[cfg(debug_assertions)] // self-test modes exist only in debug builds
#[test]
fn without_checks_the_worker_contains_the_pocs() {
    let mut c = with_self_test("noguard");
    let e = import_err(&c, "recursive.pdf", &recursive_pdf());
    assert!(e.starts_with(UNSAFE_PDF), "{e}");
    assert!(e.contains("abgebrochen"), "{e}");
    c.memory_limit = 256 * 1024 * 1024;
    let e = import_err(&c, "bomb.pdf", &bomb_pdf(300));
    assert!(e.starts_with(UNSAFE_PDF), "{e}");
    assert!(
        e.contains("abgebrochen") || e.contains("Arbeitsspeicher"),
        "{e}"
    );
}

#[cfg(debug_assertions)] // self-test modes exist only in debug builds
#[test]
fn stack_overflow_in_the_worker() {
    let e = import_err(&with_self_test("stack"), "a.pdf", b"%PDF-1.4");
    assert!(e.contains("abgebrochen"), "{e}");
}

#[cfg(debug_assertions)] // self-test modes exist only in debug builds
#[test]
fn memory_limit_of_the_worker() {
    let e = import_err(&with_self_test("alloc"), "a.pdf", b"%PDF-1.4");
    assert!(e.starts_with(UNSAFE_PDF), "{e}");
    assert!(
        e.contains("abgebrochen") || e.contains("mehr als 512 MB Arbeitsspeicher"),
        "{e}"
    );
}

#[cfg(debug_assertions)] // self-test modes exist only in debug builds
#[test]
fn hanging_worker_is_killed() {
    let mut c = with_self_test("hang");
    c.phase_timeout = Duration::from_secs(2);
    let started = Instant::now();
    let e = import_err(&c, "a.docx", b"PK\x03\x04");
    assert!(
        e.starts_with("Die Datei konnte nicht sicher gelesen werden"),
        "{e}"
    );
    assert!(e.contains("länger als 2 s"), "{e}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[cfg(debug_assertions)] // self-test modes exist only in debug builds
#[test]
fn progress_does_not_extend_the_total_limit() {
    let mut c = with_self_test("chatty");
    c.total_timeout = Duration::from_secs(3);
    let e = import_err(&c, "a.pdf", b"%PDF-1.4");
    assert!(e.contains("länger als 3 s"), "{e}");
}

#[cfg(debug_assertions)] // self-test modes exist only in debug builds
#[test]
fn flooding_and_garbage_output() {
    let e = import_err(&with_self_test("flood"), "a.pdf", b"%PDF-1.4");
    assert!(e.contains("zu viele Daten"), "{e}");
    let e = import_err(&with_self_test("garbage"), "a.pdf", b"%PDF-1.4");
    assert!(e.contains("ungültige Daten"), "{e}");
    let e = import_err(&with_self_test("exit"), "a.pdf", b"%PDF-1.4");
    assert!(e.contains("abgebrochen"), "{e}");
}
