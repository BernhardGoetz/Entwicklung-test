# Design-System „Code-Prüfer“

Stand: Redesign 2026. Die Werte stehen maschinenlesbar in
[`design-tokens.json`](design-tokens.json) und als CSS-Variablen in
[`src/styles.css`](../../src/styles.css). Eine Vorschau aller Tokens und
Bausteine in Hell und Dunkel zeigt [`design-preview.html`](design-preview.html)
(ohne externe Abhängigkeiten im Browser öffnen).

## 1. Leitlinien

Rückmeldung zum alten Design: „sieht nach Entwicklung von 19xx mit Abrundung
und Farbe aus“. Das neue Design ist deshalb **modern-minimal** (Vorbild
GitHub/Linear):

1. **Der Code steht im Mittelpunkt.** Der Editor nutzt immer die volle Breite;
   alles andere liegt in Tabs darunter.
2. **Fläche statt Kästen.** Keine umrandeten Karten. Abschnitte werden durch
   Weißraum, Überschriften und Haarlinien (1 px) getrennt.
3. **Farbe ist Information.** Genau eine Akzentfarbe (zurückhaltendes Blau)
   für Hauptaktion, Fokus und aktiven Tab. Grün/Rot/Bernstein nur für Status.
   Keine Verläufe, keine farbigen Banner-Hintergründe, keine schweren Schatten.
4. **Status = Punkt + Text.** Zustände (Engine, Tests, Hinweise) werden als
   kleiner farbiger Punkt mit normalem Text gezeigt; die Bedeutung steckt immer
   auch im Text (nie nur in der Farbe).
5. **Ruhige Typografie.** Systemschrift, wenige Größen, klare Gewichte.
6. **Zugänglich.** WCAG 2.2 AA in Hell und Dunkel, vollständige
   Tastaturbedienung, sichtbarer Fokus, `prefers-reduced-motion` wird beachtet.

## 2. Typografie

| Rolle | Token | Wert |
|---|---|---|
| UI-Schrift | `--font-sans` | -apple-system, BlinkMacSystemFont, "Segoe UI Variable", "Segoe UI", system-ui, sans-serif |
| Code-Schrift | `--font-mono` | "Cascadia Code", "JetBrains Mono", Consolas, ui-monospace, monospace |

Größenskala (px): **12 · 13 · 14 · 16 · 18**

| Token | Größe | Verwendung |
|---|---|---|
| `--text-xs` | 12 | Feldbeschriftungen, Metadaten, Badges, Ausgaben (`pre`) |
| `--text-sm` | 13 | Buttons, Eingaben, Kopfzeile, Statuszeilen, Textfelder (Monospace) |
| `--text-md` | 14 | Fließtext, Tabs, Monaco-Editor |
| `--text-lg` | 16 | Abschnittsüberschriften (`h2`), App-Name, Ergebnis-Zusammenfassung |
| `--text-xl` | 18 | reserviert |

Gewichte: 400 (Text), 500 (Buttons, Tabs, Labels), 600 (Überschriften,
Hauptbutton, Statuswörter). Zeilenhöhe 1,5 für Text, 1,3 für Überschriften.
Zahlen in Tab-Badges mit `tabular-nums`.

## 3. Abstände (4/8-px-Raster)

| Token | Wert | typische Verwendung |
|---|---|---|
| `--space-0-5` | 2 px | Innenabstand Segment-Schalter |
| `--space-1` | 4 px | Label ↔ Feld, kleine Lücken |
| `--space-2` | 8 px | Lücke zwischen Bedienelementen |
| `--space-3` | 12 px | Button-Innenabstand, Abschnitt intern |
| `--space-4` | 16 px | Abstand zwischen Testfällen/Ergebnissen; Seitenrand < 900 px |
| `--space-6` | 24 px | Seitenrand (Standard), Abstand Tabs ↔ Inhalt |
| `--space-8` | 32 px | Spaltenabstand, Seitenrand ≥ 1600 px |
| `--space-12` | 48 px | Seitenende, Modal-Abstand oben |

