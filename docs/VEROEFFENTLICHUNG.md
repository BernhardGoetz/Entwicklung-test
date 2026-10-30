# Veröffentlichung einrichten: Schritt für Schritt

Anleitung für Bernhard (GitHub `bernhardgoetz`, Repository
`bernhardgoetz/entwicklung-test`, Standard-Branch `main`). Hier steht alles, was einmal
eingerichtet werden muss, und was danach für jede Version zu tun ist. Die Details stehen in
[`RELEASE_QAS.md`](RELEASE_QAS.md), [`RELEASE_SIGNING.md`](RELEASE_SIGNING.md) und
[`UPDATER.md`](UPDATER.md). Bei Widersprüchen gilt
[`.github/workflows/release.yml`](../.github/workflows/release.yml).

> **Ungetestet:** Keiner der Workflows (`release.yml`, `pin-models.yml`, `ci.yml`,
> `images.yml`) ist bisher auf GitHub gelaufen. Geprüft sind Syntax und Sicherheitsregeln
> (actionlint, zizmor) und Skriptteile lokal. Rechne beim ersten Lauf mit Nacharbeit.
> Punkte, die ich nicht nachprüfen konnte (GitHub-/SignPath-Oberfläche), sind mit
> **(ungeprüft)** markiert.

## 0. Überblick

```
 Feature-Branch ──PR──► main ──► Branch qas (push oder „Run workflow“ auf qas)
                                    │
  Workflow „Release (QAS und Abnahme)“ (.github/workflows/release.yml)
                                    ▼
  version ─ Versionen gleich? v<version> frei? Modell-Pins gegen Hugging Face prüfen
     ├─ images ─────────── Images nach GHCR (:<version>-qas.<n>, :sha-<commit>)
     ├─ build-windows ──── code-pruefer.exe + KI-Laufzeit, unsigniert
     ├─ bundle-windows ─── Installer, SignPath Test-Signing  (Environment qas)
     └─ bundle-macos ───── DMGs, Apple-Signatur optional      (Environment qas)
                                    ▼
  qas-release ─ Pre-Release v<version>-qas.<n> zum Testen
                                    ▼
  abnahme ───── wartet auf dich: „Approve and deploy“          (Environment production)
                                    ▼
     ├─ promote-images ─── :<version> = derselbe Digest wie getestet
     └─ promote-windows ── dieselben Dateien, Release-Signing   (Environment release)
                                    ▼
  updater-sign ─ Updater-Signaturen (minisign)                 (Environment release)
                                    ▼
  release ───── Tag v<version> + ENTWURFS-Release (latest.json, SHA256SUMS.txt, Attestierung)
                                    ▼
  pull-request ─ PR qas → main

  Du: Entwurf veröffentlichen („Set as the latest release“) → installierte Apps fragen
  releases/latest/download/latest.json ab und bieten das Update an.
```

**Kosten**

| Was | Kosten | Pflicht? |
|---|---|---|
| GitHub Actions (Standard-Runner inkl. Windows/macOS), Releases, GHCR | kostenlos für öffentliche Repositories | ja |
| SignPath Foundation (Windows-Signatur) | kostenlos für Open Source, nach Antrag und Prüfung | nein, ohne: unsigniert mit Warnung |
| Apple Developer Program (macOS-Signatur und Notarisierung) | 99 USD pro Jahr | nein, ohne: unsignierte DMGs |
| Update-Schlüssel (App-Updater) | kostenlos, lokal erzeugt | nein, aber dringend empfohlen (ohne ihn bekommt die Version nie Updates) |

Ohne SignPath, Apple und Update-Schlüssel läuft die Pipeline trotzdem durch, nur mit
Warnungen. Du kannst also mit den Schritten 1a–1e anfangen und SignPath (1f) und Apple
(1g) später ergänzen.

## 1. Einmalige Vorbereitung

Reihenfolge einhalten. Wichtig: **Den Branch `qas` erst in Abschnitt 2 anlegen.** Jeder
Push auf `qas` (auch das Anlegen des Branches) startet sofort einen Release-Lauf. Bis dahin
müssen die Environments existieren, sonst legt GitHub sie beim ersten Lauf automatisch
**ohne** Schutzregeln an (siehe Abschnitt 5).

### a. GitHub-Grundeinstellungen

- [ ] **2FA einschalten:** Profilbild → Settings → Password and authentication →
      „Enable two-factor authentication“. SignPath verlangt MFA bei GitHub und SignPath.
- [ ] **Repository öffentlich** (Settings → General → Danger Zone → Change visibility).
      Nötig für die SignPath Foundation und für kostenlose Runner.
