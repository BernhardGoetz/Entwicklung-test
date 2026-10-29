//! Checks a PDF before `lopdf`/`pdf-extract` work on it.
//!
//! Neither crate limits its work: `lopdf` inflates streams with
//! `read_to_end` (a 1 MB file can hold gigabytes of zeros), `pdf-extract`
//! follows `Do` operators recursively without depth limit or cycle check (a
//! form XObject that draws itself overflows the stack). The import runs in
//! a separate, memory- and time-limited process anyway (see
//! [`super::worker`]); these checks reject such files early with a clear
//! message instead of letting the worker hit its limits:
//!
//! 1. [`check_raw`] (before `Document::load_mem`): the object and xref
//!    streams `load_mem` inflates itself, decoded with a size limit.
//! 2. [`check_document`] (after loading): every stream decoded with a size
//!    limit (per stream and in total), then the XObject graph as
//!    `pdf-extract` walks it: cycles, depth and the number of form
//!    invocations (`Do` repeated in nested forms multiplies).
//!
//! The decoders mirror the filters `lopdf` implements (Flate incl. its raw
//! deflate fallback, LZW, ASCII85); other filters are not decoded by
//! `lopdf` either.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::Write;

use pdf_extract::{Dictionary, Document, Object, ObjectId, Stream};

use super::DocsError;

/// Size limits for decoded streams.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Largest decoded size of a single stream.
    pub stream_bytes: u64,
    /// Largest decoded size of all streams together.
    pub total_bytes: u64,
}

/// The limits of the import: generous for scans with Flate-compressed
/// images (A4, 600 dpi, RGB: about 100 MB).
pub const LIMITS: Limits = Limits {
    stream_bytes: 128 * 1024 * 1024,
    total_bytes: 1024 * 1024 * 1024,
};
/// Deepest nesting of XObjects (forms in forms).
pub const MAX_XOBJECT_DEPTH: usize = 12;
/// Most XObject invocations per page (`Do` in nested forms multiplies).
pub const MAX_XOBJECT_CALLS: u64 = 100_000;
/// How far before `stream` the raw check looks for the dictionary.
const RAW_DICT_WINDOW: usize = 64 * 1024;
/// Parent levels followed for inherited page resources.
const MAX_PARENT_LEVELS: usize = 64;

fn unsafe_pdf(detail: &str) -> DocsError {
    DocsError::Invalid(format!(
        "Das PDF konnte nicht sicher gelesen werden: {detail} Vermutlich ist die Datei beschädigt \
         oder absichtlich manipuliert. Tipp: die Datei neu als PDF speichern (z. B. über „Drucken \
         als PDF“) oder den Text direkt einfügen."
    ))
}

fn mb(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

fn too_large_stream(limits: &Limits) -> DocsError {
    unsafe_pdf(&format!(
        "Es enthält extrem stark komprimierte Daten (ein Datenstrom wäre entpackt größer als {} \
         MB).",
        mb(limits.stream_bytes)
    ))
}

fn too_large_total(limits: &Limits) -> DocsError {
    unsafe_pdf(&format!(
        "Es enthält zu viele komprimierte Daten (entpackt insgesamt mehr als {} MB).",
        mb(limits.total_bytes)
    ))
}

fn nesting() -> DocsError {
    unsafe_pdf(
        "Es enthält Grafikobjekte, die sich selbst enthalten oder zu tief bzw. zu oft \
         verschachtelt sind.",
    )
}

// ---------------------------------------------------------------------------
// Bounded decoding

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    Flate,
    Lzw {
        early_change: bool,
    },
    Ascii85,
    /// Not implemented by lopdf: decoding stops there.
    Other,
}

fn filter_from_name(name: &[u8], early_change: bool) -> Filter {
    match name {
        b"FlateDecode" => Filter::Flate,
        b"LZWDecode" => Filter::Lzw { early_change },
        b"ASCII85Decode" => Filter::Ascii85,
        _ => Filter::Other,
    }
}