## 4. Radius, Größen, Bewegung

- `--radius-sm` 4 px: Code-Markierungen, Tab-Fokus.
- `--radius-md` **6 px**: Buttons, Eingaben, Editor-Rahmen, Ausgaben.
- `--radius-lg` 8 px: nur das Modal.
- `--radius-full`: Statuspunkte, Tab-Zähler, Badges.
- Bedienelemente 32 px hoch (`--control-height`), kompakt 26 px.
- Statuspunkt 8 px (`--dot-size`).
- Übergänge 120 ms nur für Farbe/Rahmen; „läuft“-Punkte pulsieren sanft,
  bei `prefers-reduced-motion: reduce` nicht.

## 5. Farbrollen

Kontrast gegen den Hintergrund (`bg`) in Klammern.

| Rolle | Token | Hell | Dunkel |
|---|---|---|---|
| Hintergrund | `--color-bg` | `#ffffff` | `#0d1117` |
| dezente Fläche | `--color-surface` | `#f6f8fa` | `#151b23` |
| Fläche Hover / aktiv | `--color-surface-hover` / `-active` | `#eef1f4` / `#e6eaef` | `#1c232c` / `#232a33` |
| Overlay (Modal) | `--color-overlay` | `#ffffff` | `#151b23` |
| Haarlinie | `--color-border` | `#d1d9e0` | `#30363d` |
| Haarlinie leise | `--color-border-muted` | `#e6eaef` | `#21262d` |
| Rahmen Eingabefeld | `--color-border-control` | `#818b98` (3,5:1) | `#656c76` (3,6:1) |
| Text | `--color-text` | `#1f2328` (15,8:1) | `#e6edf3` (16,0:1) |
| Text gedämpft | `--color-text-muted` | `#59636e` (6,1:1) | `#9198a1` (6,5:1) |
| Akzent (Links, Fokus, aktiver Tab) | `--color-accent` | `#0969da` (5,2:1) | `#4493f8` (6,1:1) |
| Akzent-Fläche (Hauptbutton) | `--color-accent-emphasis` | `#0969da` (Weiß 5,2:1) | `#1f6feb` (Weiß 4,6:1) |
| Akzent-Fläche Hover | `--color-accent-emphasis-hover` | `#0757b8` | `#1a5fd0` |
| Akzent dezent | `--color-accent-subtle` | `#ddf4ff` | `#10243e` |
| Erfolg | `--color-success` | `#1a7f37` (5,1:1) | `#3fb950` (7,5:1) |
| Fehler | `--color-danger` | `#d1242f` (5,2:1) | `#f85149` (5,7:1) |
| Warnung | `--color-warning` | `#9a6700` (4,9:1) | `#d29922` (7,5:1) |
| Diff erwartet (−) | `--color-diff-del-bg` | `#ffebe9` | `#3c1e22` |
| Diff tatsächlich (+) | `--color-diff-add-bg` | `#dafbe1` | `#12301c` |
| Markierung im Diff | `--color-diff-del-mark` / `-add-mark` | `#ffcecb` / `#aceebb` | `#6e2a2d` / `#1f5530` |
| Hintergrund Vorschau | `--color-preview-paper` | `#ffffff` | `#ffffff` (Papier bleibt weiß) |

Alle Textfarben erreichen ≥ 4,5:1 auf `bg` und `surface`, auch Text auf den
Diff-Tönungen (≥ 12:1). Rahmen von Eingabefeldern erreichen ≥ 3:1
(WCAG 1.4.11). Haarlinien sind rein dekorativ.

### Farbschema

- Standard: **System** (`prefers-color-scheme`).
- Umschalter in der Kopfzeile: *System · Hell · Dunkel*; gespeichert in
  `localStorage` (`code-pruefer.theme`, Zugriff mit try/catch – ohne Speicher
  gilt die Wahl bis zum Schließen).
