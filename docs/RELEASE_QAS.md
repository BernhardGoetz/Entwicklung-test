# Release mit QAS-Stufe und Abnahme

Jedes Release durchläuft zuerst eine **QAS-Stufe** (Qualitätssicherung). Dort wird es
genau so gebaut wie ein Release und als GitHub-**Pre-Release** zum Testen
bereitgestellt. Erst nach der **manuellen Abnahme** („QAS-Abnahme“) wird **genau dieser
Build** zum Release befördert. Neu gebaut wird dabei nichts, nur neu signiert.
Workflow: [`.github/workflows/release.yml`](../.github/workflows/release.yml).

## Ablauf

```
 Entwicklung (Feature-Branches, PRs nach qas)
        │
        ▼
 push auf Branch qas ───────────────────────────────── (oder „Run workflow“ auf qas)
        │
        ▼
 version ── Versionen gleich? v<version> noch frei? Modell-Pins gegen Hugging Face prüfen
        │
        ├─► images ─────────── Images bauen, push :<version>-qas.<n> und :sha-<commit>
        │                      (nie :<version>), Archive + image-digests.json
        ├─► build-windows ──── code-pruefer.exe + KI-Laufzeit (unsigniert, SHA256SUMS)
        │
        ├─► bundle-windows ─── Installer, SignPath Test-Signing (Env. „qas“, falls eingerichtet)
        ├─► bundle-macos ───── DMGs (Apple-Signatur, falls eingerichtet, Env. „qas“)
        │
        ▼
 qas-release ──────── Pre-Release v<version>-qas.<n> „QAS – nicht für Produktion“
        │             mit Installern und SHA256SUMS.txt
        ▼
 abnahme ──────────── wartet auf „Approve and deploy“ (Environment „production“)
        │             „Reject“ → Ende, kein Release
        ▼
        ├─► promote-images ── :<version> per Digest auf das getestete Image setzen
        ├─► promote-windows ─ dieselben unsignierten Dateien mit Release-Signing
        │                     neu bündeln (Env. „release“), Inhalt mit QAS vergleichen
        ▼
 updater-sign ─────── Updater-Signaturen über den finalen Installer und die
        │             macOS-Archive (Env. „release“, docs/UPDATER.md)
        ▼
 release ──────────── Tag v<version> auf dem QAS-Commit, Entwurfs-Release mit
        │             latest.json, SHA256SUMS.txt + Build-Provenance-Attestierung
        ▼
 pull-request ─────── PR „Release v<version> nach main“ (qas → main)
```

`<n>` ist die Laufnummer des Workflows (`github.run_number`), z. B. `v0.2.0-qas.17`.

## QAS-Lauf starten

1. Version in `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` und `package.json`
   erhöhen (alle drei gleich). Ist die Version schon als Tag oder Release vorhanden,
   bricht der Job `version` ab.
2. Den Stand auf den Branch `qas` bringen, z. B. per Pull Request nach `qas` oder
   `git push origin <commit>:qas`. **Jeder Push auf `qas` startet einen QAS-Lauf.**
3. Alternativ: Actions → „Release (QAS und Abnahme)“ → „Run workflow“ mit Branch `qas`.
   Auf anderen Branches bricht der Lauf ab.

Ein Push auf `qas`, während ein Lauf noch läuft oder auf die Abnahme wartet, wartet in
der Warteschlange (Concurrency-Gruppe je Branch). Kommt ein weiterer Push dazu, ersetzt
er den wartenden Lauf. Den alten Lauf bei Bedarf ablehnen („Reject“), dann startet der
neue.

Einen versionierten Tag `v*` von Hand zu pushen, startet **keinen** Release mehr.
Releases entstehen nur noch über QAS und Abnahme.

## Testen

- Unter Releases das Pre-Release `v<version>-qas.<n>` öffnen (Link auch in der
  Zusammenfassung des Laufs und am wartenden Job `abnahme`) und Installer bzw. DMGs laden.
- Prüfsummen: `sha256sum -c SHA256SUMS.txt`.
- Das Pre-Release enthält keine `latest.json`: Installierte Apps bieten QAS-Stände nie
  als Update an. Den Updater selbst testet man auf einem Fork ([`UPDATER.md`](UPDATER.md)).
