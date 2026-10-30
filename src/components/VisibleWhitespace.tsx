import { Fragment } from "react";

/** CRLF and lone CR become LF, like normalize_newlines in src-tauri/src/runner/compare.rs. */
export const normalizeNewlines = (s: string): string => s.replace(/\r\n?/g, "\n");

/** Whether the text ends with a line break (CRLF/CR count as well). */
export const endsWithNewline = (s: string): boolean => /[\r\n]$/.test(s);

/** Splits a line into its content and its trailing spaces/tabs. */
function splitTrailing(line: string): [string, string] {
  const m = /[ \t]+$/.exec(line);
  return m ? [line.slice(0, m.index), m[0]] : [line, ""];
}

/** 1-based numbers of the lines that end with spaces or tabs. */
export function linesWithTrailingSpace(s: string): number[] {
  const out: number[] = [];
  normalizeNewlines(s)
    .split("\n")
    .forEach((l, i) => {
      if (splitTrailing(l)[1] !== "") out.push(i + 1);
    });
  return out;
}

/**
 * German status line for an expected output: line break at the end and
 * lines with trailing spaces (the textarea itself shows neither). Empty
 * text gives "".
 */
export function endingStatus(s: string): string {
  if (s === "") return "";
  const parts = [endsWithNewline(s) ? "endet mit Zeilenumbruch ⏎" : "ohne Zeilenumbruch am Ende"];
  const lines = linesWithTrailingSpace(s);
  if (lines.length > 0) {
    const shown = lines.slice(0, 5).join(", ");
    const more = lines.length > 5 ? ` … (${lines.length} Zeilen)` : "";
    parts.push(`Leerzeichen am Zeilenende in Zeile ${shown}${more}`);
  }
  return parts.join(" · ");
}

/**
 * Trailing whitespace shown as markers (· for spaces, → for tabs). The real
 * characters stay in the DOM (visually hidden) so copying gives the
 * original text; the markers themselves are not selectable.
 */
function Trailing({ ws }: { ws: string }) {
  if (ws === "") return null;
  return (
    <span className="ws" title="Leerzeichen am Zeilenende">
      <span className="ws-real">{ws}</span>
      <span className="ws-mark" aria-hidden="true">
        {ws.replace(/ /g, "·").replace(/\t/g, "→")}
      </span>
    </span>
  );
}

/** ⏎ marker for a line break (not selectable). */
export function NewlineMark() {
  return (
    <span className="ws-mark" aria-hidden="true" title="Zeilenumbruch">
      ⏎
    </span>
  );
}

/** One line (without its line break) with visible trailing whitespace. */
export function VisibleLine({ text }: { text: string }) {
  const [body, ws] = splitTrailing(text);
  return (
    <>
      {body}
      <Trailing ws={ws} />
    </>
  );
}

/** Text with visible trailing whitespace per line and a visible final line break. */
export function VisibleWhitespace({ text }: { text: string }) {
  const t = normalizeNewlines(text);
  const finalNewline = t.endsWith("\n");
  const lines = (finalNewline ? t.slice(0, -1) : t).split("\n");
  return (
    <>
      {lines.map((l, i) => (
        <Fragment key={i}>
          <VisibleLine text={l} />
          {i < lines.length - 1 && "\n"}
        </Fragment>
      ))}
      {finalNewline && (
        <>
          <NewlineMark />
          {"\n"}
        </>
      )}
    </>
  );
}
