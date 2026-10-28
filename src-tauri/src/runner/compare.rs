//! Pure output comparison: newline normalization, comparison modes (lines,
//! tokens, numbers with tolerance), a small LCS-based line diff and capped
//! output buffers.

use serde::{Deserialize, Serialize};

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

fn default_true() -> bool {
    true
}

/// How program output is compared with the expected output.
///
/// `Tokens` and `Float` follow the default output validator of Kattis /
/// the ICPC problem package format: the output is split into tokens at any
/// whitespace (spaces, tabs, line breaks), so the amount of whitespace and
/// the line structure do not matter. One deliberate difference: string
/// tokens are compared case-sensitively by default (Kattis: case-insensitive
/// unless `case_sensitive`), because exercises usually expect the exact
/// text; `Tokens { case_sensitive: false }` gives the Kattis behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CompareMode {
    /// Line by line; with `trim`, trailing whitespace per line and trailing
    /// empty lines are ignored (the original behaviour).
    Lines {
        #[serde(default = "default_true")]
        trim: bool,
    },
    /// Token by token, whitespace-insensitive.
    Tokens {
        #[serde(default = "default_true")]
        case_sensitive: bool,
    },
    /// Like `Tokens`, but if the expected token is a number, the actual
    /// token must be a number within `abs_tol` **or** `rel_tol * |expected|`.
    /// Other tokens are compared exactly.
    Float { abs_tol: f64, rel_tol: f64 },
}

impl Default for CompareMode {
    fn default() -> Self {
        CompareMode::Lines { trim: true }
    }
}

/// Upper bound for tolerances (also mirrored in the frontend).
pub const MAX_TOLERANCE: f64 = 1e6;

/// Whether a token is a decimal number: optional sign, digits with an
/// optional fraction (`1`, `-2.5`, `.5`, `3.`) and an optional exponent.
/// `inf`, `nan` and hexadecimal numbers are not numbers here.
pub fn is_number(token: &str) -> bool {
    let s = token.strip_prefix(['+', '-']).unwrap_or(token);
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok = digits(int) && digits(frac) && !(int.is_empty() && frac.is_empty());
    let exponent_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && digits(e)
    });
    mantissa_ok && exponent_ok
}

fn tokens(s: &str) -> impl Iterator<Item = &str> {
    s.split_whitespace()
}

fn floats_match(expected: f64, actual: f64, abs_tol: f64, rel_tol: f64) -> bool {
    if expected == actual {
        return true;
    }
    let diff = (expected - actual).abs();
    diff <= abs_tol || diff <= rel_tol * expected.abs()
}

impl CompareMode {
    pub fn lines(trim: bool) -> Self {
        CompareMode::Lines { trim }
    }

    /// German error message for invalid settings (negative or non-finite
    /// tolerances).
    pub fn validate(&self) -> Result<(), String> {
        if let CompareMode::Float { abs_tol, rel_tol } = *self {
            for t in [abs_tol, rel_tol] {
                if !t.is_finite() || !(0.0..=MAX_TOLERANCE).contains(&t) {
                    return Err(format!(
                        "Ungültige Toleranz {t}: erlaubt sind Zahlen von 0 bis {MAX_TOLERANCE}."
                    ));
                }
            }
        }
        Ok(())
    }

    /// Whether `actual` is accepted for `expected`.
    pub fn matches(&self, expected: &str, actual: &str) -> bool {
        match *self {
            CompareMode::Lines { trim } => outputs_match(expected, actual, trim),
            CompareMode::Tokens { case_sensitive } => {
                let (mut e, mut a) = (tokens(expected), tokens(actual));
                loop {
                    match (e.next(), a.next()) {
                        (None, None) => return true,
                        (Some(x), Some(y))
                            if x == y
                                || (!case_sensitive && x.to_lowercase() == y.to_lowercase()) => {}
                        _ => return false,
                    }
                }
            }
            CompareMode::Float { abs_tol, rel_tol } => {
                let (mut e, mut a) = (tokens(expected), tokens(actual));
                loop {
                    match (e.next(), a.next()) {
                        (None, None) => return true,
                        (Some(x), Some(y)) if x == y => {}
                        (Some(x), Some(y)) if is_number(x) && is_number(y) => {
                            match (x.parse::<f64>(), y.parse::<f64>()) {
                                (Ok(x), Ok(y)) if floats_match(x, y, abs_tol, rel_tol) => {}
                                _ => return false,
                            }
                        }
                        _ => return false,
                    }
                }
            }
        }
    }

