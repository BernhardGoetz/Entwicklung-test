//! Rebuilds plain text with line breaks and indentation from OCR word boxes.
//!
//! The system OCR engines (Windows.Media.Ocr, Apple Vision) return words or
//! line segments with bounding boxes but drop leading whitespace. For code
//! (Python!) the indentation matters, so it is reconstructed from the left
//! edge of each line relative to the leftmost line, measured in estimated
//! character widths. Pure and unit-tested.

/// A recognized word (or text segment) with its bounding box.
///
/// Coordinates: origin top left, y grows downwards. The unit does not matter
/// (pixels, points, normalized * constant), but x and y must each use one
/// consistent scale.
#[derive(Debug, Clone, PartialEq)]
pub struct WordBox {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl WordBox {
    fn bottom(&self) -> f64 {
        self.y + self.height
    }

    fn right(&self) -> f64 {
        self.x + self.width
    }

    fn center_y(&self) -> f64 {
        self.y + self.height / 2.0
    }
}

/// Gaps between two words up to this many character widths become a single
/// space; wider gaps (tab stops, table columns) keep their width.
const WIDE_GAP_CHARS: f64 = 2.5;
/// Indentations closer than this (in character widths) count as the same level.
const INDENT_CLUSTER_CHARS: f64 = 0.6;
/// At most this many empty lines are inserted for a vertical gap.
const MAX_BLANK_LINES: usize = 2;

struct Line<'a> {
    words: Vec<&'a WordBox>,
    top: f64,
    bottom: f64,
}

impl Line<'_> {
    fn left(&self) -> f64 {
        self.words.first().map_or(0.0, |w| w.x)
    }

    fn center(&self) -> f64 {
        (self.top + self.bottom) / 2.0
    }
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    Some(values[values.len() / 2])
}

fn lower_quartile(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    Some(values[values.len() / 4])
}

/// Groups words into lines: a word joins the current line if it overlaps it
/// vertically by at least half of the smaller height.
fn group_lines(words: &[WordBox]) -> Vec<Line<'_>> {
    let mut sorted: Vec<&WordBox> = words
        .iter()
        .filter(|w| !w.text.trim().is_empty() && w.width > 0.0 && w.height > 0.0)
        .collect();
    sorted.sort_by(|a, b| a.center_y().total_cmp(&b.center_y()));

    let mut lines: Vec<Line> = Vec::new();
    for w in sorted {
        if let Some(line) = lines.last_mut() {
            let overlap = line.bottom.min(w.bottom()) - line.top.max(w.y);
            let line_height = line.bottom - line.top;
            if overlap >= 0.5 * w.height.min(line_height) {
                line.top = line.top.min(w.y);
                line.bottom = line.bottom.max(w.bottom());
                line.words.push(w);
                continue;
            }
        }
        lines.push(Line {
            words: vec![w],
            top: w.y,
            bottom: w.bottom(),
        });
    }
    for line in &mut lines {
        line.words.sort_by(|a, b| a.x.total_cmp(&b.x));
    }
    lines
}

/// Maps the left edges of all lines to indentation widths in spaces.
///
/// Nearby edges are clustered (OCR boxes jitter by a few pixels). If the
/// smallest indentation is 3–5 columns and every level is a clean multiple
/// of it, the levels are normalized to 4 spaces each (the usual Python
/// indentation; character widths are only estimated).
fn indentations(lines: &[Line], char_width: f64) -> Vec<usize> {
    let min_x = lines.iter().map(Line::left).fold(f64::INFINITY, f64::min);
    let offsets: Vec<f64> = lines
        .iter()
        .map(|l| (l.left() - min_x) / char_width)
        .collect();

    let mut sorted = offsets.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    // Cluster representatives (smallest offset of each cluster).
    let mut clusters: Vec<f64> = Vec::new();
    for o in sorted {
        match clusters.last() {
            Some(&c) if o - c < INDENT_CLUSTER_CHARS => {}
            _ => clusters.push(o),
        }
    }
    let snap = |o: f64| -> f64 {
        clusters
            .iter()
            .rev()
            .find(|&&c| o >= c - f64::EPSILON)
            .copied()
            .unwrap_or(0.0)
    };
    let cols: Vec<usize> = offsets.iter().map(|&o| snap(o).round() as usize).collect();

    let unit = cols.iter().copied().filter(|&c| c > 0).min();
    if let Some(unit) = unit {
        let unit_f = clusters
            .iter()
            .copied()
            .find(|&c| c.round() as usize == unit)
            .unwrap_or(unit as f64);
        let clean = (3..=5).contains(&unit)
            && offsets.iter().all(|&o| {
                let level = snap(o) / unit_f;
                (level - level.round()).abs() < 0.25
            });
        if clean {
            return offsets
                .iter()
                .map(|&o| (snap(o) / unit_f).round() as usize * 4)
                .collect();
        }
    }
    cols
}

