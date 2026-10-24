import type { LanguageInfo } from "../api";

interface Props {
  languages: LanguageInfo[];
  value: string;
  onChange: (id: string) => void;
  disabled?: boolean;
}

export function LanguageSelect({ languages, value, onChange, disabled }: Props) {
  return (
    <label className="inline">
      Sprache:{" "}
      <select value={value} onChange={(e) => onChange(e.target.value)} disabled={disabled}>
        {languages.map((l) => (
          <option key={l.id} value={l.id}>
            {l.name}
          </option>
        ))}
      </select>
    </label>
  );
}
