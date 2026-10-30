# Entscheidungsgrundlage: Lokale Offline-KI für einen Programmieraufgaben-Checker

> **Zweck dieses Dokuments:** Kontext und Best Practices für Claude Code zur Architektur- und Technologieentscheidung.
> **Stand der Recherche:** Oktober 2026. Modell- und Toolversionen ändern sich monatlich. Vor jeder Festlegung die aktuellen Versionen, Lizenzen und Benchmarks erneut prüfen.
> **Kennzeichnung:** **[FAKT]** = durch Quelle belegt · **[EINSCHÄTZUNG]** = abgeleitete Bewertung, nicht gemessen · **[OFFEN]** = nicht verifiziert, muss vor Entscheidung geprüft werden.

---

## 1. Projektziel und harte Anforderungen

### Funktion
1. Der Nutzer lädt eine Programmieraufgabe hoch: Bild (Foto/Screenshot), Word (.docx), PDF (digital oder gescannt).
2. Lokale Texterkennung bzw. lokales Dokument-Parsing extrahiert den Aufgabentext.
3. Eine lokale KI interpretiert die Aufgabe und erzeugt **bei jeder Aufgabe live** eine Musterlösung.
4. Der Nutzer gibt eigenen Code ein. Das Programm prüft, ob sein Code dieselbe Ausgabe liefert wie erwartet.

### Harte Randbedingungen (nicht verhandelbar)
| Anforderung | Wert |
|---|---|
| Plattformen | Windows; macOS Intel **und** Apple Silicon |
| Netzwerk zur Laufzeit | **Keine** Verbindung zu Claude, OpenAI oder anderen Cloud-Diensten |
| Hardware | Standard-Office-PC, **keine dedizierte GPU**, möglichst geringer RAM-Bedarf |
| Sprachen (Nutzercode) | Zuerst **Java**, später alle an Unis gängigen Sprachen (Python, C, C++ …) |
| Musterlösung | Wird bei jeder Aufgabe live erzeugt, nicht vorab hinterlegt |

---

## 2. Zentrale Designentscheidung (wichtigste Best Practice)

**Das lokale Modell ist unzuverlässig. Die Korrektheit muss deshalb durch das Programm selbst abgesichert werden, nicht durch das Modell.**

- **[FAKT]** Kleine Code-Modelle erreichen auf kurzen Einzelfunktions-Benchmarks gute Werte, auf schwereren Aufgaben deutlich schlechtere (siehe Abschnitt 4).
- **[EINSCHÄTZUNG]** Wenn eine falsche Musterlösung als Referenz dient, wird korrekter Nutzercode als falsch bewertet. Das schadet Lernenden direkt.

### Daraus folgende Regeln
1. **Erwartete Ausgaben niemals direkt vom Modell erzeugen lassen.** LLMs „rechnen“ Ausgaben nicht zuverlässig. Das Modell erzeugt nur **Eingaben**. Die erwarteten Ausgaben entstehen durch **Ausführen** einer verifizierten Musterlösung.
2. **Jede Musterlösung kompilieren und gegen die Beispiele aus dem Aufgabenblatt testen**, bevor sie als Referenz gilt.
3. **Mehrere Lösungen erzeugen und per Mehrheitsentscheid über die Ausgaben auswählen** (Prinzip „CodeT – Dual Execution Agreement“).
4. **Den extrahierten Aufgabentext immer vom Nutzer bestätigen lassen**, bevor die KI arbeitet. OCR-Fehler in Konstanten oder Beispielwerten machen alles Weitere wertlos.
5. **Eine Konfidenz anzeigen und bei niedriger Konfidenz nicht bewerten.**

---

## 3. Empfohlene Pipeline pro Aufgabe

```
[Upload] → [Extraktion] → [Nutzer bestätigt Text] → [LLM: Struktur als JSON]
   → [LLM: N Lösungen] → [Kompilieren + Reparaturrunde] → [Test gegen Blatt-Beispiele]
   → [LLM: nur Testeingaben] → [Mehrheitsentscheid → erwartete Ausgaben]
   → [Konfidenz grün/gelb/rot] → [Nutzercode in Sandbox prüfen]
```

1. **Import**
   - DOCX → Apache POI
   - Digitales PDF → Apache PDFBox
   - Gescanntes PDF / Bild → Seite rendern → OCR (siehe Abschnitt 6)