- [ ] **Diesen Stand nach `main` bringen:** `main` enthält bisher nur den „Initial
      commit“ ohne Workflows. Pull Request `claude/wonderful-pascal-maugv1` → `main`
      anlegen und mergen. Grund: Den Knopf „Run workflow“ zeigt GitHub nur für Workflows,
      die auf dem Standard-Branch liegen.
- [ ] **Actions erlauben:** Settings → Actions → General → „Actions permissions“:
      „Allow all actions and reusable workflows“. Wenn du einschränkst, müssen erlaubt
      sein: `actions/*`, `docker/*`, `dtolnay/rust-toolchain`, `Swatinem/rust-cache`,
      `signpath/github-action-submit-signing-request` (alle per Commit-SHA gepinnt).
- [ ] **Workflow permissions** (gleiche Seite, unten):
  - [ ] „Read repository contents and packages permissions“ (die Jobs fordern ihre
        Rechte selbst an).
  - [ ] Haken bei **„Allow GitHub Actions to create and approve pull requests“**. Sonst
        scheitern `pin-models.yml` (Pin-PR) und der Job `pull-request` (PR `qas` → `main`).
  - [ ] „Save“.
- [ ] **„Automatically delete head branches“ aus lassen** (Settings → General → Pull
      Requests). Sonst löscht GitHub `qas` nach dem Merge des PRs `qas` → `main`.
- [ ] **GHCR-Pakete öffentlich machen.** Das geht erst nach dem ersten Push der Images
      (Abschnitt 2). Die Pakete heißen klein geschrieben
      `ghcr.io/bernhardgoetz/code-pruefer-python` und
      `ghcr.io/bernhardgoetz/code-pruefer-java`. Profil → Packages → Paket → „Package
      settings“ → Danger Zone → „Change visibility“ → Public. Sonst funktionieren der
      Image-Download und „Images aktualisieren“ in der App nicht.

Vorarbeit von mir in: `.github/workflows/*.yml` (`permissions: {}` auf Workflow-Ebene,
Rechte je Job), README-Abschnitt „Nötige GitHub-Einstellungen“.

### b. Rulesets für Branch `qas` und Tags `v*`

Rulesets wirken auch auf Branches, die es noch nicht gibt. Lege sie deshalb jetzt an.

**Branch-Ruleset `qas`:** Settings → Rules → Rulesets → New ruleset → „New branch
ruleset“.

- [ ] Ruleset Name: `qas`, Enforcement status: **Active**.
- [ ] Bypass list → „Add bypass“ → Rolle **Repository admin** (das bist du).
- [ ] Target branches → „Add target“ → „Include by pattern“ → `qas`.
- [ ] Rules: **Restrict deletions**, **Block force pushes**, dazu **Restrict updates**
      (dann darf nur, wer in der Bypass-Liste steht, auf `qas` pushen oder mergen)
      **(ungeprüft: ob „Restrict updates“ mit Admin-Bypass genau so wirkt)**.
- [ ] „Create“.

Warum: Die Environments `qas` und `release` vertrauen jedem Lauf auf `qas`. Wer auf `qas`
pushen kann, kommt an deren Secrets.

**Tag-Ruleset `v*`:** Settings → Rules → Rulesets → New ruleset → „New tag ruleset“.

- [ ] Name: `release-tags`, Enforcement status: Active.
- [ ] Target tags → „Include by pattern“ → `v*`.
- [ ] Rules: **Restrict updates**, **Restrict deletions**.
- [ ] **„Restrict creations“ NICHT anhaken.** Der Workflow legt die Tags
      `v<version>-qas.<n>` (Job `qas-release`) und `v<version>` (Job `release`) selbst mit
      dem `GITHUB_TOKEN` an. Alternative: „Restrict creations“ an und die App „GitHub
      Actions“ in die Bypass-Liste **(ungeprüft: ob „GitHub Actions“ dort auswählbar
      ist)**.

Vorarbeit von mir in: `docs/RELEASE_QAS.md` („Nötige GitHub-Einstellungen“), README.

### c. Environments, Secrets und Variablen

Settings → Environments → „New environment“. Für jedes der drei Environments:
„Deployment branches and tags“ → „Selected branches and tags“ → „Add deployment branch or
tag rule“ → Ref type **Branch**, Name pattern **`qas`**.

- [ ] **`qas`** (Jobs `bundle-windows`, `bundle-macos`): keine Reviewer, Branch `qas`.
- [ ] **`production`** (Job `abnahme`): Branch `qas`, **Required reviewers** anhaken →
      `bernhardgoetz` → „Save protection rules“. **„Prevent self-review“ aus lassen**,
      weil du selbst pushst und selbst abnimmst. Keine Secrets.
- [ ] **`release`** (Jobs `promote-windows`, `updater-sign`): Branch `qas`, **keine**
      Reviewer (die Freigabe passiert in `production`), **keine** Tag-Regel `v*`.