/// A sink that counts bytes (and keeps them if `keep`) and fails once more
/// than `limit` bytes arrive.
struct Bounded {
    limit: u64,
    count: u64,
    keep: bool,
    buf: Vec<u8>,
    exceeded: bool,
}

impl Bounded {
    fn new(limit: u64, keep: bool) -> Self {
        Bounded {
            limit,
            count: 0,
            keep,
            buf: Vec::new(),
            exceeded: false,
        }
    }
}

impl Write for Bounded {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.count += data.len() as u64;
        if self.count > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::other("limit"));
        }
        if self.keep {
            self.buf.extend_from_slice(data);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Exceeded;

fn flate(input: &[u8], sink: &mut Bounded) -> Result<(), Exceeded> {
    if input.is_empty() {
        return Ok(());
    }
    let _ = std::io::copy(&mut flate2::read::ZlibDecoder::new(input), sink);
    if sink.exceeded {
        return Err(Exceeded);
    }
    // lopdf retries as raw deflate if zlib produced nothing.
    if sink.count == 0 && input.len() > 2 {
        let _ = std::io::copy(&mut flate2::read::DeflateDecoder::new(&input[2..]), sink);
        if sink.exceeded {
            return Err(Exceeded);
        }
    }
    Ok(())
}

fn lzw(input: &[u8], early_change: bool, sink: &mut Bounded) -> Result<(), Exceeded> {
    use weezl::{decode::Decoder, BitOrder};
    let mut decoder = if early_change {
        Decoder::with_tiff_size_switch(BitOrder::Msb, 8)
    } else {
        Decoder::new(BitOrder::Msb, 8)
    };
    let _ = decoder.into_stream(&mut *sink).decode_all(input);
    if sink.exceeded {
        Err(Exceeded)
    } else {
        Ok(())
    }
}

/// ASCII85 as lopdf decodes it (stops at the first invalid character).
fn ascii85(input: &[u8], sink: &mut Bounded) -> Result<(), Exceeded> {
    let input = input.strip_suffix(b"~>").unwrap_or(input);
    let mut group = [0u8; 5];
    let mut n = 0;
    let mut out = Vec::with_capacity(4096);
    let emit = |bytes: &[u8], out: &mut Vec<u8>, sink: &mut Bounded| {
        out.extend_from_slice(bytes);
        if out.len() >= 4096 {
            let r = sink.write_all(out);
            out.clear();
            r.map_err(|_| Exceeded)
        } else {
            Ok(())
        }
    };
    for &c in input {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'z' && n == 0 {
            emit(&[0; 4], &mut out, sink)?;
            continue;
        }
        if !(b'!'..=b'u').contains(&c) {
            break;
        }
        group[n] = c - b'!';
        n += 1;
        if n == 5 {
            let v = group
                .iter()
                .fold(0u32, |acc, &d| acc.wrapping_mul(85).wrapping_add(d.into()));
            emit(&v.to_be_bytes(), &mut out, sink)?;
            n = 0;
        }
    }
    if n > 1 {
        for d in group.iter_mut().skip(n) {
            *d = 84;
        }
        let v = group
            .iter()
            .fold(0u32, |acc, &d| acc.wrapping_mul(85).wrapping_add(d.into()));
        emit(&v.to_be_bytes()[..n - 1], &mut out, sink)?;
    }
    sink.write_all(&out).map_err(|_| Exceeded)
}

/// Decoded size of `data` after `filters` (in decoding order), at most
/// `limit` (larger: `Err`). Intermediate results are kept in memory, the
/// last stage is only counted.
fn decoded_size(data: &[u8], filters: &[Filter], limit: u64) -> Result<u64, Exceeded> {
    let mut current: Cow<[u8]> = Cow::Borrowed(data);
    for (i, &filter) in filters.iter().enumerate() {
        let last = i + 1 == filters.len();
        let mut sink = Bounded::new(limit, !last);
        match filter {
            Filter::Flate => flate(&current, &mut sink)?,
            Filter::Lzw { early_change } => lzw(&current, early_change, &mut sink)?,
            Filter::Ascii85 => ascii85(&current, &mut sink)?,
            // lopdf gives up here (no decompressed content at all).
            Filter::Other => return Ok(current.len() as u64),
        }
        if last {
            return Ok(sink.count);
        }
        current = Cow::Owned(sink.buf);
    }
    Ok(current.len() as u64)
}

/// Adds a stream's decoded size to `total`, checking both limits.
fn account(
    limits: &Limits,
    total: &mut u64,
    data: &[u8],
    filters: &[Filter],
) -> Result<(), DocsError> {
    if filters.is_empty() {
        return Ok(());
    }
    let size =
        decoded_size(data, filters, limits.stream_bytes).map_err(|_| too_large_stream(limits))?;
    *total += size;
    if *total > limits.total_bytes {
        return Err(too_large_total(limits));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Raw check (before load_mem)

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

fn is_delimiter(c: u8) -> bool {
    c.is_ascii_whitespace() || b"()<>[]{}/%".contains(&c)
}

/// Names after `/Filter` in a raw dictionary (a single name or an array).
fn raw_filters(dict: &[u8]) -> Vec<Filter> {
    let early_change = !raw_contains_token(dict, b"/EarlyChange 0");
    let Some(i) = find(dict, b"/Filter") else {
        return Vec::new();
    };
    let mut rest = &dict[i + 7..];
    while let [c, tail @ ..] = rest {
        if c.is_ascii_whitespace() {
            rest = tail;
        } else {
            break;
        }
    }
    let names: &[u8] = match rest.first() {
        Some(b'[') => {
            let end = rest.iter().position(|&c| c == b']').unwrap_or(rest.len());
            &rest[1..end]
        }
        Some(b'/') => {
            let end = rest[1..]
                .iter()
                .position(|&c| is_delimiter(c))
                .map_or(rest.len(), |e| e + 1);
            &rest[..end]
        }
        _ => return Vec::new(),
    };
    names
        .split(|&c| c == b'/' || c.is_ascii_whitespace())
        .filter(|n| !n.is_empty())
        .map(|n| filter_from_name(n, early_change))
        .collect()
}

fn raw_contains_token(dict: &[u8], token: &[u8]) -> bool {
    let mut from = 0;
    while let Some(i) = find(&dict[from..], token) {
        let end = from + i + token.len();
        if dict.get(end).is_none_or(|&c| is_delimiter(c)) {
            return true;
        }
        from = end;
    }
    false
}

/// A direct `/Length n` (not `n 0 R`).
fn raw_length(dict: &[u8]) -> Option<usize> {
    let i = find(dict, b"/Length")?;
    let rest = &dict[i + 7..];
    let rest = &rest[rest.iter().position(|c| !c.is_ascii_whitespace())?..];
    let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    let number: usize = std::str::from_utf8(&rest[..digits]).ok()?.parse().ok()?;
    let after = &rest[digits..];
    let after = &after[after
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(after.len())..];
    // "12 0 R" is a reference: the length is elsewhere.
    let is_reference = after.first().is_some_and(u8::is_ascii_digit);
    (!is_reference).then_some(number)
}

/// Checks the object and xref streams in the raw file, which
/// `Document::load_mem` inflates before anything else can be checked.
pub fn check_raw(bytes: &[u8], limits: &Limits) -> Result<(), DocsError> {
    let mut total = 0u64;
    let mut pos = 0;
    while let Some(i) = find(&bytes[pos..], b"stream") {
        let keyword = pos + i;
        pos = keyword + 6;
        if bytes[..keyword].ends_with(b"end") {
            continue;
        }
        let data_start = match &bytes[pos..] {
            [b'\r', b'\n', ..] => pos + 2,
            [b'\n', ..] | [b'\r', ..] => pos + 1,
            _ => continue,
        };
        let window = &bytes[keyword.saturating_sub(RAW_DICT_WINDOW)..keyword];
        let Some(obj) = rfind(window, b"obj") else {
            continue;
        };
        let dict = &window[obj + 3..];
        if !raw_contains_token(dict, b"/ObjStm") && !raw_contains_token(dict, b"/XRef") {
            continue;
        }
        let end = raw_length(dict)
            .and_then(|len| data_start.checked_add(len))
            .filter(|&end| end <= bytes.len())
            .or_else(|| find(&bytes[data_start..], b"endstream").map(|j| data_start + j))
            .unwrap_or(bytes.len());
        account(
            limits,
            &mut total,
            &bytes[data_start..end],
            &raw_filters(dict),
        )?;
        pos = end.max(pos);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Structured check (after loading)

fn stream_filters(stream: &Stream) -> Vec<Filter> {
    let early_change = stream
        .dict
        .get(b"DecodeParms")
        .ok()
        .and_then(|p| p.as_dict().ok())
        .and_then(|p| p.get(b"EarlyChange").ok())
        .and_then(|v| v.as_i64().ok())
        .is_none_or(|v| v != 0);
    stream
        .filters()
        .map(|names| {
            names
                .into_iter()
                .map(|n| filter_from_name(n, early_change))
                .collect()
        })
        .unwrap_or_default()
}

fn deref<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Object> {
    doc.dereference(object).ok().map(|(_, o)| o)
}

fn dict_entry<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Dictionary> {
    deref(doc, dict.get(key).ok()?)?.as_dict().ok()
}

/// Page resources, inherited from the page tree like `pdf-extract` does.
fn page_resources(doc: &Document, page: ObjectId) -> Option<&Dictionary> {
    let mut dict = doc.get_object(page).ok()?.as_dict().ok()?;
    for _ in 0..MAX_PARENT_LEVELS {
        if let Some(r) = dict_entry(doc, dict, b"Resources") {
            return Some(r);
        }
        let parent = dict.get(b"Parent").ok()?;
        dict = deref(doc, parent)?.as_dict().ok()?;
    }
    None
}

/// Content of a stream as `pdf-extract` uses it (decoded if filtered).
fn stream_content(stream: &Stream) -> Vec<u8> {
    if stream.dict.has(b"Filter") {
        stream.decompressed_content().unwrap_or_default()
    } else {
        stream.content.clone()
    }
}

/// XObject names a content stream invokes with `Do`, and how often.
pub fn invoked_xobjects(content: &[u8]) -> HashMap<Vec<u8>, u64> {
    let mut calls: HashMap<Vec<u8>, u64> = HashMap::new();
    let mut from = 0;
    while let Some(i) = find(&content[from..], b"Do") {
        let at = from + i;
        from = at + 2;
        let before_ok = at > 0 && is_delimiter(content[at - 1]);
        let after_ok = content.get(at + 2).is_none_or(|&c| is_delimiter(c));
        if !before_ok || !after_ok {
            continue;
        }
        // The operand: the name token before "Do".
        let head = content[..at].trim_ascii_end();
        let start = head
            .iter()
            .rposition(|&c| c == b'/' || is_delimiter(c))
            .unwrap_or(0);
        if head.get(start) == Some(&b'/') && start + 1 < head.len() {
            *calls.entry(head[start + 1..].to_vec()).or_default() += 1;
        }
    }
    calls
}

struct Walk<'a> {
    doc: &'a Document,
    path: Vec<ObjectId>,
    /// Invocation cost of forms with their own resources (independent of
    /// the caller).
    memo: HashMap<ObjectId, u64>,
}

impl<'a> Walk<'a> {
    /// Number of `Do` invocations `content` causes (recursively) with
    /// `resources`; `Err` on cycles and too deep nesting.
    fn calls(
        &mut self,
        content: &[u8],
        resources: Option<&'a Dictionary>,
    ) -> Result<u64, DocsError> {
        let invoked = invoked_xobjects(content);
        if invoked.is_empty() {
            return Ok(0);
        }
        let Some(xobjects) = resources.and_then(|r| dict_entry(self.doc, r, b"XObject")) else {
            return Ok(0);
        };
        let mut total = 0u64;
        for (name, times) in invoked {
            let Ok(Object::Reference(id)) = xobjects.get(&name) else {
                continue;
            };
            let id = *id;
            let Ok(Object::Stream(stream)) = self.doc.get_object(id) else {
                continue;
            };
            if self.path.contains(&id) || self.path.len() >= MAX_XOBJECT_DEPTH {
                return Err(nesting());
            }
            let own = dict_entry(self.doc, &stream.dict, b"Resources");
            let inner = match (own, self.memo.get(&id)) {
                (Some(_), Some(&cached)) => cached,
                _ => {
                    self.path.push(id);
                    let inner = self.calls(&stream_content(stream), own.or(resources));
                    self.path.pop();
                    let inner = inner?;
                    if own.is_some() {
                        self.memo.insert(id, inner);
                    }
                    inner
                }
            };
            total = total.saturating_add(times.saturating_mul(inner.saturating_add(1)));
            if total > MAX_XOBJECT_CALLS {
                return Err(nesting());
            }
        }
        Ok(total)
    }
}

/// Decoded sizes of all streams and the XObject graph of every page.
pub fn check_document(doc: &Document, limits: &Limits) -> Result<(), DocsError> {
    let mut total = 0u64;
    for object in doc.objects.values() {
        if let Object::Stream(stream) = object {
            account(limits, &mut total, &stream.content, &stream_filters(stream))?;
        }
    }
    let mut seen_pages = HashSet::new();
    for page in doc.get_pages().into_values() {
        if !seen_pages.insert(page) {
            continue;
        }
        let content = doc.get_page_content(page).unwrap_or_default();
        let mut walk = Walk {
            doc,
            path: Vec::new(),
            memo: HashMap::new(),
        };
        walk.calls(&content, page_resources(doc, page))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PDF from raw object bodies (1-based object numbers, object 1 is
    /// the catalog), with a correct xref table.
    pub(crate) fn raw_pdf(objects: &[Vec<u8>]) -> Vec<u8> {
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

    pub(crate) fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
        let mut s = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        s.extend_from_slice(data);
        s.extend_from_slice(b"\nendstream");
        s
    }

    pub(crate) fn deflate(data_len_mib: usize) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        let chunk = vec![b' '; 1024 * 1024];
        for _ in 0..data_len_mib {
            e.write_all(&chunk).unwrap();
        }
        e.finish().unwrap()
    }

    /// Like the audit PoC `bomb.pdf`: one page whose content stream
    /// inflates to `mib` MiB of spaces.
    pub(crate) fn bomb_pdf(mib: usize) -> Vec<u8> {
        raw_pdf(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << >> /Contents 4 0 R >>"
                .to_vec(),
            stream("/Filter /FlateDecode", &deflate(mib)),
        ])
    }

    /// Like the audit PoC `recursive.pdf`: a form XObject that draws itself.
    pub(crate) fn recursive_pdf() -> Vec<u8> {
        raw_pdf(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /XObject << /X 5 0 R >> >> /Contents 4 0 R >>"
                .to_vec(),
            stream("", b"/X Do"),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /X 5 0 R >> >>",
                b"/X Do",
            ),
        ])
    }

    fn load(bytes: &[u8]) -> Document {
        Document::load_mem(bytes).unwrap()
    }

    /// Small limits keep the tests fast (debug builds inflate slowly).
    const SMALL: Limits = Limits {
        stream_bytes: 4 * 1024 * 1024,
        total_bytes: 16 * 1024 * 1024,
    };

    fn check(bytes: &[u8]) -> Result<(), DocsError> {
        check_document(&load(bytes), &SMALL)
    }

    #[test]
    fn bounded_decoders() {
        let small = deflate(1);
        assert_eq!(
            decoded_size(&small, &[Filter::Flate], 1 << 30).ok(),
            Some(1024 * 1024)
        );
        assert!(decoded_size(&small, &[Filter::Flate], 1000).is_err());
        // Doubly compressed: the inner layer is counted.
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        e.write_all(&small).unwrap();
        let double = e.finish().unwrap();
        assert_eq!(
            decoded_size(&double, &[Filter::Flate, Filter::Flate], 1 << 30).ok(),
            Some(1024 * 1024)
        );
        // Unknown filters stop decoding (lopdf does not decode them).
        assert_eq!(
            decoded_size(&small, &[Filter::Other, Filter::Flate], 10).ok(),
            Some(small.len() as u64)
        );
        // ASCII85: "z" = four zero bytes; a padded final group.
        assert_eq!(decoded_size(b"zz~>", &[Filter::Ascii85], 100).ok(), Some(8));
        assert!(decoded_size(b"zzzz", &[Filter::Ascii85], 15).is_err());
        let mut sink = Bounded::new(100, true);
        assert!(ascii85(b"87cURD]i,\"Ebo80~>", &mut sink).is_ok());
        assert_eq!(sink.buf, b"Hello World!");
        // LZW: compressed with weezl (as TIFF/PDF with early change).
        let data = vec![b'a'; 100_000];
        let lzw_data = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
            .encode(&data)
            .unwrap();
        assert_eq!(
            decoded_size(&lzw_data, &[Filter::Lzw { early_change: true }], 1 << 20).ok(),
            Some(100_000)
        );
        assert!(decoded_size(&lzw_data, &[Filter::Lzw { early_change: true }], 50_000).is_err());
    }

    #[test]
    fn raw_dictionary_parsing() {
        assert_eq!(
            raw_filters(b"<< /Filter /FlateDecode /Length 3 >>"),
            vec![Filter::Flate]
        );
        assert_eq!(
            raw_filters(b"<</Filter[/ASCII85Decode/FlateDecode]>>"),
            vec![Filter::Ascii85, Filter::Flate]
        );
        assert_eq!(
            raw_filters(b"<< /Filter /LZWDecode /DecodeParms << /EarlyChange 0 >> >>"),
            vec![Filter::Lzw {
                early_change: false
            }]
        );
        assert!(raw_filters(b"<< /Length 3 >>").is_empty());
        assert_eq!(
            raw_length(b"<< /Length 12 /Filter /FlateDecode >>"),
            Some(12)
        );
        assert_eq!(raw_length(b"<< /Length 12 0 R >>"), None);
        assert!(raw_contains_token(b"<< /Type /ObjStm /N 3 >>", b"/ObjStm"));
        assert!(!raw_contains_token(b"<< /Type /ObjStmX >>", b"/ObjStm"));
    }

    #[test]
    fn raw_check_limits_object_streams() {
        let ok = super::super::pdf::tests::pdf(&["Text"]);
        assert!(check_raw(&ok, &SMALL).is_ok());
        // An object stream that would inflate to 8 MiB (load_mem would do
        // that before any other check).
        let bomb = raw_pdf(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            stream(
                "/Type /ObjStm /N 1 /First 4 /Filter /FlateDecode",
                &deflate(8),
            ),
        ]);
        let e = check_raw(&bomb, &SMALL).unwrap_err().to_string();
        assert!(
            e.starts_with("Das PDF konnte nicht sicher gelesen werden"),
            "{e}"
        );
        assert!(e.contains("größer als 4 MB"), "{e}");
        // Content streams are left to the structured check.
        assert!(check_raw(&bomb_pdf(8), &SMALL).is_ok());
    }

    #[test]
    fn decompression_bomb_is_rejected() {
        let e = check(&bomb_pdf(8)).unwrap_err().to_string();
        assert!(e.contains("extrem stark komprimierte Daten"), "{e}");
        // Below the limit it passes.
        assert!(check(&bomb_pdf(3)).is_ok());
    }

    #[test]
    fn total_limit() {
        // Nine streams of 3 MiB each: every one is below the stream limit,
        // together they exceed the total.
        let mut objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec(),
        ];
        let data = deflate(3);
        for _ in 0..9 {
            objects.push(stream("/Filter /FlateDecode", &data));
        }
        let e = check(&raw_pdf(&objects)).unwrap_err().to_string();
        assert!(e.contains("insgesamt mehr als 16 MB"), "{e}");
    }

    #[test]
    fn self_drawing_form_is_rejected() {
        let e = check(&recursive_pdf()).unwrap_err().to_string();
        assert!(e.contains("Grafikobjekte"), "{e}");
    }

    fn forms_pdf(levels: usize, calls_per_level: usize, inherit: bool) -> Vec<u8> {
        // Page draws form 1; form k draws form k+1 `calls_per_level` times.
        let first_form = 5;
        let mut objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /XObject << /F0 {first_form} 0 R >> >> /Contents 4 0 R >>"
            )
            .into_bytes(),
            stream("", b"q /F0 Do Q"),
        ];
        for k in 0..levels {
            let content = if k + 1 < levels {
                format!("/F{} Do\n", k + 1).repeat(calls_per_level)
            } else {
                "0 0 m 1 1 l S".to_string()
            };
            let resources = if k + 1 < levels && !inherit {
                format!(
                    "/Resources << /XObject << /F{} {} 0 R >> >>",
                    k + 1,
                    first_form + k + 1
                )
            } else {
                String::new()
            };
            objects.push(stream(
                &format!("/Type /XObject /Subtype /Form /BBox [0 0 1 1] {resources}"),
                content.as_bytes(),
            ));
        }
        raw_pdf(&objects)
    }

    #[test]
    fn nesting_depth_and_fan_out() {
        assert!(check(&forms_pdf(4, 2, false)).is_ok());
        let e = check(&forms_pdf(MAX_XOBJECT_DEPTH + 2, 1, false))
            .unwrap_err()
            .to_string();
        assert!(e.contains("Grafikobjekte"), "{e}");
        // 10 levels with 10 calls each: 10^9 invocations in pdf-extract.
        let e = check(&forms_pdf(10, 10, false)).unwrap_err().to_string();
        assert!(e.contains("Grafikobjekte"), "{e}");
    }

    #[test]
    fn inherited_resources_without_recursion_are_fine() {
        // A form without own resources inherits the page's; it is only a
        // cycle if its content actually draws itself.
        let pdf = raw_pdf(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /XObject << /A 5 0 R /B 6 0 R >> >> >>"
                .to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R >>".to_vec(),
            stream("", b"/A Do /B Do"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1]", b"/B Do"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1]", b"0 0 m S"),
        ]);
        assert!(check(&pdf).is_ok());
        let pdf = raw_pdf(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /XObject << /A 5 0 R >> >> >>"
                .to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R >>".to_vec(),
            stream("", b"/A Do"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1]", b"/A Do"),
        ]);
        assert!(check(&pdf).is_err());
    }

    #[test]
    fn do_operands_are_found() {
        let calls = invoked_xobjects(b"q /Im1 Do Q\n/Fm2 Do/Fm2 Do (Do) Tj /Done Tf DoX");
        assert_eq!(calls.get(b"Im1".as_slice()), Some(&1));
        assert_eq!(calls.get(b"Fm2".as_slice()), Some(&2));
        assert_eq!(calls.len(), 2, "{calls:?}");
    }
}
