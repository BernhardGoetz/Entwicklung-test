//! System OCR: Windows.Media.Ocr on Windows, Apple Vision on macOS.
//!
//! Both return word or line boxes; the text (with indentation) is rebuilt by
//! [`super::layout::reconstruct_text`]. All functions block and must run on
//! a blocking thread.

use super::layout::WordBox;
use super::DocsError;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod win;

#[cfg(target_os = "macos")]
use mac as platform;
#[cfg(windows)]
use win as platform;

/// Whether this build has a system OCR.
pub const AVAILABLE: bool = cfg!(any(windows, target_os = "macos"));

/// Longest side of rendered PDF pages and of preview images, in pixels.
pub const MAX_RENDER_PIXELS: f64 = 3000.0;
/// Render resolution for scanned PDF pages (dots per inch).
pub const RENDER_DPI: f64 = 300.0;
/// Longest side of the PDF preview image, in pixels.
pub const PREVIEW_PIXELS: f64 = 1000.0;

/// Target size for rendering a page of `width` x `height` points
/// (1/72 inch) at `dpi`, scaled down so the longer side fits `max`.
pub fn render_size(width: f64, height: f64, dpi: f64, max: f64) -> (u32, u32) {
    let mut scale = dpi / 72.0;
    let longest = width.max(height) * scale;
    if longest > max {
        scale *= max / longest;
    }
    (
        ((width * scale).round() as u32).max(1),
        ((height * scale).round() as u32).max(1),
    )
}

#[cfg(not(any(windows, target_os = "macos")))]
fn unavailable() -> DocsError {
    DocsError::OcrUnavailable(
        "Texterkennung ist auf diesem System nicht verfügbar. Bitte den Text einfügen oder ein \
         DOCX bzw. ein PDF mit Textebene hochladen."
            .into(),
    )
}

/// Recognizes the text in a PNG or JPEG image.
pub fn recognize_image(bytes: &[u8]) -> Result<Vec<WordBox>, DocsError> {
    #[cfg(any(windows, target_os = "macos"))]
    return platform::recognize_image(bytes);
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = bytes;
        Err(unavailable())
    }
}

/// Renders the given PDF pages (1-based) and recognizes their text.
/// `on_page(i)` is called before the i-th entry of `pages` is processed.
pub fn recognize_pdf_pages(
    pdf: &[u8],
    pages: &[u32],
    on_page: &mut dyn FnMut(usize),
) -> Result<Vec<Vec<WordBox>>, DocsError> {
    #[cfg(any(windows, target_os = "macos"))]
    return platform::recognize_pdf_pages(pdf, pages, on_page);
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (pdf, pages, on_page);
        Err(unavailable())
    }
}

/// First page of a PDF as PNG (for the preview), if the system can render it.
pub fn render_pdf_preview(pdf: &[u8]) -> Option<Vec<u8>> {
    #[cfg(any(windows, target_os = "macos"))]
    return platform::render_pdf_preview(pdf);
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = pdf;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_size_respects_dpi_and_limit() {
        // A4 at 300 dpi: 2480 x 3508 px, limited to 3000 on the long side.
        assert_eq!(render_size(595.0, 842.0, 300.0, 3000.0), (2120, 3000));
        assert_eq!(render_size(595.0, 842.0, 72.0, 3000.0), (595, 842));
        assert_eq!(render_size(0.0, 0.0, 300.0, 3000.0), (1, 1));
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn other_systems_report_missing_ocr() {
        let e = recognize_image(b"x").unwrap_err().to_string();
        assert!(
            e.contains("Texterkennung ist auf diesem System nicht verfügbar"),
            "{e}"
        );
        assert!(render_pdf_preview(b"x").is_none());
    }
}
