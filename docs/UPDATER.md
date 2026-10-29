# App-Updater

Der Code-Prüfer sucht nach der ersten Installation selbst nach neuen Versionen und bietet
sie in einem Dialog an. Angeboten wird nur, was auf GitHub als **Release veröffentlicht**
ist, also der abgenommene Stand aus [`RELEASE_QAS.md`](RELEASE_QAS.md). QAS-Pre-Releases
und Entwürfe werden nie angeboten.

Kennzeichnung: **[geprüft]** = im Code/Quelltext nachvollzogen oder lokal getestet,
**[ungeprüft]** = konnte hier nicht laufen (echtes GitHub-Release, Windows, macOS).

## So funktioniert es

| Was | Wie |
|---|---|
| Komponente | offizielles [`tauri-plugin-updater`](https://v2.tauri.app/plugin/updater/) 2.14.0, nur aus Rust benutzt (`src-tauri/src/app_update.rs`). Die Oberfläche bekommt **keine** Updater- oder Prozess-Rechte, sie ruft nur die eigenen Befehle `update_info`, `set_update_interval`, `check_for_update`, `install_update` auf. |
| Quelle | fest im Code: `https://github.com/bernhardgoetz/entwicklung-test/releases/latest/download/latest.json` (nur HTTPS) |
| Prüfung | Jede heruntergeladene Datei wird vor der Installation gegen den öffentlichen Schlüssel geprüft, der in die App eingebaut ist (minisign/Ed25519, Pflicht im Plugin). Zusätzlich `requireSignedVersion`: Die Signatur muss genau die Version nennen, die `latest.json` ankündigt. Eine manipulierte `latest.json` kann also keine alte, gültig signierte Datei als „neue Version“ unterschieben. **[geprüft lokal]** |
| Zeitpunkt | 5 s nach dem Start und danach stündlich, solange die App läuft. Gefragt wird GitHub aber nur, wenn der gewählte Abstand seit der letzten erfolgreichen Suche vergangen ist: **Täglich** (Standard, 24 h), **Wöchentlich** oder **Nie**. |
| Speicherort | `updater.json` im Konfigurationsordner der App (Windows `%APPDATA%\de.bernhardgoetz.codepruefer\`, macOS `~/Library/Application Support/de.bernhardgoetz.codepruefer/`): Abstand und Zeitpunkt der letzten erfolgreichen Suche. Kein `localStorage`. |
| Fehler | Die automatische Suche meldet nichts (nur Protokoll auf stderr). Offline wird beim nächsten Start bzw. zur nächsten vollen Stunde erneut gesucht. „Jetzt nach Updates suchen“ zeigt eine deutsche Meldung. |
| Laufende Arbeit | Läuft eine Prüfung, die KI, die Texterkennung-Korrektur, die Image-Vorbereitung oder ein Engine-Start, oder ist ein anderer Dialog offen, erscheint der Dialog erst danach. Wird er trotzdem (über die Schaltfläche) geöffnet, während etwas läuft, ist „Jetzt aktualisieren“ gesperrt, mit Hinweis. Vor der Installation beendet die App llama-server und ihre Container (sonst wären Dateien im Programmordner gesperrt). |

### Der Dialog

„Update verfügbar: Version X (installiert: Y)“, darunter die Versionshinweise als
**reiner Text** (React setzt sie als Textknoten, HTML/Markdown wird nicht ausgewertet;
Steuerzeichen werden entfernt, höchstens 4000 Zeichen). Hinweis, dass ungespeicherte
Eingaben beim Neustart verloren gehen.

- **„Jetzt aktualisieren“**: Download mit Fortschritt → Signaturprüfung → Installation →
  Neustart. Während dessen lässt sich der Dialog nicht schließen.
  - Windows: Der NSIS-Installer startet im **Passiv-Modus** (`installMode: "passive"` →
    `/P /UPDATE /R`): keine Fragen, nur Fortschrittsanzeige, danach startet er die App
    neu. Die Engine-Auswahl aus der Erstinstallation bleibt erhalten, die Seite
    „Container-Engine“ und die winget-Nachfrage entfallen (`hooks.nsh` überspringt sie
    bei `/P`). Installiert wird wie bisher pro Benutzer (ohne UAC). **[ungeprüft unter
    Windows]**
  - macOS: Die `.app` wird ersetzt (liegt sie in `/Applications` ohne Schreibrecht,
    fragt macOS nach dem Admin-Passwort), danach Neustart. **[ungeprüft]**
- **„Später“** (oder „Schließen“/Esc): schließt. Gefragt wird bei der nächsten fälligen
  Suche wieder (bei „Täglich“ also frühestens nach 24 h). „Version überspringen“ gibt es
  bewusst nicht.

### Einstellungen → Updates

Installierte Version, „Automatisch suchen“ (Täglich · Wöchentlich · Nie), „Zuletzt
gesucht“ und „Jetzt nach Updates suchen“. Sonderfälle:

- **Ohne eingebauten Schlüssel** (lokale Builds, CI, `tauri dev`, Releases ohne die
  Variable `TAURI_UPDATER_PUBKEY`): „Updates in dieser Version nicht verfügbar“. Das
  Plugin wird dann gar nicht registriert, die App sucht nie. Hintergrund: Das Plugin
  verlangt in seiner Konfiguration einen Schlüssel; fehlt er, würde der Start der App
  abbrechen. **[geprüft: tauri-plugin-updater 2.14.0, Quelltext; Start ohne Schlüssel
  lokal getestet]**
- **Portable Windows-Version** (ZIP, ohne `uninstall.exe` neben `code-pruefer.exe`):
  kein Selbst-Update, nur ein Link zur Release-Seite. Die Rechte der Oberfläche
  erlauben genau diese eine zusätzliche URL
  (`https://github.com/bernhardgoetz/entwicklung-test/releases`,
  `capabilities/default.json`, abgeglichen per Test in `lib.rs`).

### Warum `releases/latest` keine Pre-Releases liefert

GitHub definiert das „latest release“ als das neueste **veröffentlichte, nicht als
Pre-Release markierte** Release; Entwürfe sind nicht öffentlich. `…/releases/latest/
download/<datei>` leitet auf die Datei dieses Releases um (REST-Doku „Get the latest
release“, Hilfe „Linking to releases“). **[ungeprüft hier, github.com war aus der
Entwicklungsumgebung nicht abrufbar; Stand der GitHub-Doku]** Zusätzlich:

- QAS-Pre-Releases bekommen nie eine `latest.json` (der Job `qas-release` bricht ab, wenn
  dort eine `*.json`, `*.sig` oder `*.tar.gz` liegt).
- Beim Veröffentlichen des Entwurfs **„Set as the latest release“ angehakt lassen**
  (Standard). Sonst zeigt `latest` weiter auf das vorige Release.
- Hat das neueste Release keine `latest.json` (z. B. ohne eingerichtete Secrets
  veröffentlicht), findet die App nichts: automatische Suche still, manuelle Suche „Auf
  GitHub wurden keine gültigen Update-Informationen gefunden“.

## Release-Workflow

`.github/workflows/release.yml`:

1. **build-windows** und **bundle-macos** schreiben mit `node scripts/updater.mjs
   config` das Overlay `src-tauri/tauri.updater.conf.json` aus der Repository-Variable
   `TAURI_UPDATER_PUBKEY` (Schlüssel, `requireSignedVersion: true`,
   `windows.installMode: "passive"`) und bauen mit `--config` dazu. Ohne Variable
   entsteht `{}` (Warnung) und die App läuft ohne Updater. Eine Code-Änderung nach dem
   Erzeugen des Schlüssels ist nicht nötig.
2. **bundle-macos** packt die fertige (ggf. signierte und notarisierte) `.app` als
   `Code-Pruefer_<version>_aarch64.app.tar.gz` bzw. `_x64.app.tar.gz` (Artefakte
   `updater-macos-*`, nicht im QAS-Pre-Release).
3. **updater-sign** (neu, nach `promote-windows` und `bundle-macos`, Environment
   `release`): signiert mit `tauri signer sign --app-version <version>` den
   **endgültigen** Windows-Installer aus `promote-windows`, also nach der letzten
   SignPath-Runde, und die beiden macOS-Archive. Danach prüft
   `node scripts/updater.mjs verify` jede Signatur gegen `TAURI_UPDATER_PUBKEY` (gleicher
   Algorithmus wie das Plugin). Passt sie nicht, schlägt der Job fehl.
   - Warum nicht `bundle.createUpdaterArtifacts`? Dann signiert `tauri bundle` die
     Updater-Dateien **im selben Lauf** wie das Bündeln. Unter Windows wäre das vor der
     letzten SignPath-Runde, die Authenticode-Signatur würde die Datei danach ändern und
     die Updater-Signatur ungültig machen. Außerdem bräuchte der Build-Job den privaten
     Schlüssel (QAS-Environment), und ohne Schlüssel bricht `tauri build` dann ab.
     **[geprüft: tauri-cli 2.12.4, `bundle.rs` `sign_updaters`]** Die `.app.tar.gz`
     entsteht deshalb mit `tar` genauso wie beim Bundler (`.app` als oberstes Verzeichnis).
   - Wegen `--no-binary-patching` fehlt der Windows-Exe die Bundle-Markierung
     „nsis“. Das stört nicht: Der Updater nimmt dann den Eintrag `windows-x86_64` und erkennt den
     NSIS-Installer am Dateiinhalt. **[geprüft im Quelltext 2.14.0]**
4. **release** benennt die Dateien in ASCII um (`Code-Prüfer_…` → `Code-Pruefer_…`,
   GitHub ersetzt Umlaute in Asset-Namen). Dann schreibt es `latest.json`
   (`version`, `notes`, `pub_date`, `platforms` `windows-x86_64` / `darwin-aarch64` /
   `darwin-x86_64` mit `url` auf das Asset unter `releases/download/v<version>/` und
   `signature`; jede Signatur wird dabei noch einmal geprüft). `latest.json` und die
   `.app.tar.gz` kommen in `SHA256SUMS.txt`, in die Attestierung und ins
   Entwurfs-Release. Nach dem Hochladen prüft der Job, dass alle Asset-Namen exakt
   stimmen.
   - Versionshinweise: Liegt `docs/updates/<version>.txt` (Klartext, UTF-8) im
     QAS-Stand, steht ihr Inhalt im Dialog. Sonst steht dort ein Standardtext mit
     Hinweis auf die Release-Seite.
5. Fehlen Secret **und** Variable: keine Updater-Dateien, Warnung in der
   Zusammenfassung, das Release läuft sonst normal durch, wie bei SignPath. Ist nur eins
   von beiden gesetzt, bricht `updater-sign` ab (Tippfehler sollen nicht still ohne
   Updater enden).

## Einrichtung durch Bernhard (einmalig)

### 1. Schlüssel erzeugen (lokal, nie in CI)

```bash
npm run tauri signer generate -- -w ~/.tauri/code-pruefer-updater.key
```

Dabei ein **Passwort** vergeben. Es entstehen
`~/.tauri/code-pruefer-updater.key` (privat) und `~/.tauri/code-pruefer-updater.key.pub`
(öffentlich).

**Wichtig:** Der private Schlüssel und sein Passwort sind die einzige Möglichkeit,
Updates zu signieren, die installierte Apps annehmen. **Gehen sie verloren, bekommen alle
installierten Versionen keine Updates mehr.** Die Nutzer müssen dann einmal von Hand eine
Version mit neuem Schlüssel installieren. Deshalb:

- Schlüsseldatei und Passwort getrennt sichern, z. B. Datei verschlüsselt auf einem
  USB-Stick oder im Passwort-Manager als Anhang, Passwort im Passwort-Manager.
- Nie ins Repository, nie in Chats/Tickets. `*.key` wird hier nirgends benötigt außer im
  GitHub-Secret.
- Wird der Schlüssel öffentlich, kann jeder „Updates“ signieren. Dann sofort ein neues
  Paar erzeugen, die Secrets und die Variable ersetzen und ein Release ausliefern.
  Installierte Apps mit dem alten Schlüssel nehmen dieses Release allerdings nicht an
  (siehe oben).

### 2. GitHub einrichten

| Wo | Name | Inhalt |
|---|---|---|
| Settings → Secrets and variables → Actions → **Variables → Repository variables** | `TAURI_UPDATER_PUBKEY` | Inhalt von `code-pruefer-updater.key.pub` (eine Zeile Base64, so wie er in der Datei steht) |
| Settings → Environments → **`release`** → Environment secrets | `TAURI_SIGNING_PRIVATE_KEY` | Inhalt von `code-pruefer-updater.key` (eine Zeile Base64) |
| Settings → Environments → **`release`** → Environment secrets | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | das Passwort |

Der öffentliche Schlüssel ist kein Geheimnis. Er muss eine **Repository**-Variable sein,
weil `build-windows` (ohne Environment) und `bundle-macos` (Environment `qas`) ihn
einbauen. Der private Schlüssel liegt nur in `release` und wird erst nach der Abnahme
benutzt.

### 3. Erstes Release mit Updater

- Die erste Version **mit** Updater muss jeder einmal von Hand installieren. Ältere
  Versionen haben keinen Updater.
- Ab der folgenden Version kommt das Update automatisch: Version erhöhen, QAS-Lauf,
  Abnahme, Entwurf prüfen (enthält `latest.json`, `*.app.tar.gz`, Installer, DMGs) und
  veröffentlichen, mit „Set as the latest release“ angehakt.
- Kontrolle nach dem Veröffentlichen:
  `curl -sL https://github.com/bernhardgoetz/entwicklung-test/releases/latest/download/latest.json`
  muss die neue Version zeigen.

## Testen

### Lokal (ohne GitHub, so wurde hier getestet)

Debug-Builds akzeptieren über `CODE_PRUEFER_UPDATE_ENDPOINT` einen anderen Endpunkt,
auch `http://` (nur Debug, in Release-Builds ist der Endpunkt fest).

```bash
# Wegwerf-Schlüssel AUSSERHALB des Repositorys (nie den echten verwenden)
T=$(mktemp -d)
npm run tauri signer generate -- --ci -p test -w "$T/test.key"
TAURI_UPDATER_PUBKEY="$(cat "$T/test.key.pub")" node scripts/updater.mjs config "$T/overlay.json"

# Dummy-Paket signieren und latest.json schreiben (Version höher als installiert)
head -c 300 /dev/urandom > "$T/dummy.bin"
TAURI_SIGNING_PRIVATE_KEY="$(cat "$T/test.key")" TAURI_SIGNING_PRIVATE_KEY_PASSWORD=test \
  npm run tauri signer sign -- --app-version 9.9.9 "$T/dummy.bin"
# latest.json von Hand: {"version":"9.9.9","notes":"…","pub_date":"2026-10-29T10:00:00Z",
#   "platforms":{"linux-x86_64":{"signature":"<Inhalt von dummy.bin.sig>",
#   "url":"http://127.0.0.1:8765/dummy.bin"}}}  (für Windows: "windows-x86_64")
(cd "$T" && python3 -m http.server 8765 --bind 127.0.0.1) &

rm -f ~/.config/de.bernhardgoetz.codepruefer/updater.json   # Linux: „fällig“ erzwingen
CODE_PRUEFER_UPDATE_ENDPOINT=http://127.0.0.1:8765/latest.json \
  npm run tauri dev -- --config "$T/overlay.json"
# danach: rm -rf "$T"
```

Ergebnis hier (Linux, Xvfb, `tauri dev`): Dialog nach ca. 5 s mit Version 9.9.9 und
Notizen (ein `<script>`-Tag erscheint als Text). „Jetzt aktualisieren“ lädt herunter, die
Signatur ist gültig, und die Linux-Installation lehnt die Zufallsdatei ab („passt nicht zu
diesem System“). Ein manipuliertes Paket ergibt „Die Signatur des Updates ist ungültig“,
eine `latest.json` mit anderer Version als signiert ebenfalls. Ohne Overlay startet die
App normal, sucht nicht und zeigt „Updates in dieser Version nicht verfügbar“.
**[geprüft]** Ein echter Installer-Austausch unter Windows/macOS ist damit nicht getestet.

### Echtes Update unter Windows/macOS (empfohlen vor dem ersten produktiven Einsatz)

Ohne Risiko für die echten Nutzer, auf einem **Fork**:

1. Repository forken (z. B. `bernhardgoetz-test/entwicklung-test`). Im Fork in
   `src-tauri/src/app_update.rs` `ENDPOINT` und `RELEASES_URL` auf den Fork ändern
   (`capabilities/default.json` mit, der Test `cargo test` erinnert daran).
2. Im Fork die Environments wie oben anlegen, mit einem **eigenen Test-Schlüssel**
   (nicht dem echten). SignPath/Apple können fehlen.
3. Version `0.9.0` per QAS-Lauf → Abnahme → veröffentlichen → installieren.
4. Version `0.9.1` genauso veröffentlichen. Die installierte 0.9.0 muss beim nächsten
   Start (oder über „Jetzt nach Updates suchen“) das Update anbieten, passiv
   installieren und als 0.9.1 neu starten. Dabei prüfen: Engine-Auswahl erhalten,
   Startmenü-Eintrag unverändert, laufende KI wurde vorher beendet.

Ein Pre-Release darf dabei nie angeboten werden: zum Test einen QAS-Lauf ohne Abnahme
stehen lassen. Die App bietet weiter nur das veröffentlichte Release an.

## Grenzen

- Windows-Installation (passiv, Neustart), macOS-Austausch der `.app` und die Rechte bei
  `/Applications` sind nicht getestet, nur der Ablauf bis zur Signaturprüfung (Linux).
- Der Workflow konnte nicht auf GitHub laufen. Geprüft sind die Skriptteile (Signieren,
  Prüfen, Umbenennen, `latest.json`) lokal mit Wegwerf-Schlüssel und Dummy-Dateien sowie
  actionlint/zizmor.
- Eine Proxy-Umgebung nimmt das Plugin aus den Systemeinstellungen (Windows/macOS,
  Feature `system-proxy`). **[ungeprüft]**
