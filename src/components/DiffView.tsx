import type { DiffLine } from "../api";

const PREFIX = { equal: "  ", expected: "- ", actual: "+ " } as const;

export function DiffView({ diff }: { diff: DiffLine[] }) {
  if (diff.length === 0) return null;
  return (
    <div className="diff">
      <div className="diff-legend small">
        <span className="diff-expected">- erwartet</span>{" "}
        <span className="diff-actual">+ tatsächlich</span>
      </div>
      <pre>
        {diff.map((l, i) => (
          <div key={i} className={`diff-${l.kind}`}>
            {PREFIX[l.kind]}
            {l.text === "" ? "⏎" : l.text}
          </div>
        ))}
      </pre>
    </div>
  );
}
