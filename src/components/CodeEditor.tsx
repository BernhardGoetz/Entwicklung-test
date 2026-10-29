import Editor from "@monaco-editor/react";
import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import type { LanguageInfo } from "../api";
import { readStored, writeStored, type Theme } from "../theme";

interface Props {
  language: LanguageInfo | undefined;
  code: string;
  onChange: (code: string) => void;
  theme: Theme;
  /** Left part of the toolbar (language selection). */
  toolbarStart?: ReactNode;
  /** Right part of the toolbar ("Prüfen" and its status). */
  toolbarEnd?: ReactNode;
}

const MAX_FILE_BYTES = 1024 * 1024;
const HEIGHT_KEY = "code-pruefer.editorHeight";
export const MIN_EDITOR_HEIGHT = 200;
const STEP = 24;

const maxHeight = () => Math.max(MIN_EDITOR_HEIGHT, Math.round(window.innerHeight * 0.85));
const clamp = (h: number) => Math.min(maxHeight(), Math.max(MIN_EDITOR_HEIGHT, Math.round(h)));

function storedHeight(): number | null {
  const v = Number(readStored(HEIGHT_KEY));
  return Number.isFinite(v) && v >= MIN_EDITOR_HEIGHT ? v : null;
}

/** Default height (CSS: about half the window) or the height set with the drag handle. */
function useEditorHeight() {
  const [height, setHeightState] = useState<number | null>(storedHeight);
  const setHeight = (h: number | null) => {
    const v = h === null ? null : clamp(h);
    setHeightState(v);
    writeStored(HEIGHT_KEY, v === null ? null : String(v));
  };
  // Keep a stored height inside the window when it gets smaller.
  useEffect(() => {
    const onResize = () => setHeightState((h) => (h === null ? null : clamp(h)));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return [height, setHeight] as const;
}

export function CodeEditor({ language, code, onChange, theme, toolbarStart, toolbarEnd }: Props) {
  const fileInput = useRef<HTMLInputElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const drag = useRef<{ startY: number; startH: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [height, setHeight] = useEditorHeight();
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

  const currentHeight = () => height ?? box.current?.getBoundingClientRect().height ?? 400;

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { startY: e.clientY, startH: currentHeight() };
    document.body.classList.add("resizing");
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    setHeight(drag.current.startH + e.clientY - drag.current.startY);
  };
  const endDrag = () => {
    drag.current = null;
    document.body.classList.remove("resizing");
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const h = currentHeight();
    if (e.key === "ArrowUp") setHeight(h - STEP);
    else if (e.key === "ArrowDown") setHeight(h + STEP);
    else if (e.key === "Home") setHeight(MIN_EDITOR_HEIGHT);
    else if (e.key === "End") setHeight(maxHeight());
    else if (e.key === "Enter" || e.key === "Escape") setHeight(null);
    else return;
    e.preventDefault();
  };

  return (
    <section className="code" aria-labelledby="code-heading">
      <h2 id="code-heading" className="sr-only">
        Code
      </h2>
      <div className="code-toolbar">
        <div className="toolbar-group">
          {toolbarStart}
          <button onClick={() => fileInput.current?.click()} title="Quelltext-Datei in den Editor laden">
            Datei hochladen …
          </button>
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
          {language && (
            <span className="muted small hide-narrow" title="So heißt die Datei im Container">
              {language.sourceFile}
            </span>
          )}
        </div>
        <div className="toolbar-group end">{toolbarEnd}</div>
      </div>
      {(language?.id === "java" || error) && (
        <div className="code-notes small">
          {language?.id === "java" && (
            <span className="muted">
              Hinweis: Die Klasse mit der <code>main</code>-Methode muss <code>Main</code> heißen.
            </span>
          )}
          {error && <span className="note error">{error}</span>}
        </div>
      )}
      <div className="editor" id="code-editor" ref={box} style={height === null ? undefined : { height }}>
        <Editor
          height="100%"
          language={language?.id ?? "plaintext"}
          value={code}
          theme={theme === "dark" ? "cp-dark" : "cp-light"}
          onChange={(v) => onChange(v ?? "")}
          options={{
            minimap: { enabled: false },
            fontSize: 14,
            fontFamily: '"Cascadia Code", "JetBrains Mono", Consolas, ui-monospace, monospace',
            lineHeight: 21,
            padding: { top: 12, bottom: 12 },
            scrollBeyondLastLine: false,
            automaticLayout: true,
            renderLineHighlight: "line",
            overviewRulerBorder: false,
            hideCursorInOverviewRuler: true,
            tabSize: 4,
          }}
        />
      </div>
      <div
        className="resizer"
        role="separator"
        aria-orientation="horizontal"
        aria-label="Höhe des Code-Editors"
        aria-controls="code-editor"
        aria-valuemin={MIN_EDITOR_HEIGHT}
        aria-valuemax={maxHeight()}
        aria-valuenow={Math.round(height ?? box.current?.getBoundingClientRect().height ?? 0) || undefined}
        tabIndex={0}
        title="Ziehen oder Pfeiltasten: Höhe ändern · Doppelklick oder Enter: Standardhöhe"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onLostPointerCapture={endDrag}
        onDoubleClick={() => setHeight(null)}
        onKeyDown={onKeyDown}
      >
        <span className="resizer-grip" aria-hidden="true" />
      </div>
    </section>
  );
}