Secrets: im Environment unter „Environment secrets“ → „Add environment secret“.
Variablen: „Environment variables“ → „Add environment variable“. Repository-Variable:
Settings → Secrets and variables → Actions → Tab **Variables** → „New repository
variable“.

| Wo | Name | Art | Wert | Wann |
|---|---|---|---|---|
| Repository | `TAURI_UPDATER_PUBKEY` | Variable | Inhalt von `code-pruefer-updater.key.pub` (Schritt 1e) | sobald der Schlüssel existiert |
| Env `release` | `TAURI_SIGNING_PRIVATE_KEY` | Secret | Inhalt von `code-pruefer-updater.key` | zusammen mit der Variable |
| Env `release` | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Secret | Passwort des Schlüssels | zusammen mit der Variable |
| Env `qas` | `SIGNPATH_API_TOKEN` | Secret | API-Token aus SignPath | nach 1f |
| Env `qas` | `SIGNPATH_ORGANIZATION_ID` | Variable | Organisations-ID aus SignPath | nach 1f |
| Env `qas` | `SIGNPATH_PROJECT_SLUG` | Variable | Projekt-Slug, z. B. `code-pruefer` | nach 1f |
| Env `qas` | `SIGNPATH_SIGNING_POLICY_SLUG` | Variable | `test-signing` | nach 1f |
| Env `qas` | `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` | Variable | nur wenn die Artefaktkonfiguration nicht Default ist | optional |
| Env `qas` | `SIGNPATH_SIGN_UPSTREAM` | Variable | `true`, nur nach schriftlicher Zusage der Foundation | optional |
| Env `release` | dieselben fünf/sechs `SIGNPATH_*` | wie oben | wie oben, aber `SIGNPATH_SIGNING_POLICY_SLUG` = `release-signing` | nach 1f |
| Env `qas` | `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | Secrets | siehe 1g | optional |

Regeln, die der Workflow erzwingt:

- SignPath zählt nur als eingerichtet, wenn **alle vier** gesetzt sind
  (`SIGNPATH_API_TOKEN`, `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`,
  `SIGNPATH_SIGNING_POLICY_SLUG`). Keiner gesetzt: unsigniert mit Warnung. Ein Teil
  gesetzt: Abbruch.
- Der Slug der Test-Policy muss „test“ enthalten. Nur dann akzeptiert der Schritt
  „Signaturen prüfen“ das nicht vertrauenswürdige Testzertifikat. Der Release-Slug darf
  „test“ nicht enthalten.
- `SIGNPATH_SIGN_UPSTREAM` in `qas` und `release` **gleich** setzen. Sonst unterscheiden
  sich die Signatur-Manifeste und `promote-windows` bricht beim Vergleich mit dem
  QAS-Installer ab.
- `TAURI_UPDATER_PUBKEY` muss eine **Repository**-Variable sein: `build-windows` läuft
  ohne Environment und baut den Schlüssel in die App ein. Secret und Variable beim Updater
  nur zusammen setzen; nur eins von beiden bricht `updater-sign` ab.

Vorarbeit von mir in: `.github/workflows/release.yml` (Jobs `bundle-windows`,
`bundle-macos`, `abnahme`, `promote-windows`, `updater-sign`), Tabelle in
`docs/RELEASE_QAS.md`, `docs/RELEASE_SIGNING.md` Abschnitt 5, `docs/UPDATER.md`
Abschnitt „Einrichtung durch Bernhard“.

### d. KI-Modellkatalog pinnen

`src-tauri/src/ai/catalog.json` ist derzeit **nicht gepinnt** (`revision` leer, `files`
leer). Der Job `version` bricht dann jeden Release-Lauf ab. Pins entstehen nur über einen
Pull Request, den du prüfst.

- [ ] Actions → „KI-Modellkatalog pinnen“ → „Run workflow“ → Branch **`main`** → „Run
      workflow“. (Auf `qas` würde der Merge des PRs sofort einen Release-Lauf starten.)
- [ ] Der Lauf legt den PR „KI-Modellkatalog pinnen“ an (Branch
      `pin-models/run-<id>`). Im PR die Tabelle prüfen: Revision in der Commit-Historie
      von `Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF` bzw. `Qwen/Qwen2.5-Coder-7B-Instruct-GGUF`,
      Dateiname, Größe und SHA-256 unter „Files and versions“ auf huggingface.co.
- [ ] Die CI startet für diesen PR nicht von selbst (PRs des `GITHUB_TOKEN` lösen keine
      Workflows aus). Bei Bedarf PR schließen und wieder öffnen.
- [ ] PR mergen. Lokal prüfen: `scripts/pin-models.sh --check` meldet keinen Fehler mehr.

Vorarbeit von mir in: `.github/workflows/pin-models.yml`, `scripts/pin-models.sh`
(`--check`, `--verify`), `src-tauri/src/ai/catalog.json`.

### e. Update-Schlüssel erzeugen

Lokal im Repository (nach `npm ci`), nie in CI:

```bash
npm run tauri signer generate -- -w ~/.tauri/code-pruefer-updater.key
```

- [ ] Ein Passwort vergeben. Es entstehen `~/.tauri/code-pruefer-updater.key` (privat)
      und `~/.tauri/code-pruefer-updater.key.pub` (öffentlich).
- [ ] Inhalt von `.key.pub` → Repository-Variable `TAURI_UPDATER_PUBKEY`.
- [ ] Inhalt von `.key` → Secret `TAURI_SIGNING_PRIVATE_KEY` im Environment `release`.
- [ ] Passwort → Secret `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` im Environment `release`.
- [ ] **Sichern:** Schlüsseldatei und Passwort getrennt aufbewahren (z. B. Datei
      verschlüsselt auf USB-Stick, Passwort im Passwort-Manager). **Geht der Schlüssel
      verloren, bekommen alle installierten Versionen nie wieder Updates.** Nie ins
      Repository, nie in Chats.
- [ ] Vor der ersten echten Veröffentlichung erledigen. Eine Version ohne eingebauten
      Schlüssel kann sich nie selbst aktualisieren.

Vorarbeit von mir in: `scripts/updater.mjs` (`config`, `verify`, `manifest`),
`scripts/install-signer.sh`, `src-tauri/src/app_update.rs`, `docs/UPDATER.md`.

### f. SignPath Foundation (Windows-Signatur)

Dauert erfahrungsgemäß, weil die Foundation Anträge manuell prüft **(ungeprüft)**. Bis
dahin sind die Windows-Installer unsigniert (SmartScreen warnt, Smart App Control
blockiert).

1. [ ] **Antrag** auf <https://signpath.org/> → „Apply“ **(ungeprüft: genaue Felder)**.
       Angeben:
   - Projekt „Code-Prüfer“, Repository `https://github.com/BernhardGoetz/Entwicklung-test`
     (zugleich Projektseite), Lizenz MIT.
   - Kurzbeschreibung: Desktop-App für Studierende, prüft Programmieraufgaben lokal in
     Containern; Windows-Installer (NSIS) aus GitHub Actions.
   - Code-Signing-Policy: `docs/CODE_SIGNING_POLICY.md` (enthält Datenschutz und Rollen).
   - Build-System: GitHub Actions, GitHub-hosted Runner, Workflow `release.yml`, Branch
     `qas`.
   - Signiert werden `code-pruefer.exe`, `uninstall.exe` und der Installer.
   - **Ausdrücklich fragen:** Dürfen die unveränderte llama.cpp-Laufzeit (Release
     `b11700`, SHA-256 gepinnt in `scripts/fetch-llama-server.sh`) und die NSIS-Plugins
     mit dem Projektzertifikat signiert werden? Ohne schriftliche Zusage bleiben sie
     unsigniert.
