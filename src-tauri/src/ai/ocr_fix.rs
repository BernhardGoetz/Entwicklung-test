//! "Erkennungsfehler mit KI korrigieren": the local model proposes
//! character-level corrections for text from the system OCR (`Ha110` →
//! `Hallo`). The model only returns a list of changes (line, old, new),
//! never a new text; a strict plausibility filter keeps small, local
//! changes and the frontend shows each one for review. Nothing is applied
//! automatically.
//!
//! Filter rules ([`plausible_fixes`]):
//! - `old` must occur exactly once in the given line (or, since small
//!   models miscount, in a neighbouring line); `new` differs from `old` and
//!   has no line break.
//! - The change is split into character-level hunks; hunks within one
//!   word (whitespace-delimited token) are merged. A suggestion with more
//!   than [`MAX_HUNKS`] hunks is a rewrite and rejected as a whole, so whole
//!   lines are never replaced.
//! - Each hunk may touch at most one token on either side (no inserted or
//!   removed words) and its edit distance must be at most
//!   `max(2, 40 % of the length of the affected token)`; substitutions of
//!   typically confused characters (1/l/I/!, 0/O/o, quotes, …) count half.
//! - Overlapping changes are dropped, at most [`MAX_FIXES`] are returned.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::llama::{ChatMessage, ChatRequest, Llm};
use super::pipeline::{parse_json, AiProgress, Phase, MAX_TASK_CHARS};
use super::AiError;

/// At most this many fixes are returned.
pub const MAX_FIXES: usize = 30;
/// At most this many suggestions of the model are looked at.
const MAX_SUGGESTIONS: usize = 100;
/// More hunks in one suggestion means it rewrites rather than corrects.
pub const MAX_HUNKS: usize = 3;
/// Longer `old`/`new` strings are not diffed (and rejected).
const MAX_DIFF_CHARS: usize = 400;
/// Low temperature: the answer should be as deterministic as possible.
pub const FIX_TEMPERATURE: f32 = 0.1;
const FIX_MAX_TOKENS: u32 = 1536;
const FIX_SEED: u64 = 7;
const MAX_REASON_CHARS: usize = 80;

/// A plausible correction, ready for review in the frontend.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrFix {
    /// 1-based line number.
    pub line: usize,
    /// Range of `old` in the line, in UTF-16 code units (JavaScript string
    /// indices).
    pub start: usize,
    pub end: usize,
    /// The affected word as recognized, and its corrected form.
    pub old: String,
    pub new: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrFixResult {
    pub fixes: Vec<OcrFix>,
    /// Suggestions of the model that the plausibility filter rejected.
    pub rejected: usize,
}

/// One change as proposed by the model.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Suggestion {
    pub line: i64,
    pub old: String,
    pub new: String,
    pub reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Answer {
    fixes: Vec<Suggestion>,
}

// ---------------------------------------------------------------------------
// Prompt

pub const FIX_SYSTEM: &str = "You correct OCR errors. The user's text was recognized from an \
image of a programming exercise (task text and code); some characters may have been misread. \
Only fix character-level recognition errors, for example: confused characters (1/l/I/!, 0/O/o, \
5/S, 8/B, rn/m, ;/:), misread or missing quotes, brackets and punctuation inside code. \
Never rephrase, never change wording, grammar, spelling variants or style, never add or remove \
words or lines, never fix the program's logic. If you are not sure, leave the text unchanged. \
Lines are numbered as `<number>| <text>`; the number and the bar are not part of the text.\n\
Answer with JSON: {\"fixes\": [{\"line\": <number>, \"old\": <wrong text>, \"new\": <corrected \
text>, \"reason\": <very short reason>}]}. `old` must be copied exactly from that line (only the \
wrong word with a few neighbouring characters, so that it occurs once in the line), `new` is the \
same piece corrected. One item per error. Return {\"fixes\": []} if there are no recognition \
errors.";