2. **Bestätigung:** Original und extrahierten Text nebeneinander anzeigen, Nutzer korrigiert.
3. **Strukturierung durch das LLM** mit grammatik-gebundener JSON-Ausgabe (llama.cpp unterstützt Grammatiken/JSON-Schema):
   - Ein-/Ausgabeformat (stdin/stdout oder Methodensignatur)
   - **wörtlich übernommene** Beispiel-Ein-/Ausgaben aus dem Blatt
   - Vergleichsmodus (exakt / tokenweise / Gleitkomma-Toleranz / Reihenfolge egal)
4. **Lösungsgenerierung:** N = 3–5 Samples, Temperatur ca. 0,6–0,8 **[EINSCHÄTZUNG]**; N nach Hardwarestufe anpassen.
5. **Selbstprüfung**
   - Kompilieren mit `javax.tools.JavaCompiler` aus dem gebündelten JDK
   - Bei Compilerfehlern: Fehlermeldung an das Modell zurückgeben, max. 1–2 Reparaturrunden
   - Gegen Beispiele aus dem Blatt ausführen; durchgefallene Lösungen verwerfen
6. **Testeingaben:** Modell erzeugt nur Eingaben inkl. Grenzfällen. Alle überlebenden Lösungen laufen darauf. Die Ausgaben der größten übereinstimmenden Gruppe werden zum Erwartungswert.
7. **Konfidenz**
   - **Grün:** Blatt-Beispiele bestanden **und** Mehrheit ≥ 80 %
   - **Gelb:** keine Beispiele im Blatt **oder** knappe Mehrheit → Ergebnis nur als Hinweis
   - **Rot:** keine Lösung besteht die Beispiele → **keine** Bewertung, ehrlicher Hinweis an den Nutzer
8. **Nutzercode prüfen:** gleiche Sandbox, gleicher Vergleich. Bei Abweichung konkrete Eingabe, erwartete und tatsächliche Ausgabe anzeigen.

---

## 4. Modellauswahl

### Benchmarks (aus Herstellerberichten / Model Cards)
| Modell | Lizenz | Kennzahlen | Rolle |
|---|---|---|---|
| Qwen2.5-Coder-1.5B-Instruct | Apache 2.0 | **[FAKT]** MultiPL-E Java 55,7 % | Stufe S (8 GB RAM) |
| Qwen2.5-Coder-7B-Instruct | Apache 2.0 | **[FAKT]** MultiPL-E Java 76,5 %; HumanEval 88,4 % | Stufe M (16 GB) |
| Qwen3.5-4B (multimodal) | Apache 2.0 | **[FAKT]** LiveCodeBench v6 55,8; OCRBench 85,0 | Alternative Stufe M |
| Qwen3.5-9B (multimodal) | Apache 2.0 | **[FAKT]** LiveCodeBench v6 65,6 | Stufe L (≥ 32 GB / Apple Silicon) |
| Qwen3-Coder-30B-A3B-Instruct (MoE) | Apache 2.0 | **[FAKT]** 30,5B gesamt, 3,3B aktiv | Stufe L |
| ~~Qwen2.5-Coder-3B~~ | **Research License, nicht kommerziell** | — | **AUSSCHLIESSEN** |

### Wichtige Einordnung
- **[FAKT]** HumanEval/MultiPL-E bestehen aus kurzen Einzelfunktionen. 76 % dort heißt **nicht**, dass 76 % der Uni-Übungsblätter korrekt gelöst werden.
- **[FAKT]** Auf Wettbewerbsaufgaben (LiveCodeBench) ist Qwen2.5-Coder-7B deutlich schwächer. Der technische Bericht nennt widersprüchliche Werte (18,2 in Tabelle 16 vs. 37,6 im Fließtext).
- **[EINSCHÄTZUNG]** Die Qwen3.5-LiveCodeBench-Werte stammen wahrscheinlich aus dem „Thinking“-Modus mit langen Denkketten, der auf der CPU sehr langsam ist.
- **[OFFEN]** Ob Qwen3.5-4B **ohne Thinking** bei Java-Übungsaufgaben besser ist als Qwen2.5-Coder-7B, ist nicht belegt. Mit eigenem Testsatz messen.
- **[OFFEN]** Phi-, Granite-Code- und StarCoder2-Modelle wurden nicht auf Java-Werte geprüft.

