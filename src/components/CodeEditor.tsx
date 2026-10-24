import Editor from "@monaco-editor/react";
import { useRef, useState } from "react";
import type { LanguageInfo } from "../api";

interface Props {
  language: LanguageInfo | undefined;
  code: string;
  onChange: (code: string) => void;
}

const MAX_FILE_BYTES = 1024 * 1024;

export function CodeEditor({ language, code, onChange }: Props) {
  const fileInput = useRef<HTMLInputElement>(null);
  const [error, setError] = useState<string | null>(null);
  const exts = language?.extensions ?? [];

  const onFile = async (file: File | undefined) => {
    setError(null);
    if (!file) return;
    const ext = file.name.split(".").pop()?.toLowerCase() ?? "";
    if (!exts.includes(ext)) {
      setError(`Nur ${exts.map((e) => "." + e).join(", ")}-Dateien sind für ${language?.name} erlaubt.`);
      return;
    }
    if (file.size > MAX_FILE_BYTES) {
      setError("Datei ist zu groß (max. 1 MB).");
      return;
    }
    onChange((await file.text()).replace(/\r\n?/g, "\n"));
  };

  return (
    <section className="card editor-card">
      <div className="card-header">
        <h2>Code</h2>
        <div className="row">
          {language && <span className="muted">Datei im Container: {language.sourceFile}</span>}
          <button onClick={() => fileInput.current?.click()}>Datei hochladen …</button>
          <input
            ref={fileInput}
            type="file"
            accept={exts.map((e) => "." + e).join(",")}
            hidden
            onChange={(e) => {
              void onFile(e.target.files?.[0]);
              e.target.value = "";
            }}
          />
        </div>
      </div>
      {language?.id === "java" && (
        <p className="muted small">
          Hinweis: Die Klasse mit der <code>main</code>-Methode muss <code>Main</code> heißen.
        </p>
      )}
      {error && <p className="error">{error}</p>}
      <div className="editor">
        <Editor
          height="100%"
          language={language?.id ?? "plaintext"}
          value={code}
          onChange={(v) => onChange(v ?? "")}
          options={{
            minimap: { enabled: false },
            fontSize: 14,
            scrollBeyondLastLine: false,
            automaticLayout: true,
            tabSize: 4,
          }}
        />
      </div>
    </section>
  );
}