2. [ ] **MFA** in SignPath aktivieren, sobald die Einladung da ist (Benutzerprofil).
3. [ ] **Projekt** anlegen bzw. das angelegte nutzen. Slug merken (z. B. `code-pruefer`)
       → `SIGNPATH_PROJECT_SLUG`. Repository-URL eintragen.
4. [ ] **Trusted Build System** „GitHub.com“ mit dem Projekt verknüpfen (Origin
       Verification). Falls SignPath eine GitHub-App-Installation verlangt, der Anleitung
       dort folgen **(ungeprüft)**.
5. [ ] **Artifact Configuration** (Project → Artifact Configurations): Inhalt von
       `.signpath/artifact-configurations/default.xml` einfügen, als Default setzen.
       `with-upstream.xml` nur nach Zusage aus Punkt 1 (dann zusätzlich
       `SIGNPATH_SIGN_UPSTREAM` = `true` in `qas` und `release`). Kein Default-Slug →
       Variable `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG`. SignPath liest die Dateien nicht
       aus dem Repository.
6. [ ] **Signing Policies:**
   - `test-signing`: Testzertifikat, ohne manuelle Freigabe, erlaubter Ref
     `refs/heads/qas`.
   - `release-signing`: Zertifikat der Foundation, Origin Verification an, manuelle
     Freigabe durch dich (Approver), erlaubter Ref `refs/heads/qas` (nicht Tags `v*`).
7. [ ] **API-Token:** CI-Benutzer (oder dein Konto) in beiden Policies als **Submitter**
       eintragen, API-Token erzeugen → Secret `SIGNPATH_API_TOKEN` in `qas` und `release`.
