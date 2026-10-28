# Code-Prüfer

Desktop-App (Windows und macOS) für Studierende: Programmieraufgaben **lokal** gegen
eigene Testfälle prüfen.

1. Aufgabenstellung einfügen (dient zur Orientierung).
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
   (Auswahlfeld „Container-Engine“ im Statusbalken);
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
unterstützt kein TLS); dafür zeigt der Statusbalken eine Warnung.

### Runner-Images (Python, Java)

Release-Installer enthalten die Runner-Images als komprimierte Archive
(`images/code-pruefer-<sprache>-<arch>.tar.gz` im Ressourcenordner der App). Sobald die
Engine läuft, lädt die App fehlende Images automatisch in die **gewählte** Engine
(Docker und Podman verwenden dasselbe Image-Format, aber getrennte Image-Speicher) – ganz
ohne Internet und ohne manuelles `docker build`. Fortschritt wird im Statusbalken angezeigt.

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

**„Images aktualisieren“** (Button im Statusbalken) lädt die gepinnten Images dieser
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

## Ausgabevergleich

Zeilenenden (CRLF/LF) werden immer vereinheitlicht. Standardmäßig werden außerdem
Leerzeichen am Zeilenende und Leerzeilen am Ende ignoriert (abschaltbar).

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
```

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
npm run tauri build -- --config src-tauri/tauri.release.conf.json
# Windows-Installer unter Linux:
npm run tauri build -- --target x86_64-pc-windows-gnu --bundles nsis --config src-tauri/tauri.release.conf.json
```

Portable Windows-Version: den Ordner `images` (mit den Archiven und `image-digests.json`)
neben `code-pruefer.exe` legen – unter Windows ist der Tauri-Ressourcenordner immer der
Ordner der exe.

### Projektstruktur

```
src/                     React-Frontend (Monaco-Editor, Testfälle, Ergebnisse)
src-tauri/src/
  lib.rs                 Tauri-Commands: docker_status, get_engine, set_engine,
                         start_engine, list_languages, run_tests,
                         prepare_images, update_images
  docker_detect.rs       Erkennung: nicht installiert / nicht gestartet / ok
  engine.rs              Engine-Auswahl (Datei/Registry), Start, Podman-Pipe
  runner/mod.rs          Ablauf einer Prüfung, Bewertung der Testfälle
  runner/docker.rs       Engine-Verbindung, Image-Pull/-Load, gehärteter Container
  runner/images.rs       Image-Auflösung: vorhanden → Archiv → GHCR → :dev
  runner/languages.rs    Sprachtabelle (Image, Dateiname, Befehle, Zeitlimits)
  runner/compare.rs      Normalisierung, Zeilen-Diff, Ausgabebegrenzung
src-tauri/windows/       NSIS-Installer: Template + Hooks (Engine-Auswahl, winget)
src-tauri/tauri.release.conf.json
                         Release-Overlay: bündelt resources/images/*.tar.gz
                         und resources/images/image-digests.json
images/                  Dockerfiles der Runner-Images
scripts/export-image-archives.sh
                         Image-Archive und image-digests.json erzeugen
.github/workflows/       ci.yml, images.yml (edge), release.yml (Tags v*)
.github/dependabot.yml   Updates für Actions, npm, cargo, Basis-Images
```

Eine neue Sprache benötigt einen Eintrag in `runner/languages.rs` und ein Dockerfile
unter `images/`.

## Einschränkungen des MVP

- Nur Python und Java; Java-Code muss eine Klasse `Main` enthalten (Datei `Main.java`).
- Testfälle werden manuell eingegeben; keine Speicherung von Aufgaben/Testfällen.
- Vergleich nur über stdout (exakt bzw. mit Whitespace-Toleranz).
- Builds sind nicht signiert (Windows SmartScreen / macOS Gatekeeper warnen; Signatur ist
  vorbereitet), kein Auto-Update der App (die Images lassen sich über „Images aktualisieren“ neu laden).
- Mit eingebetteten Images ist der Installer deutlich größer (Java-Image ≈ 200 MB).
- Benötigt eine Container-Engine (Docker Desktop oder Podman); der Windows-Installer kann
  sie über winget nachinstallieren.

Geplant für spätere Phasen: weitere Sprachen (C/C++/C#/JavaScript), signierter
Aufgabenkatalog, Code-Signing, Updater, automatisch generierte Tests.
