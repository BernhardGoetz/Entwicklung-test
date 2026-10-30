//! Windows: Windows.Media.Ocr, PDF pages rendered with Windows.Data.Pdf.
//!
//! Runs on a blocking worker thread; the WinRT async operations are awaited
//! synchronously with `join()`.

use ::windows::core::{Interface, Result as WinResult};
use ::windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use ::windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapEncoder, BitmapInterpolationMode, BitmapPixelFormat,
    BitmapTransform, ColorManagementMode, ExifOrientationMode, SoftwareBitmap,
};
use ::windows::Media::Ocr::OcrEngine;
use ::windows::Storage::Streams::{
    DataReader, DataWriter, IRandomAccessStream, InMemoryRandomAccessStream,
};
use ::windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

use super::prepare::Prepared;
use super::{prepared, render_size, MAX_RENDER_PIXELS, PREVIEW_PIXELS, RENDER_DPI};
use crate::docs::layout::WordBox;
use crate::docs::DocsError;

const LANGUAGE_HINT: &str = "Windows hat keine Sprache mit Texterkennung installiert. Bitte unter \
     Einstellungen → Zeit und Sprache → Sprache eine Sprache wie „Deutsch (Deutschland)“ \
     hinzufügen (mit dem Sprachfeature „Optische Zeichenerkennung“) und erneut hochladen.";

/// Enters the multithreaded WinRT apartment for the current (worker) thread.
struct Apartment {
    initialized: bool,
}

impl Apartment {
    fn enter() -> Self {
        // S_FALSE (already initialized) also counts as success and must be
        // balanced; RPC_E_CHANGED_MODE (an STA thread) is an error and must not.
        let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
        Apartment { initialized }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { RoUninitialize() };
        }
    }
}

fn ocr_error(e: ::windows::core::Error) -> DocsError {
    DocsError::Invalid(format!("Texterkennung fehlgeschlagen: {}", e.message()))
}

fn image_error(e: ::windows::core::Error) -> DocsError {
    DocsError::Invalid(format!(
        "Das Bild konnte nicht gelesen werden ({}). Unterstützt werden PNG und JPEG.",
        e.message()
    ))
}

fn pdf_error(e: ::windows::core::Error) -> DocsError {
    DocsError::Invalid(format!(
        "Die PDF-Seite konnte für die Texterkennung nicht dargestellt werden ({}).",
        e.message()
    ))
}

fn stream_from_bytes(bytes: &[u8]) -> WinResult<IRandomAccessStream> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.join()?;
    writer.FlushAsync()?.join()?;
    writer.DetachStream()?;
    stream.Seek(0)?;
    stream.cast()
}

fn bytes_from_stream(stream: &IRandomAccessStream) -> WinResult<Vec<u8>> {
    let size = stream.Size()? as u32;
    stream.Seek(0)?;
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
    reader.LoadAsync(size)?.join()?;
    let mut data = vec![0u8; size as usize];
    reader.ReadBytes(&mut data)?;
    Ok(data)
}

fn engine() -> Result<OcrEngine, DocsError> {
    // Returns null (an error here) if no profile language supports OCR.
    OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|_| DocsError::OcrUnavailable(LANGUAGE_HINT.into()))
}

/// Decodes an image stream, scaled so that its longer side fits `max`.
fn decode(stream: &IRandomAccessStream, max: u32) -> WinResult<SoftwareBitmap> {
    let decoder = BitmapDecoder::CreateAsync(stream)?.join()?;
    // Scaling is applied before the EXIF rotation, so use the stored size.
    let (w, h) = (decoder.PixelWidth()?, decoder.PixelHeight()?);
    let transform = BitmapTransform::new()?;
    let longest = w.max(h);
    if longest > max {
        let scale = f64::from(max) / f64::from(longest);
        transform.SetScaledWidth(((f64::from(w) * scale).floor() as u32).max(1))?;
        transform.SetScaledHeight(((f64::from(h) * scale).floor() as u32).max(1))?;
        transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
    }
    decoder
        .GetSoftwareBitmapTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &transform,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )?
        .join()
}

