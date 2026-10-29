import type { DiffLine } from "../api";
import { NewlineMark, VisibleLine } from "./VisibleWhitespace";

const PREFIX = { equal: "  ", expected: "- ", actual: "+ " } as const;

export function DiffView({ diff }: { diff: DiffLine[] }) {
  if (diff.length === 0) return null;
  return (
    <div className="diff">
      <div className="diff-legend small">
        <span className="diff-expected">- erwartet</span>{" "}
        <span className="diff-actual">+ tatsächlich</span>{" "}
        <span className="muted">· Leerzeichen am Zeilenende, ⏎ Zeilenumbruch</span>
      </div>
      <pre>
        {diff.map((l, i) => (
          <div key={i} className={`diff-${l.kind}`}>
            {PREFIX[l.kind]}
            <VisibleLine text={l.text} />
            {/* An empty line, or the line break at the very end of the output. */}
            {(l.text === "" || l.finalNewline) && <NewlineMark />}
          </div>
        ))}
      </pre>
    </div>
  );
}
