import { useRef, type KeyboardEvent, type ReactNode } from "react";

export type StatusTone = "neutral" | "ok" | "fail" | "warn" | "busy";

export interface TabDef<K extends string> {
  id: K;
  label: string;
  /** Short visible status, e.g. "3" or "✓ 3/3" (aria-hidden). */
  badge?: string;
  /** Same status for screen readers, e.g. "3 von 3 bestanden". */
  badgeText?: string;
  tone?: StatusTone;
}

interface Props<K extends string> {
  tabs: TabDef<K>[];
  active: K;
  onChange: (id: K) => void;
  label: string;
  /** Right-aligned content in the tab bar (optional). */
  end?: ReactNode;
}

export const tabId = (id: string) => `tab-${id}`;
export const panelId = (id: string) => `panel-${id}`;

/** ARIA tablist with roving tabindex; arrow keys, Home and End switch tabs. */
export function TabBar<K extends string>({ tabs, active, onChange, label, end }: Props<K>) {
  const refs = useRef<Record<string, HTMLButtonElement | null>>({});

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, index: number) => {
    let next: number | null = null;
    if (e.key === "ArrowRight") next = (index + 1) % tabs.length;
    else if (e.key === "ArrowLeft") next = (index - 1 + tabs.length) % tabs.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = tabs.length - 1;
    if (next === null) return;
    e.preventDefault();
    const id = tabs[next].id;
    onChange(id);
    refs.current[id]?.focus();
  };

  return (
    <div className="tabbar">
      <div role="tablist" aria-label={label} className="tabs">
        {tabs.map((t, i) => {
          const selected = t.id === active;
          return (
            <button
              key={t.id}
              ref={(el) => {
                refs.current[t.id] = el;
              }}
              role="tab"
              type="button"
              id={tabId(t.id)}
              aria-selected={selected}
              aria-controls={panelId(t.id)}
              tabIndex={selected ? 0 : -1}
              className="tab"
              onClick={() => onChange(t.id)}
              onKeyDown={(e) => onKeyDown(e, i)}
            >
              <span>{t.label}</span>
              {t.badge && (
                <>
                  <span className={`tab-badge tone-${t.tone ?? "neutral"}`} aria-hidden="true">
                    {t.badge}
                  </span>
                  {t.badgeText && <span className="sr-only">, {t.badgeText}</span>}
                </>
              )}
            </button>
          );
        })}
      </div>
      {end}
    </div>
  );
}

/** Panels stay mounted (running AI jobs, local state); inactive ones are hidden. */
export function TabPanel({ id, active, children }: { id: string; active: boolean; children: ReactNode }) {
  return (
    <div role="tabpanel" id={panelId(id)} aria-labelledby={tabId(id)} hidden={!active} tabIndex={0} className="tabpanel">
      {children}
    </div>
  );
}
