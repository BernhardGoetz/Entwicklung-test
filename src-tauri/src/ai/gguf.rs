//! Plausibility check of a GGUF file's header (metadata and tensor list)
//! before an imported file is handed to llama-server.
//!
//! llama.cpp parses GGUF in native code; its parser had memory bugs in the
//! past. llama-server has no option to limit the metadata, so imported
//! files are walked here first (pure Rust, nothing is allocated from the
//! file's size fields) and refused if anything is outside the ranges real
//! models use: version, number of tensors and key/value pairs, string and
//! array lengths, nested arrays, tensor names and dimensions, total size of
//! the metadata.

use std::io::{self, Read};

/// GGUF versions llama.cpp b11700 reads (v1 is long obsolete).
pub const VERSIONS: [u32; 2] = [2, 3];
/// Large mixture-of-experts models have a few thousand tensors.
pub const MAX_TENSORS: u64 = 65_536;
/// Real models have a few dozen key/value pairs.
pub const MAX_KV: u64 = 4_096;
/// Longest key (llama.cpp keys are short dotted names).
pub const MAX_KEY_BYTES: u64 = 1_024;
/// Longest string value (chat templates are a few KB; some files embed a
/// whole `tokenizer.json`).
pub const MAX_STRING_BYTES: u64 = 64 * 1024 * 1024;
/// Most array elements (vocabularies and merges: a few 100 000).
pub const MAX_ARRAY_LEN: u64 = 16 * 1024 * 1024;
/// Total size of keys and values.
pub const MAX_METADATA_BYTES: u64 = 256 * 1024 * 1024;
/// `GGML_MAX_NAME` (64, including the terminating zero).
pub const MAX_TENSOR_NAME: u64 = 63;
/// `GGML_MAX_DIMS`.
pub const MAX_DIMS: u32 = 4;

const STRING: u32 = 8;
const ARRAY: u32 = 9;

/// Size in bytes of a scalar GGUF value type.
fn scalar_size(ty: u32) -> Option<u64> {
    Some(match ty {
        0 | 1 | 7 => 1, // u8, i8, bool
        2 | 3 => 2,     // u16, i16
        4..=6 => 4,     // u32, i32, f32
        10..=12 => 8,   // u64, i64, f64
        _ => return None,
    })
}

struct Walker<R> {
    input: R,
    metadata: u64,
}

impl<R: Read> Walker<R> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let mut buf = [0u8; N];
        self.input
            .read_exact(&mut buf)
            .map_err(|_| "Datei endet mitten in den Metadaten".to_string())?;
        Ok(buf)
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.bytes()?))
    }

    fn skip(&mut self, n: u64) -> Result<(), String> {
        let copied =
            io::copy(&mut (&mut self.input).take(n), &mut io::sink()).map_err(|e| e.to_string())?;
        if copied == n {
            Ok(())
        } else {
            Err("Datei endet mitten in den Metadaten".into())
        }
    }

    fn count(&mut self, n: u64) -> Result<(), String> {
        self.metadata = self.metadata.saturating_add(n);
        if self.metadata > MAX_METADATA_BYTES {
            return Err(format!(
                "Metadaten größer als {} MB",
                MAX_METADATA_BYTES / (1024 * 1024)
            ));
        }
        Ok(())
    }

    /// A string of at most `max` bytes (skipped).
    fn string(&mut self, max: u64, what: &str) -> Result<(), String> {
        let len = self.u64()?;
        if len > max {
            return Err(format!("{what} mit {len} Bytes"));
        }
        self.count(len + 8)?;
        self.skip(len)
    }

    fn value(&mut self, ty: u32, in_array: bool) -> Result<(), String> {
        match ty {
            STRING => self.string(MAX_STRING_BYTES, "Text"),
            ARRAY if in_array => Err("verschachteltes Array".into()),
            ARRAY => {
                let elem = self.u32()?;
                let len = self.u64()?;
                if len > MAX_ARRAY_LEN {
                    return Err(format!("Array mit {len} Einträgen"));
                }
                self.count(12)?;
                if let Some(size) = scalar_size(elem) {
                    let total = size * len;
                    self.count(total)?;
                    self.skip(total)
                } else if elem == STRING {
                    (0..len).try_for_each(|_| self.value(STRING, true))
                } else {
                    Err(format!("unbekannter Array-Typ {elem}"))
                }
            }
            _ => {
                let size = scalar_size(ty).ok_or_else(|| format!("unbekannter Typ {ty}"))?;
                self.count(size)?;
                self.skip(size)
            }
        }
    }
}

