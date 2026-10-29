import { useEffect, useState } from "react";

/** Color scheme chosen in the app: follow the OS, or fixed light/dark. */
export type ThemePref = "system" | "light" | "dark";
export type Theme = "light" | "dark";

const KEY = "code-pruefer.theme";

/** localStorage can be unavailable (private mode, blocked storage). */
export function readStored(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function writeStored(key: string, value: string | null): void {
  try {
    if (value === null) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch {
    // Not stored – the choice still applies until the app is closed.
  }
}

function initialPref(): ThemePref {
  const v = readStored(KEY);
  return v === "light" || v === "dark" ? v : "system";
}

const darkQuery = () => window.matchMedia?.("(prefers-color-scheme: dark)");

/**
 * Theme preference (stored) and the effective theme. "system" removes
 * data-theme, so the CSS media query decides; light/dark set it.
 */
export function useTheme(): { pref: ThemePref; setPref: (p: ThemePref) => void; theme: Theme } {
  const [pref, setPrefState] = useState<ThemePref>(initialPref);
  const [systemDark, setSystemDark] = useState(() => darkQuery()?.matches ?? false);

  useEffect(() => {
    const q = darkQuery();
    if (!q) return;
    const onChange = (e: MediaQueryListEvent) => setSystemDark(e.matches);
    q.addEventListener("change", onChange);
    return () => q.removeEventListener("change", onChange);
  }, []);

  useEffect(() => {
    const root = document.documentElement;
    if (pref === "system") delete root.dataset.theme;
    else root.dataset.theme = pref;
  }, [pref]);

  const setPref = (p: ThemePref) => {
    setPrefState(p);
    writeStored(KEY, p === "system" ? null : p);
  };

  const theme: Theme = pref === "system" ? (systemDark ? "dark" : "light") : pref;
  return { pref, setPref, theme };
}