- Windows: Ist SignPath im Environment `qas` eingerichtet, sind die Dateien mit dem
  **Test-Zertifikat** signiert. Windows zeigt sie als nicht vertrauenswürdig an; das ist
  in QAS so gewollt. Die Signaturtabelle steht in der Zusammenfassung des Jobs
  `bundle-windows`.
- Die Images liegen als `ghcr.io/bernhardgoetz/code-pruefer-<sprache>:<version>-qas.<n>`
  vor. Die App erwartet online `:<version>`, das erst nach der Abnahme existiert. Bis dahin
  lädt sie die Images aus dem Installer; „Images aktualisieren“ funktioniert in QAS noch
  nicht.

## Abnehmen oder ablehnen

- Actions → der Lauf → oben „Review deployments“ → Environment `production` anhaken →
  Kommentar → **„Approve and deploy“** (abnehmen) oder **„Reject“** (ablehnen).
- Ablehnen beendet den Lauf. Nichts wird veröffentlicht, die QAS-Images und das
  Pre-Release bleiben stehen. Das Pre-Release kann man von Hand löschen.
- **Zeitlimit:** GitHub wartet höchstens **30 Tage** auf eine Freigabe, danach schlägt
  der Job fehl. Die Artefakte des Laufs bleiben ebenfalls 30 Tage erhalten. Später ist
  ein neuer QAS-Lauf nötig.

## Was nach der Abnahme passiert

1. **promote-images:** Für jede Sprache wird geprüft, dass `:<version>-qas.<n>` noch auf
   den getesteten Digest zeigt. Dann wird `:<version>` per
   `docker buildx imagetools create` auf **denselben Digest** gesetzt, ohne Neubau. Gibt
   es `:<version>` schon mit anderem Digest, bricht der Job ab.
2. **promote-windows:** Nimmt die unsignierten Programmdateien aus `build-windows`. Ihre
   `SHA256SUMS.txt` muss dasselbe SHA-256 haben wie im QAS-Lauf, sonst Abbruch. Dann
   bündelt er sie mit der **Release-Signing**-Policy aus dem Environment `release` neu.
   Neu gebündelt wird nur, weil die Signatur es erfordert: Uninstaller und Installer
   betten signierte Dateien ein und müssen deshalb neu erzeugt werden.
   - **Unterschied zum getesteten QAS-Installer: nur die Signaturen.** Neu signiert sind
     `code-pruefer.exe` und `uninstall.exe` (und mit `SIGNPATH_SIGN_UPSTREAM` die
     Fremdbinaries), außerdem ist der Installer selbst neu gebaut und signiert. Der
     Workflow entpackt beide Installer und vergleicht sie. Die Dateiliste muss gleich
     sein, jede Datei muss bitgleich sein, außer signierten exe/dll. Bei denen müssen die
     unsignierten Ausgangsdateien (SHA-256 im Signatur-Manifest) gleich sein. Die
     Installer-Datei selbst ist nicht bitgleich (neu gepackt, andere Signaturen). Die
     Liste der neu signierten Dateien steht in der Job-Zusammenfassung.
   - Ist SignPath weder in `qas` noch in `release` eingerichtet, wird der QAS-Installer
     **unverändert** übernommen.
   - macOS: Die DMGs aus QAS werden unverändert veröffentlicht. Apple hat keine
     Test-Signatur, sie sind also schon in QAS final signiert (falls eingerichtet).
3. **updater-sign:** Prüft den fertigen Windows-Installer aus `promote-windows` und die
   macOS-Archive (`*.app.tar.gz`) gegen die Manifeste der erzeugenden Jobs, signiert sie
   für den App-Updater und prüft die Signaturen.
   Ohne Updater-Schlüssel: übersprungen mit Warnung. Siehe [`UPDATER.md`](UPDATER.md).
4. **release:** Prüft erneut, dass `v<version>` frei ist. Ein erneuter Lauf desselben
   Commits mit Entwurf ist erlaubt. Dann setzt der Job den Tag `v<version>` auf den
   QAS-Commit und legt ein **Entwurfs-Release** an, mit Windows-Installer (Release),
   DMGs, `latest.json` und `*.app.tar.gz` (App-Updater, falls eingerichtet),
   `SHA256SUMS.txt` und Build-Provenance-Attestierung. Dateinamen ohne Umlaut
   (`Code-Pruefer_…`). **Veröffentlichen musst du selbst** (Release öffnen → „Publish
   release“, „Set as the latest release“ angehakt lassen). Erst dann bieten installierte
   Apps das Update an.
