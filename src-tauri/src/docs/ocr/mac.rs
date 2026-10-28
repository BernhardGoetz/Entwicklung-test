//! macOS: Apple Vision (`VNRecognizeTextRequest`), PDF pages rendered with
//! PDFKit.
//!
//! Note: this file is type-checked in development (`cargo check --target
//! x86_64-apple-darwin`) but has not been run on a Mac yet.

use objc2::rc::{autoreleasepool, Retained};
use objc2::AllocAnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_pdf_kit::{PDFDisplayBox, PDFDocument, PDFPage};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

use super::prepare::Prepared;
use super::{prepared, render_size, MAX_RENDER_PIXELS, PREVIEW_PIXELS, RENDER_DPI};
use crate::docs::layout::WordBox;
use crate::docs::DocsError;

/// Vision returns normalized coordinates (0…1, origin bottom left); they are
/// scaled to this range per axis (only the relative layout matters).
const SCALE: f64 = 10_000.0;

fn recognize_data(data: &NSData) -> Result<Vec<WordBox>, DocsError> {
    let handler = VNImageRequestHandler::initWithData_options(
        VNImageRequestHandler::alloc(),
        data,
        &NSDictionary::new(),
    );
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(false);
    let languages =
        NSArray::from_retained_slice(&[NSString::from_str("de-DE"), NSString::from_str("en-US")]);
    request.setRecognitionLanguages(&languages);

    let as_request: &VNRequest = &request;
    let requests = NSArray::from_slice(&[as_request]);
    handler.performRequests_error(&requests).map_err(|e| {
        DocsError::Invalid(format!(
            "Texterkennung fehlgeschlagen: {}",
            e.localizedDescription()
        ))
    })?;

    let mut words = Vec::new();
    for observation in request.results().unwrap_or_default().iter() {
        let candidates = observation.topCandidates(1);
        let Some(best) = candidates.firstObject() else {
            continue;
        };
        let bbox = unsafe { observation.boundingBox() };
        words.push(WordBox {
            text: best.string().to_string(),
            x: bbox.origin.x * SCALE,
            y: (1.0 - bbox.origin.y - bbox.size.height) * SCALE,
            width: bbox.size.width * SCALE,
            height: bbox.size.height * SCALE,
        });
    }
    Ok(words)
}

/// Vision has no documented size limit; this keeps preprocessing sane
/// (the memory cap in `prepare` applies as well).
pub fn max_image_dimension() -> u32 {
    8192
}

/// Vision boxes are normalized, i.e. independent of scaling; the crop
/// offset only shifts the whole page, which keeps the relative layout
/// (indentation is measured from the leftmost line).
pub fn restore(_p: &Prepared, words: Vec<WordBox>) -> Vec<WordBox> {
    words
}

pub fn recognize_image(bytes: &[u8]) -> Result<Vec<WordBox>, DocsError> {
    autoreleasepool(|_| recognize_data(&NSData::with_bytes(bytes)))
}

fn load_pdf(pdf: &[u8]) -> Option<Retained<PDFDocument>> {
    let data = NSData::with_bytes(pdf);
    unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }
}

/// Renders a page as TIFF data (what `NSImage` produces natively).
fn render_page(page: &PDFPage, dpi: f64, max: f64) -> Option<Retained<NSData>> {
    let bounds = unsafe { page.boundsForBox(PDFDisplayBox::MediaBox) };
    let (w, h) = render_size(bounds.size.width, bounds.size.height, dpi, max);
    let image = unsafe {
        page.thumbnailOfSize_forBox(
            CGSize::new(f64::from(w), f64::from(h)),
            PDFDisplayBox::MediaBox,
        )
    };
    image.TIFFRepresentation()
}

fn pdf_error() -> DocsError {
    DocsError::Invalid(
        "Die PDF-Seite konnte für die Texterkennung nicht dargestellt werden.".into(),
    )
}

pub fn recognize_pdf_pages(
    pdf: &[u8],
    pages: &[u32],
    on_page: &mut dyn FnMut(usize),
) -> Result<Vec<Vec<WordBox>>, DocsError> {
    autoreleasepool(|_| {
        let doc = load_pdf(pdf).ok_or_else(pdf_error)?;
        let mut out = Vec::with_capacity(pages.len());
        for (i, &number) in pages.iter().enumerate() {
            on_page(i);
            let index = number.saturating_sub(1) as usize;
            let page = unsafe { doc.pageAtIndex(index) }.ok_or_else(pdf_error)?;
            let tiff = render_page(&page, RENDER_DPI, MAX_RENDER_PIXELS).ok_or_else(pdf_error)?;
            let prep = tiff_to_png(&tiff).and_then(|png| prepared(&png, MAX_RENDER_PIXELS as u32));
            match prep {
                Some(p) => out.push(recognize_data(&NSData::with_bytes(&p.png))?),
                None => out.push(recognize_data(&tiff)?),
            }
        }
        Ok(out)
    })
}

/// Converts TIFF data (from `render_page`) to PNG.
fn tiff_to_png(tiff: &NSData) -> Option<Vec<u8>> {
    let rep = NSBitmapImageRep::imageRepWithData(tiff)?;
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(png.to_vec())
}

pub fn render_pdf_preview(pdf: &[u8]) -> Option<Vec<u8>> {
    autoreleasepool(|_| {
        let doc = load_pdf(pdf)?;
        let page = unsafe { doc.pageAtIndex(0) }?;
        let tiff = render_page(&page, RENDER_DPI, PREVIEW_PIXELS)?;
        tiff_to_png(&tiff)
    })
}