pub fn fix_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "fixes": {
                "type": "array",
                "maxItems": MAX_FIXES,
                "items": {
                    "type": "object",
                    "properties": {
                        "line": { "type": "integer" },
                        "old": { "type": "string" },
                        "new": { "type": "string" },
                        "reason": { "type": "string" }
                    },
                    "required": ["line", "old", "new"]
                }
            }
        },
        "required": ["fixes"]
    })
}

/// The text with line numbers, as the model sees it.
pub fn numbered(text: &str) -> String {
    text.split('\n')
        .enumerate()
        .map(|(i, l)| format!("{}| {l}", i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn fix_messages(text: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage::system(FIX_SYSTEM),
        ChatMessage::user(format!("Recognized text:\n<<<\n{}\n>>>", numbered(text))),
    ]
}

pub fn fix_request(text: &str) -> ChatRequest {
    ChatRequest {
        messages: fix_messages(text),
        temperature: FIX_TEMPERATURE,
        max_tokens: FIX_MAX_TOKENS,
        json_schema: Some(fix_schema()),
        seed: Some(FIX_SEED),
    }
}

// ---------------------------------------------------------------------------
// Plausibility filter (pure)

/// Groups of characters that OCR engines typically confuse.
const CONFUSABLE: [&str; 11] = [
    "1lI|!i",
    "0Oo",
    "5Ss",
    "8B",
    "2Z",
    "6G",
    "\"'`\u{201c}\u{201d}\u{201e}\u{2018}\u{2019}",
    ",.;:",
    "([{",
    ")]}",
    "-\u{2013}_",
];

fn confusable(a: char, b: char) -> bool {
    CONFUSABLE.iter().any(|g| g.contains(a) && g.contains(b))
}

/// Levenshtein distance in characters; substituting typically confused
/// characters (1/l, 0/O, quotes, …) costs only 0.5.
pub fn edit_distance(a: &[char], b: &[char]) -> f64 {
    let mut prev: Vec<f64> = (0..=b.len()).map(|j| j as f64).collect();
    let mut cur = vec![0.0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = (i + 1) as f64;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb {
                0.0
            } else if confusable(ca, cb) {
                0.5
            } else {
                1.0
            };
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1.0).min(cur[j] + 1.0);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// A changed range of `old` (character indices) and its replacement.
#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub start: usize,
    pub end: usize,
    pub new: String,
}

/// Character diff (longest common subsequence) of `old` and `new` as hunks;
/// hunks separated only by non-whitespace characters (same word) are merged.
pub fn hunks(old: &[char], new: &[char]) -> Vec<Hunk> {
    let (n, m) = (old.len(), new.len());
    // lcs[i][j] = LCS length of old[i..] and new[j..].
    let mut lcs = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut raw: Vec<Hunk> = Vec::new();
    let mut open: Option<Hunk> = None;
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && old[i] == new[j] && lcs[i][j] == lcs[i + 1][j + 1] + 1 {
            raw.extend(open.take());
            i += 1;
            j += 1;
            continue;
        }
        let h = open.get_or_insert(Hunk {
            start: i,
            end: i,
            new: String::new(),
        });
        if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            i += 1;
            h.end = i;
        } else {
            h.new.push(new[j]);
            j += 1;
        }
    }
    raw.extend(open);

    let mut merged: Vec<Hunk> = Vec::new();
    for h in raw {
        if let Some(last) = merged.last_mut() {
            let gap = &old[last.end..h.start];
            if !gap.iter().any(|c| c.is_whitespace()) {
                last.new.extend(gap);
                last.new.push_str(&h.new);
                last.end = h.end;
                continue;
            }
        }
        merged.push(h);
    }
    merged
}

fn token_count(s: &str) -> usize {
    s.split_whitespace().count()
}

fn utf16_len(chars: &[char]) -> usize {
    chars.iter().map(|c| c.len_utf16()).sum()
}

/// Finds `old` in line `line` (1-based) or a neighbouring line; it must
/// occur exactly once there. Returns the 0-based line index and the
/// character offset.
fn locate(lines: &[&str], line: i64, old: &str) -> Option<(usize, usize)> {
    for candidate in [line, line - 1, line + 1] {
        if candidate < 1 || candidate as usize > lines.len() {
            continue;
        }
        let idx = candidate as usize - 1;
        let text = lines[idx];
        let mut found = text.match_indices(old);
        if let (Some((byte, _)), None) = (found.next(), found.next()) {
            return Some((idx, text[..byte].chars().count()));
        }
    }
    None
}