5. **pull-request:** Legt den PR `qas` → `main` mit dem Titel „Release v<version> nach
   main“ an. Existiert schon ein offener PR, legt er keinen zweiten an. Auf `main` wird
   nie direkt geschrieben. Den PR prüfst und mergst du selbst.

## KI-Modellkatalog: Pins per Pull Request

Der Release pinnt den Katalog nicht selbst. Wäre das Hugging-Face-Repository des
Herausgebers in dem Moment kompromittiert, würde sonst die bösartige Datei gepinnt
(Vertrauen beim ersten Abruf). Stattdessen:

1. **Einmalig (und bei jedem gewollten Modell-Update):** Actions → „KI-Modellkatalog
   pinnen“ (`.github/workflows/pin-models.yml`) → „Run workflow“ auf dem Branch, in den
   der PR soll (z. B. `qas`). Der Workflow trägt Revision, Datei(en), Größe und SHA-256
   aus der Hugging-Face-API ein (`scripts/pin-models.sh`) und legt einen Pull Request mit
   Tabelle und Links an. Voraussetzung: „Allow GitHub Actions to create and approve pull
   requests“ (siehe unten). Für PRs des `GITHUB_TOKEN` startet GitHub keine weiteren
   Workflows; CI für den PR ggf. durch Schließen und Wiederöffnen oder einen eigenen
   Commit anstoßen.
2. **Bernhard prüft** die Werte auf huggingface.co (Revision in der Commit-Historie des
   Herausgebers, Größe und SHA-256 der Datei unter „Files and versions“) und mergt.
3. **Jeder Release** (Job `version`): `scripts/pin-models.sh --check` (vollständig
   gepinnt, sonst Abbruch mit Hinweis auf den Workflow) und `--verify` (die API muss für
   die gepinnte Revision genau dieselben Dateien, Größen und SHA-256 melden; gelöschte
   Revision oder jede Abweichung → Abbruch). Eine neuere Revision auf `main` wird nur als
   Hinweis gemeldet; übernommen wird sie nur über einen neuen Pin-PR.

## Integrität der Artefakte zwischen den Jobs

Jeder Schritt eines späteren Jobs desselben Laufs könnte Artefakte ersetzen
(`upload-artifact` mit `overwrite`, das Laufzeit-Token hängt nicht an `permissions:`).
Ein kompromittierter Baustein in z. B. `promote-images` könnte so das macOS-Updater-Archiv
austauschen, das `updater-sign` danach gültig signieren würde. Deshalb gilt:

- Jeder Job, der ein Release-Artefakt erzeugt, gibt ein **Manifest** (SHA-256 und
  Dateiname je Datei, `scripts/artifact-manifest.sh write`) als Job-Output weiter:
  `version` (gepinnter `catalog.json`), `images` (je Sprache `image-digests.json`, die
  Image-Archive amd64/arm64 und der Registry-Digest), `build-windows` (`SHA256SUMS.txt`
  der unsignierten Programmdateien), `bundle-windows` und `promote-windows` (Installer,
  Signatur-Manifest), `bundle-macos` (je Architektur DMG und `.app.tar.gz`) und
  `updater-sign` (Updater-Signaturen).
- Jeder Job, der ein Artefakt verwendet, prüft es **vor** dem Bündeln, Signieren oder
  Veröffentlichen (`scripts/artifact-manifest.sh verify`): gleiche Dateinamen, gleiche
  SHA-256, keine zusätzlichen und keine fehlenden Dateien. `promote-images` prüft
  außerdem, dass `image-digests.json` genau den Digest nennt, den `images` gebaut hat.
  Abweichung oder fehlender Output → Abbruch.
- Matrix-Jobs (`images`, `bundle-macos`) schreiben je Eintrag einen eigenen Output-Namen
  (z. B. `dmg_macos_arm64`). Kommt ein Wert nicht an, scheitert die Prüfung (nie still
  ungeprüft).