/// Walks the header of a GGUF file; `Err` names what is out of range (for
/// a German message).
pub fn check_header(input: impl Read) -> Result<(), String> {
    let mut w = Walker { input, metadata: 0 };
    if &w.bytes::<4>()? != b"GGUF" {
        return Err("keine GGUF-Datei".into());
    }
    let version = w.u32()?;
    if !VERSIONS.contains(&version) {
        return Err(format!("GGUF-Version {version}"));
    }
    let tensors = w.u64()?;
    if tensors > MAX_TENSORS {
        return Err(format!("{tensors} Tensoren"));
    }
    let kv = w.u64()?;
    if kv > MAX_KV {
        return Err(format!("{kv} Metadaten-Einträge"));
    }
    for _ in 0..kv {
        w.string(MAX_KEY_BYTES, "Schlüssel")?;
        let ty = w.u32()?;
        w.value(ty, false)?;
    }
    for _ in 0..tensors {
        w.string(MAX_TENSOR_NAME, "Tensorname")?;
        let dims = w.u32()?;
        if dims == 0 || dims > MAX_DIMS {
            return Err(format!("Tensor mit {dims} Dimensionen"));
        }
        let mut elements = 1u64;
        for _ in 0..dims {
            let d = w.u64()?;
            elements = elements
                .checked_mul(d)
                .ok_or_else(|| "Tensorgröße läuft über".to_string())?;
        }
        if elements > (1 << 40) {
            return Err("Tensor mit unrealistisch vielen Elementen".into());
        }
        let _type = w.u32()?;
        let _offset = w.u64()?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A GGUF header: `kv` (key, raw value with type) and tensor names.
    pub(crate) struct Builder {
        version: u32,
        kv: Vec<(String, u32, Vec<u8>)>,
        tensors: Vec<(String, Vec<u64>)>,
        n_tensors: Option<u64>,
    }

    fn string(s: &[u8]) -> Vec<u8> {
        let mut v = (s.len() as u64).to_le_bytes().to_vec();
        v.extend_from_slice(s);
        v
    }

    impl Builder {
        pub(crate) fn new() -> Self {
            Builder {
                version: 3,
                kv: Vec::new(),
                tensors: Vec::new(),
                n_tensors: None,
            }
        }

        pub(crate) fn string(mut self, key: &str, value: &str) -> Self {
            self.kv.push((key.into(), STRING, string(value.as_bytes())));
            self
        }

        fn u32(mut self, key: &str, value: u32) -> Self {
            self.kv.push((key.into(), 4, value.to_le_bytes().to_vec()));
            self
        }

        fn strings(mut self, key: &str, values: &[&str]) -> Self {
            let mut v = STRING.to_le_bytes().to_vec();
            v.extend_from_slice(&(values.len() as u64).to_le_bytes());
            for s in values {
                v.extend_from_slice(&string(s.as_bytes()));
            }
            self.kv.push((key.into(), ARRAY, v));
            self
        }

        fn raw(mut self, key: &str, ty: u32, value: Vec<u8>) -> Self {
            self.kv.push((key.into(), ty, value));
            self
        }

        pub(crate) fn tensor(mut self, name: &str, dims: &[u64]) -> Self {
            self.tensors.push((name.into(), dims.to_vec()));
            self
        }

        pub(crate) fn bytes(&self) -> Vec<u8> {
            let mut out = b"GGUF".to_vec();
            out.extend_from_slice(&self.version.to_le_bytes());
            let n = self.n_tensors.unwrap_or(self.tensors.len() as u64);
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&(self.kv.len() as u64).to_le_bytes());
            for (k, ty, v) in &self.kv {
                out.extend_from_slice(&string(k.as_bytes()));
                out.extend_from_slice(&ty.to_le_bytes());
                out.extend_from_slice(v);
            }
            for (name, dims) in &self.tensors {
                out.extend_from_slice(&string(name.as_bytes()));
                out.extend_from_slice(&(dims.len() as u32).to_le_bytes());
                for d in dims {
                    out.extend_from_slice(&d.to_le_bytes());
                }
                out.extend_from_slice(&0u32.to_le_bytes());
                out.extend_from_slice(&0u64.to_le_bytes());
            }
            out
        }
    }

    /// The smallest valid header (no tensors), distinguishable by `tag`.
    pub(crate) fn minimal(tag: &str) -> Vec<u8> {
        Builder::new().string("general.name", tag).bytes()
    }

    fn model() -> Builder {
        Builder::new()
            .string("general.architecture", "llama")
            .u32("llama.block_count", 2)
            .strings("tokenizer.ggml.tokens", &["<s>", "</s>", "a"])
            .string("tokenizer.chat_template", "{{ messages }}")
            .tensor("token_embd.weight", &[64, 261])
            .tensor("output_norm.weight", &[64])
    }

    #[test]
    fn plausible_header_passes() {
        assert_eq!(check_header(model().bytes().as_slice()), Ok(()));
        // Tensor data after the header is not read.
        let mut file = model().bytes();
        file.extend_from_slice(&[0u8; 4096]);
        assert_eq!(check_header(file.as_slice()), Ok(()));
    }

    #[test]
    fn implausible_headers_are_refused() {
        let err = |b: Builder| check_header(b.bytes().as_slice()).unwrap_err();
        let mut b = model();
        b.version = 1;
        assert!(err(b).contains("Version 1"));
        let mut b = model();
        b.n_tensors = Some(MAX_TENSORS + 1);
        assert!(err(b).contains("Tensoren"));
        assert!(err(model()
            .strings("x", &[&"a".repeat(10)])
            .tensor("t", &[1, 2, 3, 4, 5]))
        .contains("5 Dimensionen"));
        assert!(err(model().tensor(&"n".repeat(64), &[1])).contains("Tensorname"));
        // A huge string length (the parser would try to allocate it).
        let mut v = (MAX_STRING_BYTES + 1).to_le_bytes().to_vec();
        v.extend_from_slice(b"x");
        assert!(err(model().raw("big", STRING, v)).contains("Text mit"));
        // Nested arrays, unknown types, huge arrays.
        let mut nested = ARRAY.to_le_bytes().to_vec();
        nested.extend_from_slice(&1u64.to_le_bytes());
        nested.extend_from_slice(&4u32.to_le_bytes());
        nested.extend_from_slice(&0u64.to_le_bytes());
        assert!(err(model().raw("n", ARRAY, nested)).contains("Array-Typ 9"));
        assert!(err(model().raw("t", 77, vec![0; 8])).contains("unbekannter Typ"));
        let mut huge = 4u32.to_le_bytes().to_vec();
        huge.extend_from_slice(&(MAX_ARRAY_LEN + 1).to_le_bytes());
        assert!(err(model().raw("a", ARRAY, huge)).contains("Einträgen"));
        // The total metadata size is limited.
        let mut w = Walker {
            input: &b""[..],
            metadata: MAX_METADATA_BYTES - 1,
        };
        assert!(w.count(1).is_ok());
        assert!(w
            .count(1)
            .unwrap_err()
            .contains("Metadaten größer als 256 MB"));
        // Truncated file, wrong magic, overflowing tensor dimensions.
        let full = model().bytes();
        assert!(check_header(&full[..full.len() - 5])
            .unwrap_err()
            .contains("endet"));
        assert!(check_header(&b"GGML\x03\0\0\0"[..])
            .unwrap_err()
            .contains("keine GGUF"));
        assert!(err(model().tensor("t", &[u64::MAX, 2])).contains("läuft über"));
    }
}
