# Code-Prüfer

Desktop-App (Windows und macOS) für Studierende: Programmieraufgaben **lokal** gegen
eigene Testfälle prüfen.

1. Aufgabenstellung einfügen oder als Bild, PDF bzw. Word-Datei hochladen.
2. Code im Editor schreiben oder eine Quelldatei hochladen (`.py` / `.java`).
3. Sprache wählen (MVP: **Python 3.12** und **Java 21**).
4. Testfälle anlegen: Eingabe (stdin) und erwartete Ausgabe (stdout).
5. **Prüfen** klicken.

Pro Testfall zeigt die App *Bestanden* / *Falsche Ausgabe* / *Zeitüberschreitung* /
*Laufzeitfehler*, einen Zeilen-Diff (erwartet vs. tatsächlich), stderr sowie
Kompilierfehler.

Der Code wird ausschließlich auf dem eigenen Rechner in einem abgeschotteten
Docker-Container ausgeführt – es gibt keinen zentralen Server.

## Voraussetzungen

Eine Docker-kompatible Container-Engine wird benötigt:

- [Docker Desktop](https://www.docker.com/products/docker-desktop/) – kostenlos für
  private Nutzung/Bildung – bitte Lizenzbedingungen selbst prüfen.
- oder [Podman](https://podman.io/) (Open Source). Unter macOS/Windows läuft Podman in
  einer „Podman-Maschine“ (unter Windows auf Basis von WSL2).

### Windows-Installer

Der Installer (`Code-Prüfer_<version>_x64-setup.exe`) ist deutschsprachig und fragt auf
der Seite **„Container-Engine“**, ob Docker Desktop (Standard) oder Podman verwendet
werden soll. Ist die gewählte Engine nicht installiert, bietet er an, sie über `winget`
zu installieren (`Docker.DockerDesktop` bzw. `RedHat.Podman` aus der Quelle `winget`;
Windows fragt nach Administratorrechten, ggf. ist ein Neustart nötig). `winget` wird dabei
nur über den absoluten Pfad `%LOCALAPPDATA%\Microsoft\WindowsApps\winget.exe` gestartet,
nie als bloßes `winget` (das Windows sonst auch neben dem Installer, z. B. im
Download-Ordner, suchen würde). Ohne `winget` wird die offizielle Download-Seite geöffnet. Schlägt die Installation der Engine fehl, wird der Code-Prüfer
trotzdem installiert. Bei stiller/passiver Installation (`/S`, `/P`) wird nichts gefragt
und nichts nachinstalliert.

Umsetzung: `src-tauri/windows/hooks.nsh` (Installer-Hooks mit Auswahlseite und
winget-Aufruf) und `src-tauri/windows/installer.nsi` – das unveränderte Tauri-Template
(tauri-cli 2.12.4) mit **einer** Ergänzung vor der Installationsseite, die die Seite aus
`hooks.nsh` einfügt. Bei einem Update der Tauri-CLI das Template neu kopieren und diese
Stelle (`CODE_PRUEFER_CUSTOM_PAGES`) wieder einfügen.

### macOS

Die DMGs (`*_aarch64.dmg` für Apple Silicon, `*_x64.dmg` für Intel) sind noch nicht
signiert/notarisiert. Gatekeeper blockiert deshalb den ersten Start: im Finder
Rechtsklick auf „Code-Prüfer“ → **Öffnen** → „Öffnen“ bestätigen. Alternativ im
Terminal: `xattr -dr com.apple.quarantine "/Applications/Code-Prüfer.app"`.

### Engine-Auswahl und automatischer Start

Die gewählte Engine wird so ermittelt (erste Quelle gewinnt):

1. `engine.json` im App-Konfigurationsordner (Windows:
   `%APPDATA%\de.bernhardgoetz.codepruefer\engine.json`, Inhalt z. B.
   `{"engine": "podman"}`) – wird geschrieben, wenn man die Engine in der App umstellt
   (Zahnrad oben rechts → „Einstellungen“ → „Container-Engine“);
2. Registry `HKCU\Software\Code-Pruefer`, Wert `Engine` (`docker` | `podman`) – wird
   vom Installer geschrieben;
3. Standard: Docker.

Beim Start der App (bzw. wenn die Prüfung ergibt, dass die Engine nicht läuft) startet
der Code-Prüfer die gewählte Engine und wartet, bis sie antwortet (alle 2 s, max. 120 s):

- **Docker Desktop:** `Docker Desktop.exe` wird gestartet (macOS:
  `open -a /Applications/Docker.app`; Socket `~/.docker/run/docker.sock`).
- **Podman:** existiert noch keine Maschine, wird einmalig `podman machine init`
  ausgeführt (lädt ein VM-Image, kann einige Minuten dauern), danach
  `podman machine start`. Unter Windows verbindet sich die App über die Named Pipe, die
  `podman machine inspect` meldet (Standard: `\\.\pipe\podman-machine-default`), unter
  macOS über den Socket der Maschine (`podman machine inspect`, Podman 5:
  `$TMPDIR/podman/podman-machine-default-api.sock`). Der `podman`-Befehl wird auch ohne
  PATH unter `/opt/podman/bin`, `/opt/homebrew/bin` und `/usr/local/bin` gefunden.
- **Linux:** kein automatischer Start (Systemdienst) – bitte Docker bzw. Podman selbst
  starten.

Die Engine wird nicht mit Windows gestartet, sondern nur mit dem Code-Prüfer.

Die App erkennt die Engine automatisch (Docker-Socket, Docker-Desktop-Socket unter
`~/.docker/run/docker.sock`, Podman-Sockets, Named Pipes `docker_engine` und
`podman-machine-default` unter Windows); die Endpunkte der gewählten Engine werden
zuerst probiert. Mit der Umgebungsvariable `DOCKER_HOST` (z. B.
`unix:///pfad/zum/podman.sock` oder `npipe:////./pipe/podman-machine-default`) lässt
sich der Endpunkt explizit festlegen; sie hat Vorrang vor der Engine-Auswahl. Ein
`tcp://`- oder `http://`-Endpunkt ist immer unverschlüsselt und ohne Anmeldung (die App
unterstützt kein TLS); dafür zeigt das Hauptfenster unter der Kopfzeile eine Warnung.

### Runner-Images (Python, Java)

Release-Installer enthalten die Runner-Images als komprimierte Archive
(`images/code-pruefer-<sprache>-<arch>.tar.gz` im Ressourcenordner der App). Sobald die
Engine läuft, lädt die App fehlende Images automatisch in die **gewählte** Engine
(Docker und Podman verwenden dasselbe Image-Format, aber getrennte Image-Speicher) – ganz
ohne Internet und ohne manuelles `docker build`. Fortschritt wird neben „Prüfen“ und in den
Einstellungen angezeigt.

Reihenfolge, wenn eine Sprache verwendet wird (`src-tauri/src/runner/images.rs`):

1. nur Debug-Builds: Umgebungsvariable `CODE_PRUEFER_PYTHON_IMAGE` /
   `CODE_PRUEFER_JAVA_IMAGE` (exklusiv: lokal vorhanden, sonst Download);
2. das gepinnte Image `ghcr.io/bernhardgoetz/code-pruefer-<sprache>:<App-Version>` ist in
   der Engine bereits vorhanden;
3. Laden aus dem mitgelieferten Archiv (Engine-API `POST /images/load`, auch von Podman
   unterstützt; das Archiv wird von der App entpackt und als Tar gestreamt). Trägt das
   geladene Image einen anderen Namen, wird nur das Image mit der erwarteten Image-ID bzw.
   dem passenden Namen (`code-pruefer-<sprache>`) auf die erwartete Referenz getaggt;
4. Download von GHCR;
5. nur Debug-Builds: lokal gebautes `code-pruefer-<sprache>:dev`;
6. sonst eine deutsche Fehlermeldung mit Lösungsweg.

Release-Builds (`npm run tauri build`, Installer) beachten weder die Umgebungsvariablen
noch `:dev`-Images; sie verwenden nur die eingebetteten bzw. veröffentlichten Images.

**Prüfsummen (`images/image-digests.json`):** Builds mit eingebetteten Images enthalten
zusätzlich diese Datei (erzeugt von `scripts/export-image-archives.sh`). Sie enthält je
Sprache und Architektur die SHA-256 des Archivs und die Image-IDs (Manifest- und
Config-Digest), bei Release-Builds außerdem den Registry-Digest des gepushten Images. Ist
sie vorhanden,

- gilt ein bereits vorhandenes Image unter dem gepinnten Tag nur, wenn seine ID (bzw. sein
  Registry-Digest) dort steht – sonst wird es aus dem Archiv ersetzt;
- wird die SHA-256 des Archivs vor dem Laden geprüft und die Image-ID nach dem Laden; bei
  Abweichung erscheint eine deutsche Fehlermeldung und das Image wird wieder entfernt;
- lädt die App per `ghcr.io/…/code-pruefer-<sprache>@sha256:…` statt per Tag. Ohne
  Registry-Digest (lokaler Build, Images nie gepusht) gibt es keinen Download, und
  „Images aktualisieren“ meldet, dass die mitgelieferten Images weiter verwendet werden.

Fehlt die Datei (Entwicklungs- und CI-Builds ohne eingebettete Images), gilt das bisherige
Verhalten: Vertrauen über den Tag, Download per Tag.

Die Architektur des Archivs richtet sich nach der Engine (`amd64` / `arm64`, ersatzweise
nach dem Rechner: `x86_64` → `amd64`, `aarch64` → `arm64`).

**„Images aktualisieren“** (Zahnrad → „Einstellungen“ → „Container-Engine“) lädt die gepinnten Images dieser
App-Version erneut von GHCR. Schlägt das fehl (offline, Registry nicht erreichbar), bleiben
die vorhandenen Images nutzbar.

Ressourcenordner: Windows (Installer und portable Version) = Ordner der `code-pruefer.exe`,
also `<Programmordner>\images\…`; macOS = `Code-Prüfer.app/Contents/Resources/images/`.

## Sicherheit der Ausführung

Jede Prüfung startet einen frischen Container, der danach immer gelöscht wird:

- kein Netzwerk, read-only Root-Dateisystem, `tmpfs` für `/work` und `/tmp` (je 64 MB)
- 512 MB RAM ohne Swap, 1 CPU, max. 64 Prozesse, max. Dateigröße 16 MB
- nicht-root (UID 1000), alle Capabilities entfernt, `no-new-privileges`
- Zeitlimit pro Test: Python 5 s, Java 8 s; Kompilieren 30 s. Ein Programm, das seine
  Ausgabe schließt, aber weiterläuft, gilt nach Ablauf des Limits als Zeitüberschreitung
- nach jedem Test (und dem Kompilieren) werden alle übrigen Prozesse des Sandbox-Benutzers
  beendet (`kill -9 -1`), damit nichts im Hintergrund in den nächsten Test hineinläuft
- höchstens 50 Testfälle pro Prüfung
- stdout/stderr werden nach 64 KB abgeschnitten (wird angezeigt)
- übrig gebliebene Container (Label `de.code-pruefer.sandbox`, z. B. nach einem Absturz)
  werden beim Start und vor jeder Prüfung entfernt: eigene sofort, die anderer bzw.
  abgestürzter App-Instanzen nach 15 Minuten; beim Beenden der App ihre eigenen

## Sicherheit

### Bedrohungsmodell

- **Geprüfter Code ist nicht vertrauenswürdig** (eigener oder fremder Code, z. B. aus
  einem Forum). Er läuft nur im Container (siehe oben): ohne Netzwerk, ohne Rechte, mit
  Ressourcen-Limits, ohne Zugriff auf Dateien des Rechners.
- **Die App selbst und die Container-Engine sind vertrauenswürdig.** Wer die Engine
  steuern kann (Docker-Socket, Named Pipe, `DOCKER_HOST`), hat faktisch
  Administratorrechte – das gilt unabhängig vom Code-Prüfer.
- **Lieferkette:** Runner-Images und Installer entstehen ausschließlich im
  Release-Workflow aus einem Versions-Tag. Basis-Images und GitHub-Actions sind per
  Digest bzw. Commit-SHA gepinnt (Updates über Dependabot, 7 Tage Wartezeit), Release-Jobs
  laufen ohne Caches und mit minimalen Token-Rechten, Versions-Tags der Images werden nur
  bei Versions-Tags gepusht. Die App prüft eingebettete Archive und Downloads gegen
  `image-digests.json`; zum Release gehören `SHA256SUMS.txt` und eine
  Build-Provenance-Attestierung (`gh attestation verify <Datei> --repo BernhardGoetz/Entwicklung-test`).

### Restrisiken

- Container teilen sich den Kernel der Engine-VM (WSL2 bzw. Podman-/Docker-VM); eine
  Kernel-Lücke könnte aus der Sandbox ausbrechen (bis in die VM, nicht direkt auf
  Windows/macOS). Gegenmaßnahme: Engine aktuell halten.
- Installer und App sind (noch) nicht signiert; SmartScreen/Gatekeeper warnen, und die
  Echtheit lässt sich nur über `SHA256SUMS.txt` und die Attestierung prüfen.
  Windows-Signatur ist vorbereitet (Secrets `WINDOWS_CERTIFICATE`,
  `WINDOWS_CERTIFICATE_PASSWORD` im Environment `release`), ebenso macOS (siehe unten).
- `image-digests.json` liegt neben den Archiven im Programmordner. Wer dort schreiben
  kann, kann beides austauschen; die Prüfung schützt vor beschädigten und einzeln
  ausgetauschten Archiven sowie vor fremden Images unter dem gepinnten Tag, nicht vor
  einem manipulierten Installationsordner (Abhilfe: signierte Installer).
- Prozesse, die ein Test im Hintergrund startet, werden beendet, bleiben aber bis zum
  Ende der Prüfung als Zombie-Prozess im Container (zählen zum Limit von 64 Prozessen).
- Ohne Pflicht-Reviewer im Environment `release` kann jeder mit Schreibrecht auf das
  Repository einen Release auslösen (siehe nächster Abschnitt).

### Nötige GitHub-Einstellungen (einmalig, durch den Repository-Inhaber)

1. **Environment `release`** anlegen (Settings → Environments → New environment):
   „Required reviewers“ = Bernhard, „Deployment branches and tags“ = „Selected branches and
   tags“ mit der Tag-Regel `v*`. Die Apple-Secrets (`APPLE_CERTIFICATE`, …) und optional
   `WINDOWS_CERTIFICATE`/`WINDOWS_CERTIFICATE_PASSWORD` dort als **Environment-Secrets**
   hinterlegen, nicht als Repository-Secrets. Die Jobs `bundle-windows` und
   `bundle-macos` warten dann auf die Freigabe.
2. **Tag-Schutz:** Settings → Rules → Rulesets → New tag ruleset, Ziel `v*`, „Restrict
   creations“, „Restrict updates“ und „Restrict deletions“, Bypass nur für Bernhard.
3. **GHCR-Pakete öffentlich:** `code-pruefer-python` und `code-pruefer-java` unter
   GitHub → Packages → Package settings → „Change visibility“ auf **public** stellen
   (sonst funktionieren Download und „Images aktualisieren“ nicht).
4. Optional: Settings → Actions → General → „Workflow permissions“ auf „Read repository
   contents and packages permissions“ stellen (die Workflows fordern ihre Rechte selbst an).

## Aufgabenblatt hochladen

„Aufgabe hochladen …“ übernimmt den Text eines Aufgabenblatts in das Feld
„Aufgabenstellung“: Word (`.docx`), PDF oder ein Bild (`.png`, `.jpg`). Höchstens 20 MB
und 20 Seiten. Alles läuft lokal:

- **DOCX:** Absätze, Tabulatoren und Zeilenumbrüche werden übernommen; eingebettete
  Bilder nicht.
- **PDF:** Zuerst wird die Textebene gelesen. Seiten mit weniger als 50 Zeichen gelten
  als Scan und werden per Texterkennung gelesen.
- **Bilder und Scans:** systemeigene Texterkennung – unter Windows `Windows.Media.Ocr`
  (dafür muss eine Sprache mit Texterkennung installiert sein: Einstellungen → Zeit und
  Sprache → Sprache), unter macOS Apple Vision. Die Einrückung (wichtig für Python) wird
  aus der Lage der Wörter rekonstruiert. Unter Linux gibt es keine Texterkennung.
- **Bildvorbereitung** (`docs/ocr/prepare.rs`): Vor der Texterkennung wird das Bild in
  Graustufen umgewandelt, bei dunklem Hintergrund (dunkles Editor-Design) invertiert, im
  Kontrast gestreckt, auf den Inhalt zugeschnitten und bei kleiner Schrift hochskaliert
  (Ziel: etwa 32 Pixel Zeilenhöhe, höchstens 4-fach). Die Zeilenhöhe wird aus dem Bild
  selbst geschätzt (Zeilen mit Schrift zählen); die Obergrenze der Windows-Texterkennung
  (`MaxImageDimension`) und 24 Megapixel werden eingehalten. Kleine Schrift in
  Screenshots wird so deutlich besser erkannt.

Erkannter Text kann Fehler enthalten. Daneben steht eine Vorschau des Originals (bei
Bildern; bei PDFs die erste Seite, soweit das System sie darstellen kann).

**Erkennungsfehler mit KI korrigieren** (nur nach einer Texterkennung, mit eingerichtetem
KI-Modell; `src-tauri/src/ai/ocr_fix.rs`): Das lokale Modell bekommt die strikte Anweisung,
nur Zeichenfehler zu korrigieren (1/l, 0/O, fehlende Anführungszeichen oder Klammern) und
nichts umzuformulieren. Es liefert eine JSON-Liste von Änderungen (Zeile, alt, neu), keinen
neuen Text. Ein **Plausibilitätsfilter** lässt nur kleine, örtliche Änderungen durch: `alt`
muss genau einmal in der Zeile stehen (eine Zeile daneben wird toleriert), jede Änderung
betrifft höchstens ein Wort auf jeder Seite (keine neuen oder entfernten Wörter, keine
Zeilenumbrüche), die Bearbeitungsdistanz ist höchstens max(2, 40 % der Wortlänge) – typische
Verwechslungen wie 1/l oder 0/O zählen halb –, und mehr als drei Änderungen in einem
Vorschlag gelten als Umformulierung. Ganze Zeilen werden nie als Ganzes ersetzt; höchstens
30 Vorschläge. Die Vorschläge erscheinen farbig (alt → neu) und werden einzeln oder mit
„Alle übernehmen“ übernommen; „Ursprünglichen Text wiederherstellen“ setzt auf das Ergebnis
der Texterkennung zurück. Jede Übernahme setzt das Häkchen „Text geprüft und korrigiert“
zurück. Zeilennummern oder Texte der Bedienoberfläche aus Screenshots werden nicht
entfernt. Das Häkchen
„Text geprüft und korrigiert“ wird bei jedem Import zurückgesetzt; es ist Voraussetzung für
„Testfälle mit KI erzeugen“ (siehe „KI und Texterkennung“).

## Ausgabevergleich

Zeilenenden (CRLF/LF) werden immer vereinheitlicht. Drei Modi („Vergleich:“ über den
Testfällen):

- **Zeilen (exakt)** – Standard. Zeile für Zeile; Leerzeichen am Zeilenende und
  Leerzeilen am Ende werden ignoriert (abschaltbar).
- **Wörter (Leerraum egal)** – die Ausgabe wird an Leerzeichen, Tabulatoren und
  Zeilenumbrüchen in Wörter zerlegt; nur die Wörter zählen (wie der Standard-Validator von
  Kattis, aber mit Beachtung der Groß-/Kleinschreibung).
- **Zahlen mit Toleranz** – wie „Wörter“; ist das erwartete Wort eine Zahl, darf die
  Ausgabe absolut oder relativ um die Toleranz abweichen (z. B. `1e-6`).

Der Diff in den Ergebnissen ist immer zeilenbasiert. Leerzeichen am Zeilenende werden dort
und in der stdout-Anzeige als `·` dargestellt, ein Zeilenumbruch am Ende als `⏎` (nur
Anzeige; kopiert wird der Originaltext). Unter jeder erwarteten Ausgabe steht, ob sie mit
einem Zeilenumbruch endet und in welchen Zeilen Leerzeichen am Ende stehen. Unterscheiden
sich erwartete und tatsächliche Ausgabe nur dort, zeigt das Ergebnis den Hinweis
„Unterschied nur am Zeilenende (Leerzeichen/Zeilenumbruch)“.

**Zeilenumbruch am Ende verbieten** (unabhängig vom Modus): Endet die Ausgabe mit einem
Zeilenumbruch (z. B. durch `println` oder `print()`), ist der Test nicht bestanden – egal,
was die erwartete Ausgabe enthält. Die KI-Testfälle übernehmen die Ausgabe der Musterlösung
unverändert; endet diese selbst mit einem Zeilenumbruch, weist der Testfall-Editor einmalig
darauf hin.

## KI und Texterkennung

„Testfälle mit KI erzeugen“ erstellt aus der geprüften Aufgabenstellung Testfälle – mit
einem **lokalen** Sprachmodell (Qwen2.5-Coder über llama.cpp), ohne Cloud-Dienst.
Grundlage ist die Recherche in
[`docs/ENTSCHEIDUNGSGRUNDLAGE_lokale-KI-Aufgabenchecker.md`](docs/ENTSCHEIDUNGSGRUNDLAGE_lokale-KI-Aufgabenchecker.md).

**Kernregel:** Die KI schreibt **nie** die erwarteten Ausgaben. Sie schreibt Lösungen und
Testeingaben; die erwarteten Ausgaben entstehen durch **Ausführen** geprüfter Lösungen in
derselben Sandbox wie beim „Prüfen“.

Ablauf (`src-tauri/src/ai/pipeline.rs`):

1. **Struktur:** Ein-/Ausgabeformat, Beispiele und ein Vorschlag für den Vergleichsmodus
   (JSON, per JSON-Schema grammatikgebunden). Ein Beispiel zählt nur, wenn Ein- und
   Ausgabe **wörtlich** im Aufgabentext stehen (Zeilenenden und Leerraum am Zeilenrand
   egal) – erfundene Beispiele werden verworfen.
2. **Lösungen:** 5 (1.5B-Modell) bzw. 3 (7B-Modell) unabhängige Lösungen, Temperatur 0,7,
   **gleichzeitig** erzeugt (llama-server mit einem Slot je Lösung; auf der CPU begrenzt die
   Speicherbandbreite, mehrere Sequenzen kosten kaum mehr als eine). Die Antwort beginnt im
   Codeblock und endet an dessen Ende (keine Erklärtexte).
3. **Kompilieren** in der Sandbox, sobald eine Lösung fertig ist – ein Container je Lösung,
   der auch für die Beispiele und den Mehrheitsentscheid dient (nur ein Kompilierlauf).
   Höchstens eine Reparaturrunde mit der Fehlermeldung; sie entfällt, wenn schon eine klare
   Mehrheit (mehr als die Hälfte, mindestens zwei) die Beispiele bestanden hat.
4. **Beispiele des Blatts:** Lösungen, die sie nicht bestehen, fallen heraus.
5. **Testeingaben:** höchstens 10, inklusive Grenzfällen – nur Eingaben; parallel zu den
   Lösungen erzeugt.
6. **Mehrheitsentscheid:** Alle verbliebenen Lösungen laufen auf jede Eingabe; die
   Ausgaben werden im gewählten Vergleichsmodus gruppiert, die größte Gruppe liefert den
   Erwartungswert. Ohne echte Mehrheit (z. B. 2 gegen 2) wird die Eingabe verworfen.
7. **Konfidenz:**
   - **Grün:** Die Beispiele des Blatts wurden bestanden, mindestens zwei Lösungen sind
     übrig und bei jedem Testfall stimmen mindestens 80 % von ihnen überein.
   - **Gelb:** keine Beispiele im Text, nur eine Lösung übrig, knappe Mehrheit oder
     verworfene Eingaben – die Testfälle sind **nur ein Hinweis**.
   - **Rot:** Keine Lösung besteht (oder keine Eingabe hat eine Mehrheit). Dann gibt es
     **keine** Testfälle, sondern eine ehrliche Meldung.

Vor dem Laden des Modells prüft die App die Container-Engine und startet sie bei Bedarf
selbst (wie beim „Prüfen“); fällt sie während des Laufs aus, wird sie neu verbunden bzw.
gestartet und der Schritt wiederholt. Ist ein Modell gewählt, lädt die App es schon nach
„Text geprüft und korrigiert“ im Hintergrund. Die Fortschrittsanzeige nennt Phase,
„Lösung i von N“, Token/s und eine grobe Restzeit.

Die Testfälle werden an die vorhandenen angehängt (höchstens 50 insgesamt), mit „KI“
markiert und bleiben bearbeitbar. Die Musterlösung (die Lösung, die am häufigsten mit der
Mehrheit übereinstimmte) lässt sich aufklappen. Der vorgeschlagene Vergleichsmodus wird
nur übernommen, wenn man „Vorschlag übernehmen“ klickt; sonst gilt der eingestellte.

**Grenzen** (Recherche §11) – hier ist das Ergebnis höchstens ein Hinweis:
Aufgaben ohne Beispiele, mehrdeutige Texte, mehrere richtige Ausgaben, GUI/Dateien/
Netzwerk/Zufall/Nebenläufigkeit, Strukturvorgaben („rekursiv lösen“, „Klasse X
implementieren“) und algorithmisch anspruchsvolle Aufgaben (kleine Modelle scheitern
dort häufig). Die Konfidenzanzeige ist deshalb Pflicht.

**Hardware und Dauer** (Schätzungen, nicht auf echten Schüler-/Studierenden-PCs gemessen):

| Arbeitsspeicher | Vorschlag | Größe | Dauer je Aufgabe (CPU) |
|---|---|---|---|
| unter 12 GB | Qwen2.5-Coder 1.5B Instruct, Q5_K_M | ≈ 1,1 GB | ca. 1–4 Minuten |
| ab 12 GB | Qwen2.5-Coder 7B Instruct, Q4_K_M | ≈ 4,7 GB | ca. 4–10 Minuten |

Auf Apple Silicon (Metal) deutlich schneller. Ohne AVX2 warnt die App (sehr langsam).
Qwen2.5-Coder-3B wird nicht angeboten (Lizenz verbietet kommerzielle Nutzung); 1.5B und 7B
stehen unter Apache-2.0.

**Datenschutz:** Alles läuft lokal. Die **einzige** Netzwerkverbindung der KI-Funktion ist
der einmalige Modell-Download von `huggingface.co` (nur https, nur Hosts von Hugging Face,
auch bei Weiterleitungen; feste Revision, SHA-256 und Größe aus dem Katalog,
`src-tauri/src/ai/catalog.json`). llama-server lauscht nur auf `127.0.0.1` mit zufälligem
API-Schlüssel und startet mit `--offline`. Aufgabentexte und Code verlassen den Rechner nie.

**Modell einrichten:** Unter Zahnrad → „Einstellungen“ → „KI-Modell verwalten …“ (oder
„KI-Modell einrichten …“ im Tab „Testfälle“) beim empfohlenen Eintrag „Herunterladen“, danach
„Auswählen“. Revision, Dateiname, Größe und SHA-256 trägt der Release-Workflow aus der
Hugging-Face-API ein (`scripts/pin-models.sh`) und nennt sie in den Release-Notizen; im
Repository ist der Katalog nicht gepinnt, Entwickler-Builds haben deshalb keinen
Download-Knopf.

**Modell offline einrichten** (unter „Weitere Optionen (ohne Internet)“): Die `.gguf`-Datei (Links und Prüfsummen in `catalog.json`)
auf einem Rechner mit Internet herunterladen, z. B. per USB-Stick übertragen und unter
„KI-Modell verwalten …“ beim passenden Eintrag mit „Datei importieren …“ einlesen (erwartet:
`*q5_k_m*.gguf` für 1.5B, `*q4_k_m*.gguf` für 7B; ist die Datei auf Hugging Face in Teile
`…-00001-of-0000N.gguf` aufgeteilt, alle Teile zusammen auswählen – fehlende Teile lassen sich
nachträglich importieren). Die App prüft den GGUF-Dateianfang und warnt bei unpassendem
Dateinamen. Ist der Eintrag gepinnt, muss die SHA-256 mit dem Katalog übereinstimmen. Ist er
nicht gepinnt, wird die Datei nach ausdrücklicher Bestätigung diesem Eintrag zugeordnet (mit
seinen Einstellungen, z. B. Anzahl der Lösungen), unter `models/unverified/<id>/` mit
Originalnamen abgelegt und als „nicht geprüft (Prüfsumme fehlt in dieser Version)“ markiert.
„Anderes Modell importieren (.gguf) …“ nimmt eine beliebige einzelne Datei: Stimmt ihre
SHA-256 mit dem Katalog überein, gilt sie als geprüftes Katalogmodell, sonst wird sie nur nach
Bestätigung als „unbekanntes Modell“ übernommen. Modelle liegen im App-Datenordner
(`models/`), die Auswahl in `ai.json` im Konfigurationsordner.

**Modellkatalog pinnen:** `scripts/pin-models.sh` trägt Revision, Größe und SHA-256 aus der
Hugging-Face-API ein (bei aufgeteilten GGUF-Dateien alle Teile; llama.cpp lädt sie über den
ersten Teil). Solange das nicht geschehen ist, verweigert die App den Download (Import
geht) und der Release-Workflow bricht ab (`scripts/pin-models.sh --check`).
**Stand:** In dieser Version ist der Katalog noch **nicht** gepinnt – der Build-Umgebung war
huggingface.co nicht erreichbar. Vor dem nächsten Release einmal mit Internet ausführen und
`catalog.json` committen.

**KI-Laufzeit:** llama.cpp Release `b11700` (offizielle Binaries, SHA-256 je Plattform in
`scripts/fetch-llama-server.sh`), im Installer unter `llama/`. Unter Windows braucht
llama-server die Visual-C++-Laufzeit; der Release-Installer legt sie bei. Fehlt sie (z. B.
in lokal gebauten Testversionen), meldet die App das mit Link zum Microsoft-Download.

## Entwicklung

Voraussetzungen: Node.js ≥ 20, Rust (stable, via [rustup](https://rustup.rs/)) und die
[Tauri-Systemabhängigkeiten](https://v2.tauri.app/start/prerequisites/)
(unter Linux u. a. `libwebkit2gtk-4.1-dev`).

```sh
npm install
npm run tauri dev
```

Weitere Befehle:

```sh
npm run build                      # Typecheck + Frontend-Build
cd src-tauri && cargo test         # Unit-Tests
cd src-tauri && cargo clippy --all-targets -- -D warnings
npm run tauri build                # Installer (unsigniert) bauen

# Windows-Installer unter Linux cross-kompilieren (benötigt mingw-w64 und nsis)
npm run tauri build -- --target x86_64-pc-windows-gnu --bundles nsis

# Nur die Oberfläche im Browser, Backend simuliert (src/dev/mockBackend.ts,
# nicht im Produktions-Build): http://localhost:1420/ (?engine=missing|stopped)
VITE_MOCK=1 npm run dev
```

Gestaltung (Tokens, Farben hell/dunkel, Bausteine, Layout):
[`docs/design/DESIGN.md`](docs/design/DESIGN.md), Vorschau in
`docs/design/design-preview.html`.

### Runner-Images lokal bauen

```sh
docker build -t code-pruefer-python:dev images/python
docker build -t code-pruefer-java:dev images/java

# App mit den lokalen Images starten (Umgebungsvariablen und :dev nur in Debug-Builds)
CODE_PRUEFER_PYTHON_IMAGE=code-pruefer-python:dev \
CODE_PRUEFER_JAVA_IMAGE=code-pruefer-java:dev \
npm run tauri dev

# Integrationstests (benötigen Docker, standardmäßig mit den :dev-Images)
cd src-tauri && cargo test -- --ignored --test-threads=1
```

KI (Entwickler-Builds): llama-server für Linux holen und per Umgebungsvariable setzen
(gilt nur in Debug-Builds), Modell unter „KI-Modell verwalten …“ importieren:

```sh
scripts/fetch-llama-server.sh linux-x64 /tmp/llama
CODE_PRUEFER_LLAMA_SERVER=/tmp/llama/llama-server npm run tauri dev

# Integrationstests: echter llama-server, beliebiges GGUF (ideal: Qwen2.5-Coder-1.5B)
cd src-tauri && CODE_PRUEFER_LLAMA_SERVER=/tmp/llama/llama-server \
  CODE_PRUEFER_TEST_MODEL=/pfad/zum/modell.gguf \
  cargo test --test ai_integration -- --ignored --nocapture --test-threads=1
```

Integrationstest für das Laden aus einem Archiv:
`cd src-tauri && cargo test --test image_bundle -- --ignored --test-threads=1`.

### Image-Versionen und Release

- Image-Referenz der App: `<IMAGE_REGISTRY>/code-pruefer-<sprache>:<version>` mit
  `IMAGE_REGISTRY = ghcr.io/bernhardgoetz` (`runner/languages.rs`) und `<version>` =
  `CARGO_PKG_VERSION`. `Cargo.toml`, `tauri.conf.json` und `package.json` müssen dieselbe
  Version haben (Unit-Test `versions_are_in_sync`).
- Release auslösen: Version in den drei Dateien erhöhen, committen, Tag `v<version>`
  pushen (z. B. `git tag v0.2.0 && git push origin v0.2.0`). Alternativ
  „Run workflow“ für `.github/workflows/release.yml` (Option „Entwurfs-Release anlegen“);
  ein manueller Lauf pusht die Images nur als `:sha-<commit>`, nie als `:<version>`.
- Der Workflow prüft Tag/Versionen/Registry-Namensraum, baut die Images multi-arch
  (linux/amd64, linux/arm64, ohne Build-Cache), pusht sie als `:<version>` nach GHCR,
  exportiert daraus per Digest die Archive je Architektur
  (`scripts/export-image-archives.sh`, `MODE=pull` – identische Digests wie online) samt
  `image-digests.json`, baut den Windows-Installer (windows-latest, MSVC, NSIS deutsch) und
  die macOS-DMGs (Apple Silicon und Intel) mit eingebetteten Images
  (`--config src-tauri/tauri.release.conf.json`) und legt ein **Entwurfs-Release** mit
  `SHA256SUMS.txt` und Build-Provenance-Attestierung an.
  macOS-Signatur/Notarisierung ist vorbereitet, läuft aber nur, wenn das Secret
  `APPLE_CERTIFICATE` (plus `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
  `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`) im Environment `release` gesetzt ist;
  die Windows-Signatur entsprechend mit `WINDOWS_CERTIFICATE` (siehe „Sicherheit“).
- `.github/workflows/images.yml` baut die Images bei Änderungen nur noch als `:edge` /
  `:sha-…`; Versions-Tags kommen ausschließlich aus dem Release-Workflow.
- GHCR-Namen müssen klein geschrieben sein (der Workflow wandelt den Owner um).
- **Noch zu prüfen:** Nach dem ersten Push sind GHCR-Pakete standardmäßig **privat**
  (siehe „Sicherheit“ → „Nötige GitHub-Einstellungen“).

Normale Builds (`npm run tauri build`, CI) enthalten keine Images und bleiben klein; die
App funktioniert dann über GHCR (bzw. `:dev` in Debug-Builds). Lokaler Build mit
eingebetteten Images (das Skript schreibt auch `image-digests.json`, aber ohne
Registry-Digest – die App prüft die Archive, lädt aber nichts herunter):

```sh
scripts/export-image-archives.sh amd64     # -> src-tauri/resources/images/ (Archive + image-digests.json, nicht eingecheckt)
scripts/fetch-llama-server.sh win-x64      # -> src-tauri/resources/llama/ (bzw. macos-arm64 / macos-x64)
npm run tauri build -- --config src-tauri/tauri.release.conf.json
# Windows-Installer unter Linux:
npm run tauri build -- --target x86_64-pc-windows-gnu --bundles nsis --config src-tauri/tauri.release.conf.json
```

Portable Windows-Version: die Ordner `images` (mit den Archiven und `image-digests.json`)
und `llama` (llama-server und DLLs) neben `code-pruefer.exe` legen – unter Windows ist der Tauri-Ressourcenordner immer der
Ordner der exe.

### Projektstruktur

```
src/                     React-Frontend (Monaco-Editor, Tabs Aufgabe/Testfälle/Ergebnis)
src/styles.css           Design-Tokens als CSS-Variablen (hell/dunkel) und Komponenten
src/dev/mockBackend.ts   Simuliertes Backend für VITE_MOCK=1 npm run dev (nur Entwicklung)
docs/design/             DESIGN.md, design-tokens.json, design-preview.html
src-tauri/src/
  lib.rs                 Tauri-Commands: docker_status, get_engine, set_engine,
                         start_engine, list_languages, run_tests,
                         prepare_images, update_images, import_task_document,
                         ai_status, download_model, import_model, delete_model,
                         select_model, generate_tests, suggest_ocr_fixes,
                         cancel_ai
  ai/llama.rs            llama-server als Sidecar (Start, /health, Client, Abbruch)
  ai/models.rs           Modellkatalog (catalog.json), Hardware, Download, Import
  ai/pipeline.rs         Testfälle erzeugen: Lösungen, Beispiele, Mehrheit, Konfidenz
  ai/prompts.rs          Prompts, JSON-Schemas, Code aus Antworten extrahieren
  ai/ocr_fix.rs          KI-Korrektur von OCR-Text: Prompt, Plausibilitätsfilter
  docs/mod.rs            Import von Aufgabenblättern (Bild, PDF, DOCX), Limits
  docs/docx.rs           Text aus word/document.xml
  docs/pdf.rs            Textebene pro Seite, Erkennung gescannter Seiten
  docs/layout.rs         Zeilen und Einrückung aus OCR-Wortboxen rekonstruieren
  docs/ocr/              Systemeigene Texterkennung: win.rs (Windows.Media.Ocr,
                         Windows.Data.Pdf), mac.rs (Vision, PDFKit),
                         prepare.rs (Bildvorbereitung: Kontrast, Hochskalieren)
  docker_detect.rs       Erkennung: nicht installiert / nicht gestartet / ok
  engine.rs              Engine-Auswahl (Datei/Registry), Start, Podman-Pipe
  runner/mod.rs          Ablauf einer Prüfung, Bewertung der Testfälle
  runner/docker.rs       Engine-Verbindung, Image-Pull/-Load, gehärteter Container
  runner/images.rs       Image-Auflösung: vorhanden → Archiv → GHCR → :dev
  runner/languages.rs    Sprachtabelle (Image, Dateiname, Befehle, Zeitlimits)
  runner/compare.rs      Normalisierung, Zeilen-Diff, Ausgabebegrenzung
src-tauri/windows/       NSIS-Installer: Template + Hooks (Engine-Auswahl, winget)
src-tauri/tauri.release.conf.json
                         Release-Overlay: bündelt resources/images/*.tar.gz,
                         resources/images/image-digests.json und resources/llama/*
images/                  Dockerfiles der Runner-Images
scripts/export-image-archives.sh
                         Image-Archive und image-digests.json erzeugen
scripts/fetch-llama-server.sh
                         llama-server (gepinnt, SHA-256) nach resources/llama/
scripts/pin-models.sh    Modellkatalog aus der Hugging-Face-API pinnen
THIRD_PARTY_NOTICES.txt  Lizenzen von llama.cpp u. a. (wird mitgeliefert)
.github/workflows/       ci.yml, images.yml (edge), release.yml (Tags v*)
.github/dependabot.yml   Updates für Actions, npm, cargo, Basis-Images
```

Eine neue Sprache benötigt einen Eintrag in `runner/languages.rs` und ein Dockerfile
unter `images/`.

## Einschränkungen des MVP

- Nur Python und Java; Java-Code muss eine Klasse `Main` enthalten (Datei `Main.java`).
- Testfälle werden manuell eingegeben oder mit der lokalen KI vorgeschlagen (siehe „KI und
  Texterkennung“); keine Speicherung von Aufgaben/Testfällen.
- Vergleich nur über stdout (Zeilen, Wörter oder Zahlen mit Toleranz).
- Builds sind nicht signiert (Windows SmartScreen / macOS Gatekeeper warnen; Signatur ist
  vorbereitet), kein Auto-Update der App (die Images lassen sich über „Images aktualisieren“ neu laden).
- Mit eingebetteten Images ist der Installer deutlich größer (Java-Image ≈ 200 MB).
- Benötigt eine Container-Engine (Docker Desktop oder Podman); der Windows-Installer kann
  sie über winget nachinstallieren.

Geplant für spätere Phasen: weitere Sprachen (C/C++/C#/JavaScript), signierter
Aufgabenkatalog, Code-Signing, Updater.

## Recherche: lokale KI und Texterkennung

[`docs/ENTSCHEIDUNGSGRUNDLAGE_lokale-KI-Aufgabenchecker.md`](docs/ENTSCHEIDUNGSGRUNDLAGE_lokale-KI-Aufgabenchecker.md)
ist die Entscheidungsgrundlage für das Hochladen von Aufgabenblättern (Bild, PDF, DOCX),
die systemeigene Texterkennung und die lokale Erzeugung von Testfällen. Sie ist eine
Recherche, keine Spezifikation: Übernommen werden vor allem Pipeline, Mehrheitsentscheid,
Konfidenz und Ausgabevergleich. Die Abschnitte 8 (Sandbox) und 10 (Verteilung/JavaFX)
gelten für diese App nicht, weil sie auf Tauri/Rust mit Docker bzw. Podman aufbaut.