fn recognize(engine: &OcrEngine, bitmap: &SoftwareBitmap) -> Result<Vec<WordBox>, DocsError> {
    let result = engine
        .RecognizeAsync(bitmap)
        .and_then(|op| op.join())
        .map_err(ocr_error)?;
    let mut words = Vec::new();
    for line in result.Lines().map_err(ocr_error)? {
        for word in line.Words().map_err(ocr_error)? {
            let rect = word.BoundingRect().map_err(ocr_error)?;
            words.push(WordBox {
                text: word.Text().map_err(ocr_error)?.to_string_lossy(),
                x: f64::from(rect.X),
                y: f64::from(rect.Y),
                width: f64::from(rect.Width),
                height: f64::from(rect.Height),
            });
        }
    }
    Ok(words)
}

fn max_dimension() -> u32 {
    OcrEngine::MaxImageDimension().unwrap_or(4096).max(1)
}

/// Largest image side Windows OCR accepts (`OcrEngine.MaxImageDimension`).
pub fn max_image_dimension() -> u32 {
    let _apartment = Apartment::enter();
    max_dimension()
}

/// Windows OCR boxes are in pixels of the prepared image.
pub fn restore(p: &Prepared, words: Vec<WordBox>) -> Vec<WordBox> {
    p.restore(words)
}

pub fn recognize_image(bytes: &[u8]) -> Result<Vec<WordBox>, DocsError> {
    let _apartment = Apartment::enter();
    let engine = engine()?;
    let stream = stream_from_bytes(bytes).map_err(image_error)?;
    let bitmap = decode(&stream, max_dimension()).map_err(image_error)?;
    recognize(&engine, &bitmap)
}

/// Renders page `index` (0-based) to a PNG stream.
fn render_page(
    doc: &PdfDocument,
    index: u32,
    dpi: f64,
    max: f64,
) -> WinResult<IRandomAccessStream> {
    let page = doc.GetPage(index)?;
    // Page size in DIPs (1/96 inch); `render_size` expects points.
    let size = page.Size()?;
    let (w, h) = render_size(
        f64::from(size.Width) * 72.0 / 96.0,
        f64::from(size.Height) * 72.0 / 96.0,
        dpi,
        max,
    );
    let options = PdfPageRenderOptions::new()?;
    options.SetDestinationWidth(w)?;
    options.SetDestinationHeight(h)?;
    options.SetBitmapEncoderId(BitmapEncoder::PngEncoderId()?)?;
    let out = InMemoryRandomAccessStream::new()?;
    page.RenderWithOptionsToStreamAsync(&out, &options)?
        .join()?;
    out.Seek(0)?;
    out.cast()
}

fn load_pdf(pdf: &[u8]) -> WinResult<PdfDocument> {
    let stream = stream_from_bytes(pdf)?;
    PdfDocument::LoadFromStreamAsync(&stream)?.join()
}

pub fn recognize_pdf_pages(
    pdf: &[u8],
    pages: &[u32],
    on_page: &mut dyn FnMut(usize),
) -> Result<Vec<Vec<WordBox>>, DocsError> {
    let _apartment = Apartment::enter();
    let engine = engine()?;
    let doc = load_pdf(pdf).map_err(pdf_error)?;
    let max = f64::from(max_dimension()).min(MAX_RENDER_PIXELS);
    let mut out = Vec::with_capacity(pages.len());
    for (i, &number) in pages.iter().enumerate() {
        on_page(i);
        let png =
            render_page(&doc, number.saturating_sub(1), RENDER_DPI, max).map_err(pdf_error)?;
        let prep = bytes_from_stream(&png)
            .ok()
            .and_then(|bytes| prepared(&bytes, max as u32));
        let words = match prep {
            Some(p) => {
                let stream = stream_from_bytes(&p.png).map_err(pdf_error)?;
                let bitmap = decode(&stream, max as u32).map_err(pdf_error)?;
                p.restore(recognize(&engine, &bitmap)?)
            }
            None => {
                let bitmap = decode(&png, max as u32).map_err(pdf_error)?;
                recognize(&engine, &bitmap)?
            }
        };
        out.push(words);
    }
    Ok(out)
}

pub fn render_pdf_preview(pdf: &[u8]) -> Option<Vec<u8>> {
    let _apartment = Apartment::enter();
    let doc = load_pdf(pdf).ok()?;
    let png = render_page(&doc, 0, RENDER_DPI, PREVIEW_PIXELS).ok()?;
    bytes_from_stream(&png).ok()
}