8. [ ] **Organisations-ID** (Organisationseinstellungen bzw. oben rechts beim
       Organisationsnamen **(ungeprüft)**) → `SIGNPATH_ORGANIZATION_ID` in `qas` und
       `release`.
9. [ ] Erst nur `qas` befüllen und einen QAS-Lauf testen (Abschnitt 2), dann `release`.

Ob SignPath zusätzliche Policy-Dateien im Repository verlangt, ist unbekannt
**(ungeprüft)**.

Vorarbeit von mir in: `.signpath/artifact-configurations/default.xml`,
`.signpath/artifact-configurations/with-upstream.xml`, `scripts/signpath-stage.mjs`,
`scripts/signpath-round.sh`, `docs/RELEASE_SIGNING.md`, `docs/CODE_SIGNING_POLICY.md`
(in der README verlinkt).

### g. Optional: macOS signieren und notarisieren

Nur mit Apple Developer Program (99 USD/Jahr). Ohne läuft macOS unsigniert.

- [ ] Developer-ID-Application-Zertifikat als `.p12` exportieren und base64 kodieren
      (`base64 -i cert.p12 | pbcopy`).
- [ ] App-spezifisches Passwort unter appleid.apple.com erzeugen.
- [ ] Im Environment **`qas`** (nicht `release`) als Secrets: `APPLE_CERTIFICATE`
      (base64 .p12), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` (z. B.
      „Developer ID Application: Name (TEAMID)“), `APPLE_ID`, `APPLE_PASSWORD`
      (app-spezifisch), `APPLE_TEAM_ID`.
- [ ] Alle sechs setzen. Der Workflow prüft nur `APPLE_CERTIFICATE`; fehlen die anderen,
      scheitert `tauri build` ohne eigene Meldung.

macOS wird nur in QAS gebaut und signiert (Apple hat kein Testzertifikat); nach der
Abnahme werden genau diese DMGs veröffentlicht.

Vorarbeit von mir in: `.github/workflows/release.yml`, Job `bundle-macos`, Schritt „App
und DMG bauen (signiert und notarisiert)“.

## 2. Erster Testlauf (QAS ohne Freigabe)

Voraussetzungen: 1a–1d erledigt, Environments `qas`, `production`, `release` existieren.

- [ ] **Branch `qas` anlegen** (startet den Lauf): Code → Branch-Auswahl → „View all
      branches“ → „New branch“ → Name `qas`, Source `main`. Oder lokal
      `git push origin main:qas`.
- [ ] Actions → „Release (QAS und Abnahme)“ → Lauf öffnen. Später auch: „Run workflow“
      → Branch `qas`.
- [ ] **Zusammenfassung prüfen** (unten auf der Lauf-Seite):
  - Job `version`: „QAS-Lauf für Version …“, Tabelle der KI-Modelle.
  - Warnungen, die ohne SignPath bzw. Schlüssel normal sind: „Windows-Installer
    unsigniert“, „Ohne App-Updater“, „Variable TAURI_UPDATER_PUBKEY ist nicht gesetzt“.
  - Mit Test-Signing: Tabelle „Windows: Signaturen im Installer“ im Job
    `bundle-windows`, Status `UnknownError`/`NotTrusted` ist hier normal. In SignPath
    zwei Signing Requests (drei mit `SIGNPATH_SIGN_UPSTREAM`).
- [ ] **Artefakte** (Abschnitt „Artifacts“ des Laufs): u. a. `model-catalog`,
      `windows-unsigned`, `qas-bundle-windows-x64`, `bundle-macos-macos-arm64`,
      `bundle-macos-macos-x64`, `updater-macos-*`, `image-*-amd64/arm64`, `digests-*`.
- [ ] **Pre-Release** unter Releases: `v<version>-qas.<n>` („Code-Prüfer <version> –
      QAS-Lauf <n>“) mit Installer, DMGs und `SHA256SUMS.txt`, ohne `latest.json`.
      Herunterladen, `sha256sum -c SHA256SUMS.txt`, installieren, starten,
      deinstallieren.
- [ ] **GHCR:** Images `ghcr.io/bernhardgoetz/code-pruefer-python:<version>-qas.<n>` und
      `…-java:…` vorhanden. Jetzt beide Pakete auf **Public** stellen (1a).
- [ ] **Ablehnen:** Oben im Lauf „Review deployments“ → `production` anhaken → Kommentar
      → **„Reject“**. Es entsteht kein Tag `v<version>` und kein Release. Pre-Release und
      QAS-Tag bleiben stehen und können von Hand gelöscht werden.

## 3. Erste echte Veröffentlichung

- [ ] Update-Schlüssel eingerichtet (1e), sonst kann diese Version nie aktualisiert
      werden.
- [ ] **Version festlegen.** `0.1.0` kann bleiben, solange es dafür weder Tag noch
      Release gibt (QAS-Tags `v0.1.0-qas.<n>` zählen nicht). Sonst erhöhen, z. B.
      `npm version 0.2.0 --no-git-tag-version` und die übrigen Dateien von Hand. Diese
      Dateien müssen übereinstimmen (Tabelle unten).
- [ ] `IMAGE_REGISTRY = "ghcr.io/bernhardgoetz"` in `src-tauri/src/runner/languages.rs`
      unverändert lassen (wird gegen den Repository-Owner geprüft).

| Datei | Feld | vom Workflow geprüft |
|---|---|---|
| `src-tauri/Cargo.toml` | `version` unter `[package]` | ja |
| `src-tauri/tauri.conf.json` | `"version"` | ja |
| `package.json` | `"version"` | ja |
| `package-lock.json` | `"version"` (oben und unter `packages[""]`) | nein, aber mitziehen (`npm version` erledigt das) |
| `src-tauri/Cargo.lock` | Paket `code-pruefer` | nein, aber mitziehen (`cargo check` im Ordner `src-tauri`) |

- [ ] Optional: Update-Hinweis für den Dialog in `docs/updates/<version>.txt` (Klartext,
      UTF-8). Ohne Datei steht dort ein Standardtext.
- [ ] Committen, per PR nach `main` (CI grün), dann `main` nach `qas`:
      `git push origin main:qas` oder PR `main` → `qas`.
- [ ] **Testen** wie in Abschnitt 2 (Pre-Release `v<version>-qas.<n>`).
- [ ] **Abnehmen:** „Review deployments“ → `production` → **„Approve and deploy“**.
- [ ] **SignPath** (falls in `release` eingerichtet): In SignPath jeden Signing Request
      von `promote-windows` prüfen (Repository, Branch, Commit, Lauf) und freigeben, zwei
      bzw. drei Stück. Jeder wartet höchstens eine Stunde.
- [ ] **Zusammenfassung prüfen:** `promote-images` (`:<version> -> sha256:…`),
      `promote-windows` („Vergleich mit dem QAS-Installer“, bei Release-Signing Status
      `Valid`), `updater-sign` (Signaturen), `release` (`latest.json`).
- [ ] **Entwurf veröffentlichen:** Releases → Entwurf „Code-Prüfer <version>“ → Edit →
      Dateien prüfen (Installer `Code-Pruefer_<version>_x64-setup.exe`, DMGs,
      `latest.json`, `*.app.tar.gz`, `SHA256SUMS.txt`) → **„Set as the latest release“
      angehakt lassen** → „Publish release“.
- [ ] Kontrolle: `curl -sL https://github.com/bernhardgoetz/entwicklung-test/releases/latest/download/latest.json`
      zeigt die neue Version.
