//! Import of task sheets: DOCX, PDF (text layer, scanned pages via OCR) and
//! images (OCR).
//!
//! Everything here blocks (file I/O, parsing, system OCR) and runs on a
//! blocking thread (see `import_task_document` in `lib.rs`).

pub mod docx;
pub mod layout;
pub mod ocr;
pub mod pdf;

use std::path::Path;

use base64::Engine as _;
use serde::Serialize;

use crate::runner::compare::normalize_newlines;
use crate::runner::Progress;

/// Maximum file size. Mirrored in `src/components/TaskStatement.tsx`.
pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
/// Maximum number of PDF pages. Mirrored in `src/components/TaskStatement.tsx`.
pub const MAX_PAGES: usize = 20;
/// Images up to this size are returned as preview (data URL).
pub const MAX_PREVIEW_BYTES: usize = 5 * 1024 * 1024;
/// The extracted text is cut off after this many bytes.
pub const MAX_TEXT_BYTES: usize = crate::runner::MAX_INPUT_BYTES;

/// Accepted file extensions (the dialog filter uses the same list).
pub const EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "pdf", "docx"];

#[derive(Debug, thiserror::Error)]
pub enum DocsError {
    #[error("Die Datei konnte nicht gelesen werden: {0}")]
    Read(#[from] std::io::Error),
    #[error("Die Datei ist zu groß ({0} MB) – höchstens 20 MB.")]
    TooLarge(u64),
    #[error("Die PDF-Datei hat {0} Seiten – höchstens 20 Seiten werden unterstützt.")]
    TooManyPages(usize),
    #[error(
        "Dateityp nicht unterstützt. Möglich sind Bilder (PNG, JPEG), PDF- und Word-Dateien \
         (DOCX)."
    )]
    Unsupported,
    #[error("Die Datei passt nicht zu ihrer Endung „.{0}“ (beschädigt oder umbenannt?).")]
    Mismatch(String),
    #[error("{0}")]
    OcrUnavailable(String),
    #[error("{0}")]
    Invalid(String),
}

impl Serialize for DocsError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    Docx,
    Pdf,
    Image,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Png,
    Jpeg,
    Pdf,
    Docx,
}

