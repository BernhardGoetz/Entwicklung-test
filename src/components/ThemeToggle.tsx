import type { ThemePref } from "../theme";

const OPTIONS: { value: ThemePref; label: string; title: string }[] = [
  { value: "system", label: "System", title: "Farbschema des Betriebssystems verwenden" },
  { value: "light", label: "Hell", title: "Immer helles Farbschema" },
  { value: "dark", label: "Dunkel", title: "Immer dunkles Farbschema" },
];

/** Segmented control (native radio buttons: arrow keys work as usual). */
export function ThemeToggle({ value, onChange }: { value: ThemePref; onChange: (p: ThemePref) => void }) {
  return (
    <fieldset className="segmented" aria-label="Farbschema">
      <legend className="sr-only">Farbschema</legend>
      {OPTIONS.map((o) => (
        <label key={o.value} title={o.title} className={value === o.value ? "checked" : undefined}>
          <input
            type="radio"
            name="theme"
            value={o.value}
            checked={value === o.value}
            onChange={() => onChange(o.value)}
          />
          <span>{o.label}</span>
        </label>
      ))}
    </fieldset>
  );
}