- [ ] Optional: `gh attestation verify <Datei> --repo bernhardgoetz/entwicklung-test`.
- [ ] **PR „Release v<version> nach main“** (`qas` → `main`) prüfen und mergen. Den
      Branch `qas` dabei **nicht** löschen.

Diese erste Version mit Updater muss jeder einmal von Hand installieren.

## 4. Jede weitere Version

- [ ] Änderungen per PR nach `main`.
- [ ] Bei Modell-Update: „KI-Modellkatalog pinnen“ auf `main`, PR prüfen und mergen.
- [ ] Version in `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `package.json`
      (+ `package-lock.json`, `src-tauri/Cargo.lock`) erhöhen; optional
      `docs/updates/<version>.txt`.
- [ ] `main` nach `qas` bringen → QAS-Lauf startet.
- [ ] Pre-Release testen → „Approve and deploy“ (oder „Reject“).
- [ ] SignPath-Requests freigeben.
- [ ] Entwurf veröffentlichen, „Set as the latest release“ angehakt.
- [ ] `latest.json` per `curl` prüfen; PR `qas` → `main` mergen.

## 5. Fehlerbehebung

| Meldung / Symptom | Ursache | Abhilfe |
|---|---|---|
| `Releases laufen nur über den Branch qas (gestartet auf …)` | „Run workflow“ auf anderem Branch | Auf `qas` starten. |
| `Versionen passen nicht: Cargo.toml=… tauri.conf.json=… package.json=…` | Version nicht in allen drei Dateien gleich | Angleichen, erneut nach `qas`. |
| `Die App erwartet Images unter …, der Workflow würde nach … pushen` | `IMAGE_REGISTRY` in `languages.rs` passt nicht zum Owner | `ghcr.io/bernhardgoetz` lassen (nur im Fork ändern). |
| `Version … ist bereits vergeben (Tag: …, Release: …)` (Job `version`) | Tag `v<version>` oder Release existiert | Version erhöhen. |
| `Tag v… existiert bereits auf … (nicht auf …)` / `Release v… ist bereits veröffentlicht.` (Job `release`) | Version zwischenzeitlich anders vergeben | Neue Version, neuer QAS-Lauf. |
| `catalog.json ist nicht (vollständig) gepinnt. Workflow „KI-Modellkatalog pinnen“ …` | Pins fehlen | Schritt 1d. |
| `scripts/pin-models.sh --verify` schlägt fehl | Hugging Face meldet für die gepinnte Revision andere Dateien/Größen/SHA-256 oder die Revision ist gelöscht | Ursache klären, neuen Pin-PR prüfen und mergen. |
| `SignPath ist nur teilweise eingerichtet (…)` | Nur ein Teil der vier SignPath-Einträge im Environment | Alle vier setzen oder alle entfernen. |
| Warnung `SignPath ist im Environment … nicht eingerichtet – der Windows-Installer ist UNSIGNIERT` | Keine SignPath-Einträge | Gewollt bis 1f fertig ist. |
| `… Signaturfehler` im Schritt „Signaturen prüfen“ | Release-Policy liefert keine gültige Signatur, oder Test-Slug enthält kein „test“, oder eine unbekannte exe/dll ist unsigniert | Policy und Slug prüfen; neue Dateien in `scripts/signpath-stage.mjs` und der Artefaktkonfiguration ergänzen. |
| Zeitüberschreitung bei „Runde …: bei SignPath signieren“ | Request nicht innerhalb von 3600 s freigegeben | In SignPath freigeben, „Re-run failed jobs“. |
| `Build artifact … cannot be signed because of build system validation errors` (aus `RELEASE_SIGNING.md`) | Trusted Build System fehlt, Ref `refs/heads/qas` nicht erlaubt, Repository-URL falsch | SignPath-Projekt/Policy korrigieren. |
| `Release-Installer weicht inhaltlich vom QAS-Installer ab` / `Signatur-Manifeste (app/llama) unterscheiden sich` | z. B. `SIGNPATH_SIGN_UPSTREAM` in `qas` und `release` verschieden | Gleich setzen, neuer QAS-Lauf. |
| `Nach der letzten Runde ist noch … unsigniert.` | Uninstaller nicht reproduzierbar oder neue Datei | Log der Runde ansehen (`RELEASE_SIGNING.md`, „Fehlerbilder“). |
| `Updater nur teilweise eingerichtet: …` | Nur Secret oder nur Variable gesetzt | Beide setzen (1e). |
| Warnung `Updater nicht eingerichtet …` | Weder Secret noch Variable | Gewollt nur ohne Updater; sonst 1e. |
| Lauf hängt bei `abnahme`, nach 30 Tagen fehlgeschlagen | GitHub bricht offene Freigaben nach 30 Tagen ab; Artefakte halten 30 Tage | Neuer QAS-Lauf (Push auf `qas` oder „Run workflow“). |
| `abnahme` läuft ohne Freigabe durch | Environment `production` fehlte; GitHub legt ein referenziertes, nicht vorhandenes Environment automatisch **ohne** Schutzregeln an (laut GitHub-Doku, hier ungeprüft) | Sofort Required reviewer in `production` setzen; Entwurf nicht veröffentlichen, wenn nicht getestet. Gleiches gilt für `qas`/`release` (dann ohne Branch-Regel und ohne Secrets → unsigniert). |
| Job `pull-request`: GitHub Actions darf keine PRs anlegen | Haken aus 1a fehlt | Setzen, Job neu starten. Das Release ist trotzdem fertig. |
| Tag anlegen scheitert (Ruleset) in `qas-release` oder `release` | Tag-Ruleset mit „Restrict creations“ | Haken entfernen (1b), Job neu starten. |
| `…:<version> existiert bereits mit anderem Digest` (promote-images) | Image-Tag wurde anders gesetzt | Version erhöhen. |
| `Asset … fehlt im Release (GitHub hat den Namen geändert?)` | Asset-Name verändert | Log prüfen, Job neu starten. |
| `… existiert und ist kein Pre-Release.` (qas-release) | QAS-Tag als normales Release markiert | Release/Tag `v<version>-qas.<n>` löschen, neu starten. |
| Pakete in GHCR privat, App lädt keine Images | Standard nach dem ersten Push | Auf Public stellen (1a). |
| „Run workflow“-Knopf fehlt | Workflow liegt nicht auf `main` | Stand nach `main` mergen (1a). |
| Neuer Push auf `qas` startet nicht sofort | Concurrency: ein Lauf je Branch, wartet auf den laufenden (auch auf offene Abnahme) | Alten Lauf abnehmen oder ablehnen. |

„Re-run failed jobs“ nach der Abnahme geht, solange die Artefakte existieren (30 Tage);
`release` und `promote-images` sind dafür wiederholbar ausgelegt.

## 6. Wo liegt was

| Datei | Zweck |
|---|---|
| `.github/workflows/release.yml` | Release-Pipeline: QAS-Lauf, Abnahme, Beförderung, Entwurfs-Release, PR `qas` → `main` |
| `.github/workflows/pin-models.yml` | „KI-Modellkatalog pinnen“: Pins per PR |
| `.github/workflows/ci.yml` | CI auf `main`, `qas` und PRs (Build, Tests, Clippy, Docker-Tests) |
| `.github/workflows/images.yml` | Runner-Images `:edge`/`:sha-…` bei Änderungen unter `images/` |
| `.signpath/artifact-configurations/default.xml` | SignPath-Artefaktkonfiguration (nur eigene Dateien) |
| `.signpath/artifact-configurations/with-upstream.xml` | dito, mit llama.cpp und NSIS-Plugins (nur nach Zusage) |
| `scripts/check-version-unreleased.sh` | prüft, ob `v<version>` noch frei ist |
| `scripts/pin-models.sh` | Katalog pinnen, `--check`, `--verify` |
| `scripts/signpath-stage.mjs` | Brücke Tauri-`signCommand` ↔ SignPath (Runden) |
| `scripts/signpath-round.sh` | wertet eine Signaturrunde aus |
| `scripts/updater.mjs` | Updater-Overlay (`config`), Signatur prüfen (`verify`), `latest.json` (`manifest`) |
| `scripts/install-signer.sh` | installiert nur die Tauri-CLI für `updater-sign` |
| `scripts/artifact-manifest.sh` | SHA-256-Manifeste der Artefakte zwischen den Jobs |
| `scripts/release-notes.sh` | Release-Notizen (QAS und Release) |
| `scripts/fetch-llama-server.sh` | llama.cpp-Laufzeit `b11700`, SHA-256 gepinnt |
| `scripts/export-image-archives.sh` | Image-Archive und `image-digests.json` |
| `src-tauri/src/ai/catalog.json` | KI-Modellkatalog (Pins) |
| `src-tauri/tauri.conf.json` | App-Konfiguration, Version, NSIS |
| `src-tauri/tauri.release.conf.json` | Overlay für Release-Builds (Images, llama) |
| `src-tauri/tauri.updater.conf.json` | wird im Workflow erzeugt (nicht eingecheckt) |
| `src-tauri/tauri.sign.conf.json` | wird im Workflow erzeugt (nicht eingecheckt) |
| `src-tauri/Cargo.toml`, `package.json` | Version |
| `src-tauri/src/runner/languages.rs` | `IMAGE_REGISTRY` |
| `src-tauri/src/app_update.rs` | App-Updater (Endpunkt `latest.json`) |
| `docs/updates/<version>.txt` | optionaler Update-Hinweis (gibt es noch nicht) |
| `docs/RELEASE_QAS.md` | Ablauf QAS und Abnahme im Detail |
| `docs/RELEASE_SIGNING.md` | Windows-Signatur über SignPath im Detail |
| `docs/UPDATER.md` | App-Updater im Detail |
| `docs/CODE_SIGNING_POLICY.md` | Code-Signing-Policy für SignPath (englisch) |

## Abweichungen zwischen Doku und Workflow

- `.github/workflows/images.yml` (Kommentar): „Die Versions-Tags (:<version>) pusht
  ausschließlich release.yml beim Versions-Tag v<version>.“ Tatsächlich startet
  `release.yml` beim Push auf `qas`, und `:<version>` setzt erst `promote-images` nach der
  Abnahme. Ein Tag `v*` startet nichts.
- `docs/RELEASE_QAS.md` schlägt vor, den Pin-Workflow z. B. auf `qas` zu starten. Das
  geht, aber der Merge des Pin-PRs in `qas` startet sofort einen Release-Lauf. Diese
  Anleitung empfiehlt deshalb `main`.
- `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` wird an SignPath übergeben, zählt aber nicht zu
  den vier Einträgen, die der Workflow auf Vollständigkeit prüft. Ein Tippfehler darin
  fällt erst bei SignPath auf.
- Die Docs nennen `SIGNPATH_SIGN_UPSTREAM` je Environment, sagen aber nicht, dass der Wert
  in `qas` und `release` gleich sein muss. Der Vergleichsschritt in `promote-windows`
  verlangt das.
- Die Apple-Secrets prüft der Workflow nur über `APPLE_CERTIFICATE`; eine teilweise
  Apple-Einrichtung bricht nicht mit eigener Meldung ab (anders als SignPath und Updater).
