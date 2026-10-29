import React from "react";
import ReactDOM from "react-dom/client";
import "./monacoSetup";
import App from "./App";
import "./styles.css";
import { readStored } from "./theme";

// Apply a stored light/dark choice before the first render (no flash).
const storedTheme = readStored("code-pruefer.theme");
if (storedTheme === "light" || storedTheme === "dark") document.documentElement.dataset.theme = storedTheme;

async function start() {
  // Browser preview without Tauri (npm run dev): mocked backend. The
  // import.meta.env.DEV branch is removed from production builds.
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window) && import.meta.env.VITE_MOCK === "1") {
    await import("./dev/mockBackend").then((m) => m.installMock());
  }
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

void start();