impl Kind {
    fn source(self) -> Source {
        match self {
            Kind::Png | Kind::Jpeg => Source::Image,
            Kind::Pdf => Source::Pdf,
            Kind::Docx => Source::Docx,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub file_name: String,
    pub text: String,
    pub source: Source,
    /// Number of pages (PDF), 1 for images, `None` for DOCX.
    pub pages: Option<usize>,
    /// Whether (part of) the text comes from OCR and needs extra care.
    pub ocr_used: bool,
    /// German notes for the user (unread pages, ignored images, …).
    pub warnings: Vec<String>,
    /// Preview of the original as data URL (image, or first PDF page where
    /// the system can render it).
    pub preview: Option<String>,
}

fn extension(file_name: &str) -> String {
    Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

fn kind_from_extension(file_name: &str) -> Result<Kind, DocsError> {
    Ok(match extension(file_name).as_str() {
        "png" => Kind::Png,
        "jpg" | "jpeg" => Kind::Jpeg,
        "pdf" => Kind::Pdf,
        "docx" => Kind::Docx,
        _ => return Err(DocsError::Unsupported),
    })
}

/// Detects the file type from extension and content (magic bytes).
fn detect(file_name: &str, bytes: &[u8]) -> Result<Kind, DocsError> {
    let kind = kind_from_extension(file_name)?;
    let magic_ok = match kind {
        Kind::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        Kind::Jpeg => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        // The header may follow a few junk bytes (allowed by the spec).
        Kind::Pdf => bytes[..bytes.len().min(1024)]
            .windows(5)
            .any(|w| w == b"%PDF-"),
        Kind::Docx => bytes.starts_with(b"PK\x03\x04"),
    };
    if !magic_ok {
        return Err(DocsError::Mismatch(extension(file_name)));
    }
    Ok(kind)
}

fn non_ws_chars(s: &str) -> usize {
    s.chars().filter(|c| !c.is_whitespace()).count()
}

/// Unifies line endings, removes trailing whitespace, collapses runs of more
/// than two empty lines and trims empty lines at both ends. Leading
/// whitespace (indentation) is kept.
pub fn tidy_text(s: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut blank_run = 0;
    let normalized = normalize_newlines(s);
    for line in normalized.split('\n').map(str::trim_end) {
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 2 || out.is_empty() {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push(line);
    }
    while out.last() == Some(&"") {
        out.pop();
    }
    out.join("\n")
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

fn phase(message: impl Into<String>) -> Progress {
    Progress::Phase {
        message: message.into(),
    }
}

/// Reads and imports the file at `path`.
pub fn import_file(
    path: &Path,
    mut on_progress: impl FnMut(Progress),
) -> Result<ImportResult, DocsError> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Check type and size before reading the whole file.
    kind_from_extension(&file_name)?;
    let size = std::fs::metadata(path)?.len();
    if size > MAX_FILE_BYTES {
        return Err(DocsError::TooLarge(size.div_ceil(1024 * 1024)));
    }
    on_progress(phase(format!("Lese „{file_name}“ …")));
    let bytes = std::fs::read(path)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(DocsError::TooLarge(
            (bytes.len() as u64).div_ceil(1024 * 1024),
        ));
    }
    import_bytes(&file_name, &bytes, on_progress)
}

/// Imports a document held in memory; `file_name` determines the type.
pub fn import_bytes(
    file_name: &str,
    bytes: &[u8],
    mut on_progress: impl FnMut(Progress),
) -> Result<ImportResult, DocsError> {
    let kind = detect(file_name, bytes)?;
    let mut warnings = Vec::new();
    let mut ocr_used = false;
    let mut preview = None;
    let (text, pages) = match kind {
        Kind::Docx => {
            on_progress(phase("Lese Word-Dokument …"));
            let doc = docx::extract(bytes)?;
            if doc.has_images {
                warnings.push(
                    "Bilder im Word-Dokument werden nicht erkannt – nur der Text wurde übernommen."
                        .to_string(),
                );
            }
            (doc.text, None)
        }
        Kind::Png | Kind::Jpeg => {
            if bytes.len() <= MAX_PREVIEW_BYTES {
                let mime = if kind == Kind::Png {
                    "image/png"
                } else {
                    "image/jpeg"
                };
                preview = Some(data_url(mime, bytes));
            } else {
                warnings.push("Das Bild ist für die Vorschau zu groß (über 5 MB).".to_string());
            }
            on_progress(phase("Texterkennung läuft …"));
            let words = ocr::recognize_image(bytes)?;
            ocr_used = true;
            (layout::reconstruct_text(&words), Some(1))
        }
        Kind::Pdf => {
            let (text, pages, used) = import_pdf(bytes, &mut warnings, &mut on_progress)?;
            ocr_used = used;
            preview = ocr::render_pdf_preview(bytes)
                .filter(|png| png.len() <= MAX_PREVIEW_BYTES)
                .map(|png| data_url("image/png", &png));
            (text, Some(pages))
        }
    };

    let mut text = tidy_text(&text);
    if text.is_empty() {
        warnings.push("In der Datei wurde kein Text gefunden.".to_string());
    }
    if text.len() > MAX_TEXT_BYTES {
        let mut cut = MAX_TEXT_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        warnings.push("Der Text war länger als 1 MB und wurde abgeschnitten.".to_string());
    }
    Ok(ImportResult {
        file_name: file_name.to_string(),
        text,
        source: kind.source(),
        pages,
        ocr_used,
        warnings,
        preview,
    })
}

fn page_list(pages: &[u32]) -> String {
    pages
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Text layer first; pages with (almost) no text are rendered and OCRed.
/// Returns the text, the page count and whether OCR was used.
fn import_pdf(
    bytes: &[u8],
    warnings: &mut Vec<String>,
    on_progress: &mut impl FnMut(Progress),
) -> Result<(String, usize, bool), DocsError> {
    on_progress(phase("Lese PDF-Datei …"));
    let doc = pdf::load(bytes)?;
    let count = pdf::page_count(&doc);
    let mut texts: Vec<Option<String>> = Vec::with_capacity(count);
    for n in 1..=count {
        on_progress(phase(format!("Lese Text: Seite {n} von {count} …")));
        texts.push(pdf::page_text(&doc, n as u32));
    }
    drop(doc);

    let scanned: Vec<u32> = (1..=count as u32)
        .filter(|&n| pdf::needs_ocr(texts[n as usize - 1].as_deref()))
        .collect();
    let all_scanned = scanned.len() == count;
    let mut ocr_used = false;
    if !scanned.is_empty() {
        let result = ocr::recognize_pdf_pages(bytes, &scanned, &mut |i| {
            on_progress(phase(format!(
                "Texterkennung: Seite {} ({} von {}) …",
                scanned[i],
                i + 1,
                scanned.len()
            )))
        });
        match result {
            Ok(pages) => {
                for (&n, words) in scanned.iter().zip(pages) {
                    let recognized = layout::reconstruct_text(&words);
                    let slot = &mut texts[n as usize - 1];
                    if non_ws_chars(&recognized) > slot.as_deref().map_or(0, non_ws_chars) {
                        *slot = Some(recognized);
                        ocr_used = true;
                    }
                }
            }
            Err(e) if all_scanned => {
                return Err(match e {
                    DocsError::OcrUnavailable(msg) => DocsError::OcrUnavailable(format!(
                        "Die PDF-Datei enthält keine Textebene (eingescannt?). {msg}"
                    )),
                    e => e,
                })
            }
            Err(e) => warnings.push(format!(
                "Seite {} enthält kaum Text (eingescannt?) und wurde nicht erkannt: {e}",
                page_list(&scanned)
            )),
        }
    }
    let unreadable: Vec<u32> = (1..=count as u32)
        .filter(|&n| texts[n as usize - 1].is_none())
        .collect();
    if !unreadable.is_empty() {
        warnings.push(format!(
            "Seite {} konnte nicht gelesen werden.",
            page_list(&unreadable)
        ));
    }
    let text = texts
        .into_iter()
        .map(Option::unwrap_or_default)
        .map(|t| tidy_text(&t))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok((text, count, ocr_used))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_checks_extension_and_content() {
        assert_eq!(
            detect("a.PNG", b"\x89PNG\r\n\x1a\nrest").unwrap(),
            Kind::Png
        );
        assert_eq!(
            detect("a.jpeg", &[0xFF, 0xD8, 0xFF, 0xE0]).unwrap(),
            Kind::Jpeg
        );
        assert_eq!(detect("a.pdf", b"%PDF-1.7").unwrap(), Kind::Pdf);
        assert_eq!(detect("a.docx", b"PK\x03\x04").unwrap(), Kind::Docx);
        assert!(matches!(detect("a.txt", b""), Err(DocsError::Unsupported)));
        assert!(matches!(detect("a", b""), Err(DocsError::Unsupported)));
        let e = detect("bild.png", b"%PDF-1.7").unwrap_err().to_string();
        assert!(e.contains("„.png“"), "{e}");
    }

    #[test]
    fn tidy_keeps_indentation_and_limits_blank_lines() {
        assert_eq!(
            tidy_text("\n\r\n  def f():  \r\n\tpass\n\n\n\n\nx \n\n"),
            "  def f():\n\tpass\n\n\nx"
        );
    }

    #[test]
    fn docx_import() {
        let xml = docx::tests::body(
            r#"<w:p><w:r><w:t>Aufgabe</w:t></w:r></w:p><w:p><w:r><w:t xml:space="preserve">    print(1)</w:t></w:r></w:p>"#,
        );
        let file = docx::tests::docx(&xml, &[("word/media/a.png", b"x")]);
        let mut phases = Vec::new();
        let r = import_bytes("Blatt.docx", &file, |p| phases.push(p)).unwrap();
        assert_eq!(r.text, "Aufgabe\n    print(1)");
        assert_eq!(r.source, Source::Docx);
        assert_eq!(r.pages, None);
        assert!(!r.ocr_used);
        assert_eq!(r.warnings.len(), 1);
        assert!(r.preview.is_none());
        assert!(!phases.is_empty());
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["source"], "docx");
        assert_eq!(json["fileName"], "Blatt.docx");
        assert_eq!(json["ocrUsed"], false);
    }

    #[test]
    fn pdf_with_text_layer() {
        let text = "Schreibe ein Programm, das zwei Zahlen addiert und ausgibt.";
        let file = pdf::tests::pdf(&[text, text]);
        let r = import_bytes("blatt.pdf", &file, |_| {}).unwrap();
        assert_eq!(r.source, Source::Pdf);
        assert_eq!(r.pages, Some(2));
        assert!(!r.ocr_used);
        assert_eq!(
            r.text.matches("zwei Zahlen addiert").count(),
            2,
            "{}",
            r.text
        );
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn scanned_pdf_without_ocr() {
        let text = "Schreibe ein Programm, das zwei Zahlen addiert und ausgibt.";
        // Mixed: page 2 is a "scan" → warning, text of page 1 is kept.
        let r = import_bytes("a.pdf", &pdf::tests::pdf(&[text, ""]), |_| {}).unwrap();
        assert!(r.text.contains("zwei Zahlen"));
        assert_eq!(r.warnings.len(), 1);
        assert!(
            r.warnings[0].starts_with("Seite 2 enthält kaum Text"),
            "{:?}",
            r.warnings
        );
        // Only scans → error.
        let e = import_bytes("a.pdf", &pdf::tests::pdf(&["", ""]), |_| {})
            .unwrap_err()
            .to_string();
        assert!(e.contains("keine Textebene"), "{e}");
        assert!(e.contains("nicht verfügbar"), "{e}");
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn image_without_ocr_is_an_error() {
        let e = import_bytes("a.png", b"\x89PNG\r\n\x1a\n", |_| {})
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("Texterkennung ist auf diesem System nicht verfügbar"),
            "{e}"
        );
    }

    #[test]
    fn file_limits() {
        let dir = std::env::temp_dir().join(format!("code-pruefer-docs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("gross.pdf");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_FILE_BYTES + 1).unwrap();
        let e = import_file(&big, |_| {}).unwrap_err().to_string();
        assert!(e.contains("zu groß (21 MB)"), "{e}");
        let txt = dir.join("a.txt");
        std::fs::write(&txt, "x").unwrap();
        assert!(matches!(
            import_file(&txt, |_| {}),
            Err(DocsError::Unsupported)
        ));
        let missing = import_file(&dir.join("fehlt.pdf"), |_| {}).unwrap_err();
        assert!(missing
            .to_string()
            .starts_with("Die Datei konnte nicht gelesen werden"));
        let ok = dir.join("ok.pdf");
        std::fs::write(
            &ok,
            pdf::tests::pdf(&["Ein Text mit genug Zeichen, damit die Seite nicht als Scan gilt."]),
        )
        .unwrap();
        assert_eq!(import_file(&ok, |_| {}).unwrap().file_name, "ok.pdf");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
