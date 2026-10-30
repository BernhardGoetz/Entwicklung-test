//! Image preprocessing before the system OCR.
//!
//! Screenshots of code often use small fonts (12–14 px), which Windows OCR
//! misreads (`Hallo` → `Ha110`). Before recognition the image is
//!
//! 1. converted to grayscale (transparency composited over white),
//! 2. normalized to dark text on a light background (dark IDE themes are
//!    inverted) with a percentile-based contrast stretch,
//! 3. cropped to its content (plus a margin), so that more of the engine's
//!    maximum image size is left for the text,
//! 4. upscaled so that a text line is about [`TARGET_LINE_PX`] pixels high.
//!
//! The line height is estimated from the image itself (horizontal
//! projection profile: runs of rows that contain ink) instead of a first
//! OCR pass – cheaper, and it fails safe: merged or noisy profiles look
//! like tall lines and lead to no upscaling. Without any estimate, small
//! images are doubled. The scale never exceeds the engine's maximum image
//! dimension nor [`MAX_PIXELS`].
//!
//! All functions are pure; [`Prepared::restore`] maps word boxes back to
//! the coordinates of the original image.

use std::io::Cursor;

use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, ImageDecoder, ImageReader, Luma};

use crate::docs::layout::WordBox;

/// Target height of a text line's ink (ascender to descender), in pixels.
pub const TARGET_LINE_PX: f64 = 32.0;
/// Largest upscaling factor.
pub const MAX_UPSCALE: f64 = 4.0;
/// Smaller upscaling is not worth the resampling.
pub const MIN_UPSCALE: f64 = 1.2;
/// Without a line height estimate, images whose shorter side is below this
/// are doubled.
pub const SMALL_IMAGE_SIDE: u32 = 1000;
/// Memory cap for the prepared image (8-bit grayscale: bytes).
pub const MAX_PIXELS: u64 = 24_000_000;
/// Contrast stretch: this percentile of all pixels becomes white …
const HIGH_PERCENTILE: f64 = 0.99;
/// … and this percentile of the text pixels black. Text pixels are those
/// at least [`TEXT_MARGIN`] darker than the background (median), so that
/// sparse text (a few lines on a large screenshot) is stretched as well.
const LOW_PERCENTILE: f64 = 0.05;
const TEXT_MARGIN: u8 = 24;
/// Pixels darker than this (after normalization) count as ink.
const INK_THRESHOLD: u8 = 128;
/// Pixels darker than this count as content when cropping (lenient, so
/// that faint text such as grey comments or line numbers is kept).
const CONTENT_THRESHOLD: u8 = 224;
/// A row needs at least this many ink pixels to belong to a text line.
const MIN_INK_PER_ROW: usize = 2;
/// Columns with ink in more than this share of rows are rules or borders
/// (gutter separators, scroll bars), not text; they are ignored.
const BORDER_COLUMN_SHARE: f64 = 0.8;
/// Ink runs lower than this are noise or horizontal rules.
const MIN_LINE_PX: usize = 3;
/// Margin kept around the content when cropping, in pixels.
const CROP_MARGIN: u32 = 16;

/// A preprocessed image (PNG) and how it relates to the original.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub png: Vec<u8>,
    /// Factor applied after cropping (prepared = (original - offset) * scale).
    pub scale: f64,
    pub offset_x: u32,
    pub offset_y: u32,
}

impl Prepared {
    /// Maps word boxes from the prepared image back to the original image.
    pub fn restore(&self, words: Vec<WordBox>) -> Vec<WordBox> {
        words
            .into_iter()
            .map(|w| WordBox {
                x: w.x / self.scale + f64::from(self.offset_x),
                y: w.y / self.scale + f64::from(self.offset_y),
                width: w.width / self.scale,
                height: w.height / self.scale,
                text: w.text,
            })
            .collect()
    }
}

/// Decodes a PNG or JPEG (EXIF orientation applied) and prepares it for an
/// OCR engine that accepts images up to `max_dimension` pixels per side.
pub fn prepare(bytes: &[u8], max_dimension: u32) -> Result<Prepared, image::ImageError> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    // The default limits cap decoder allocations at 512 MB.
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);

    let mut gray = to_gray(&img);
    drop(img);
    normalize(&mut gray);
    let (x, y, w, h) = content_bounds(&gray, CROP_MARGIN);
    if (w, h) != gray.dimensions() {
        gray = image::imageops::crop_imm(&gray, x, y, w, h).to_image();
    }
    let line = estimate_line_height(&gray);
    let scale = choose_scale(w, h, line, max_dimension, MAX_PIXELS);
    if scale != 1.0 {
        let (sw, sh) = scaled_size(w, h, scale);
        gray = image::imageops::resize(&gray, sw, sh, FilterType::CatmullRom);
    }
    let mut png = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Sub,
    );
    gray.write_with_encoder(encoder)?;
    // The effective scale after rounding to whole pixels.
    let effective = f64::from(gray.width()) / f64::from(w.max(1));
    Ok(Prepared {
        png,
        scale: effective,
        offset_x: x,
        offset_y: y,
    })
}