### Best Practice
- Modelle als **austauschbare GGUF-Dateien** behandeln. Die Architektur ist wichtiger als die konkrete Modellwahl.
- Quantisierung Q4_K_M (Stufe M/L) bzw. Q5 (Stufe S) **[EINSCHÄTZUNG]**.
- Hardware beim Start erkennen (RAM, Apple Silicon, AVX2) und passende Stufe vorschlagen.

---

## 5. Geschwindigkeit und Latenz

- **[FAKT]** Auf der CPU begrenzt die **Speicherbandbreite** das Tempo, nicht die Rechenleistung. Pro Token wird das ganze Modell einmal aus dem RAM gelesen.
- **[FAKT, Community-Messungen, nicht unabhängig geprüft]**
  - 7B Q4 auf DDR4-Laptop: ca. 5–8 Token/s; DDR5-Desktop: ca. 11–14 Token/s
  - Gewöhnlicher i5-Laptop: < 2B: 18–36 Token/s; 3–4B: 7–10 Token/s; 7–8B: 3–4 Token/s
  - Qwen3.5-4B auf Ryzen-5-Laptop: ca. 11 Token/s
  - Qwen3.5-9B auf Apple M4 (24 GB): ca. 10 Token/s, bei großen Prompts lange Zeit bis zum ersten Token
- **[FAKT]** 7B Q4_K_M: Datei ca. 4–4,7 GB, ca. 6 GB RAM-Bedarf. Faustregel: Auf 16 GB ist 7B Q4 das praktische Limit.

| Konfiguration | Geschätzte Dauer **[EINSCHÄTZUNG]** |
|---|---|
| 1 Lösung (300–600 Token), 7B, DDR4-Laptop | 1–2 min |
| 5 Lösungen, 7B, DDR4-Laptop | 5–10 min → kaum interaktiv |
| 1 Lösung, 1.5B–4B | 15–60 s |

**Konsequenz:** Auf schwacher Hardware kleinere Modelle mit mehr Samples bevorzugen oder N reduzieren. Fortschritt im UI anzeigen und Abbruch ermöglichen.

---

## 6. Texterkennung und Dokument-Parsing

| Quelle | Empfehlung | Hinweis |
|---|---|---|
| DOCX | Apache POI | nativ in Java, keine OCR nötig |
| Digitales PDF | Apache PDFBox | Formeln werden oft zerrissen |
| Bild / Scan (macOS) | Apple Vision `VNRecognizeTextRequest`, Modus `.accurate` | **Sprachkorrektur für Code abschalten** **[EINSCHÄTZUNG]** |
| Bild / Scan (Windows) | `Windows.Media.Ocr` | nur installierte Sprachpakete; max. Bildgröße beachten; Aufruf aus Java z. B. über Hilfsprozess |
| Fallback beide | Tesseract (Apache 2.0) | **gibt Einrückung nicht aus** → aus Bounding-Boxes rekonstruieren (für Python zwingend) |
| Schwierige Seiten | Granite-Docling-258M (Apache 2.0) | **[FAKT]** Code-Erkennung F1 0,988 laut Model Card; optimiert für Dokumente, nicht für Handyfotos |
| Komplettlösung | Docling (MIT) | Python-Paket → eingebettete Python-Laufzeit nötig |

- **[EINSCHÄTZUNG]** Vision-Language-Modelle (z. B. Qwen3.5) können OCR und Interpretation in einem Schritt, sind auf der CPU aber teuer und schwer überprüfbar. **Empfehlung: zweistufig** (erst Text extrahieren und bestätigen lassen, dann Code erzeugen), damit jeder Fehler einer Stufe zuzuordnen ist.
- **[EINSCHÄTZUNG]** Handyfotos mit Perspektive und Schatten bleiben fehleranfällig. Die Bestätigung durch den Nutzer ist deshalb Pflicht.

---

## 7. Inferenz-Laufzeit

