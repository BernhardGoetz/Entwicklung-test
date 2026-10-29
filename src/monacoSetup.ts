// Bundle Monaco locally (no CDN) – the app must work offline.
import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import { loader } from "@monaco-editor/react";

self.MonacoEnvironment = {
  getWorker: () => new EditorWorker(),
};

loader.config({ monaco });

// Editor themes matching the app tokens (docs/design/design-tokens.json).
monaco.editor.defineTheme("cp-light", {
  base: "vs",
  inherit: true,
  rules: [],
  colors: {
    "editor.background": "#ffffff",
    "editor.lineHighlightBackground": "#f6f8fa",
    "editor.lineHighlightBorder": "#00000000",
    "editorLineNumber.foreground": "#818b98",
    "editorLineNumber.activeForeground": "#1f2328",
    "editorGutter.background": "#ffffff",
    "editor.selectionBackground": "#b6d7ff",
    "editorCursor.foreground": "#0969da",
    "editorWidget.border": "#d1d9e0",
  },
});
monaco.editor.defineTheme("cp-dark", {
  base: "vs-dark",
  inherit: true,
  rules: [],
  colors: {
    "editor.background": "#0d1117",
    "editor.lineHighlightBackground": "#151b23",
    "editor.lineHighlightBorder": "#00000000",
    "editorLineNumber.foreground": "#656c76",
    "editorLineNumber.activeForeground": "#e6edf3",
    "editorGutter.background": "#0d1117",
    "editor.selectionBackground": "#264f78",
    "editorCursor.foreground": "#4493f8",
    "editorWidget.border": "#30363d",
  },
});