/// Grayscale; transparent pixels are composited over white (screenshots
/// with a transparent background would otherwise turn black).
pub fn to_gray(img: &DynamicImage) -> GrayImage {
    if !img.color().has_alpha() {
        return img.to_luma8();
    }
    let rgba = img.to_rgba8();
    GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let luma = 0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b);
        let alpha = f64::from(a) / 255.0;
        Luma([(luma * alpha + 255.0 * (1.0 - alpha)).round() as u8])
    })
}

fn histogram(img: &GrayImage) -> [u64; 256] {
    let mut h = [0u64; 256];
    for p in img.as_raw() {
        h[*p as usize] += 1;
    }
    h
}

/// Smallest value with at least `share` of all pixels at or below it.
fn percentile(hist: &[u64; 256], share: f64) -> u8 {
    let total: u64 = hist.iter().sum();
    let target = (total as f64 * share).ceil().max(1.0) as u64;
    let mut seen = 0;
    for (v, &n) in hist.iter().enumerate() {
        seen += n;
        if seen >= target {
            return v as u8;
        }
    }
    255
}

/// Dark text on a light background with full contrast: inverts the image if
/// the background (median) is dark, then stretches the text level (5th
/// percentile of the text pixels) .. 99th percentile of all pixels to
/// 0..255. Images without text-like contrast stay unchanged.
pub fn normalize(img: &mut GrayImage) {
    let mut hist = histogram(img);
    let mut background = percentile(&hist, 0.5);
    if background < 128 {
        for p in img.iter_mut() {
            *p = 255 - *p;
        }
        hist.reverse();
        background = 255 - background;
    }
    let Some(text_max) = background.checked_sub(TEXT_MARGIN) else {
        return;
    };
    let mut text_hist = [0u64; 256];
    text_hist[..text_max as usize].copy_from_slice(&hist[..text_max as usize]);
    if text_hist.iter().sum::<u64>() == 0 {
        return;
    }
    let lo = percentile(&text_hist, LOW_PERCENTILE);
    let hi = percentile(&hist, HIGH_PERCENTILE);
    if hi <= lo || hi - lo < 16 {
        return;
    }
    let lut: Vec<u8> = (0..=255u16)
        .map(|v| {
            let v = f64::from(v);
            let s = (v - f64::from(lo)) / f64::from(hi - lo) * 255.0;
            s.clamp(0.0, 255.0).round() as u8
        })
        .collect();
    for p in img.iter_mut() {
        *p = lut[*p as usize];
    }
}

/// Columns that look like borders (ink in most rows).
fn border_columns(img: &GrayImage) -> Vec<bool> {
    let (w, h) = img.dimensions();
    let mut ink = vec![0usize; w as usize];
    for (x, _, p) in img.enumerate_pixels() {
        if p.0[0] < INK_THRESHOLD {
            ink[x as usize] += 1;
        }
    }
    ink.iter()
        .map(|&n| h > 0 && n as f64 > BORDER_COLUMN_SHARE * f64::from(h))
        .collect()
}

/// Ink pixels per row, ignoring border columns.
fn row_profile(img: &GrayImage) -> Vec<usize> {
    let borders = border_columns(img);
    let mut rows = vec![0usize; img.height() as usize];
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[0] < INK_THRESHOLD && !borders[x as usize] {
            rows[y as usize] += 1;
        }
    }
    rows
}

/// Median height of the runs of ink rows (text lines), if there are any.
pub fn estimate_line_height(img: &GrayImage) -> Option<f64> {
    let mut runs = Vec::new();
    let mut current = 0usize;
    for n in row_profile(img).into_iter().chain([0]) {
        if n >= MIN_INK_PER_ROW {
            current += 1;
        } else {
            if current >= MIN_LINE_PX {
                runs.push(current);
            }
            current = 0;
        }
    }
    if runs.is_empty() {
        return None;
    }
    runs.sort_unstable();
    Some(runs[runs.len() / 2] as f64)
}