/// Checks a suggestion and turns it into fixes (one per changed word).
/// `None` = rejected.
fn check(lines: &[&str], s: &Suggestion) -> Option<Vec<OcrFix>> {
    if s.old.is_empty() || s.old == s.new || s.new.contains(['\n', '\r']) {
        return None;
    }
    let old: Vec<char> = s.old.chars().collect();
    let new: Vec<char> = s.new.chars().collect();
    if old.len() > MAX_DIFF_CHARS || new.len() > MAX_DIFF_CHARS {
        return None;
    }
    if new.iter().any(|c| c.is_control() && *c != '\t') {
        return None;
    }
    let (idx, offset) = locate(lines, s.line, &s.old)?;
    let line: Vec<char> = lines[idx].chars().collect();
    let hunks = hunks(&old, &new);
    if hunks.is_empty() || hunks.len() > MAX_HUNKS {
        return None;
    }
    let reason = s
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| r.chars().take(MAX_REASON_CHARS).collect::<String>());

    let mut fixes = Vec::new();
    for h in hunks {
        let old_part: String = old[h.start..h.end].iter().collect();
        if token_count(&old_part) > 1 || token_count(&h.new) > 1 {
            return None;
        }
        // The affected word(s) in the line.
        let (a, b) = (offset + h.start, offset + h.end);
        let mut start = a;
        while start > 0 && !line[start - 1].is_whitespace() {
            start -= 1;
        }
        let mut end = b;
        while end < line.len() && !line[end].is_whitespace() {
            end += 1;
        }
        let word = &line[start..end];
        let distance = edit_distance(&old[h.start..h.end], &h.new.chars().collect::<Vec<_>>());
        let limit = (word.len() as f64 * 0.4).max(2.0);
        if distance > limit {
            return None;
        }
        let old_word: String = word.iter().collect();
        let new_word: String = line[start..a]
            .iter()
            .chain(h.new.chars().collect::<Vec<_>>().iter())
            .chain(line[b..end].iter())
            .collect();
        if old_word == new_word {
            continue;
        }
        let start16 = utf16_len(&line[..start]);
        fixes.push(OcrFix {
            line: idx + 1,
            start: start16,
            end: start16 + utf16_len(word),
            old: old_word,
            new: new_word,
            reason: reason.clone(),
        });
    }
    (!fixes.is_empty()).then_some(fixes)
}

/// Applies the plausibility filter to the model's suggestions for `text`.
/// Returns the accepted fixes (sorted by position) and the number of
/// rejected suggestions.
pub fn plausible_fixes(text: &str, suggestions: &[Suggestion]) -> (Vec<OcrFix>, usize) {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut accepted: Vec<OcrFix> = Vec::new();
    let mut rejected = 0;
    for s in suggestions.iter().take(MAX_SUGGESTIONS) {
        let Some(fixes) = check(&lines, s) else {
            rejected += 1;
            continue;
        };
        for f in fixes {
            let overlaps = accepted
                .iter()
                .any(|a| a.line == f.line && a.start < f.end && f.start < a.end);
            if overlaps || accepted.len() >= MAX_FIXES {
                continue;
            }
            accepted.push(f);
        }
    }
    rejected += suggestions.len().saturating_sub(MAX_SUGGESTIONS);
    accepted.sort_by_key(|f| (f.line, f.start));
    (accepted, rejected)
}

