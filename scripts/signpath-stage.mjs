#!/usr/bin/env node
// Brücke zwischen Tauris Windows-Signierung (bundle.windows.signCommand) und
// SignPath (asynchron, über GitHub-Artefakte; docs/RELEASE_SIGNING.md).
//
// SignPath signiert nur Dateien, die als Artefakt dieses Workflow-Laufs
// hochgeladen wurden (Origin Verification). Eine synchrone Signatur je Datei
// aus dem signCommand heraus ist damit nicht möglich. Stattdessen läuft das
// Bündeln in Runden (.github/workflows/release.yml, Job bundle-windows):
//
//   1. Tauri ruft dieses Skript für jede zu signierende Datei auf
//      (Haupt-Exe, exe/dll in den Ressourcen, NSIS-Plugins, Uninstaller,
//      Installer). Das Skript ersetzt die Datei durch ihre signierte Fassung,
//      falls eine frühere Runde sie schon signiert hat (Zuordnung über das
//      SHA-256 der unsignierten Datei), und sammelt sonst eine Kopie in
//      SIGNPATH_STAGE_DIR. Die Datei selbst bleibt dann in dieser Runde
//      unsigniert.
//   2. Der Workflow lädt SIGNPATH_STAGE_DIR als Artefakt hoch, SignPath
//      signiert, das Ergebnis wird mit --import nach SIGNPATH_SIGNED_DIR
//      übernommen; dann folgt die nächste Runde (`tauri bundle`).
//
// Reihenfolge: Uninstaller und Installer enthalten andere Dateien. Der
// Uninstaller wird erst gesammelt, wenn in derselben Runde kein NSIS-Plugin
// mehr gesammelt wurde (er bettet die Plugins ein), der Installer erst, wenn
// sonst nichts mehr zu signieren ist. Der Uninstaller wird von makensis
// reproduzierbar erzeugt (gleiche Eingaben -> gleiches SHA-256; lokal
// geprüft), deshalb findet die nächste Runde seine signierte Fassung.
//
// Nie neu signiert werden Dateien, die schon eine Authenticode-Signatur haben
// (z. B. die Visual-C++-Laufzeit von Microsoft). Fremdbinaries ("upstream":
// llama.cpp-Laufzeit, NSIS-Plugins) werden nur mit SIGNPATH_SIGN_UPSTREAM=true
// gesammelt: Die Bedingungen der SignPath Foundation erlauben das Signieren
// nur für selbst gebaute Software (siehe docs/RELEASE_SIGNING.md).
//
// Aufrufe:
//   node scripts/signpath-stage.mjs <datei>              (als signCommand)
//   node scripts/signpath-stage.mjs --import <signiert> <stage>
//   node scripts/signpath-stage.mjs --apply <datei>       (signierte Fassung
//                                                          einsetzen, sonst Fehler)
//   node scripts/signpath-stage.mjs --has-signature <datei>
//
// Umgebung: SIGNPATH_STAGE_DIR, SIGNPATH_SIGNED_DIR, SIGNPATH_SIGN_UPSTREAM.

import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const MAIN_BINARY = "code-pruefer.exe";
// Von Tauri signierte Plugins (tauri-bundler NSIS_PLUGIN_FILES) und nsExec.dll,
// das hooks.nsh (winget) zusätzlich verwendet und Tauri nicht signiert.
const EXTRA_PLUGINS = ["nsExec.dll"];

function log(msg) {
  // stdout landet im Tauri-Log ("Output of signing command").
  console.log(`[signpath-stage] ${msg}`);
}

function fail(msg) {
  console.error(`[signpath-stage] FEHLER: ${msg}`);
  process.exit(1);
}

