//! System OCR: Windows.Media.Ocr on Windows, Apple Vision on macOS.
//!
//! Both return word or line boxes; the text (with indentation) is rebuilt by
//! [`super::layout::reconstruct_text`]. Images and rendered PDF pages are
//! preprocessed first (grayscale, contrast, upscaling of small text; see
//! [`prepare`]). All functions block and must run on a blocking thread.

use super::layout::WordBox;
use super::DocsError;

#[cfg(target_os = "macos")]
mod mac;
pub mod prepare;
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

/// Preprocesses an encoded image for the platform OCR; `None` if it cannot
/// be decoded (the engine then gets the original). Only for images whose
/// size is known to be within limits (rendered pages, checked uploads).
#[cfg(any(windows, target_os = "macos"))]
fn prepared(bytes: &[u8], max_dimension: u32) -> Option<prepare::Prepared> {
    prepare::prepare(bytes, max_dimension).ok()
}

/// Longest side of an uploaded image.
pub const MAX_IMAGE_SIDE: u32 = 20_000;
/// Most pixels of an uploaded image (100 megapixels: 400 MB as RGBA).
pub const MAX_IMAGE_PIXELS: u64 = 100_000_000;

/// Width and height from the image header (nothing is decoded); images
/// above [`MAX_IMAGE_SIDE`] or [`MAX_IMAGE_PIXELS`] are refused, so neither
/// the own decoder nor the system decoder (WIC, Apple Vision) gets them.
pub fn check_image_dimensions(bytes: &[u8]) -> Result<(u32, u32), DocsError> {
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .ok_or_else(|| {
            DocsError::Invalid(
                "Das Bild konnte nicht gelesen werden (Bildgröße nicht lesbar). Ist die Datei \
                 beschädigt? Unterstützt werden PNG und JPEG."
                    .into(),
            )
        })?;
    if w == 0 || h == 0 {
        return Err(DocsError::Invalid(
            "Das Bild ist leer (0 Pixel breit oder hoch).".into(),
        ));
    }
    if w.max(h) > MAX_IMAGE_SIDE || u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS {
        return Err(DocsError::Invalid(format!(
            "Das Bild ist zu groß für die Texterkennung ({w} × {h} Pixel; höchstens 20 000 Pixel \
             je Seite und 100 Megapixel). Bitte das Bild verkleinern oder einen Ausschnitt \
             hochladen."
        )));
    }
    Ok((w, h))
}

/// Preprocessing of an upload whose header passed
/// [`check_image_dimensions`]: a decoder limit (`ImageError::Limits`, e.g.
/// 16-bit RGBA close to the pixel limit) is an error – the system decoder
/// must not get what ours refused –, other decoding problems (an encoding
/// variant the `image` crate does not support) leave the image to the
/// system decoder.
#[cfg(any(windows, target_os = "macos"))]
fn prepare_upload(
    bytes: &[u8],
    max_dimension: u32,
) -> Result<Option<prepare::Prepared>, DocsError> {
    match prepare::prepare(bytes, max_dimension) {
        Ok(p) => Ok(Some(p)),
        Err(e) if is_limit_error(&e) => Err(too_much_memory()),
        Err(_) => Ok(None),
    }
}

pub fn is_limit_error(e: &image::ImageError) -> bool {
    matches!(e, image::ImageError::Limits(_))
}

pub fn too_much_memory() -> DocsError {
    DocsError::Invalid(
        "Das Bild braucht zum Lesen zu viel Arbeitsspeicher. Bitte das Bild verkleinern (oder als \
         8-Bit-PNG/JPEG speichern) und erneut hochladen."
            .into(),
    )
}

/// Recognizes the text in a PNG or JPEG image (preprocessed; the original
/// is used if preprocessing fails for a reason other than a limit, or
/// yields no text).
pub fn recognize_image(bytes: &[u8]) -> Result<Vec<WordBox>, DocsError> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        check_image_dimensions(bytes)?;
        if let Some(p) = prepare_upload(bytes, platform::max_image_dimension())? {
            let words = platform::recognize_image(&p.png)?;
            if !words.is_empty() {
                return Ok(platform::restore(&p, words));
            }
        }
        platform::recognize_image(bytes)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = bytes;
        Err(unavailable())
    }
}

/// Renders the given PDF pages (1-based), preprocesses them and recognizes
/// their text.
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

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::GrayImage::new(w, h)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }

    /// A PNG claiming `w` x `h` with no pixel data at all (empty IDAT): the
    /// check must not need more than the header chunks.
    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8-bit grayscale
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &[]);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn image_size_is_checked_from_the_header() {
        assert_eq!(check_image_dimensions(&png(40, 30)).unwrap(), (40, 30));
        assert_eq!(
            check_image_dimensions(&png_header(20_000, 5_000)).unwrap(),
            (20_000, 5_000)
        );
        // 60 000 x 60 000 (the audit example): refused from 57 bytes.
        let e = check_image_dimensions(&png_header(60_000, 60_000))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("zu groß für die Texterkennung (60000 × 60000 Pixel"),
            "{e}"
        );
        // Side length and pixel count separately.
        assert!(check_image_dimensions(&png_header(20_001, 10)).is_err());
        assert!(check_image_dimensions(&png_header(12_000, 9_000)).is_err());
        let e = check_image_dimensions(b"\x89PNG\r\n\x1a\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("Bildgröße nicht lesbar"), "{e}");
        // Zero width: invalid already for the decoder.
        assert!(check_image_dimensions(&png_header(0, 10)).is_err());
    }

    #[test]
    fn decoder_limits_are_recognized() {
        // A header within our limits whose decoding still exceeds the
        // decoder's allocation limit is not handed to the system decoder.
        let mut reader = image::ImageReader::new(std::io::Cursor::new(png(4000, 4000)))
            .with_guessed_format()
            .unwrap();
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(1024 * 1024);
        reader.limits(limits);
        let e = reader.decode().unwrap_err();
        assert!(is_limit_error(&e), "{e:?}");
        let other = image::ImageReader::new(std::io::Cursor::new(b"\x89PNG\r\n\x1a\nxx".to_vec()))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap_err();
        assert!(!is_limit_error(&other), "{other:?}");
        assert!(too_much_memory()
            .to_string()
            .contains("zu viel Arbeitsspeicher"));
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