- `updater-sign` installiert nur die Tauri-CLI (`scripts/install-signer.sh`: Version und
  sha512-Integrität aus `package-lock.json`, `npm ci --ignore-scripts` in einem eigenen
  Ordner), kein vollständiges `npm ci`. Der private Updater-Schlüssel steht nur im Schritt
  „Signieren“; geprüft wird in einem eigenen Schritt ohne Schlüssel.

## Nötige GitHub-Einstellungen (einmalig)

Settings → Environments:

| Environment | Pflicht-Reviewer | Deployment branches and tags | Secrets / Variablen |
|---|---|---|---|
| `qas` | keine | „Selected branches and tags“: Branch `qas` | SignPath **Test-Signing** (`SIGNPATH_API_TOKEN`; `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`, `SIGNPATH_SIGNING_POLICY_SLUG` = `test-signing`, optional `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG`, `SIGNPATH_SIGN_UPSTREAM`), Apple-Secrets (`APPLE_CERTIFICATE`, …) |
| `production` | **Bernhard** (Required reviewers); optional „Prevent self-review“ **aus** lassen, da Bernhard selbst pusht | Branch `qas` | keine (nur Freigabe) |
| `release` | keine (die Freigabe erfolgt über `production`) | Branch `qas` | SignPath **Release-Signing** (dieselben Namen, `SIGNPATH_SIGNING_POLICY_SLUG` = `release-signing`); App-Updater: Secrets `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` ([`UPDATER.md`](UPDATER.md)) |

Repository-Variable (Settings → Secrets and variables → Actions → Variables):
`TAURI_UPDATER_PUBKEY` (öffentlicher Updater-Schlüssel; wird in die App eingebaut).

Weitere Einstellungen:

- **Actions darf PRs anlegen:** Settings → Actions → General → Workflow permissions →
  „**Allow GitHub Actions to create and approve pull requests**“ anhaken. Sonst scheitert
  der Job `pull-request`. Der Rest des Releases ist dann trotzdem fertig.
- **Branch `qas` schützen** (Settings → Rules → Rulesets → New branch ruleset, Ziel
  `qas`): Pushen bzw. Mergen nur durch Bernhard (Bypass), „Block force pushes“. Die
  Environments `qas`/`release` vertrauen jedem Workflow-Lauf auf `qas`. Wer auf `qas`
  pushen darf, kommt an deren Secrets. Die Abnahme in `production` verhindert nur die
  Beförderung durch diesen Workflow.
- **Tag-Regel `v*`** (falls vorhanden, siehe README): Der Workflow legt die Tags
  `v<version>-qas.<n>` und `v<version>` selbst an, mit dem `GITHUB_TOKEN` als
  „github-actions[bot]“. Entweder in der Ruleset-Bypass-Liste die App „GitHub Actions“
  eintragen, oder „Restrict creations“ abschalten und nur „Restrict updates“ und
  „Restrict deletions“ behalten. **[ungeprüft: ob „GitHub Actions“ in der Bypass-Liste
  auswählbar ist]**
- Das bisherige Environment `release` hatte einen Pflicht-Reviewer und die Tag-Regel
  `v*`. Beides entfernen und durch die Branch-Regel `qas` ersetzen (Tabelle oben). Die
  Apple-Secrets von `release` nach `qas` verschieben.
- **SignPath:** Die Policy `release-signing` muss Signaturen vom Branch `qas` zulassen
  (`refs/heads/qas`), nicht mehr nur von Tags `v*`. Siehe
  [`RELEASE_SIGNING.md`](RELEASE_SIGNING.md).

## Grenzen

- Der Workflow konnte hier nicht laufen. Geprüft sind Syntax und Sicherheitsregeln
  (actionlint, zizmor). Er nutzt **YAML-Anker** (`&windows-bundle-steps`), damit
  `bundle-windows` und `promote-windows` dieselben Schritte haben. GitHub Actions
  unterstützt Anker; actionlint ab 1.7.8 auch. zizmor weist darauf hin, dass er Anker
  nur eingeschränkt auswertet.
- Ein erneuter Versuch („Re-run failed jobs“) nach der Abnahme ist möglich, solange die
  Artefakte existieren (30 Tage). `release` und `promote-images` sind dafür
  wiederholbar ausgelegt.