    /// Text shown in the line diff (the diff stays line-based in all modes).
    pub fn display_text(&self, s: &str) -> String {
        match *self {
            CompareMode::Lines { trim } => normalize_output(s, trim),
            _ => normalize_output(s, true),
        }
    }
}

/// Canonical form of an output for grouping equal outputs (majority vote).
///
/// Two outputs with the same canonical form always match each other. For
/// `Lines` and `Tokens` the converse holds as well. For `Float`, numbers are
/// only written canonically (`1.50` → `1.5`), so outputs that differ within
/// the tolerance can still have different forms – use [`group_outputs`] to
/// merge those.
pub fn normalize_for_grouping(mode: &CompareMode, s: &str) -> String {
    match *mode {
        CompareMode::Lines { trim } => normalize_output(s, trim),
        CompareMode::Tokens { case_sensitive } => {
            let joined = tokens(s).collect::<Vec<_>>().join(" ");
            if case_sensitive {
                joined
            } else {
                joined.to_lowercase()
            }
        }
        CompareMode::Float { .. } => tokens(s)
            .map(|t| match t.parse::<f64>() {
                Ok(v) if is_number(t) => format!("{}", v + 0.0),
                _ => t.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Groups outputs that match each other under `mode`, largest group first
/// (ties: the group whose first member comes first). Each output joins the
/// first group whose first member (representative) it matches.
pub fn group_outputs(mode: &CompareMode, outputs: &[&str]) -> Vec<Vec<usize>> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, out) in outputs.iter().enumerate() {
        let key = normalize_for_grouping(mode, out);
        let found = groups
            .iter_mut()
            .find(|(k, members)| *k == key || mode.matches(outputs[members[0]], out));
        match found {
            Some((_, members)) => members.push(i),
            None => groups.push((key, vec![i])),
        }
    }
    let mut groups: Vec<Vec<usize>> = groups.into_iter().map(|(_, m)| m).collect();
    groups.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    groups
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
    fn compare_mode_json_and_default() {
        let parse = |j: &str| serde_json::from_str::<CompareMode>(j).unwrap();
        assert_eq!(
            parse(r#"{"kind":"lines","trim":false}"#),
            CompareMode::lines(false)
        );
        assert_eq!(parse(r#"{"kind":"lines"}"#), CompareMode::lines(true));
        assert_eq!(
            parse(r#"{"kind":"tokens","caseSensitive":false}"#),
            CompareMode::Tokens {
                case_sensitive: false
            }
        );
        assert_eq!(
            parse(r#"{"kind":"tokens"}"#),
            CompareMode::Tokens {
                case_sensitive: true
            }
        );
        assert_eq!(
            parse(r#"{"kind":"float","absTol":1e-6,"relTol":0.001}"#),
            CompareMode::Float {
                abs_tol: 1e-6,
                rel_tol: 1e-3
            }
        );
        assert!(serde_json::from_str::<CompareMode>(r#"{"kind":"regex"}"#).is_err());
        assert_eq!(CompareMode::default(), CompareMode::lines(true));
        let json = serde_json::to_string(&CompareMode::Float {
            abs_tol: 0.5,
            rel_tol: 0.0,
        })
        .unwrap();
        assert_eq!(json, r#"{"kind":"float","absTol":0.5,"relTol":0.0}"#);
    }

    #[test]
    fn lines_mode_is_the_old_behaviour() {
        for trim in [true, false] {
            for (e, a) in [
                ("a\nb\n", "a  \nb\n\n"),
                ("a", "a\n"),
                ("1\r\n", "1\n"),
                ("a", " a"),
            ] {
                assert_eq!(
                    CompareMode::lines(trim).matches(e, a),
                    outputs_match(e, a, trim),
                    "{e:?} {a:?} {trim}"
                );
            }
        }
    }

    #[test]
    fn tokens_mode_ignores_whitespace() {
        let m = CompareMode::Tokens {
            case_sensitive: true,
        };
        assert!(m.matches("1 2 3\n", "1\n2   3"));
        assert!(m.matches("  a\tb ", "a b\r\n\n"));
        assert!(m.matches("", " \n "));
        assert!(!m.matches("1 2 3", "1 2"));
        assert!(!m.matches("1 2", "1 2 3"));
        assert!(!m.matches("12", "1 2"));
        assert!(!m.matches("Hallo", "hallo"));
        let ci = CompareMode::Tokens {
            case_sensitive: false,
        };
        assert!(ci.matches("Hallo WELT", "hallo welt"));
        assert!(ci.matches("ÄRGER", "ärger"));
        assert!(!ci.matches("1.0", "1"));
    }

    #[test]
    fn float_mode_uses_absolute_or_relative_tolerance() {
        let m = CompareMode::Float {
            abs_tol: 1e-6,
            rel_tol: 1e-4,
        };
        assert!(m.matches("3.14159265", "3.1415926"));
        assert!(m.matches("0.5 x", "5e-1   x"));
        assert!(m.matches("1000000", "1000050")); // relative: 50 <= 100
        assert!(!m.matches("1000000", "1000200"));
        assert!(m.matches("0", "0.0000009")); // absolute
        assert!(!m.matches("0", "0.00001"));
        assert!(m.matches("-2.5", "-2.50000001"));
        assert!(!m.matches("2.5", "-2.5"));
        // Strings are exact; numbers do not match words.
        assert!(!m.matches("Summe 3", "summe 3"));
        assert!(!m.matches("3", "drei"));
        assert!(!m.matches("nan", "NaN"));
        assert!(m.matches("inf", "inf"));
        assert!(!m.matches("1 2", "1"));
        let exact = CompareMode::Float {
            abs_tol: 0.0,
            rel_tol: 0.0,
        };
        assert!(exact.matches("1.0", "1"));
        assert!(!exact.matches("1.0", "1.0000001"));
    }

    #[test]
    fn number_tokens() {
        for t in ["0", "-1", "+2.5", ".5", "3.", "1e9", "1.5E-3", "-0.0e+1"] {
            assert!(is_number(t), "{t}");
        }
        for t in [
            "", ".", "-", "e5", "1e", "1e+", "inf", "NaN", "0x10", "1,5", "1.2.3", "1_000",
        ] {
            assert!(!is_number(t), "{t}");
        }
    }

    #[test]
    fn tolerance_validation() {
        assert!(CompareMode::default().validate().is_ok());
        let f = |abs_tol, rel_tol| CompareMode::Float { abs_tol, rel_tol }.validate();
        assert!(f(0.0, 1e-6).is_ok());
        assert!(f(-1.0, 0.0).unwrap_err().contains("Ungültige Toleranz"));
        assert!(f(0.0, f64::NAN).is_err());
        assert!(f(f64::INFINITY, 0.0).is_err());
        assert!(f(MAX_TOLERANCE * 2.0, 0.0).is_err());
    }

    #[test]
    fn grouping_normalizes_per_mode() {
        let lines = CompareMode::lines(true);
        assert_eq!(normalize_for_grouping(&lines, "a \r\nb\n\n"), "a\nb");
        let tok = CompareMode::Tokens {
            case_sensitive: false,
        };
        assert_eq!(normalize_for_grouping(&tok, " A\n b  c "), "a b c");
        let fl = CompareMode::Float {
            abs_tol: 0.0,
            rel_tol: 0.0,
        };
        assert_eq!(
            normalize_for_grouping(&fl, "1.50 -0.0 x\n2e1"),
            "1.5 0 x 20"
        );
    }

    #[test]
    fn groups_are_sorted_by_size() {
        let fl = CompareMode::Float {
            abs_tol: 0.01,
            rel_tol: 0.0,
        };
        let outs = ["1.0", "2.0", "1.001", "1", "2.0\n"];
        assert_eq!(group_outputs(&fl, &outs), vec![vec![0, 2, 3], vec![1, 4]]);
        let lines = CompareMode::lines(false);
        assert_eq!(
            group_outputs(&lines, &["b", "a", "a", "b"]),
            vec![vec![0, 3], vec![1, 2]]
        );
        assert!(group_outputs(&lines, &[]).is_empty());
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