- Technik: Tokens hell auf `:root`; dunkel unter
  `@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) }` und
  `:root[data-theme="dark"]`. `color-scheme` wird mitgesetzt (Scrollbalken,
  native Auswahllisten).
- Monaco nutzt eigene Themes `cp-light` (Basis `vs`) und `cp-dark` (Basis
  `vs-dark`) mit Hintergrund, Zeilenmarkierung und Zeilennummern aus den
  Tokens (`src/monacoSetup.ts`).

## 6. Bausteine

### Buttons
- **Primär** (`button.primary`): Akzent-Fläche, weißer Text, 600, min. 96 px.
  Genau eine Hauptaktion pro Ansicht: **„Prüfen“**.
- **Sekundär** (Standard): `surface`, Haarlinie, Text in `--color-text`;
  Hover `surface-hover`.
- **Gefahr** (`button.danger`): sekundär mit roter Schrift; Hover rote
  Tönung + roter Rahmen. Mit `ghost` ohne Fläche (z. B. „Entfernen“).
- **Aufmerksamkeit** (`button.attention`): Akzent-Rahmen + Akzentschrift,
  nur für „KI-Modell einrichten …“ solange kein Modell da ist.
- Deaktiviert: 50 % Deckkraft, Cursor `not-allowed`; Grund steht im Tooltip
  bzw. daneben als gedämpfter Text.

### Eingaben
Hintergrund `bg`, Rahmen `border-control`, Radius 6 px, 32 px hoch;
Textfelder in Monospace 13 px. Fokus: Akzent-Rahmen + 1 px Akzent-Ring.
Ungültig: roter Rahmen + `aria-invalid`. Name eines Testfalls: randlos,
Rahmen erst bei Hover/Fokus.

### Tabs
ARIA `tablist`/`tab`/`tabpanel`, Roving-Tabindex; ←/→ wechseln, Pos1/Ende
springen, Aktivierung sofort. Aktiver Tab: Text in `--color-text` und 2 px
Akzentlinie unten; inaktiv gedämpft. Panels bleiben gemountet (laufende KI,
lokaler Zustand) und werden mit `hidden` ausgeblendet. Der aktive Tab wird
gespeichert (`code-pruefer.tab`).

Tab-Zähler (`.tab-badge`, nur visuell; für Screenreader als Text ergänzt):
- Aufgabe: ✓ geprüft · ● (Bernstein) noch nicht als geprüft markiert · … KI-Korrektur läuft
- Testfälle: Anzahl, während der KI-Erzeugung „n · KI …“
- Ergebnis: „✓ 3/3“ (grün) · „✗ 1/3“ (rot) · „✗“ Kompilierfehler · „!“ Fehler · „…“ läuft

Automatischer Wechsel: nach „Prüfen“ → *Ergebnis*; nach Ende der
KI-Erzeugung → *Testfälle*.

### Statuspunkt, Hinweise
- `.dot` 8 px: grün ok, bernstein Warnung, rot Fehler, blau (pulsierend)
  läuft, grau neutral.
- `.note` / `.hint` / `.banner`: Punkt links, Text in normaler Textfarbe, keine
  Hintergrundfläche. `.warn` bernstein, `.error` rot, `.busy` blau, `.ok` grün.
- Testergebnis (`.status`): Punkt + Statuswort in Statusfarbe
  („Bestanden“, „Falsche Ausgabe“ …).

### Badges
Pillenform, 1 px Haarlinie, 12 px, Text gedämpft. `ai`/`info` (KI,
empfohlen) mit Akzentrahmen; `passed`/`timeout`/`failed` mit 6-px-Punkt.

### Diff und unsichtbare Zeichen
- Zeilen „− erwartet“ auf `diff-del-bg`, „+ tatsächlich“ auf `diff-add-bg`,
  gleiche Zeilen gedämpft. Text bleibt in Textfarbe.