| Option | Lizenz | Bewertung |
|---|---|---|
| **llama.cpp `llama-server` als Sidecar-Prozess** | MIT | **Empfohlen.** Absturzisolation, Abbruch per Prozess-Kill, Metal auf Apple Silicon, CPU sonst |
| java-llama.cpp (`de.kherud:llama`, JNI) | MIT | Alternative im selben Prozess; Native-Absturz reißt die App mit |
| Ollama | MIT | Weiterverteilbar, aber eigener Daemon und Modellspeicher → für eingebettete App unnötig schwer **[EINSCHÄTZUNG]** |
| LM Studio | proprietär | **Ausgeschlossen** – Weiterverteilung nicht erlaubt |
| MLX | — | nur Apple Silicon, deckt Windows/Intel-Mac nicht ab |
| ONNX Runtime GenAI, OpenVINO, llamafile | — | **[OFFEN]** nicht im Detail geprüft |

**Best Practice:** Nur **einen** Inferenzpfad (llama.cpp) für alle Plattformen. Ein zweiter Pfad verdoppelt den Testaufwand.

---

## 8. Sichere Ausführung von Nutzer- und generiertem Code

- **[FAKT]** Der Java Security Manager ist seit **JDK 24 dauerhaft deaktiviert** (JEP 486). Isolation innerhalb der JVM ist keine Option mehr. **Jede Ausführung muss in einem eigenen Prozess laufen.**

| Plattform | Empfehlung | Status |
|---|---|---|
| Windows | **Job Object** (Speicher-, CPU-Zeit-, Prozesslimit, `KILL_ON_JOB_CLOSE`) + **AppContainer ohne Capabilities** (kein Netzwerk, kein Zugriff aufs Profil) | **[FAKT]** von mehreren Open-Source-Projekten 2025/2026 so eingesetzt |
| macOS | `sandbox-exec` (Netzwerk- und Schreibverbot) + `setrlimit` + temporäres Arbeitsverzeichnis + Timeout | **[FAKT]** offiziell „deprecated“, Profilsprache undokumentiert, funktioniert aber noch → **Risiko** |
| Beide | JVM-Optionen `-Xmx`, harte Wall-Clock-Timeouts, Ausgabegröße begrenzen, Prozessbaum beim Abbruch beenden | — |

- **[EINSCHÄTZUNG]** VMs/Container sind für Office-PCs zu schwer.
- **[FAKT]** Läuft die macOS-App selbst im App Sandbox, erben Kindprozesse diese Sandbox. Ein `exec` auf ein anders sandboxiertes Programm führt zum Absturz. → Sandbox-Strategie früh festlegen und testen.

---

## 9. Ausgabevergleich

**Best Practice:** Die Semantik des Kattis/DOMjudge-Standard-Validators übernehmen statt eigene Regeln zu erfinden.

- **[FAKT]** Standard: tokenweiser Vergleich, Whitespace ignoriert, Groß-/Kleinschreibung ignoriert.
- **[FAKT]** Optionale Flags: `case_sensitive`, `space_change_sensitive`, `float_absolute_tolerance`, `float_relative_tolerance`, `float_tolerance`.
- Den Vergleichsmodus pro Aufgabe aus Schritt 3 der Pipeline (LLM-Strukturierung) übernehmen und dem Nutzer anzeigen.
- **[EINSCHÄTZUNG]** Aufgaben mit mehreren gültigen Ausgaben (z. B. beliebige Reihenfolge) brauchen Spezialvergleiche (sortieren, Mengenvergleich). Ein vom LLM generierter Checker pro Aufgabe ist riskant.

---

## 10. Verteilung und Paketierung

- App: **Java 21/25 + JavaFX**, mit `jlink`/`jpackage` gebündelt. Das JDK wird ohnehin zum Kompilieren gebraucht.
- **Modelle nicht in den Installer packen.** Kleine Basisinstallation plus Modelldownload beim ersten Start **oder** separates Offline-Paket. Mit Prüfsumme und Versionsdatei; Modelle unabhängig von der App aktualisierbar. Zur Laufzeit vollständig offline.
- Weitere Sprachen **[OFFEN]**:
  - Python: portable Laufzeit mitliefern
  - C/C++: Windows z. B. MinGW- oder LLVM-Toolchain; macOS Xcode Command Line Tools (muss der Nutzer installieren)
  - Lizenz und Größe dieser Toolchains vor der Auslieferung prüfen