function sha256(file) {
  return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

/// true, wenn die PE-Datei eine Authenticode-Signatur trägt (Eintrag
/// "Certificate Table" im Datenverzeichnis). Prüft nicht die Gültigkeit –
/// das macht der Prüfschritt im Workflow (Get-AuthenticodeSignature).
export function hasSignature(file) {
  const buf = fs.readFileSync(file);
  if (buf.length < 0x40 || buf.readUInt16LE(0) !== 0x5a4d) {
    fail(`${file} ist keine PE-Datei`);
  }
  const pe = buf.readUInt32LE(0x3c);
  if (pe + 24 > buf.length || buf.readUInt32LE(pe) !== 0x00004550) {
    fail(`${file} ist keine PE-Datei`);
  }
  const magic = buf.readUInt16LE(pe + 24);
  const dataDirs = pe + 24 + (magic === 0x20b ? 112 : 96);
  const certEntry = dataDirs + 4 * 8; // IMAGE_DIRECTORY_ENTRY_SECURITY
  if (certEntry + 8 > buf.length) {
    return false;
  }
  return buf.readUInt32LE(certEntry + 4) > 0;
}

function realDir(p) {
  try {
    return fs.realpathSync.native(p).toLowerCase();
  } catch {
    return path.resolve(p).toLowerCase();
  }
}

/// Ordnet eine Datei einer Art zu; unbekannte unsignierte Dateien sind ein
/// Fehler (lieber abbrechen als unbemerkt unsigniert ausliefern).
export function classify(file) {
  const norm = file.replace(/\\/g, "/");
  const base = path.basename(file);
  const lower = base.toLowerCase();
  if (lower === MAIN_BINARY) {
    return { kind: "app", name: MAIN_BINARY };
  }
  if (/\/bundle\/nsis\/[^/]+-setup\.exe$/i.test(norm)) {
    return { kind: "installer", name: "setup.exe" };
  }
  if (/\/plugins\/x86-unicode\/(additional\/)?[^/]+\.dll$/i.test(norm)) {
    return { kind: "nsisplugin", name: base };
  }
  if (/\/resources\/llama\/[^/]+\.(exe|dll)$/i.test(norm)) {
    return { kind: "llama", name: base };
  }
  // !uninstfinalize: makensis übergibt eine temporäre Datei (Linux:
  // /tmp/makensisXXXXXX, Windows: im %TEMP%-Ordner), ohne .exe/.dll.
  const inTemp = realDir(path.dirname(file)) === realDir(os.tmpdir());
  if (inTemp || !/\.(exe|dll)$/i.test(lower)) {
    return { kind: "uninstaller", name: "uninstall.exe" };
  }
  return null;
}

const UPSTREAM = new Set(["llama", "nsisplugin"]);

function listDir(dir) {
  try {
    return fs.readdirSync(dir).filter((f) => fs.statSync(path.join(dir, f)).isFile());
  } catch {
    return [];
  }
}

/// Dateinamen im Stage-/Signed-Ordner: <art>-<sha256 der unsignierten Datei>-<name>
const NAME_RE = /^([a-z]+)-([0-9a-f]{64})-(.+)$/;

function signedIndex(signedDir) {
  const byUnsigned = new Map();
  for (const f of listDir(signedDir)) {
    const m = NAME_RE.exec(f);
    if (m) {
      byUnsigned.set(m[2], path.join(signedDir, f));
    }
  }
  let hashes = null;
  // SHA-256 der signierten Fassungen, erst bei Bedarf berechnet.
  const signedHashes = () => {
    hashes ??= new Set([...byUnsigned.values()].map(sha256));
    return hashes;
  };
  return { byUnsigned, signedHashes };
}

function env(name, required = true) {
  const v = process.env[name];
  if (required && !v) {
    fail(`Umgebungsvariable ${name} fehlt`);
  }
  return v || "";
}

function handle(file, ctx) {
  if (!fs.existsSync(file)) {
    fail(`${file} existiert nicht`);
  }
  const hash = sha256(file);
  const signed = ctx.index.byUnsigned.get(hash);
  if (signed) {
    fs.copyFileSync(signed, file);
    log(`signierte Fassung eingesetzt: ${file}`);
    return;
  }
  if (ctx.index.signedHashes().has(hash)) {
    log(`bereits signiert (diese Pipeline): ${file}`);
    return;
  }
  if (hasSignature(file)) {
    log(`hat bereits eine fremde Signatur, wird nicht neu signiert: ${file}`);
    return;
  }
  const c = classify(file);
  if (!c) {
    fail(`unbekannte unsignierte Datei ${file} – Zuordnung in scripts/signpath-stage.mjs ergänzen`);
  }
  if (UPSTREAM.has(c.kind) && !ctx.signUpstream) {
    log(`Fremdbinary bleibt unsigniert (SIGNPATH_SIGN_UPSTREAM ist nicht true): ${file}`);
    return;
  }
  const staged = listDir(ctx.stageDir);
  if (c.kind === "uninstaller" && staged.some((f) => f.startsWith("nsisplugin-"))) {
    log(`Uninstaller zurückgestellt (enthält Plugins, die erst in dieser Runde signiert werden)`);
    return;
  }
  if (c.kind === "installer" && staged.length > 0) {
    log(`Installer zurückgestellt (enthält Dateien, die erst in dieser Runde signiert werden)`);
    return;
  }
  fs.mkdirSync(ctx.stageDir, { recursive: true });
  const target = path.join(ctx.stageDir, `${c.kind}-${hash}-${c.name}`);
  fs.copyFileSync(file, target);
  log(`zum Signieren gesammelt (${c.kind}): ${file}`);
}

function signCommand(file) {
  const ctx = {
    stageDir: env("SIGNPATH_STAGE_DIR"),
    index: signedIndex(env("SIGNPATH_SIGNED_DIR")),
    signUpstream: env("SIGNPATH_SIGN_UPSTREAM", false) === "true",
  };
  handle(file, ctx);
  // nsExec.dll liegt in Tauris Plugin-Kopie, wird von Tauri aber nicht an
  // den signCommand übergeben; beim ersten Plugin der Runde mitbehandeln.
  const c = classify(file);
  if (c && c.kind === "nsisplugin" && ctx.signUpstream) {
    for (const extra of EXTRA_PLUGINS) {
      const p = path.join(path.dirname(file), extra);
      if (fs.existsSync(p) && p !== file) {
        handle(p, ctx);
      }
    }
  }
}

/// Übernimmt das Ergebnis eines Signaturauftrags: jede gesammelte Datei muss
/// signiert zurückkommen, nichts darf hinzukommen.
function importSigned(fromDir, stageDir) {
  const signedDir = env("SIGNPATH_SIGNED_DIR");
  const staged = listDir(stageDir).sort();
  const returned = listDir(fromDir).sort();
  if (staged.length === 0) {
    fail(`${stageDir} ist leer`);
  }
  if (JSON.stringify(staged) !== JSON.stringify(returned)) {
    fail(`SignPath lieferte andere Dateien zurück.\nerwartet: ${staged.join(", ")}\nerhalten: ${returned.join(", ")}`);
  }
  fs.mkdirSync(signedDir, { recursive: true });
  for (const f of returned) {
    const p = path.join(fromDir, f);
    if (!hasSignature(p)) {
      fail(`${f} kam ohne Signatur zurück`);
    }
    if (sha256(p) === sha256(path.join(stageDir, f))) {
      fail(`${f} ist unverändert`);
    }
    fs.copyFileSync(p, path.join(signedDir, f));
    log(`signiert übernommen: ${f}`);
  }
}

function apply(file) {
  const index = signedIndex(env("SIGNPATH_SIGNED_DIR"));
  const signed = index.byUnsigned.get(sha256(file));
  if (!signed) {
    fail(`keine signierte Fassung für ${file}`);
  }
  fs.copyFileSync(signed, file);
  log(`signierte Fassung eingesetzt: ${file}`);
}

const args = process.argv.slice(2);
if (args[0] === "--import" && args.length === 3) {
  importSigned(args[1], args[2]);
} else if (args[0] === "--apply" && args.length === 2) {
  apply(args[1]);
} else if (args[0] === "--has-signature" && args.length === 2) {
  process.exit(hasSignature(args[1]) ? 0 : 1);
} else if (args.length === 1 && !args[0].startsWith("--")) {
  signCommand(args[0]);
} else {
  fail("Aufruf: signpath-stage.mjs <datei> | --import <signiert> <stage> | --apply <datei> | --has-signature <datei>");
}
