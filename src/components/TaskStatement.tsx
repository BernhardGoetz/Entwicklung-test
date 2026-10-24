interface Props {
  value: string;
  onChange: (v: string) => void;
}

export function TaskStatement({ value, onChange }: Props) {
  return (
    <section className="card">
      <h2>Aufgabenstellung</h2>
      <textarea
        className="task"
        placeholder="Aufgabenstellung hier einfügen (nur zur Orientierung, wird nicht automatisch ausgewertet) …"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        rows={5}
      />
    </section>
  );
}