/// Bounding box (x, y, width, height) of all content plus `margin`, or the
/// whole image if it is blank.
pub fn content_bounds(img: &GrayImage, margin: u32) -> (u32, u32, u32, u32) {
    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[0] < CONTENT_THRESHOLD {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 == u32::MAX {
        return (0, 0, w, h);
    }
    let x = x0.saturating_sub(margin);
    let y = y0.saturating_sub(margin);
    let right = (x1 + 1 + margin).min(w);
    let bottom = (y1 + 1 + margin).min(h);
    (x, y, right - x, bottom - y)
}

/// Scaling factor for an image of `width` x `height` with the estimated
/// line height `line_px`: aims at [`TARGET_LINE_PX`], at most
/// [`MAX_UPSCALE`], skips small upscales, and keeps the result within
/// `max_dimension` per side and `max_pixels` in total (which may also
/// shrink an image).
pub fn choose_scale(
    width: u32,
    height: u32,
    line_px: Option<f64>,
    max_dimension: u32,
    max_pixels: u64,
) -> f64 {
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let desired = match line_px {
        Some(l) if l > 0.0 => (TARGET_LINE_PX / l).clamp(1.0, MAX_UPSCALE),
        _ if width.min(height) < SMALL_IMAGE_SIDE => 2.0,
        _ => 1.0,
    };
    let cap = (f64::from(max_dimension.max(1)) / w.max(h))
        .min((max_pixels.max(1) as f64 / (w * h)).sqrt());
    let scale = desired.min(cap);
    if scale < 1.0 {
        // Too large for the engine or the memory cap: shrink.
        scale
    } else if scale < MIN_UPSCALE {
        1.0
    } else {
        scale
    }
}

/// Pixel size after scaling (floor, so the limits hold; at least 1 px).
pub fn scaled_size(width: u32, height: u32, scale: f64) -> (u32, u32) {
    (
        ((f64::from(width) * scale).floor() as u32).max(1),
        ((f64::from(height) * scale).floor() as u32).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Light page (240) with dark "text lines" (40) of `line` px height,
    /// every `pitch` px, from x = 20 to x = width - 20 with gaps (words).
    fn page(width: u32, height: u32, line: u32, pitch: u32) -> GrayImage {
        GrayImage::from_fn(width, height, |x, y| {
            let in_line = y >= 10 && (y - 10) % pitch < line;
            let in_word = x >= 20 && x + 20 < width && (x / 7) % 6 != 5;
            Luma([if in_line && in_word { 40 } else { 240 }])
        })
    }

    #[test]
    fn scale_targets_line_height() {
        // 12 px lines → 32/12 ≈ 2.67.
        let s = choose_scale(800, 600, Some(12.0), 10_000, MAX_PIXELS);
        assert!((s - 32.0 / 12.0).abs() < 1e-9, "{s}");
        // Tiny text: at most 4x.
        assert_eq!(
            choose_scale(400, 300, Some(4.0), 10_000, MAX_PIXELS),
            MAX_UPSCALE
        );
        // Large enough text: unchanged; small upscales are skipped.
        assert_eq!(choose_scale(800, 600, Some(40.0), 10_000, MAX_PIXELS), 1.0);
        assert_eq!(choose_scale(800, 600, Some(29.0), 10_000, MAX_PIXELS), 1.0);
    }

    #[test]
    fn scale_without_estimate_doubles_small_images() {
        assert_eq!(choose_scale(900, 700, None, 10_000, MAX_PIXELS), 2.0);
        assert_eq!(choose_scale(2000, 1500, None, 10_000, MAX_PIXELS), 1.0);
    }

    #[test]
    fn scale_respects_engine_limit_and_memory_cap() {
        // Windows OCR limit: 1920 px wide screenshot, limit 2600 → ≤ 1.354.
        let s = choose_scale(1920, 1080, Some(10.0), 2600, MAX_PIXELS);
        assert!((s - 2600.0 / 1920.0).abs() < 1e-9, "{s}");
        let (w, h) = scaled_size(1920, 1080, s);
        assert!(w <= 2600 && h <= 2600);
        // Above the limit: shrink even without upscaling wish.
        let s = choose_scale(5200, 1000, Some(40.0), 2600, MAX_PIXELS);
        assert!((s - 0.5).abs() < 1e-9, "{s}");
        // Memory cap: 3000 x 3000 at 3x would be 81 MP.
        let s = choose_scale(3000, 3000, Some(10.0), 100_000, MAX_PIXELS);
        let (w, h) = scaled_size(3000, 3000, s);
        assert!(u64::from(w) * u64::from(h) <= MAX_PIXELS, "{w}x{h}");
        assert!(s > 1.5);
        // Limit only allows a tiny upscale: skipped.
        assert_eq!(choose_scale(2400, 1000, Some(10.0), 2600, MAX_PIXELS), 1.0);
        // Degenerate input.
        assert!(choose_scale(0, 0, None, 0, 0) > 0.0);
    }

    #[test]
    fn contrast_is_stretched() {
        // Low contrast: background 200, text 120.
        let mut img = GrayImage::from_fn(100, 100, |x, _| Luma([if x < 10 { 120 } else { 200 }]));
        normalize(&mut img);
        assert_eq!(img.get_pixel(0, 0).0[0], 0);
        assert_eq!(img.get_pixel(50, 0).0[0], 255);
    }

    #[test]
    fn sparse_text_is_stretched() {
        // 0.2 % text pixels (90) on a grey background (220).
        let mut img = GrayImage::from_fn(100, 100, |x, y| {
            Luma([if x < 4 && y < 5 { 90 } else { 220 }])
        });
        normalize(&mut img);
        assert_eq!(img.get_pixel(0, 0).0[0], 0);
        assert_eq!(img.get_pixel(50, 50).0[0], 255);
    }

    #[test]
    fn dark_theme_is_inverted() {
        // Dark background 30, light text 220 → dark text on white.
        let mut img = GrayImage::from_fn(100, 100, |x, _| Luma([if x < 10 { 220 } else { 30 }]));
        normalize(&mut img);
        assert_eq!(img.get_pixel(0, 0).0[0], 0);
        assert_eq!(img.get_pixel(50, 0).0[0], 255);
    }

    #[test]
    fn flat_images_stay_unchanged() {
        let mut img = GrayImage::from_pixel(10, 10, Luma([200]));
        normalize(&mut img);
        assert!(img.iter().all(|&p| p == 200));
    }

    #[test]
    fn line_height_from_profile() {
        let img = page(600, 400, 12, 20);
        assert_eq!(estimate_line_height(&img), Some(12.0));
        // A vertical border (gutter separator) over the full height does
        // not merge the lines.
        let mut bordered = img.clone();
        for y in 0..400 {
            bordered.put_pixel(5, y, Luma([0]));
            bordered.put_pixel(6, y, Luma([0]));
        }
        assert_eq!(estimate_line_height(&bordered), Some(12.0));
        // No ink, no estimate.
        assert_eq!(
            estimate_line_height(&GrayImage::from_pixel(50, 50, Luma([255]))),
            None
        );
    }

    #[test]
    fn content_bounds_crop_with_margin() {
        let mut img = GrayImage::from_pixel(200, 100, Luma([255]));
        for x in 50..60 {
            img.put_pixel(x, 40, Luma([0]));
        }
        assert_eq!(content_bounds(&img, 16), (34, 24, 42, 33));
        assert_eq!(content_bounds(&img, 100), (0, 0, 160, 100));
        let blank = GrayImage::from_pixel(20, 10, Luma([255]));
        assert_eq!(content_bounds(&blank, 4), (0, 0, 20, 10));
    }

    #[test]
    fn transparency_becomes_white() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([0, 0, 0, 0])
            } else {
                image::Rgba([0, 0, 0, 255])
            }
        }));
        let g = to_gray(&img);
        assert_eq!(g.get_pixel(0, 0).0[0], 255);
        assert_eq!(g.get_pixel(1, 0).0[0], 0);
    }

    fn png_of(img: &GrayImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn prepare_upscales_small_screenshot_and_restores_boxes() {
        // 12 px lines on a 640 x 300 image with an empty margin.
        let img = page(640, 300, 12, 20);
        let p = prepare(&png_of(&img), 10_000).unwrap();
        let out = image::load_from_memory(&p.png).unwrap();
        assert!(p.scale > 2.5 && p.scale < 2.8, "{}", p.scale);
        // Cropped: left margin was 20 px, minus 16 px kept.
        assert_eq!((p.offset_x, p.offset_y), (4, 0));
        assert_eq!(
            out.width(),
            ((f64::from(640 - 8) * 32.0 / 12.0).floor()) as u32
        );
        // A box in prepared coordinates maps back to the original.
        let w = WordBox {
            text: "x".into(),
            x: 16.0 * p.scale,
            y: 10.0 * p.scale,
            width: 7.0 * p.scale,
            height: 12.0 * p.scale,
        };
        let r = &p.restore(vec![w])[0];
        assert!((r.x - 20.0).abs() < 1e-6 && (r.y - 10.0).abs() < 1e-6);
        assert!((r.width - 7.0).abs() < 1e-6 && (r.height - 12.0).abs() < 1e-6);
    }

    #[test]
    fn prepare_respects_limit_and_rejects_garbage() {
        let img = page(640, 300, 12, 20);
        let p = prepare(&png_of(&img), 800).unwrap();
        let out = image::load_from_memory(&p.png).unwrap();
        assert!(out.width() <= 800 && out.height() <= 800);
        assert!(prepare(b"\x89PNG\r\n\x1a\nkaputt", 800).is_err());
    }

    #[test]
    fn prepare_reads_jpeg() {
        let img = DynamicImage::ImageLuma8(page(320, 200, 30, 45)).to_rgb8();
        let mut jpg = Vec::new();
        img.write_to(&mut Cursor::new(&mut jpg), image::ImageFormat::Jpeg)
            .unwrap();
        let p = prepare(&jpg, 10_000).unwrap();
        // 30 px lines: no upscaling needed.
        assert_eq!(p.scale, 1.0);
    }
}