- `·` = Leerzeichen am Zeilenende, `→` = Tab, `⏎` = Zeilenumbruch: gedämpft,
  nicht markierbar (`user-select: none`); die echten Zeichen bleiben
  unsichtbar im DOM und werden beim Kopieren mitgenommen.
- Ausgaben (`pre.output`): `surface`, leise Haarlinie, max. 240 px hoch.

### Modal (KI-Modell)
Einzige erhöhte Fläche: `overlay`, Haarlinie, Radius 8 px,
`--shadow-overlay`, Hintergrund abgedunkelt (`--color-backdrop`).

## 7. Layout

```
┌ Kopfzeile: Code-Prüfer · ● Engine-Status ……… [Images] [Engine ▾] [System|Hell|Dunkel] [KI-Modell …]
│  (Hinweise zur Engine volle Breite darunter, nur bei Bedarf)
├ Werkzeugleiste (klebt oben): Sprache ▾ · Datei hochladen … · main.py ……… Status · [Prüfen]
├ Monaco-Editor – volle Breite, Höhe ≈ 50 % des Fensters (min. 200 px)
├ ═ Ziehgriff (Maus; Tastatur ↑/↓, Pos1/Ende, Enter = Standardhöhe; Doppelklick = Standard)
├ Tabs: Aufgabe ✓ | Testfälle 3 | Ergebnis ✗ 2/3
└ Tab-Inhalt
```

- Kein `max-width`: Code und Inhalte nutzen die ganze Fensterbreite.
- Monaco: `automaticLayout` (ResizeObserver) – passt sich bei jeder
  Größenänderung an. Eine selbst gezogene Höhe wird gespeichert
  (`code-pruefer.editorHeight`) und bei kleinerem Fenster auf max. 85 %
  begrenzt.
- „Prüfen“ ist immer sichtbar: die Werkzeugleiste ist `position: sticky`.

### Breakpoints

| Breite | Verhalten |
|---|---|
| < 800 px | Testfall: Eingabe und erwartete Ausgabe untereinander |
| < 900 px | Seitenrand 16 px; Aufgabe-Text und Vorschau untereinander |
| < 1100 px | Beschriftungen wie „Engine“ und Dateiname nur für Screenreader |
| < 1200 px | Testfälle einspaltig, KI-Bereich oberhalb der Liste |
| ≥ 1200 px | Testfälle \| KI-Bereich (360 px, klebt) nebeneinander |
| < 1500 px | Untertitel und Engine-Erklärung ausgeblendet (Erklärung im Tooltip) |
| ≥ 1600 px | Seitenrand 32 px, KI-Bereich 400 px |

Aufgabe-Tab: Text \| Vorschau des Blatts (3 : 2), ab < 900 px untereinander.
Das Fenster ist ab 720 px Breite nutzbar (`minWidth` in `tauri.conf.json`);
es gibt nie horizontales Scrollen der Seite.

## 8. Begründung

- **Hell = Weiß, Dunkel = fast Schwarz-Blau** wie bei GitHub: neutral,
  lange Arbeit am Code ermüdet nicht, Monaco fügt sich nahtlos ein.
- **Ein Akzent** macht die Hauptaktion eindeutig; Statusfarben bleiben
  dadurch aussagekräftig.
- **Punkt + Text statt farbiger Banner**: Warnungen sind sichtbar, ohne die
  Oberfläche zu „färben“; die Aussage bleibt auch bei Farbfehlsichtigkeit im
  Text erhalten.
- **Tabs statt zwei Spalten**: Der Code bekommt die volle Breite (Wunsch der
  Nutzer), die übrigen Bereiche sind einen Klick entfernt und zeigen ihren
  Zustand bereits im Tab.
- **6 px Radius, Haarlinien, keine Schatten**: zeitgemäß, ruhig, wirkt nicht
  verspielt.
