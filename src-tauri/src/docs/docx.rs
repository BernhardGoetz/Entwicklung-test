//! Plain text from a DOCX file (`word/document.xml`).
//!
//! Deliberately simple: paragraphs become lines, `w:tab` a tab, `w:br` /
//! `w:cr` a line break. Formatting, headers/footers, footnotes and embedded
//! images are ignored.

use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;

use super::DocsError;

/// Upper bound for the uncompressed `word/document.xml` (zip bomb guard).
pub const MAX_DOCUMENT_XML_BYTES: u64 = 20 * 1024 * 1024;

const DOCUMENT_XML: &str = "word/document.xml";

fn invalid(detail: impl std::fmt::Display) -> DocsError {
    DocsError::Invalid(format!(
        "Die Word-Datei konnte nicht gelesen werden ({detail}). Bitte als .docx (Word 2007 oder \
         neuer) speichern."
    ))
}

/// Result of [`extract`]: text plus whether the document contains images
/// (which are not recognized).
#[derive(Debug, PartialEq)]
pub struct DocxText {
    pub text: String,
    pub has_images: bool,
}

pub fn extract(bytes: &[u8]) -> Result<DocxText, DocsError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(invalid)?;
    let has_images = archive
        .file_names()
        .any(|n| n.is_ok_and(|n| n.starts_with("word/media/")));
    let xml = {
        let entry = archive
            .by_name(DOCUMENT_XML)
            .map_err(|_| invalid("word/document.xml fehlt"))?;
        if entry.size() > MAX_DOCUMENT_XML_BYTES {
            return Err(DocsError::Invalid(
                "Die Word-Datei enthält zu viel Text (entpackt über 20 MB).".into(),
            ));
        }
        // The declared size may lie: never read more than the limit.
        let mut data = Vec::new();
        entry
            .take(MAX_DOCUMENT_XML_BYTES + 1)
            .read_to_end(&mut data)
            .map_err(invalid)?;
        if data.len() as u64 > MAX_DOCUMENT_XML_BYTES {
            return Err(DocsError::Invalid(
                "Die Word-Datei enthält zu viel Text (entpackt über 20 MB).".into(),
            ));
        }
        String::from_utf8(data).map_err(|_| invalid("kein UTF-8"))?
    };
    Ok(DocxText {
        text: document_text(&xml)?,
        has_images,
    })
}

/// Extracts the text of a WordprocessingML `document.xml`.
pub fn document_text(xml: &str) -> Result<String, DocsError> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    // Only the content of <w:t> elements is text; everything else (field
    // instructions, deleted text, …) is skipped.
    let mut in_text = false;
    // <w:tabs><w:tab …/></w:tabs> in paragraph properties are tab stop
    // definitions, not tab characters.
    let mut in_tab_stops = false;
    loop {
        match reader.read_event().map_err(invalid)? {
            Event::Start(e) => match e.local_name().as_ref() {
                "t" => in_text = true,
                "tabs" => in_tab_stops = true,
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "tab" if !in_tab_stops => out.push('\t'),
                "br" | "cr" => out.push('\n'),
                // Empty paragraph = empty line.
                "p" => out.push('\n'),
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "tabs" => in_tab_stops = false,
                "p" => out.push('\n'),
                _ => {}
            },
            Event::Text(t) if in_text => out.push_str(&t.xml10_content()),
            Event::CData(t) if in_text => out.push_str(&t),
            Event::GeneralRef(r) if in_text => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    out.push(c);
                } else {
                    match &*r.xml10_content() {
                        "amp" => out.push('&'),
                        "lt" => out.push('<'),
                        "gt" => out.push('>'),
                        "quot" => out.push('"'),
                        "apos" => out.push('\''),
                        _ => {}
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    pub(crate) fn body(paragraphs: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>{paragraphs}</w:body></w:document>"#
        )
    }

    /// Builds a minimal DOCX (ZIP) in memory.
    pub(crate) fn docx(document_xml: &str, extra: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("[Content_Types].xml", opts).unwrap();
            zip.write_all(b"<Types/>").unwrap();
            zip.start_file(DOCUMENT_XML, opts).unwrap();
            zip.write_all(document_xml.as_bytes()).unwrap();
            for (name, data) in extra {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn paragraphs_tabs_and_breaks() {
        let xml = body(concat!(
            r#"<w:p><w:r><w:t>Aufgabe 1</w:t></w:r></w:p>"#,
            r#"<w:p/>"#,
            r#"<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr><w:r><w:t xml:space="preserve">Eingabe:</w:t><w:tab/><w:t>3 4</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:t>def f():</w:t><w:br/><w:t xml:space="preserve">    return 1</w:t></w:r></w:p>"#,
        ));
        let text = extract(&docx(&xml, &[])).unwrap();
        assert_eq!(
            text.text,
            "Aufgabe 1\n\nEingabe:\t3 4\ndef f():\n    return 1\n"
        );
        assert!(!text.has_images);
    }

    #[test]
    fn runs_are_joined_and_entities_resolved() {
        let xml = body(concat!(
            r#"<w:p><w:r><w:t>a &lt; b</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve"> &amp;&amp; c&#x2264;d</w:t></w:r></w:p>"#,
            // Deleted text and field instructions are not part of the text.
            r#"<w:p><w:del><w:r><w:delText>weg</w:delText></w:r></w:del><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:t>ü</w:t></w:r></w:p>"#,
        ));
        assert_eq!(document_text(&xml).unwrap(), "a < b && c≤d\nü\n");
    }

    #[test]
    fn images_are_reported() {
        let xml = body("<w:p><w:r><w:t>x</w:t></w:r></w:p>");
        let text = extract(&docx(&xml, &[("word/media/image1.png", b"png")])).unwrap();
        assert!(text.has_images);
    }

    #[test]
    fn not_a_zip_or_missing_document_is_a_german_error() {
        let e = extract(b"kein zip").unwrap_err().to_string();
        assert!(e.contains("Word-Datei konnte nicht gelesen werden"), "{e}");
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            zip.start_file("x.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.finish().unwrap();
        }
        let e = extract(&buf.into_inner()).unwrap_err().to_string();
        assert!(e.contains("word/document.xml fehlt"), "{e}");
    }

    #[test]
    fn zip_bomb_is_rejected() {
        // 21 MB of zeros compress to a few KB.
        let big = body(&format!(
            "<w:p><w:r><w:t>{}</w:t></w:r></w:p>",
            "0".repeat(21 * 1024 * 1024)
        ));
        let file = docx(&big, &[]);
        assert!(file.len() < 1024 * 1024);
        let e = extract(&file).unwrap_err().to_string();
        assert!(e.contains("über 20 MB"), "{e}");
    }
}
