//! Pure output comparison: newline normalization, optional whitespace
//! trimming, a small LCS-based line diff and capped output buffers.

use serde::Serialize;

/// Maximum number of bytes kept per stream (stdout/stderr).
pub const OUTPUT_LIMIT: usize = 64 * 1024;

/// Upper bound for the LCS table (lines_expected * lines_actual). Above this
/// the diff falls back to a cheap line-by-line comparison.
const LCS_CELL_LIMIT: usize = 4_000_000;

/// Converts CRLF and lone CR line endings to LF.
pub fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Normalizes program output for comparison.
///
/// Always converts line endings to LF. With `trim` enabled, trailing
/// whitespace of every line and trailing empty lines are removed.
pub fn normalize_output(s: &str, trim: bool) -> String {
    let s = normalize_newlines(s);
    if !trim {
        return s;
    }
    let lines: Vec<&str> = s.split('\n').map(|l| l.trim_end()).collect();
    let mut out = lines.join("\n");
    let kept = out.trim_end_matches('\n').len();
    out.truncate(kept);
    out
}

pub fn outputs_match(expected: &str, actual: &str, trim: bool) -> bool {
    normalize_output(expected, trim) == normalize_output(actual, trim)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffKind {
    /// Line is present in both.
    Equal,
    /// Line is only in the expected output (missing in actual).
    Expected,
    /// Line is only in the actual output (unexpected).
    Actual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

fn split_lines(s: &str) -> Vec<&str> {
    if s.is_empty() {
        return Vec::new();
    }
    s.strip_suffix('\n').unwrap_or(s).split('\n').collect()
}

/// Line diff of already normalized texts.
pub fn line_diff(expected: &str, actual: &str) -> Vec<DiffLine> {
    let a = split_lines(expected);
    let b = split_lines(actual);
    let line = |kind, text: &str| DiffLine {
        kind,
        text: text.to_string(),
    };

    if a.len().saturating_mul(b.len()) > LCS_CELL_LIMIT {
        // Fallback: position-wise comparison.
        let mut out = Vec::new();
        for i in 0..a.len().max(b.len()) {
            match (a.get(i), b.get(i)) {
                (Some(x), Some(y)) if x == y => out.push(line(DiffKind::Equal, x)),
                (x, y) => {
                    if let Some(x) = x {
                        out.push(line(DiffKind::Expected, x));
                    }
                    if let Some(y) = y {
                        out.push(line(DiffKind::Actual, y));
                    }
                }
            }
        }
        return out;
    }

    // lcs[i][j] = LCS length of a[i..] and b[j..]
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(n.max(m));
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(line(DiffKind::Equal, a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(line(DiffKind::Expected, a[i]));
            i += 1;
        } else {
            out.push(line(DiffKind::Actual, b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| line(DiffKind::Expected, l)));
    out.extend(b[j..].iter().map(|l| line(DiffKind::Actual, l)));
    out
}

/// Byte buffer that keeps at most `limit` bytes and remembers truncation.
#[derive(Debug)]
pub struct CappedBuffer {
    data: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl CappedBuffer {
    pub fn new(limit: usize) -> Self {
        Self {
            data: Vec::new(),
            limit,
            truncated: false,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        let room = self.limit.saturating_sub(self.data.len());
        if bytes.len() > room {
            self.truncated = true;
        }
        self.data.extend_from_slice(&bytes[..bytes.len().min(room)]);
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Lossy UTF-8 conversion (a multi-byte char cut at the limit becomes U+FFFD).
    pub fn into_string(self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_and_cr_become_lf() {
        assert_eq!(normalize_newlines("a\r\nb\rc\n"), "a\nb\nc\n");
        assert!(outputs_match("1\r\n2\r\n", "1\n2\n", false));
    }

    #[test]
    fn trailing_whitespace_and_newlines_trimmed_when_enabled() {
        assert!(outputs_match("a\nb\n", "a  \nb\t\n\n\n", true));
        assert!(outputs_match("a", "a\n", true));
        assert!(!outputs_match("a", "a\n", false));
        assert!(!outputs_match("a\nb", "a  \nb", false));
    }

    #[test]
    fn leading_whitespace_is_significant() {
        assert!(!outputs_match("a", " a", true));
        assert!(!outputs_match("a\n\nb", "a\nb", true));
    }

    #[test]
    fn diff_marks_missing_and_extra_lines() {
        let d = line_diff("1\n2\n3\n", "1\nX\n3\n");
        let kinds: Vec<_> = d.iter().map(|l| (l.kind, l.text.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                (DiffKind::Equal, "1"),
                (DiffKind::Expected, "2"),
                (DiffKind::Actual, "X"),
                (DiffKind::Equal, "3"),
            ]
        );
    }

    #[test]
    fn diff_handles_empty_sides() {
        assert!(line_diff("", "").is_empty());
        let d = line_diff("a\n", "");
        assert_eq!(
            d,
            vec![DiffLine {
                kind: DiffKind::Expected,
                text: "a".into()
            }]
        );
        let d = line_diff("", "b");
        assert_eq!(
            d,
            vec![DiffLine {
                kind: DiffKind::Actual,
                text: "b".into()
            }]
        );
    }

    #[test]
    fn diff_of_equal_texts_is_all_equal() {
        let d = line_diff("x\ny", "x\ny");
        assert!(d.iter().all(|l| l.kind == DiffKind::Equal));
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn large_diff_uses_fallback() {
        let a: String = (0..3000).map(|i| format!("{i}\n")).collect();
        let b: String = (0..3000).map(|i| format!("{}\n", i + 1)).collect();
        let d = line_diff(&a, &b);
        assert!(!d.is_empty());
        assert!(d.iter().any(|l| l.kind == DiffKind::Expected));
    }

    #[test]
    fn capped_buffer_truncates() {
        let mut buf = CappedBuffer::new(5);
        buf.push(b"abc");
        assert!(!buf.truncated());
        buf.push(b"defg");
        assert!(buf.truncated());
        buf.push(b"h");
        assert_eq!(buf.into_string(), "abcde");
    }

    #[test]
    fn capped_buffer_exact_limit_not_truncated() {
        let mut buf = CappedBuffer::new(3);
        buf.push(b"abc");
        buf.push(b"");
        assert!(!buf.truncated());
        assert_eq!(buf.into_string(), "abc");
    }

    #[test]
    fn capped_buffer_cut_utf8_is_lossy_not_panicking() {
        let mut buf = CappedBuffer::new(1);
        buf.push("ä".as_bytes());
        assert!(buf.truncated());
        assert_eq!(buf.into_string(), "\u{FFFD}");
    }
}