/// Rebuilds the text of one page from its word boxes.
pub fn reconstruct_text(words: &[WordBox]) -> String {
    let lines = group_lines(words);
    if lines.is_empty() {
        return String::new();
    }
    let char_width = median(
        lines
            .iter()
            .flat_map(|l| &l.words)
            .map(|w| w.width / w.text.trim().chars().count().max(1) as f64)
            .collect(),
    )
    .filter(|cw| *cw > 0.0)
    .unwrap_or(1.0);
    let indents = indentations(&lines, char_width);

    // Typical distance between neighbouring lines (lower quartile, so that
    // paragraphs gaps do not inflate it), at least one line height.
    let line_height = median(lines.iter().map(|l| l.bottom - l.top).collect()).unwrap_or(1.0);
    let pitch = lower_quartile(
        lines
            .windows(2)
            .map(|p| p[1].center() - p[0].center())
            .collect(),
    )
    .unwrap_or(line_height)
    .max(line_height);

    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            let distance = line.center() - lines[i - 1].center();
            let blanks = ((distance / pitch).round() as usize)
                .saturating_sub(1)
                .min(MAX_BLANK_LINES);
            out.push_str(&"\n".repeat(1 + blanks));
        }
        out.push_str(&" ".repeat(indents[i]));
        let mut prev: Option<&WordBox> = None;
        for w in &line.words {
            if let Some(p) = prev {
                let gap = (w.x - p.right()) / char_width;
                let spaces = if gap > WIDE_GAP_CHARS {
                    gap.round() as usize
                } else {
                    1
                };
                out.push_str(&" ".repeat(spaces));
            }
            out.push_str(w.text.trim());
            prev = Some(w);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monospaced test font: 10 units per character, lines 20 units high.
    const CW: f64 = 10.0;
    const LH: f64 = 20.0;

    fn word(text: &str, col: f64, row: f64) -> WordBox {
        WordBox {
            text: text.into(),
            x: 50.0 + col * CW,
            y: 100.0 + row * LH * 1.4,
            width: text.chars().count() as f64 * CW,
            height: LH,
        }
    }

    /// Splits a code line into word boxes at single spaces, keeping columns.
    fn words_of(line: &str, row: f64) -> Vec<WordBox> {
        let mut out = Vec::new();
        let mut col = 0usize;
        for part in line.split(' ') {
            if !part.is_empty() {
                out.push(word(part, col as f64, row));
            }
            col += part.chars().count() + 1;
        }
        out
    }

    fn page(text: &str) -> Vec<WordBox> {
        text.lines()
            .enumerate()
            .flat_map(|(row, l)| words_of(l, row as f64))
            .collect()
    }

    #[test]
    fn empty_input_gives_empty_text() {
        assert_eq!(reconstruct_text(&[]), "");
        assert_eq!(reconstruct_text(&[word("  ", 0.0, 0.0)]), "");
    }

    #[test]
    fn python_indentation_is_restored() {
        let code = "def f(x):\n    if x > 0:\n        return 1\n    return 0";
        assert_eq!(reconstruct_text(&page(code)), code);
    }

    #[test]
    fn order_and_jitter_do_not_matter() {
        let code = "for i in range(3):\n    print(i)\nprint(\"fertig\")";
        let mut words = page(code);
        words.reverse();
        // A few pixels of noise in x and y.
        for (i, w) in words.iter_mut().enumerate() {
            let d = if i % 2 == 0 { 2.0 } else { -2.0 };
            w.x += d;
            w.y += d;
        }
        assert_eq!(reconstruct_text(&words), code);
    }

    #[test]
    fn uneven_indent_unit_is_normalized_to_four_spaces() {
        // OCR estimated 3.6 characters per level instead of 4.
        let mut words = page("if a:\nb\nc");
        words[2].x += 3.6 * CW;
        words[3].x += 7.2 * CW;
        assert_eq!(reconstruct_text(&words), "if a:\n    b\n        c");
    }

    #[test]
    fn two_space_indentation_is_kept() {
        let code = "a:\n  b:\n    c";
        assert_eq!(reconstruct_text(&page(code)), code);
    }

    #[test]
    fn wide_gaps_keep_their_width() {
        // Tab-separated table / two columns on the same line.
        let words = vec![
            word("Eingabe", 0.0, 0.0),
            word("Ausgabe", 20.0, 0.0),
            word("3", 0.0, 1.0),
            word("9", 20.0, 1.0),
        ];
        assert_eq!(
            reconstruct_text(&words),
            "Eingabe             Ausgabe\n3                   9"
        );
    }

    #[test]
    fn two_columns_stay_side_by_side() {
        let words = vec![
            word("Links", 0.0, 0.0),
            word("Rechts", 40.0, 0.0),
            word("zwei", 0.0, 1.0),
            word("vier", 40.0, 1.0),
        ];
        let text = reconstruct_text(&words);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("Links ") && lines[0].ends_with(" Rechts"));
        assert_eq!(lines[0].find("Rechts"), lines[1].find("vier"));
    }

    #[test]
    fn paragraph_gap_becomes_blank_line() {
        let mut words = page("Aufgabe 1\nText\nCode");
        // Move the last line down by two line pitches.
        words[3].y += 2.0 * LH * 1.4;
        assert_eq!(reconstruct_text(&words), "Aufgabe 1\nText\n\n\nCode");
    }

    #[test]
    fn segments_with_proportional_width_still_indent() {
        // Vision returns whole line segments instead of words.
        let seg = |text: &str, col: f64, row: f64| WordBox {
            width: text.chars().count() as f64 * CW * 1.1,
            ..word(text, col, row)
        };
        let words = vec![
            seg("while n > 1:", 0.0, 0.0),
            seg("n = n // 2", 4.0, 1.0),
            seg("print(n)", 0.0, 2.0),
        ];
        assert_eq!(
            reconstruct_text(&words),
            "while n > 1:\n    n = n // 2\nprint(n)"
        );
    }
}