/// Applies fixes to `text` (as the frontend does; used in tests).
pub fn apply_fixes(text: &str, fixes: &[&OcrFix]) -> String {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let mut sorted = fixes.to_vec();
    sorted.sort_by_key(|f| std::cmp::Reverse((f.line, f.start)));
    for f in sorted {
        let Some(line) = lines.get_mut(f.line - 1) else {
            continue;
        };
        let units: Vec<u16> = line.encode_utf16().collect();
        if f.end > units.len() || f.start > f.end {
            continue;
        }
        let mut out = String::from_utf16_lossy(&units[..f.start]);
        out.push_str(&f.new);
        out.push_str(&String::from_utf16_lossy(&units[f.end..]));
        *line = out;
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Flow

/// Asks the model for corrections of `text` and filters them.
pub async fn suggest<L: Llm>(
    llm: &L,
    text: &str,
    cancel: &CancellationToken,
) -> Result<OcrFixResult, AiError> {
    if cancel.is_cancelled() {
        return Err(AiError::Cancelled);
    }
    let r = llm.chat(&fix_request(text)).await?;
    if cancel.is_cancelled() {
        return Err(AiError::Cancelled);
    }
    let answer: Answer = parse_json(&r.content).ok_or_else(|| {
        AiError::Other(
            "Die KI hat keine verwertbare Antwort geliefert. Bitte erneut versuchen.".into(),
        )
    })?;
    let (fixes, rejected) = plausible_fixes(text, &answer.fixes);
    Ok(OcrFixResult { fixes, rejected })
}

/// Checks the text before anything is started.
pub fn validate(text: &str) -> Result<(), AiError> {
    if text.trim().is_empty() {
        return Err(AiError::Other(
            "Es gibt keinen Text zum Korrigieren.".into(),
        ));
    }
    if text.chars().count() > MAX_TASK_CHARS {
        return Err(AiError::Other(format!(
            "Der Text ist für die KI-Korrektur zu lang (höchstens {MAX_TASK_CHARS} Zeichen)."
        )));
    }
    Ok(())
}

static FIXING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `suggest_ocr_fixes`: starts llama-server with the selected model and
/// asks for corrections.
pub async fn run(
    text: String,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(AiProgress),
) -> Result<OcrFixResult, AiError> {
    use std::sync::atomic::Ordering;
    validate(&text)?;
    if FIXING.swap(true, Ordering::SeqCst) {
        return Err(AiError::Other("Die KI-Korrektur läuft bereits.".into()));
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FIXING.store(false, Ordering::SeqCst);
        }
    }
    let _reset = Reset;

    let model = super::models::selected_model()?;
    on_progress(AiProgress::new(Phase::Model, "KI-Modell wird geladen …"));
    let endpoint = super::llama::ensure_running(&model.path, cancel, |m| {
        on_progress(AiProgress::new(Phase::Model, m))
    })
    .await?;
    let llm = super::llama::LlamaClient::new(endpoint, cancel.clone())?;
    on_progress(AiProgress::new(
        Phase::Correction,
        "KI sucht Erkennungsfehler …",
    ));
    suggest(&llm, &text, cancel).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::llama::ChatResponse;
    use std::sync::Mutex;

    /// The user's example: Windows OCR result of a Python screenshot.
    const OCR_TEXT: &str = "name = input()\nprint(f\"Ha110, {name}l )";
    const EXPECTED: &str = "name = input()\nprint(f\"Hallo, {name}!\")";

    fn s(line: i64, old: &str, new: &str) -> Suggestion {
        Suggestion {
            line,
            old: old.into(),
            new: new.into(),
            reason: None,
        }
    }

    fn filtered(text: &str, sug: &[Suggestion]) -> (String, Vec<OcrFix>, usize) {
        let (fixes, rejected) = plausible_fixes(text, sug);
        let applied = apply_fixes(text, &fixes.iter().collect::<Vec<_>>());
        (applied, fixes, rejected)
    }

    struct MockLlm {
        answer: String,
        calls: Mutex<Vec<ChatRequest>>,
    }

    impl MockLlm {
        fn new(answer: &str) -> Self {
            Self {
                answer: answer.into(),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl Llm for MockLlm {
        async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, AiError> {
            self.calls.lock().unwrap().push(req.clone());
            Ok(ChatResponse {
                content: self.answer.clone(),
                ..ChatResponse::default()
            })
        }
    }

    #[test]
    fn edit_distance_counts_characters() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        // Three typical confusions (1/l, 1/l, 0/o).
        assert_eq!(edit_distance(&c("Ha110"), &c("Hallo")), 1.5);
        assert_eq!(edit_distance(&c(""), &c("ab")), 2.0);
        assert_eq!(edit_distance(&c("Größe"), &c("Grösse")), 2.0);
        assert_eq!(edit_distance(&c("l "), &c("!\"")), 1.5);
    }

    #[test]
    fn hunks_merge_within_words() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        let h = hunks(&c("Ha110"), &c("Hallo"));
        assert_eq!(
            h,
            vec![Hunk {
                start: 2,
                end: 5,
                new: "llo".into()
            }]
        );
        // Two words: two hunks.
        let h = hunks(
            &c("print(f\"Ha110, {name}l )"),
            &c("print(f\"Hallo, {name}!\")"),
        );
        assert_eq!(h.len(), 2, "{h:?}");
        assert!(hunks(&c("gleich"), &c("gleich")).is_empty());
    }

    #[test]
    fn user_example_is_fixed() {
        // One item per error, as the prompt asks.
        let (text, fixes, rejected) = filtered(
            OCR_TEXT,
            &[s(2, "Ha110", "Hallo"), s(2, "{name}l )", "{name}!\")")],
        );
        assert_eq!(text, EXPECTED);
        assert_eq!(rejected, 0);
        assert_eq!(fixes.len(), 2);
        assert_eq!(fixes[0].old, "print(f\"Ha110,");
        assert_eq!(fixes[0].new, "print(f\"Hallo,");
        assert_eq!((fixes[0].line, fixes[0].start, fixes[0].end), (2, 0, 14));
        assert_eq!(fixes[1].old, "{name}l )");
        assert_eq!(fixes[1].new, "{name}!\")");
    }

    #[test]
    fn whole_line_answer_is_split_into_word_fixes() {
        // Small models like to return the whole line: it is never applied
        // as a unit, but each changed word is checked on its own.
        let (text, fixes, _) = filtered(
            OCR_TEXT,
            &[s(
                2,
                "print(f\"Ha110, {name}l )",
                "print(f\"Hallo, {name}!\")",
            )],
        );
        assert_eq!(text, EXPECTED);
        assert_eq!(fixes.len(), 2);
    }

    #[test]
    fn wrong_line_number_is_tolerated_by_one() {
        let (text, _, rejected) = filtered(OCR_TEXT, &[s(1, "Ha110", "Hallo")]);
        assert_eq!(rejected, 0);
        assert!(text.contains("Hallo"));
        // Too far away, or not in the text at all.
        let (_, fixes, rejected) = filtered(
            "a\nb\nc\nHa110",
            &[s(1, "Ha110", "Hallo"), s(4, "Hxllo", "Hallo")],
        );
        assert!(fixes.is_empty());
        assert_eq!(rejected, 2);
    }

    #[test]
    fn rephrasing_is_rejected() {
        let text = "Schreibe ein Programm, das zwei Zahlen addiert.";
        let (out, fixes, rejected) = filtered(
            text,
            &[
                // Inserted word.
                s(1, "Schreibe ein", "Schreiben Sie ein"),
                // Different word.
                s(1, "das zwei", "welches zwei"),
                // Whole line rewritten.
                s(
                    1,
                    text,
                    "Erstelle ein Programm, welches zwei Zahlen summiert.",
                ),
                // Unchanged and empty.
                s(1, "zwei", "zwei"),
                s(1, "", "x"),
                // Ambiguous: "e" occurs many times.
                s(1, "e", "E"),
            ],
        );
        assert_eq!(out, text);
        assert!(fixes.is_empty(), "{fixes:?}");
        assert_eq!(rejected, 6);
    }

    #[test]
    fn added_lines_and_content_are_rejected() {
        let (out, fixes, rejected) = filtered(
            OCR_TEXT,
            &[
                s(2, "{name}l )", "{name}!\")\nprint(\"Tschüss\")"),
                s(1, "input()", "input().strip()"),
                s(1, "name =", "name = name ="),
                s(
                    2,
                    "print(f\"Ha110, {name}l )",
                    "print(\"Hallo Welt\")  # fixed",
                ),
            ],
        );
        assert_eq!(out, OCR_TEXT);
        assert!(fixes.is_empty(), "{fixes:?}");
        assert_eq!(rejected, 4);
    }

    #[test]
    fn offsets_are_utf16_and_fixes_are_capped() {
        // "ä" is 1 UTF-16 unit but 2 bytes; "😀" is 2 units.
        let text = "Größe 😀 0rdnung";
        let (out, fixes, _) = filtered(text, &[s(1, "0rdnung", "Ordnung")]);
        assert_eq!(out, "Größe 😀 Ordnung");
        assert_eq!((fixes[0].start, fixes[0].end), (9, 16));

        let many: String = (0..50).map(|i| format!("w{i}0rd ")).collect();
        let sug: Vec<Suggestion> = (0..50)
            .map(|i| s(1, &format!("w{i}0rd"), &format!("w{i}Ord")))
            .collect();
        let (fixes, _) = plausible_fixes(&many, &sug);
        assert_eq!(fixes.len(), MAX_FIXES);
        // Overlapping duplicates are dropped.
        let (fixes, _) =
            plausible_fixes("0rdnung", &[s(1, "0rdnung", "Ordnung"), s(1, "0rd", "Ord")]);
        assert_eq!(fixes.len(), 1);
    }

    #[test]
    fn prompt_numbers_lines_and_uses_schema() {
        let req = fix_request(OCR_TEXT);
        assert_eq!(req.temperature, FIX_TEMPERATURE);
        assert!(req.json_schema.is_some());
        let user = &req.messages[1].content;
        assert!(user.contains("1| name = input()"), "{user}");
        assert!(user.contains("2| print(f\"Ha110, {name}l )"), "{user}");
        assert!(FIX_SYSTEM.contains("Never rephrase"));
    }

    #[tokio::test]
    async fn mock_model_user_example() {
        let llm = MockLlm::new(
            r#"{"fixes":[{"line":2,"old":"Ha110","new":"Hallo","reason":"1/l und 0/o verwechselt"},
                {"line":2,"old":"{name}l )","new":"{name}!\")"}]}"#,
        );
        let r = suggest(&llm, OCR_TEXT, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.rejected, 0);
        assert_eq!(
            apply_fixes(OCR_TEXT, &r.fixes.iter().collect::<Vec<_>>()),
            EXPECTED
        );
        assert_eq!(
            r.fixes[0].reason.as_deref(),
            Some("1/l und 0/o verwechselt")
        );
        assert_eq!(llm.calls.lock().unwrap().len(), 1);
        // The result goes to the frontend in camelCase.
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["fixes"][0]["line"], 2);
        assert!(v["fixes"][0]["reason"].is_string());
    }

    #[tokio::test]
    async fn mock_model_adversarial_answers() {
        // Rephrasing and added lines: nothing survives.
        let llm = MockLlm::new(
            r#"{"fixes":[{"line":2,"old":"print(f\"Ha110, {name}l )","new":"print(\"Willkommen, \" + name)"},
                {"line":1,"old":"name = input()","new":"name = input()\nname = name.strip()"},
                {"line":3,"old":"","new":"print(\"Ende\")"}]}"#,
        );
        let r = suggest(&llm, OCR_TEXT, &CancellationToken::new())
            .await
            .unwrap();
        assert!(r.fixes.is_empty(), "{:?}", r.fixes);
        assert_eq!(r.rejected, 3);
        // Garbage answer: error.
        let e = suggest(&MockLlm::new("Gern!"), OCR_TEXT, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(e.to_string().contains("keine verwertbare Antwort"));
        // Cancelled before the call.
        let t = CancellationToken::new();
        t.cancel();
        assert!(matches!(
            suggest(&llm, OCR_TEXT, &t).await,
            Err(AiError::Cancelled)
        ));
    }

    #[test]
    fn validation() {
        assert!(validate("  \n").is_err());
        assert!(validate(&"x".repeat(MAX_TASK_CHARS + 1)).is_err());
        assert!(validate(OCR_TEXT).is_ok());
    }
}
