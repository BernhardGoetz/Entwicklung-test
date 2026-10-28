import { useState } from "react";
import { MAX_TOLERANCE, type CompareMode } from "../api";

interface Props {
  /** null while the entered tolerance is invalid. */
  value: CompareMode | null;
  onChange: (mode: CompareMode | null) => void;
  disabled?: boolean;
}

type Kind = CompareMode["kind"];

const DEFAULT_TOLERANCE = "1e-6";

/** Parses a tolerance ("0.001", "1e-6", "0,001"); null if invalid. */
export function parseTolerance(text: string): number | null {
  const t = text.trim().replace(",", ".");
  if (t === "") return null;
  const v = Number(t);
  return Number.isFinite(v) && v >= 0 && v <= MAX_TOLERANCE ? v : null;
}

/** "Vergleich: …" – how outputs are compared (see README, Ausgabevergleich). */
export function CompareModeSelect({ value, onChange, disabled }: Props) {
  const [kind, setKind] = useState<Kind>(value?.kind ?? "lines");
  const [trim, setTrim] = useState(value?.kind === "lines" ? value.trim : true);
  const [tolText, setTolText] = useState(
    value?.kind === "float" ? String(value.absTol) : DEFAULT_TOLERANCE,
  );

  const emit = (k: Kind, t: boolean, tol: string) => {
    if (k === "lines") onChange({ kind: "lines", trim: t });
    else if (k === "tokens") onChange({ kind: "tokens", caseSensitive: true });
    else {
      const v = parseTolerance(tol);
      // Like Kattis' float_tolerance: absolute or relative deviation.
      onChange(v === null ? null : { kind: "float", absTol: v, relTol: v });
    }
  };

  const tolInvalid = kind === "float" && parseTolerance(tolText) === null;

  return (
    <div className="compare-mode small">
      <label className="inline">
        Vergleich:
        <select
          value={kind}
          disabled={disabled}
          onChange={(e) => {
            const k = e.target.value as Kind;
            setKind(k);
            emit(k, trim, tolText);
          }}
        >
          <option value="lines">Zeilen (exakt)</option>
          <option value="tokens">Wörter (Leerraum egal)</option>
          <option value="float">Zahlen mit Toleranz</option>
        </select>
      </label>
      {kind === "lines" && (
        <label className="inline">
          <input
            type="checkbox"
            checked={trim}
            disabled={disabled}
            onChange={(e) => {
              setTrim(e.target.checked);
              emit(kind, e.target.checked, tolText);
            }}
          />{" "}
          Leerzeichen am Zeilenende und Leerzeilen am Ende ignorieren
        </label>
      )}
      {kind === "float" && (
        <label className="inline" title="Erlaubte absolute oder relative Abweichung, z. B. 1e-6 oder 0,001">
          Toleranz
          <input
            type="text"
            className={tolInvalid ? "tolerance invalid" : "tolerance"}
            value={tolText}
            disabled={disabled}
            aria-invalid={tolInvalid}
            onChange={(e) => {
              setTolText(e.target.value);
              emit(kind, trim, e.target.value);
            }}
          />
        </label>
      )}
      {kind === "tokens" && (
        <span className="muted">Leerzeichen und Zeilenumbrüche zählen nicht, Groß-/Kleinschreibung schon.</span>
      )}
      {kind === "float" && (
        <span className={tolInvalid ? "error" : "muted"}>
          {tolInvalid
            ? "Ungültige Toleranz – erlaubt sind Zahlen von 0 bis 1000000."
            : "Zahlen dürfen um die Toleranz abweichen (absolut oder relativ), Text muss exakt passen."}
        </span>
      )}
    </div>
  );
}
