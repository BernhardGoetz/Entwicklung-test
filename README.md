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
zu installieren (`Docker.DockerDesktop` bzw. `RedHat.Podman`; Windows fragt nach
Administratorrechten, ggf. ist ein Neustart nötig). Ohne `winget` wird die offizielle
Download-Seite geöffnet. Schlägt die Installation der Engine fehl, wird der Code-Prüfer
trotzdem installiert. Bei stiller/passiver Installation (`/S`, `/P`) wird nichts gefragt
und nichts nachinstalliert.

Umsetzung: `src-tauri/windows/hooks.nsh` (Installer-Hooks mit Auswahlseite und
winget-Aufruf) und `src-tauri/windows/installer.nsi` – das unveränderte Tauri-Template
(tauri-cli 2.12.4) mit **einer** Ergänzung vor der Installationsseite, die die Seite aus
`hooks.nsh` einfügt. Bei einem Update der Tauri-CLI das Template neu kopieren und diese
Stelle (`CODE_PRUEFER_CUSTOM_PAGES`) wieder einfügen.

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

- **Docker Desktop:** `Docker Desktop.exe` wird gestartet (macOS: `open -a Docker`).
- **Podman:** existiert noch keine Maschine, wird einmalig `podman machine init`
  ausgeführt (lädt ein VM-Image, kann einige Minuten dauern), danach
  `podman machine start`. Unter Windows verbindet sich die App über die Named Pipe, die
  `podman machine inspect` meldet (Standard: `\\.\pipe\podman-machine-default`).
- **Linux:** kein automatischer Start (Systemdienst) – bitte Docker bzw. Podman selbst
  starten.

Die Engine wird nicht mit Windows gestartet, sondern nur mit dem Code-Prüfer.

Die App erkennt die Engine automatisch (Docker-Socket, Docker-Desktop-Socket unter
`~/.docker/run/docker.sock`, Podman-Sockets, Named Pipes `docker_engine` und
`podman-machine-default` unter Windows); die Endpunkte der gewählten Engine werden
zuerst probiert. Mit der Umgebungsvariable `DOCKER_HOST` (z. B.
`unix:///pfad/zum/podman.sock` oder `npipe:////./pipe/podman-machine-default`) lässt
sich der Endpunkt explizit festlegen; sie hat Vorrang vor der Engine-Auswahl.

Beim ersten Prüfen einer Sprache wird das passende Runner-Image heruntergeladen
(Fortschritt wird angezeigt); danach funktioniert die Prüfung offline.

## Sicherheit der Ausführung

Jede Prüfung startet einen frischen Container, der danach immer gelöscht wird:

- kein Netzwerk, read-only Root-Dateisystem, `tmpfs` für `/work` und `/tmp` (je 64 MB)
- 512 MB RAM ohne Swap, 1 CPU, max. 64 Prozesse, max. Dateigröße 16 MB
- nicht-root (UID 1000), alle Capabilities entfernt, `no-new-privileges`
- Zeitlimit pro Test: Python 5 s, Java 8 s; Kompilieren 30 s
- stdout/stderr werden nach 64 KB abgeschnitten (wird angezeigt)

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

# App mit den lokalen Images starten
CODE_PRUEFER_PYTHON_IMAGE=code-pruefer-python:dev \
CODE_PRUEFER_JAVA_IMAGE=code-pruefer-java:dev \
npm run tauri dev

# Integrationstests (benötigen Docker, standardmäßig mit den :dev-Images)
cd src-tauri && cargo test -- --ignored --test-threads=1
```

Veröffentlichte Images: `ghcr.io/bernhardgoetz/code-pruefer-python:0.1.0` und
`ghcr.io/bernhardgoetz/code-pruefer-java:0.1.0` (multi-arch amd64/arm64, gebaut durch
`.github/workflows/images.yml`).

### Projektstruktur

```
src/                     React-Frontend (Monaco-Editor, Testfälle, Ergebnisse)
src-tauri/src/
  lib.rs                 Tauri-Commands: docker_status, get_engine, set_engine,
                         start_engine, list_languages, run_tests
  docker_detect.rs       Erkennung: nicht installiert / nicht gestartet / ok
  engine.rs              Engine-Auswahl (Datei/Registry), Start, Podman-Pipe
  runner/mod.rs          Ablauf einer Prüfung, Bewertung der Testfälle
  runner/docker.rs       Engine-Verbindung, Image-Pull, gehärteter Container
  runner/languages.rs    Sprachtabelle (Image, Dateiname, Befehle, Zeitlimits)
  runner/compare.rs      Normalisierung, Zeilen-Diff, Ausgabebegrenzung
src-tauri/windows/       NSIS-Installer: Template + Hooks (Engine-Auswahl, winget)
images/                  Dockerfiles der Runner-Images
```

Eine neue Sprache benötigt einen Eintrag in `runner/languages.rs` und ein Dockerfile
unter `images/`.

## Einschränkungen des MVP

- Nur Python und Java; Java-Code muss eine Klasse `Main` enthalten (Datei `Main.java`).
- Testfälle werden manuell eingegeben; keine Speicherung von Aufgaben/Testfällen.
- Vergleich nur über stdout (exakt bzw. mit Whitespace-Toleranz).
- Builds sind nicht signiert (Windows SmartScreen / macOS Gatekeeper warnen), kein
  Auto-Update.
- Benötigt eine Container-Engine (Docker Desktop oder Podman); der Windows-Installer kann
  sie über winget nachinstallieren.

Geplant für spätere Phasen: weitere Sprachen (C/C++/C#/JavaScript), signierter
Aufgabenkatalog, Code-Signing, Updater, automatisch generierte Tests.