### Lizenz-Checkliste
| Komponente | Lizenz | Status |
|---|---|---|
| llama.cpp, Ollama, java-llama.cpp, Docling | MIT | ok |
| Tesseract, Granite-Docling | Apache 2.0 | ok |
| Qwen2.5-Coder 0.5/1.5/7/14/32B, Qwen3.5, Qwen3-Coder-30B-A3B | Apache 2.0 | ok |
| PDFBox, POI | Apache 2.0 **[OFFEN, nicht gesondert geprüft]** | prüfen |
| **Qwen2.5-Coder-3B, Qwen2.5-3B** | Research License | **nicht verwenden** |
| **LM Studio** | proprietär | **nicht verwenden** |

NOTICE-Dateien und Attributionspflichten mitliefern. Keine Rechtsberatung – im Zweifel juristisch prüfen lassen.

---

## 11. Grenzen des Ansatzes (offen kommunizieren)

Folgende Aufgaben lassen sich **nicht** zuverlässig allein über Ausgabevergleich prüfen:
- Aufgaben **ohne Beispiel-Ein-/Ausgaben** im Blatt
- mehrdeutige Aufgabentexte
- mehrere korrekte Ausgaben
- GUI, Dateizugriff, Netzwerk, Zufall, Nebenläufigkeit
- Strukturvorgaben („rekursiv lösen“, „Klasse X mit Interface Y implementieren“)
- algorithmisch anspruchsvolle Aufgaben (kleine Modelle scheitern hier häufig)

Für diese Fälle ist das Programm **nur als Hilfe** nutzbar, nicht als verlässliche Bewertung. Die Konfidenzanzeige ist Pflicht.

---

## 12. Vor der Entscheidung zu erledigen (Validierungsplan)

1. **Hardware-Benchmark:** `llama-bench` auf 2–3 echten Ziel-Rechnern (Windows-Office-Laptop DDR4, Intel-Mac, Apple-Silicon-Mac) mit den Kandidatenmodellen.
2. **Eigener Testsatz:** 20–30 echte Übungsblätter (Java), Erfolgsquote der gesamten Pipeline messen – nicht nur des Modells.
3. **Modellvergleich:** Qwen2.5-Coder-7B vs. Qwen3.5-4B (ohne Thinking) vs. 1.5B auf diesem Testsatz, inkl. Latenz.
4. **OCR-Test:** Scans und Handyfotos echter Aufgabenblätter mit Apple Vision, Windows.Media.Ocr und Tesseract.
5. **Sandbox-Prototyp:** Windows (Job Object + AppContainer) und macOS (`sandbox-exec`) früh als Spike umsetzen, da hier das größte technische Risiko liegt.
6. **Offene Punkte prüfen:** ONNX Runtime GenAI, OpenVINO, llamafile, Lizenzen PDFBox/POI/Toolchains, Java-Werte weiterer Modelle.

---

## 13. Quellen (Auswahl)

- Qwen2.5-Coder Technical Report: https://arxiv.org/html/2409.12186v3
- Qwen2.5-Coder-3B Lizenz: https://huggingface.co/Qwen/Qwen2.5-Coder-3B/blob/main/LICENSE
- Qwen3.5-4B Model Card: https://huggingface.co/Qwen/Qwen3.5-4B
- Qwen3.5-2B Model Card: https://huggingface.co/Qwen/Qwen3.5-2B
- CodeT (Dual Execution Agreement): https://arxiv.org/abs/2207.10397
- Security Manager dauerhaft deaktiviert: https://docs.oracle.com/en/java/javase/25/security/security-manager-is-permanently-disabled.html
- java-llama.cpp: https://github.com/kherud/java-llama.cpp
- CPU-only LLM Erfahrungswerte: https://localaimaster.com/blog/run-llm-cpu-only · https://insiderllm.com/guides/cpu-only-llms-what-actually-works/
- llama.cpp CPU-Deployment (Paper): https://ceur-ws.org/Vol-4164/paper11.pdf
- Windows Sandbox (AppContainer + Job Object): https://github.com/armaxri/termiHub/issues/4187
- macOS `sandbox-exec` Status: https://github.com/apple/containerization/issues/737 · https://developer.apple.com/forums/thread/777509
- Docling: https://docling.org/
- OCR-Einrückung bei Code: https://pieces.app/blog/how-we-made-our-optical-character-recognition-ocr-code-more-accurate
