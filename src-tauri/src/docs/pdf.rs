//! Text layer of PDF files (pure Rust, `pdf-extract` on top of `lopdf`).
//!
//! Pages without a usable text layer (scans) are detected here; their OCR
//! happens in [`super::ocr`].

use std::panic::{catch_unwind, AssertUnwindSafe};

use pdf_extract::{Document, PlainTextOutput};

use super::{DocsError, MAX_PAGES};

/// A page with fewer non-whitespace characters than this counts as scanned.
pub const MIN_TEXT_CHARS: usize = 50;

/// Text layer of every page (index 0 = page 1). `None`: the page could not
/// be read.
#[derive(Debug)]
pub struct PdfText {
    pub pages: Vec<Option<String>>,
}

fn damaged(detail: impl std::fmt::Display) -> DocsError {
    DocsError::Invalid(format!(
        "Die PDF-Datei konnte nicht gelesen werden ({detail}). Ist sie beschädigt?"
    ))
}

/// Runs `f` and turns a panic of the PDF crates into an error message.
fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        p.downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "interner Fehler".into())
    })
}

/// Loads (and, with an empty password, decrypts) the document and checks
/// the page limit before any text is extracted.
pub fn load(bytes: &[u8]) -> Result<Document, DocsError> {
    let mut doc = guarded(|| Document::load_mem(bytes))
        .map_err(damaged)?
        .map_err(damaged)?;
    if doc.is_encrypted() {
        // Many PDFs are "encrypted" with an empty user password (only
        // printing/copying restricted); those can be read.
        let ok = guarded(|| doc.decrypt("").is_ok()).unwrap_or(false);
        if !ok {
            return Err(DocsError::Invalid(
                "Die PDF-Datei ist mit einem Passwort geschützt. Bitte eine ungeschützte Fassung \
                 hochladen."
                    .into(),
            ));
        }
    }
    let pages = doc.get_pages().len();
    if pages == 0 {
        return Err(damaged("keine Seiten"));
    }
    if pages > MAX_PAGES {
        return Err(DocsError::TooManyPages(pages));
    }
    Ok(doc)
}

pub fn page_count(doc: &Document) -> usize {
    doc.get_pages().len()
}

/// Text layer of page `number` (1-based); `None` if it cannot be read.
pub fn page_text(doc: &Document, number: u32) -> Option<String> {
    guarded(|| {
        let mut s = String::new();
        let mut output = PlainTextOutput::new(&mut s);
        pdf_extract::output_doc_page(doc, &mut output, number).map(|_| s)
    })
    .ok()?
    .ok()
}

pub fn extract(bytes: &[u8]) -> Result<PdfText, DocsError> {
    let doc = load(bytes)?;
    let pages = (1..=page_count(&doc) as u32)
        .map(|n| page_text(&doc, n))
        .collect();
    Ok(PdfText { pages })
}

/// Whether a page's text layer is too thin to be the real content.
pub fn needs_ocr(text: Option<&str>) -> bool {
    text.map_or(0, |t| t.chars().filter(|c| !c.is_whitespace()).count()) < MIN_TEXT_CHARS
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a small, valid PDF: one page per entry, each line of the
    /// entry as a Helvetica text line. An empty entry gives a page without
    /// text (like a scan).
    pub(crate) fn pdf(pages: &[&str]) -> Vec<u8> {
        let n = pages.len();
        // Objects: 1 catalog, 2 pages, 3 font, then per page: page + content.
        let mut objects: Vec<String> = Vec::new();
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
        objects.push("<< /Type /Catalog /Pages 2 0 R >>".into());
        objects.push(format!(
            "<< /Type /Pages /Kids [{}] /Count {n} >>",
            kids.join(" ")
        ));
        objects.push(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .into(),
        );
        for (i, text) in pages.iter().enumerate() {
            let mut stream = String::new();
            if !text.is_empty() {
                stream.push_str("BT /F1 12 Tf 14 TL 72 760 Td\n");
                for line in text.lines() {
                    let escaped = line
                        .replace('\\', "\\\\")
                        .replace('(', "\\(")
                        .replace(')', "\\)");
                    stream.push_str(&format!("({escaped}) Tj T*\n"));
                }
                stream.push_str("ET\n");
            }
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                5 + 2 * i
            ));
            objects.push(format!(
                "<< /Length {} >>\nstream\n{stream}endstream",
                stream.len()
            ));
        }
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, obj) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{obj}\nendobj\n", i + 1));
        }
        let xref = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for o in offsets {
            out.push_str(&format!("{o:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

    const TASK: &str =
        "Aufgabe: Lies eine Zahl n ein und gib n*n aus.\nBeispiel: Eingabe 3 ergibt Ausgabe 9.";

    #[test]
    fn text_layer_per_page() {
        let text = extract(&pdf(&[TASK, "", "kurz"])).unwrap();
        assert_eq!(text.pages.len(), 3);
        let p1 = text.pages[0].as_deref().unwrap();
        assert!(p1.contains("Lies eine Zahl n ein"), "{p1:?}");
        assert!(p1.contains("Eingabe 3 ergibt Ausgabe 9"), "{p1:?}");
        assert!(!needs_ocr(text.pages[0].as_deref()));
        // Empty and nearly empty pages are treated as scans.
        assert!(needs_ocr(text.pages[1].as_deref()));
        assert!(needs_ocr(text.pages[2].as_deref()));
        assert!(needs_ocr(None));
    }

    #[test]
    fn too_many_pages_are_rejected() {
        let pages = vec!["x"; MAX_PAGES + 1];
        let e = extract(&pdf(&pages)).unwrap_err().to_string();
        assert!(e.contains("21 Seiten"), "{e}");
        assert!(extract(&pdf(&vec!["x"; MAX_PAGES])).is_ok());
    }

    #[test]
    fn garbage_gives_german_error() {
        let e = extract(b"%PDF-1.4\nkaputt").unwrap_err().to_string();
        assert!(e.contains("PDF-Datei konnte nicht gelesen werden"), "{e}");
        // Truncated file.
        let full = pdf(&[TASK]);
        let e = extract(&full[..full.len() / 2]).unwrap_err().to_string();
        assert!(e.contains("PDF-Datei"), "{e}");
    }

    #[test]
    fn panics_become_errors() {
        let r: Result<(), String> = guarded(|| panic!("boom"));
        assert_eq!(r.unwrap_err(), "boom");
    }
}
